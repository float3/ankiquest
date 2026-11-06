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
#[cfg(test)]
mod api_examples;
mod app;
mod avatars;
mod boards;
mod capabilities;
mod cards;
mod challenges;
mod cli;
mod companions;
mod competition;
mod config;
mod conquests;
#[cfg(test)]
mod day_verdicts;
mod deck_copies;
mod decks;
mod dev;
mod feedback;
mod freezes;
mod friend_nudges;
mod game;
mod goals;
mod i18n;
mod inbox;
mod migrations;
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
mod wins;
mod worker;

use app::{App, load_player, now_ms};
use axum::Router;
use axum::extract::Path as UrlPath;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use cli::{parse_args, send_message};
use config::Config;
use game::Periods;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use store::{Error, Store};
use web_util::store_error;

async fn index() -> Html<&'static str> {
    Html(web!("index.html"))
}

async fn community_page() -> Html<&'static str> {
    Html(web!("community.html"))
}

async fn personal_page() -> Html<&'static str> {
    Html(web!("personal.html"))
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
            Some(Ok(admin::Command::AdoptPlayers)) => config.load_tokens().and_then(|()| {
                let players: Vec<admin::Configured> = config
                    .users
                    .iter()
                    .map(|(user, c)| admin::Configured {
                        user: user.clone(),
                        display: c.display.clone(),
                        token: c.token.clone(),
                    })
                    .collect();
                admin::adopt(&config.state_dir, &players, now_ms())
            }),
            Some(Ok(command)) if command.is_about_people() => {
                let backups = std::env::var_os("ANKIQUEST_BACKUP_DIR")
                    .map_or_else(|| config.state_dir.join("backups"), Into::into);
                admin::people(
                    &admin::Setup {
                        state_dir: &config.state_dir,
                        backups: &backups,
                        configured: config.users.keys().cloned().collect(),
                        sync_base: config.sync_base.as_deref(),
                    },
                    command,
                    now_ms(),
                )
            }
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
        heartbeat: Default::default(),
        rank_weeks: Default::default(),
        store: Mutex::new(store),
    });

    privacy::warn_about_unknown_players(&app);
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
        .merge(wins::routes())
        .merge(goals::routes())
        .merge(conquests::routes())
        .merge(social::routes())
        .merge(privacy::routes())
        .merge(dev::routes())
        .merge(oauth::routes())
        .merge(competition::routes())
        .merge(challenges::routes())
        .merge(reminders::routes())
        .merge(decks::routes())
        .merge(subscriptions::routes())
        .merge(deck_copies::routes())
        .merge(freezes::routes())
        .merge(inbox::routes())
        .merge(study::routes())
        .merge(boards::routes())
        .merge(cards::routes())
        .merge(capabilities::routes())
        .route("/api/health", get(worker::health))
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
        .route(
            "/api/devices/{user}",
            get(accounts::list_devices)
                .post(accounts::change_devices)
                .layer(axum::extract::DefaultBodyLimit::max(4096)),
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
    use crate::access::authorized;
    use crate::decks::{get_decks, set_decks};
    use crate::inbox::{ReplyRequest, get_notifications, reply};
    use crate::study::{profile, upload};
    use crate::subscriptions::get_incoming_preferences;
    use crate::test_support::*;

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
        cleanup(app, path);
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
        cleanup(app, path);
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
        cleanup(app, path);
    }

    #[tokio::test]
    async fn authenticated_upload_preferences_and_inbox_end_to_end() {
        let (app, path) = fixture();
        crate::test_support::used_app(&app, &["cerro", "hill", "friend"]);
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
        cleanup(app, path);
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
        cleanup(app, path);
    }

    #[tokio::test]
    async fn labels_reach_strangers_only_from_profiles_chosen_public() {
        let (app, path) = fixture();
        crate::accounts::create(&app.store.lock().unwrap().conn, "ana", "Ana", "", 0).unwrap();
        app.accounts.insert("ana", "Ana");
        let token = app.issue_token("ana", "Phone").unwrap();
        let mut ana = HeaderMap::new();
        ana.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let rows = hard_card(79);
        let at = rows.last().unwrap().id;
        let _ = upload(
            State(app.clone()),
            UrlPath("ana".into()),
            ana.clone(),
            Json(plain_upload(rows)),
        )
        .await
        .unwrap();
        conquests::set_label(
            State(app.clone()),
            UrlPath(("ana".into(), at)),
            ana.clone(),
            Json(conquests::LabelUpdate {
                label: "la ardilla".into(),
            }),
        )
        .await
        .unwrap();
        let label = |viewer: HeaderMap| async {
            conquests::show(State(app.clone()), UrlPath(("ana".into(), at)), viewer)
                .await
                .unwrap()
                .0
                .label
        };
        assert_eq!(
            label(HeaderMap::new()).await,
            None,
            "the conquest is public by default, what the card was is not"
        );
        assert_eq!(label(headers("hill")).await, None);
        assert_eq!(label(ana.clone()).await.as_deref(), Some("la ardilla"));
        app.store
            .lock()
            .unwrap()
            .conn
            .execute("insert into friendships (a, b, requested_by, accepted, created_at) values ('ana', 'hill', 'ana', 1, 0)", [])
            .unwrap();
        assert_eq!(label(headers("hill")).await.as_deref(), Some("la ardilla"));
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "insert into visibility (user, public) values ('ana', 1)",
                [],
            )
            .unwrap();
        assert_eq!(
            label(HeaderMap::new()).await.as_deref(),
            Some("la ardilla"),
            "a profile its player made public shares labels as before"
        );
        cleanup(app, path);
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
                .iter()
                .any(|h| *h == "🏆 Card conquered after 4 lapses"),
            "{:?}",
            response.feedback
        );
        let resent = upload_as(&app, "hill", plain_upload(rows)).await;
        assert!(
            !resent
                .feedback
                .headlines
                .iter()
                .any(|h| h.to_string().starts_with('🏆')),
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
        assert_eq!(mine.win, None, "no win until the worker keeps one");
        let win = crate::wins::create(
            &app.store.lock().unwrap().conn,
            "hill",
            &format!("conquered:{at}"),
            0,
            now_ms(),
            "Card conquered",
            "",
            &serde_json::json!({}),
        )
        .unwrap();
        assert!(win.is_some());
        assert_eq!(show("hill", at).await.unwrap().0.win, win);
        assert_eq!(
            show("cerro", at).await.unwrap().0.win,
            None,
            "only the owner can share the win's card"
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
        app.store
            .lock()
            .unwrap()
            .conn
            .execute_batch(
                "insert into friendships (a, b, requested_by, accepted, created_at) values ('friend', 'hill', 'hill', 1, 0);
                 insert into muted_people (recipient, sender, created_at) values ('friend', 'hill', 0);",
            )
            .unwrap();
        assert_eq!(tell("cerro").await.err(), Some(StatusCode::UNAUTHORIZED));
        assert_eq!(
            tell("hill").await.unwrap().0.told,
            2,
            "the count does not give a mute away"
        );
        assert_eq!(tell("hill").await.unwrap().0.told, 2);
        assert!(
            app.store
                .lock()
                .unwrap()
                .notifications("friend", now_ms())
                .unwrap()
                .is_empty(),
            "a friend who muted hill hears nothing"
        );
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
        cleanup(app, path);
    }

    #[tokio::test]
    async fn an_old_conquest_gets_a_quiet_win_and_telling_friends_can_be_taken_back() {
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
        // Announced before wins were kept: seen, with no win. Even an automatic policy
        // must not send it to friends now.
        app.store
            .lock()
            .unwrap()
            .conn
            .execute_batch(&format!(
                "insert or ignore into seen (user, key) values ('hill', 'conquered:{at}');
                 insert into sharing_settings (user, policies) values ('hill', '{{\"conquered\":\"auto\"}}');
                 insert into friendships (a, b, requested_by, accepted, created_at) values ('cerro', 'hill', 'hill', 1, 0);
                 insert into friendships (a, b, requested_by, accepted, created_at) values ('friend', 'hill', 'hill', 1, 0);"
            ))
            .unwrap();
        let show = |viewer: &str| {
            conquests::show(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers(viewer),
            )
        };
        let win = show("hill").await.unwrap().0.win.expect("a win kept now");
        assert_eq!(show("hill").await.unwrap().0.win, Some(win), "kept once");
        assert_eq!(show("cerro").await.unwrap().0.win, None);
        {
            let store = app.store.lock().unwrap();
            let kept = crate::wins::win(&store.conn, "hill", win).unwrap();
            assert_eq!(
                (kept.kind.as_str(), kept.created_at, kept.stats.clone()),
                ("conquered", at, serde_json::json!({}))
            );
            let queued: i64 = store
                .conn
                .query_row("select count(*) from win_auto_queue", [], |r| r.get(0))
                .unwrap();
            assert_eq!(queued, 0, "an old conquest is not sent to friends");
        }

        let tell = || {
            conquests::tell(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers("hill"),
            )
        };
        let untell = |user: &str| {
            conquests::untell(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers(user),
            )
        };
        let told = |user: &str| {
            app.store
                .lock()
                .unwrap()
                .notifications(user, now_ms())
                .unwrap()
                .into_iter()
                .filter(|n| n.kind == "conquest")
                .collect::<Vec<_>>()
        };
        assert_eq!(
            untell("hill").await.unwrap().0.retracted,
            0,
            "nothing told yet"
        );
        assert_eq!(show("hill").await.unwrap().0.undo_until, None);
        let before = now_ms();
        assert_eq!(tell().await.unwrap().0.told, 2);
        let until = show("hill")
            .await
            .unwrap()
            .0
            .undo_until
            .expect("undo offered");
        assert!(until >= before + conquests::UNDO_MS && until <= now_ms() + conquests::UNDO_MS);
        assert_eq!(
            show("cerro").await.unwrap().0.undo_until,
            None,
            "owner only"
        );
        // Friend reads it; Cerro has not yet.
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "update notifications set read_at = 1 where recipient = 'friend' and kind = 'conquest'",
                [],
            )
            .unwrap();
        assert_eq!(untell("cerro").await.err(), Some(StatusCode::UNAUTHORIZED));
        assert_eq!(untell("hill").await.unwrap().0.retracted, 1);
        assert!(told("cerro").is_empty(), "the unread row is gone");
        assert_eq!(told("friend").len(), 1, "a row already read stays");
        let shown = show("hill").await.unwrap().0;
        assert_eq!((shown.told, shown.undo_until), (Some(false), None));
        assert_eq!(untell("hill").await.unwrap().0.retracted, 0, "idempotent");

        assert_eq!(tell().await.unwrap().0.told, 2);
        assert_eq!(
            told("cerro").len(),
            1,
            "telling again reaches who was untold"
        );
        assert_eq!(told("friend").len(), 1, "and not who read it");
        let row = told("cerro")[0].id;
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "insert into reactions (reactor, notification, sender, created_at) values ('cerro', ?1, 'hill', 0)",
                [row],
            )
            .unwrap();
        assert_eq!(
            show("hill").await.unwrap().0.undo_until,
            None,
            "a cheer keeps it"
        );
        assert_eq!(untell("hill").await.err(), Some(StatusCode::CONFLICT));
        app.store
            .lock()
            .unwrap()
            .conn
            .execute_batch(&format!(
                "delete from reactions;
                 update conquest_tells set told_at = {} where user = 'hill'",
                now_ms() - conquests::UNDO_MS - 1
            ))
            .unwrap();
        assert_eq!(
            untell("hill").await.err(),
            Some(StatusCode::CONFLICT),
            "a day later"
        );
        assert_eq!(told("cerro").len(), 1);

        let store = app.store.lock().unwrap();
        privacy::forget(&store.conn, "hill").unwrap();
        let left: i64 = store
            .conn
            .query_row(
                "select (select count(*) from conquest_tells) + (select count(*) from conquest_tell_rows)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
        drop(store);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_finished_deck_reaches_only_the_friends_picked_and_once_a_day() {
        let (app, path) = fixture();
        crate::test_support::used_app(&app, &["cerro", "hill", "friend"]);
        let share = |user: &str, recipients: &[&str]| {
            decks::share_deck(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers(user),
                Json(decks::Finished {
                    id: "7".into(),
                    name: "Japanese::Kanji".into(),
                    recipients: recipients.iter().map(|r| r.to_string()).collect(),
                }),
            )
        };
        assert_eq!(
            share("hill", &["hill"]).await.err(),
            Some(StatusCode::UNAUTHORIZED)
        );
        for refused in [&[][..], &["stranger"], &["hill", "hill"], &["cerro"]] {
            assert_eq!(
                share("cerro", refused).await.err(),
                Some(StatusCode::BAD_REQUEST),
                "{refused:?}"
            );
        }
        assert_eq!(share("cerro", &["hill"]).await.unwrap().0.recipients, 1);
        assert_eq!(
            share("cerro", &["hill", "friend"])
                .await
                .unwrap()
                .0
                .recipients,
            1,
            "hill was told already today"
        );
        let inbox = |user: &str| {
            app.store
                .lock()
                .unwrap()
                .notifications(user, now_ms())
                .unwrap()
        };
        let told = inbox("hill");
        assert_eq!(told.len(), 1, "{told:?}");
        assert_eq!(
            (told[0].kind.as_str(), told[0].body.as_str()),
            (
                "completion",
                "Cerro has finished their Japanese::Kanji studies for today."
            )
        );
        assert_eq!(inbox("friend").len(), 1);
        assert!(
            app.store.lock().unwrap().decks("cerro").unwrap().is_empty(),
            "the deck name is not kept with cerro's decks"
        );
        cleanup(app, path);
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
