/// A file built from `web/` and `static/` by build.rs.
macro_rules! web {
    ($name:literal) => {
        include_str!(concat!(env!("OUT_DIR"), "/web/", $name))
    };
}

#[macro_use]
mod admin;
mod access;
mod accounts;
mod aki;
mod app;
mod avatars;
mod boards;
mod challenges;
mod cli;
mod companions;
mod competition;
mod config;
mod conquests;
mod deck_copies;
mod decks;
mod dev;
mod feedback;
mod freezes;
mod friend_nudges;
mod game;
mod i18n;
mod inbox;
mod notifications;
mod oauth;
mod privacy;
mod reminders;
mod shares;
mod social;
mod store;
mod study;
mod subscriptions;
#[cfg(test)]
mod test_support;
mod web_util;
mod weekly_challenges;
mod worker;

use access::authorized;
use app::{App, load_player, now_ms};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Datelike;
use cli::{parse_args, send_message};
use config::Config;
use game::Periods;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use store::{Error, Store};
use study::valid_year;
use web_util::store_error;
use worker::import;

async fn index() -> Html<&'static str> {
    Html(web!("index.html"))
}

async fn community_page() -> Html<&'static str> {
    Html(web!("community.html"))
}

async fn personal_page() -> Html<&'static str> {
    Html(web!("personal.html"))
}

#[derive(Deserialize, Default)]
struct CommunityQuery {
    year: Option<i32>,
    month: Option<u32>,
}

fn refresh_community(
    app: &App,
    store: &mut Store,
    players: &[competition::Participant],
    now: i64,
) -> Result<(), Error> {
    use rusqlite::OptionalExtension;
    let last: Option<i64> = store
        .conn
        .query_row(
            "select refreshed_at from community_refresh where id=1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if last.is_some_and(|last| now >= last && now - last < 60_000) {
        return Ok(());
    }
    competition::refresh(
        store,
        players,
        &app.week,
        now,
        app.config.competition_start_date.as_deref(),
    )?;
    store.conn.execute(
        "insert into community_refresh(id,refreshed_at) values (1,?1)
         on conflict(id) do update set refreshed_at=excluded.refreshed_at",
        [now],
    )?;
    Ok(())
}

async fn community_dashboard(
    State(app): State<Arc<App>>,
    Query(query): Query<CommunityQuery>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let now = now_ms();
    let date = app.week.competition_date(now);
    let year = query.year.unwrap_or(date.year());
    let month = query.month.unwrap_or(date.month());
    if !valid_year(year) || !(1..=12).contains(&month) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut store = app.store.lock().unwrap();
    if let Some(base) = &app.config.sync_base {
        import(&app, &mut store, base).map_err(|error| {
            log_error!("community sync import incomplete: {error}");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    }
    let participants = app.legacy_participants();
    refresh_community(&app, &mut store, &participants, now).map_err(store_error)?;
    let language = i18n::selected(&headers, &store, "").map_err(store_error)?;
    let mut dashboard = competition::dashboard(&store, &participants, &app.week, now, year, month)
        .map_err(store_error)?;
    i18n::community(&mut dashboard, &language);
    Ok(Json(dashboard))
}

async fn winners(State(app): State<Arc<App>>) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut store = app.store.lock().unwrap();
    if let Some(base) = &app.config.sync_base {
        import(&app, &mut store, base).map_err(|error| {
            log_error!("winner history sync import incomplete: {error}");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    }
    let participants = app.legacy_participants();
    refresh_community(&app, &mut store, &participants, now_ms()).map_err(store_error)?;
    competition::winner_history(&store, &participants)
        .map(Json)
        .map_err(store_error)
}

async fn get_reminders(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Json<reminders::Settings>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    reminders::settings(&app.store.lock().unwrap(), &user)
        .map(Json)
        .map_err(store_error)
}

async fn set_reminders(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(settings): Json<reminders::Settings>,
) -> Result<Json<reminders::Settings>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !settings.valid() {
        return Err(StatusCode::BAD_REQUEST);
    }
    reminders::save_settings(&mut app.store.lock().unwrap(), &user, &settings)
        .map_err(store_error)?;
    Ok(Json(settings))
}

#[derive(Serialize)]
struct ChallengeList {
    challenges: Vec<challenges::Challenge>,
    recipients: Vec<decks::Recipient>,
    weekly_suggestion: Option<weekly_challenges::Suggestion>,
}

/// The player and everyone they can challenge, with display names.
fn challenge_roster(
    app: &App,
    store: &Store,
    user: &str,
) -> rusqlite::Result<std::collections::BTreeMap<String, String>> {
    let mut roster = app.reachable(store, user)?;
    if app.members().contains(user) {
        roster.insert(user.to_string());
    }
    Ok(roster
        .into_iter()
        .map(|name| (name.clone(), app.display(&name)))
        .collect())
}

type ChallengeFailure = (StatusCode, Json<serde_json::Value>);

fn challenge_error(error: challenges::Error) -> ChallengeFailure {
    let (status, message) = match error {
        challenges::Error::Invalid(message) => (StatusCode::BAD_REQUEST, message),
        challenges::Error::Forbidden => (StatusCode::FORBIDDEN, "This challenge is private."),
        challenges::Error::NotFound => (StatusCode::NOT_FOUND, "Challenge not found."),
        challenges::Error::Storage(error) => {
            log_error!("challenge storage failed: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "The challenge could not be saved. Please try again.",
            )
        }
    };
    (status, Json(serde_json::json!({"error":message})))
}

fn challenge_list(
    app: &App,
    store: &Store,
    user: &str,
    players: &[competition::Participant],
) -> Result<ChallengeList, challenges::Error> {
    let recipients = app.sorted_recipients(app.reachable(store, user)?);
    Ok(ChallengeList {
        challenges: challenges::list(store, user, players, now_ms())?,
        recipients,
        weekly_suggestion: weekly_challenges::suggestion(
            store,
            user,
            &challenge_roster(app, store, user)?,
            &app.week,
            now_ms(),
        )?,
    })
}

async fn get_challenges(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Json<ChallengeList>, ChallengeFailure> {
    if !authorized(&app, &user, &headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"Check your player and token."})),
        ));
    }
    let mut store = app.store.lock().unwrap();
    let participants = app.participants();
    challenges::refresh(&mut store, &participants, now_ms()).map_err(|error| {
        (
            store_error(error),
            Json(serde_json::json!({"error":"Unable to refresh challenges."})),
        )
    })?;
    challenge_list(&app, &store, &user, &participants)
        .map(Json)
        .map_err(challenge_error)
}

async fn create_challenge(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(request): Json<challenges::Create>,
) -> Result<Json<ChallengeList>, ChallengeFailure> {
    if !authorized(&app, &user, &headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"Check your player and token."})),
        ));
    }
    let mut store = app.store.lock().unwrap();
    let eligible = app
        .reachable(&store, &user)
        .map_err(|error| challenge_error(error.into()))?;
    let participants = app.participants();
    challenges::create(&mut store, &user, &request, &eligible, now_ms())
        .map_err(challenge_error)?;
    challenge_list(&app, &store, &user, &participants)
        .map(Json)
        .map_err(challenge_error)
}

async fn act_on_challenge(
    State(app): State<Arc<App>>,
    UrlPath((user, id)): UrlPath<(String, i64)>,
    headers: HeaderMap,
    Json(action): Json<challenges::Action>,
) -> Result<Json<ChallengeList>, ChallengeFailure> {
    if !authorized(&app, &user, &headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"Check your player and token."})),
        ));
    }
    let mut store = app.store.lock().unwrap();
    let participants = app.participants();
    challenges::act(&mut store, &user, id, &action.action, now_ms()).map_err(challenge_error)?;
    challenges::refresh(&mut store, &participants, now_ms()).map_err(|error| {
        (
            store_error(error),
            Json(serde_json::json!({"error":"Unable to refresh challenges."})),
        )
    })?;
    challenge_list(&app, &store, &user, &participants)
        .map(Json)
        .map_err(challenge_error)
}

async fn act_on_weekly_challenge(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(action): Json<weekly_challenges::Action>,
) -> Result<Json<ChallengeList>, ChallengeFailure> {
    if !authorized(&app, &user, &headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"Check your player and token."})),
        ));
    }
    let mut store = app.store.lock().unwrap();
    let participants = app.participants();
    let roster =
        challenge_roster(&app, &store, &user).map_err(|error| challenge_error(error.into()))?;
    weekly_challenges::act(&mut store, &user, &action, &roster, &app.week, now_ms())
        .map_err(challenge_error)?;
    challenge_list(&app, &store, &user, &participants)
        .map(Json)
        .map_err(challenge_error)
}

/// The dashboard is one page; `/day`, `/month` and the rest pick a leaderboard period.
async fn period_page(UrlPath(period): UrlPath<String>) -> Result<Html<&'static str>, StatusCode> {
    if Periods::NAMES.contains(&period.as_str()) {
        Ok(index().await)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn manifest() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        include_str!("../static/manifest.webmanifest"),
    )
}

async fn icon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        include_str!("../static/icon.svg"),
    )
}

/// The listening socket systemd passes when a `.socket` unit starts the server.
/// The port then belongs to the socket unit, so the service's own address
/// rules can shut it out of loopback without refusing nginx.
#[cfg(unix)]
fn inherited_listener() -> Option<std::net::TcpListener> {
    use std::os::fd::FromRawFd;
    let pid: u32 = std::env::var("LISTEN_PID").ok()?.parse().ok()?;
    if pid != std::process::id() || std::env::var("LISTEN_FDS").ok()? != "1" {
        return None;
    }
    // SAFETY: systemd passes exactly one descriptor, at 3, and nothing else here owns it.
    Some(unsafe { std::net::TcpListener::from_raw_fd(3) })
}

#[cfg(not(unix))]
fn inherited_listener() -> Option<std::net::TcpListener> {
    None
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let admin_at = args.iter().position(|a| a == "admin").filter(|&i| i <= 1);
    let (given, message) = match admin_at {
        Some(at) => (args[..at].first().cloned(), None),
        None => parse_args(&args)?,
    };
    let path = given
        .or_else(|| std::env::var("ANKIQUEST_CONFIG").ok())
        .unwrap_or_else(|| "ankiquest.json".into());
    let mut config = Config::read(&path)?;

    if let Some(at) = admin_at {
        let command = match &args[at + 1..] {
            [] => None,
            [help] if matches!(help.as_str(), "help" | "--help" | "-h") => None,
            rest => Some(admin::parse(rest)),
        };
        let output = match command {
            None => Ok(format!("{}\n", admin::USAGE)),
            Some(command) => {
                command.and_then(|c| admin::run(&config.state_dir, config.settings(), c, now_ms()))
            }
        };
        match output {
            Ok(output) => print!("{output}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        }
        return Ok(());
    }

    game::set_review_weighting(config.review_weighting);

    config.check()?;

    // Sending needs no tokens, and the running service holds the only readable copy.
    if let Some(message) = message {
        return send_message(&config, &message);
    }

    config.load_tokens()?;

    config.oauth.load_secrets(config.public_url.as_deref())?;
    let access = access::Access::from_config(&config)?;
    let store = Store::open(&config.state_dir)?;
    game::set_streak_rule(admin::streak_rule(
        &store.conn,
        config.settings().effective(&store.conn)?.streak_minutes,
        now_ms(),
    )?);
    let accounts = accounts::Directory::load(&store.conn)?;
    let mut players = HashMap::new();
    for user in store.users()? {
        players.insert(user.clone(), load_player(&store, &user)?);
    }
    let week = config.week()?;
    let app = Arc::new(App {
        week,
        config,
        access,
        accounts,
        players: RwLock::new(players),
        standings: Default::default(),
        store: Mutex::new(store),
    });

    worker::spawn(app.clone());

    let router = router(app.clone());

    let listener = match inherited_listener() {
        Some(listener) => {
            listener.set_nonblocking(true)?;
            tokio::net::TcpListener::from_std(listener)?
        }
        None => tokio::net::TcpListener::bind(&app.config.addr).await?,
    };
    println!("ankiquest listening on http://{}", listener.local_addr()?);
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .merge(aki::routes())
        .merge(avatars::routes())
        .merge(friend_nudges::routes())
        .merge(shares::routes())
        .merge(conquests::routes())
        .merge(social::routes())
        .merge(privacy::routes())
        .merge(dev::routes())
        .merge(oauth::routes())
        .merge(decks::routes())
        .merge(subscriptions::routes())
        .merge(deck_copies::routes())
        .merge(freezes::routes())
        .merge(inbox::routes())
        .merge(study::routes())
        .merge(boards::routes())
        .route("/", get(index))
        .route("/records", get(index))
        .route("/community", get(community_page))
        .route("/today", get(personal_page))
        .route("/history", get(personal_page))
        .route("/settings", get(personal_page))
        .route("/friends", get(personal_page))
        .route("/join/{code}", get(personal_page))
        .route("/{period}", get(period_page))
        .route("/manifest.webmanifest", get(manifest))
        .route("/icon.svg", get(icon))
        .route("/site.css", get(access::site_css))
        .route("/site.js", get(access::site_js))
        .route("/i18n.wasm", get(access::i18n_wasm))
        .route("/personal.css", get(access::personal_css))
        .route("/personal.js", get(access::personal_js))
        .route("/login", get(access::login))
        .route("/auth/status", get(access::status))
        .route(
            "/auth/session",
            post(access::session).layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route("/auth/logout", post(access::logout))
        .route(
            "/api/accounts",
            post(accounts::signup).layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route(
            "/api/accounts/tokens",
            post(accounts::login)
                .delete(accounts::logout)
                .layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route("/api/winners", get(winners))
        .route("/api/community", get(community_dashboard))
        .route(
            "/api/community/reminders/{user}",
            get(get_reminders).post(set_reminders),
        )
        .route(
            "/api/community/challenges/{user}",
            get(get_challenges).post(create_challenge),
        )
        .route(
            "/api/community/challenges/{user}/{id}",
            post(act_on_challenge),
        )
        .route(
            "/api/community/challenges/{user}/weekly",
            post(act_on_weekly_challenge),
        )
        .route(
            "/api/companion/{user}",
            get(companions::get).post(companions::set),
        )
        .route("/api/language/{user}", get(i18n::get).post(i18n::set))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            access::gate,
        ))
        .with_state(app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decks::{get_decks, set_decks};
    use crate::game::Week;
    use crate::inbox::{ReplyRequest, get_notifications, reply};
    use crate::study::{profile, upload};
    use crate::subscriptions::{get_incoming_preferences, set_incoming_preferences};
    use crate::test_support::*;
    use crate::worker::{sync_users, tick};
    use std::path::PathBuf;

    #[tokio::test]
    async fn companion_choice_is_private_validated_and_independent_for_each_player() {
        let (app, path) = fixture();
        let get = |user: &str, token: &str| {
            companions::get(State(app.clone()), UrlPath(user.into()), headers(token))
        };
        assert_eq!(get("cerro", "cerro").await.unwrap().0.companion, "aki");
        assert_eq!(
            get("cerro", "hill").await.err(),
            Some(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            companions::set(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(companions::Preference {
                    companion: "unknown".into()
                })
            )
            .await
            .err(),
            Some(StatusCode::BAD_REQUEST),
        );
        assert_eq!(
            companions::set(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("hill"),
                Json(companions::Preference {
                    companion: "none".into()
                })
            )
            .await
            .err(),
            Some(StatusCode::UNAUTHORIZED),
        );
        let _ = companions::set(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(companions::Preference {
                companion: "ankilope".into(),
            }),
        )
        .await
        .unwrap();
        let _ = companions::set(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(companions::Preference {
                companion: "none".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(get("cerro", "cerro").await.unwrap().0.companion, "ankilope");
        assert_eq!(get("hill", "hill").await.unwrap().0.companion, "none");
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn languages_require_owner_auth_and_public_views_do_not_change_preferences() {
        let (app, _path) = fixture();
        let preference = || {
            Json(i18n::Preference {
                language: "es-AR".into(),
            })
        };
        assert_eq!(
            i18n::set(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("hill"),
                preference()
            )
            .await
            .err()
            .unwrap(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            i18n::get(
                State(app.clone()),
                UrlPath("cerro".into()),
                HeaderMap::new()
            )
            .await
            .err()
            .unwrap(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            i18n::set(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                preference()
            )
            .await
            .unwrap()
            .0
            .language,
            "es"
        );
        assert_eq!(
            i18n::set(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(i18n::Preference {
                    language: "es\r\nx".into()
                })
            )
            .await
            .err()
            .unwrap(),
            StatusCode::BAD_REQUEST
        );
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(2, false)),
        )
        .await
        .unwrap();
        let mut public = HeaderMap::new();
        public.insert(header::ACCEPT_LANGUAGE, "en-US".parse().unwrap());
        let viewed = profile(State(app.clone()), UrlPath("cerro".into()), public)
            .await
            .unwrap()
            .0;
        assert!(
            viewed
                .quests
                .iter()
                .any(|quest| quest.title.contains("Review"))
        );
        assert_eq!(app.store.lock().unwrap().language("cerro").unwrap(), "es");
        let mut owner = headers("cerro");
        owner.insert(header::ACCEPT_LANGUAGE, "en-US".parse().unwrap());
        let _ = profile(State(app.clone()), UrlPath("cerro".into()), owner)
            .await
            .unwrap();
        assert_eq!(app.store.lock().unwrap().language("cerro").unwrap(), "en");
        assert_eq!(app.store.lock().unwrap().language("hill").unwrap(), "en");
    }

    #[tokio::test]
    async fn community_reminder_preferences_require_owner_auth_and_validate_before_saving() {
        let (app, path) = fixture();
        assert_eq!(
            get_reminders(
                State(app.clone()),
                UrlPath("cerro".into()),
                HeaderMap::new()
            )
            .await
            .err()
            .unwrap(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            get_reminders(State(app.clone()), UrlPath("cerro".into()), headers("hill"))
                .await
                .err()
                .unwrap(),
            StatusCode::UNAUTHORIZED
        );
        let prefs = get_reminders(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
        )
        .await
        .unwrap()
        .0;
        assert!(!prefs.urgent_streak);
        let mut enabled = prefs.clone();
        enabled.urgent_streak = true;
        let _ = set_reminders(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(enabled.clone()),
        )
        .await
        .unwrap();
        let mut invalid = enabled.clone();
        invalid.quiet_start = 24;
        assert_eq!(
            set_reminders(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(invalid)
            )
            .await
            .err()
            .unwrap(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            get_reminders(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro")
            )
            .await
            .unwrap()
            .0,
            enabled
        );
        assert!(
            !get_reminders(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0
                .urgent_streak
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn weekly_suggestions_require_the_owner_and_cannot_choose_arbitrary_recipients() {
        let (app, path) = fixture();
        for auth in [headers("hill"), HeaderMap::new()] {
            assert_eq!(
                get_challenges(State(app.clone()), UrlPath("cerro".into()), auth.clone())
                    .await
                    .err()
                    .unwrap()
                    .0,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                act_on_weekly_challenge(
                    State(app.clone()),
                    UrlPath("cerro".into()),
                    auth,
                    Json(weekly_challenges::Action {
                        week_start: 0,
                        action: "invite".into()
                    })
                )
                .await
                .err()
                .unwrap()
                .0,
                StatusCode::UNAUTHORIZED
            );
        }
        let draft = get_challenges(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
        )
        .await
        .unwrap()
        .0
        .weekly_suggestion
        .unwrap();
        let action = || weekly_challenges::Action {
            week_start: draft.week_start,
            action: "invite".into(),
        };
        let goal = act_on_weekly_challenge(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(action()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(goal.challenges.len(), 1);
        assert_eq!(goal.challenges[0].status, "waiting");
        assert!(
            goal.challenges[0]
                .members
                .iter()
                .any(|m| m.user == draft.friend.user && m.status == "invited")
        );
        assert_eq!(
            act_on_weekly_challenge(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(action())
            )
            .await
            .unwrap()
            .0
            .challenges
            .len(),
            1
        );
        assert!(serde_json::from_value::<weekly_challenges::Action>(serde_json::json!({"week_start":draft.week_start,"action":"invite","recipient":"someone-else"})).is_err());
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn community_challenges_are_owner_authenticated_and_membership_is_opt_in() {
        let (app, path) = fixture();
        let request = || challenges::Create {
            request_id: None,
            title: "Five study days".into(),
            kind: challenges::Kind::StudyDays,
            cooperative: false,
            target: 5,
            duration_days: 7,
            recipients: vec!["hill".into()],
            start_when_ready: false,
        };
        assert_eq!(
            create_challenge(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("hill"),
                Json(request())
            )
            .await
            .err()
            .unwrap()
            .0,
            StatusCode::UNAUTHORIZED
        );
        let created = create_challenge(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(request()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(created.challenges.len(), 1);
        let id = created.challenges[0].id;
        assert_eq!(
            created.challenges[0]
                .members
                .iter()
                .find(|m| m.user == "hill")
                .unwrap()
                .status,
            "invited"
        );
        assert!(
            get_challenges(
                State(app.clone()),
                UrlPath("friend".into()),
                headers("friend")
            )
            .await
            .unwrap()
            .0
            .challenges
            .is_empty()
        );
        assert_eq!(
            act_on_challenge(
                State(app.clone()),
                UrlPath(("friend".into(), id)),
                headers("friend"),
                Json(challenges::Action {
                    action: "accept".into()
                })
            )
            .await
            .err()
            .unwrap()
            .0,
            StatusCode::FORBIDDEN
        );
        let accepted = act_on_challenge(
            State(app.clone()),
            UrlPath(("hill".into(), id)),
            headers("hill"),
            Json(challenges::Action {
                action: "accept".into(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            accepted.challenges[0]
                .members
                .iter()
                .find(|m| m.user == "hill")
                .unwrap()
                .status,
            "accepted"
        );
        assert_eq!(
            act_on_challenge(
                State(app.clone()),
                UrlPath(("hill".into(), id)),
                headers("hill"),
                Json(challenges::Action {
                    action: "cancel".into()
                })
            )
            .await
            .err()
            .unwrap()
            .0,
            StatusCode::FORBIDDEN
        );
        let cancelled = act_on_challenge(
            State(app.clone()),
            UrlPath(("cerro".into(), id)),
            headers("cerro"),
            Json(challenges::Action {
                action: "cancel".into(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(cancelled.challenges[0].status, "cancelled");
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn community_empty_history_has_no_invented_champions_and_rejects_bad_filters() {
        let (app, path) = fixture();
        let board = community_dashboard(
            State(app.clone()),
            Query(CommunityQuery::default()),
            HeaderMap::new(),
        )
        .await
        .unwrap()
        .0;
        assert!(board["calendar"].as_array().unwrap().is_empty());
        assert_eq!(board["players"].as_array().unwrap().len(), 3);
        assert_eq!(
            community_dashboard(
                State(app.clone()),
                Query(CommunityQuery {
                    year: Some(2026),
                    month: Some(13)
                }),
                HeaderMap::new()
            )
            .await
            .err()
            .unwrap(),
            StatusCode::BAD_REQUEST
        );
        assert!(serde_json::from_value::<challenges::Create>(serde_json::json!({"title":"No injected creator","kind":"reviews","cooperative":true,"target":10,"duration_days":7,"recipients":["hill"],"creator":"hill"})).is_err());
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    fn community_sync_fixture() -> (Arc<App>, PathBuf, PathBuf) {
        let (mut app, path) = fixture();
        let base = path.join("sync");
        std::fs::create_dir_all(&base).unwrap();
        Arc::get_mut(&mut app).unwrap().config.sync_base = Some(base.clone());
        (app, path, base)
    }

    fn community_history(count: i64) -> (chrono::NaiveDate, Vec<Review>) {
        let day = Week::default().competition_date(now_ms()) - chrono::Duration::days(3);
        let at = Week::default().boundary_ms(day) + 3_600_000;
        let reviews = (0..count)
            .map(|index| Review {
                id: at + index * 1000,
                cid: at + index,
                last_ivl: 10,
                ivl: None,
                time_ms: 10_000,
                kind: 1,
            })
            .collect();
        (day, reviews)
    }

    fn community_collection(base: &Path, user: &str, reviews: &[Review]) {
        std::fs::create_dir_all(base.join(user)).unwrap();
        let mut conn =
            rusqlite::Connection::open(base.join(user).join("collection.anki2")).unwrap();
        conn.execute_batch(
            "create table revlog (id integer primary key, cid integer, lastIvl integer,
                 ivl integer, time integer, type integer, ease integer);",
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        for review in reviews {
            tx.execute(
                "insert into revlog values (?1,?2,?3,?4,?5,?6,3)",
                rusqlite::params![
                    review.id,
                    review.cid,
                    review.last_ivl,
                    review.ivl.unwrap_or(1),
                    review.time_ms,
                    review.kind
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }

    fn community_query(day: chrono::NaiveDate) -> Query<CommunityQuery> {
        Query(CommunityQuery {
            year: Some(day.year()),
            month: Some(day.month()),
        })
    }

    #[tokio::test]
    async fn community_first_request_imports_fresh_sync_before_finalizing_stale_history() {
        let (app, path, base) = community_sync_fixture();
        let (day, reviews) = community_history(100);
        {
            let mut store = app.store.lock().unwrap();
            for (user, count) in [("hill", 1), ("cerro", 2)] {
                store
                    .upsert(user, &reviews[..count], &[], Clock::default())
                    .unwrap();
                app.players
                    .write()
                    .unwrap()
                    .insert(user.into(), load_player(&store, user).unwrap());
            }
        }
        community_collection(&base, "hill", &reviews);

        let board = community_dashboard(State(app.clone()), community_query(day), HeaderMap::new())
            .await
            .unwrap()
            .0;
        let archived = board["calendar"]
            .as_array()
            .unwrap()
            .iter()
            .find(|period| period["key"] == day.to_string())
            .unwrap();
        assert_eq!(archived["status"], "final");
        assert_eq!(archived["winners"], serde_json::json!(["hill"]));
        assert_eq!(archived["standings"][0]["reviews"], 100);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn winners_imports_before_archiving_and_matches_community_history() {
        let (app, path, base) = community_sync_fixture();
        let (day, reviews) = community_history(30);
        {
            let mut store = app.store.lock().unwrap();
            for (user, count) in [("hill", 1), ("cerro", 2)] {
                store
                    .upsert(user, &reviews[..count], &[], Clock::default())
                    .unwrap();
                app.players
                    .write()
                    .unwrap()
                    .insert(user.into(), load_player(&store, user).unwrap());
            }
        }
        community_collection(&base, "hill", &reviews);
        let history = winners(State(app.clone())).await.unwrap().0;
        assert_eq!(history["shared"]["start_date"], day.to_string());
        assert_eq!(history["shared"]["player_count"], 2);
        assert_eq!(history["shared"]["periods"]["day"], 1);
        let champion = history["shared"]["players"]
            .as_array()
            .unwrap()
            .iter()
            .find(|player| player["user"] == "hill")
            .unwrap();
        assert_eq!(champion["day_wins"], 1);
        assert_eq!(history["waiting_players"][0]["user"], "friend");
        let board = community_dashboard(State(app.clone()), community_query(day), HeaderMap::new())
            .await
            .unwrap()
            .0;
        assert_eq!(board["winner_history"], history);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn community_failed_import_defers_archive_until_every_collection_recovers() {
        let (app, path, base) = community_sync_fixture();
        let (day, reviews) = community_history(100);
        community_collection(&base, "cerro", &reviews[..2]);
        for user in ["hill", "friend"] {
            std::fs::create_dir_all(base.join(user)).unwrap();
            std::fs::write(
                base.join(user).join("collection.anki2"),
                b"invalid database",
            )
            .unwrap();
        }
        {
            let mut store = app.store.lock().unwrap();
            let error = import(&app, &mut store, &base).unwrap_err().to_string();
            assert!(error.contains("hill"));
            assert!(error.contains("friend"));
            assert_eq!(store.reviews("cerro").unwrap().len(), 2);
        }
        assert_eq!(
            community_dashboard(State(app.clone()), community_query(day), HeaderMap::new())
                .await
                .unwrap_err(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        tick(&app).unwrap();
        {
            let store = app.store.lock().unwrap();
            for table in [
                "competition_meta",
                "competition_periods",
                "community_refresh",
            ] {
                assert_eq!(
                    store
                        .conn
                        .query_row(&format!("select count(*) from {table}"), [], |row| {
                            row.get::<_, i64>(0)
                        })
                        .unwrap(),
                    0
                );
            }
        }
        for (user, count) in [("hill", 100), ("friend", 1)] {
            std::fs::remove_file(base.join(user).join("collection.anki2")).unwrap();
            community_collection(&base, user, &reviews[..count]);
        }
        let board = community_dashboard(State(app.clone()), community_query(day), HeaderMap::new())
            .await
            .unwrap()
            .0;
        let archived = board["calendar"]
            .as_array()
            .unwrap()
            .iter()
            .find(|period| period["key"] == day.to_string())
            .unwrap();
        assert_eq!(archived["status"], "final");
        assert_eq!(archived["winners"], serde_json::json!(["hill"]));
        assert_eq!(archived["standings"][0]["reviews"], 100);
        assert_eq!(archived["standings"].as_array().unwrap().len(), 3);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn community_unreadable_sync_directory_never_initializes_archive() {
        let (mut app, path, base) = community_sync_fixture();
        Arc::get_mut(&mut app).unwrap().config.sync_base = Some(base.join("missing"));
        assert!(sync_users(&base.join("missing")).is_err());
        assert_eq!(
            winners(State(app.clone())).await.unwrap_err(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            community_dashboard(
                State(app.clone()),
                Query(CommunityQuery::default()),
                HeaderMap::new()
            )
            .await
            .unwrap_err(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        tick(&app).unwrap();
        assert_eq!(
            app.store
                .lock()
                .unwrap()
                .conn
                .query_row("select count(*) from competition_periods", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn empty_configured_token_never_authorizes_private_data() {
        let (mut app, path) = fixture();
        Arc::get_mut(&mut app)
            .unwrap()
            .config
            .users
            .get_mut("cerro")
            .unwrap()
            .token = Some(String::new());
        assert!(!authorized(&app, "cerro", &HeaderMap::new()));
        let mut empty_bearer = HeaderMap::new();
        empty_bearer.insert(header::AUTHORIZATION, "Bearer ".parse().unwrap());
        assert!(!authorized(&app, "cerro", &empty_bearer));
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn all_private_handlers_reject_missing_and_other_users_tokens() {
        let (app, path) = fixture();
        for auth in [HeaderMap::new(), headers("hill")] {
            assert_eq!(
                get_incoming_preferences(State(app.clone()), UrlPath("cerro".into()), auth.clone())
                    .await
                    .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                set_incoming_preferences(
                    State(app.clone()),
                    UrlPath("cerro".into()),
                    auth.clone(),
                    Json(decks::IncomingSettings {
                        enabled: false,
                        muted_senders: vec![]
                    })
                )
                .await
                .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                get_decks(State(app.clone()), UrlPath("cerro".into()), auth.clone())
                    .await
                    .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                set_decks(
                    State(app.clone()),
                    UrlPath("cerro".into()),
                    auth.clone(),
                    Json(preferences(&["hill"]))
                )
                .await
                .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                get_notifications(State(app.clone()), UrlPath("cerro".into()), auth.clone())
                    .await
                    .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                reply(
                    State(app.clone()),
                    UrlPath("cerro".into()),
                    auth.clone(),
                    Json(ReplyRequest {
                        notification: 1,
                        message: "Good job!".into()
                    })
                )
                .await
                .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                upload(
                    State(app.clone()),
                    UrlPath("cerro".into()),
                    auth,
                    Json(sample_upload(0, false))
                )
                .await
                .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert!(app.store.lock().unwrap().decks("cerro").unwrap().is_empty());
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn authenticated_upload_preferences_and_inbox_end_to_end() {
        let (app, path) = fixture();
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(2, false)),
        )
        .await
        .unwrap();
        let settings = get_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
        )
        .await
        .unwrap()
        .0;
        assert!(!settings.decks[0].enabled);
        assert_eq!(settings.decks[0].name, "Spanish");
        assert_eq!(
            settings
                .recipients
                .iter()
                .map(|r| r.user.as_str())
                .collect::<Vec<_>>(),
            vec!["friend", "hill"]
        );
        assert_eq!(settings.recipients[1].display, "Hill");
        let _ = set_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(preferences(&["hill"])),
        )
        .await
        .unwrap();
        for _ in 0..2 {
            let _ = upload(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(sample_upload(0, false)),
            )
            .await
            .unwrap();
        }
        let inbox = get_notifications(State(app.clone()), UrlPath("hill".into()), headers("hill"))
            .await
            .unwrap()
            .0;
        assert_eq!(inbox.len(), 1);
        assert_eq!(
            inbox[0].body,
            "Cerro has finished their Spanish studies for today."
        );
        assert!(inbox[0].created_at <= now_ms() / 1000);
        assert!(
            get_notifications(
                State(app.clone()),
                UrlPath("friend".into()),
                headers("friend")
            )
            .await
            .unwrap()
            .0
            .is_empty()
        );
        assert!(
            get_decks(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0
                .decks
                .is_empty()
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    /// A card forgotten four times that then earns a 30-day interval, ending a minute ago.
    fn hard_card(cid: i64) -> Vec<Review> {
        let mut answers = vec![(0, 0, 1)];
        for _ in 0..4 {
            answers.extend([(1, 3, -600), (2, -600, 1)]);
        }
        answers.push((1, 1, 30));
        let start = now_ms() - 60_000 - answers.len() as i64 * 1000;
        answers
            .iter()
            .enumerate()
            .map(|(i, &(kind, last_ivl, ivl))| Review {
                id: start + i as i64 * 1000,
                cid,
                last_ivl,
                ivl: Some(ivl),
                time_ms: 8_000,
                kind,
            })
            .collect()
    }

    #[tokio::test]
    async fn only_the_owner_labels_a_conquered_card_and_friends_hear_what_it_was() {
        let (app, path) = fixture();
        upload_as(
            &app,
            "hill",
            plain_upload(reviews_at(now_ms() - 3_600_000, 2)),
        )
        .await;
        let rows = hard_card(78);
        let at = rows.last().unwrap().id;
        upload_as(&app, "hill", plain_upload(rows)).await;
        app.store
            .lock()
            .unwrap()
            .conn
            .execute("insert into friendships (a, b, requested_by, accepted, created_at) values ('cerro', 'hill', 'hill', 1, 0)", [])
            .unwrap();
        let set = |user: &str, at: i64, label: &str| {
            conquests::set_label(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers(user),
                Json(conquests::LabelUpdate {
                    label: label.into(),
                }),
            )
        };
        let show = || {
            conquests::show(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers("cerro"),
            )
        };
        assert_eq!(
            show().await.unwrap().0.label,
            None,
            "nothing is shown until chosen"
        );
        assert_eq!(
            set("cerro", at, "das Eichhörnchen").await.err(),
            Some(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            set("hill", at, " \n\t ").await.err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            set("hill", at, &"x".repeat(conquests::MAX_LABEL + 1))
                .await
                .err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            set("hill", at + 1, "das Eichhörnchen").await.err(),
            Some(StatusCode::NOT_FOUND),
            "only a card that was conquered"
        );
        assert_eq!(
            set("hill", at, "  das\nEichhörnchen\u{7}  ").await.unwrap(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            show().await.unwrap().0.label.as_deref(),
            Some("das Eichhörnchen"),
            "one plain line"
        );

        let told = conquests::tell(
            State(app.clone()),
            UrlPath(("hill".into(), at)),
            headers("hill"),
        )
        .await
        .unwrap();
        assert_eq!(told.0.told, 1);
        let inbox = app
            .store
            .lock()
            .unwrap()
            .notifications("cerro", now_ms())
            .unwrap();
        let told = inbox.iter().find(|n| n.kind == "conquest").unwrap();
        assert_eq!(
            told.body,
            "They forgot “das Eichhörnchen” 4 times and learned it anyway."
        );

        let remove = |user: &str| {
            conquests::remove_label(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers(user),
            )
        };
        assert_eq!(remove("cerro").await.err(), Some(StatusCode::UNAUTHORIZED));
        assert_eq!(remove("hill").await.unwrap(), StatusCode::NO_CONTENT);
        assert_eq!(show().await.unwrap().0.label, None);

        set("hill", at, "der Igel").await.unwrap();
        let store = app.store.lock().unwrap();
        privacy::forget(&store.conn, "hill").unwrap();
        let left: i64 = store
            .conn
            .query_row(
                "select (select count(*) from conquest_labels)
                      + (select count(*) from seen where key like 'conquest:hill:%')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            left, 0,
            "deleting the account takes labels and friends' told-markers"
        );
        drop(store);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn a_conquered_card_is_announced_shared_and_told_to_friends_once() {
        let (app, path) = fixture();
        upload_as(
            &app,
            "hill",
            plain_upload(reviews_at(now_ms() - 3_600_000, 2)),
        )
        .await;
        let rows = hard_card(77);
        let at = rows.last().unwrap().id;
        let response = upload_as(&app, "hill", plain_upload(rows.clone())).await;
        assert_eq!(response.profile.conquests.len(), 1);
        assert!(
            response
                .feedback
                .headlines
                .contains(&"🏆 Card conquered after 4 lapses".to_string()),
            "{:?}",
            response.feedback
        );
        let resent = upload_as(&app, "hill", plain_upload(rows)).await;
        assert!(
            !resent
                .feedback
                .headlines
                .iter()
                .any(|h| h.starts_with('🏆')),
            "resending history does not announce it again"
        );

        app.store
            .lock()
            .unwrap()
            .conn
            .execute("insert into friendships (a, b, requested_by, accepted, created_at) values ('cerro', 'hill', 'hill', 1, 0)", [])
            .unwrap();
        let show = |viewer: &str, at: i64| {
            conquests::show(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers(viewer),
            )
        };
        let mine = show("hill", at).await.unwrap().0;
        assert_eq!(
            (mine.conquest.lapses, mine.told, mine.friends),
            (4, Some(false), Some(1))
        );
        let theirs = show("cerro", at).await.unwrap().0;
        assert_eq!(
            (theirs.told, theirs.friends),
            (None, None),
            "only the owner sees sharing state"
        );
        assert_eq!(
            show("hill", at + 1).await.err(),
            Some(StatusCode::NOT_FOUND)
        );

        let tell = |user: &str| {
            conquests::tell(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers(user),
            )
        };
        assert_eq!(tell("cerro").await.err(), Some(StatusCode::UNAUTHORIZED));
        assert_eq!(tell("hill").await.unwrap().0.told, 1);
        assert_eq!(tell("hill").await.unwrap().0.told, 1);
        let inbox = app
            .store
            .lock()
            .unwrap()
            .notifications("cerro", now_ms())
            .unwrap();
        let told: Vec<_> = inbox.iter().filter(|n| n.kind == "conquest").collect();
        assert_eq!(told.len(), 1, "telling twice does not repeat it: {inbox:?}");
        assert_eq!(told[0].title, "Hill conquered a hard card");
        assert_eq!(show("hill", at).await.unwrap().0.told, Some(true));
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn clients_share_the_study_day_and_review_format() {
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/clients.json")).unwrap();
        for case in fixtures["study_day"].as_array().unwrap() {
            let clock = Clock {
                offset_west_min: case["offset_west_min"].as_i64().unwrap(),
                rollover_hour: case["rollover_hour"].as_i64().unwrap(),
            };
            assert_eq!(
                clock.day(case["now_ms"].as_i64().unwrap()),
                case["day"].as_i64().unwrap(),
                "{case}"
            );
        }
        for case in fixtures["review_row"].as_array().unwrap() {
            let review: Review = serde_json::from_value(case["review"].clone()).unwrap();
            let row: Vec<i64> = serde_json::from_value(case["row"].clone()).unwrap();
            assert_eq!(
                (
                    review.id,
                    review.cid,
                    review.last_ivl,
                    review.ivl,
                    review.time_ms,
                    i64::from(review.kind)
                ),
                (row[0], row[1], row[2], Some(row[3]), row[4], row[5])
            );
        }
    }
}
