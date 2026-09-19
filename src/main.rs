mod decks;
mod game;
mod store;

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use game::{Clock, Profile, Review, Week};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use store::{Error, Store};

const POLL: Duration = Duration::from_secs(20);
const MAX_PENDING: usize = 5000;
const MAX_SEPARATE_PUSHES: usize = 3;

#[derive(Deserialize, Clone, Default)]
struct UserConfig {
    display: Option<String>,
    ntfy_topic: Option<String>,
    token: Option<String>,
    token_file: Option<PathBuf>,
}

#[derive(Deserialize, Clone)]
struct Config {
    #[serde(default = "default_addr")]
    addr: String,
    sync_base: Option<PathBuf>,
    #[serde(default = "default_state")]
    state_dir: PathBuf,
    ntfy: Option<String>,
    #[serde(default = "default_remind_hour")]
    remind_hour: i64,
    public_url: Option<String>,
    #[serde(default = "default_week_timezone")]
    week_timezone: String,
    #[serde(default = "default_week_rollover_hour")]
    week_rollover_hour: u32,
    #[serde(default)]
    users: HashMap<String, UserConfig>,
}

fn default_week_timezone() -> String {
    "UTC".into()
}

fn default_week_rollover_hour() -> u32 {
    4
}

fn default_addr() -> String {
    "127.0.0.1:8097".into()
}

fn default_state() -> PathBuf {
    "state".into()
}

fn default_remind_hour() -> i64 {
    20
}

struct Player {
    reviews: Vec<Review>,
    clock: Clock,
}

struct App {
    config: Config,
    week: Week,
    store: Mutex<Store>,
    players: RwLock<HashMap<String, Player>>,
}

impl App {
    fn display(&self, user: &str) -> String {
        self.config
            .users
            .get(user)
            .and_then(|u| u.display.clone())
            .unwrap_or_else(|| user.into())
    }

    fn profile(&self, user: &str) -> Option<Profile> {
        self.preview(user, &[])
    }

    fn preview(&self, user: &str, pending: &[Review]) -> Option<Profile> {
        let players = self.players.read().unwrap();
        let player = players.get(user)?;
        let last = player.reviews.last().map_or(0, |r| r.id);
        let mut fresh: Vec<Review> = pending.iter().filter(|r| r.id > last).copied().collect();
        fresh.sort_by_key(|r| r.id);
        fresh.dedup_by_key(|r| r.id);
        let merged = [player.reviews.as_slice(), &fresh].concat();
        Some(game::compute(
            user,
            &self.display(user),
            &merged,
            &player.clock,
            &self.week,
            now_ms(),
        ))
    }

    fn profiles(&self) -> Vec<Profile> {
        let users: Vec<String> = self.players.read().unwrap().keys().cloned().collect();
        users.iter().filter_map(|u| self.profile(u)).collect()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[derive(Serialize)]
struct Standing {
    user: String,
    display: String,
    level: u64,
    xp_total: u64,
    week_xp: u64,
    streak: u64,
    today_reviews: u64,
}

async fn leaderboard(State(app): State<Arc<App>>) -> Json<Vec<Standing>> {
    let mut standings: Vec<Standing> = app
        .profiles()
        .into_iter()
        .map(|p| Standing {
            user: p.user,
            display: p.display,
            level: p.level,
            xp_total: p.xp_total,
            week_xp: p.week_xp,
            streak: p.streak,
            today_reviews: p.today.reviews,
        })
        .collect();
    standings.sort_by_key(|s| std::cmp::Reverse((s.week_xp, s.xp_total)));
    Json(standings)
}

async fn profile(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
) -> Result<Json<Profile>, StatusCode> {
    app.profile(&user).map(Json).ok_or(StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
struct Pending {
    reviews: Vec<Review>,
}

async fn preview(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    Json(pending): Json<Pending>,
) -> Result<Json<Profile>, StatusCode> {
    if pending.reviews.len() > MAX_PENDING {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    app.preview(&user, &pending.reviews)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
struct Upload {
    reviews: Vec<Review>,
    #[serde(default)]
    deleted: Vec<i64>,
    clock: Clock,
    #[serde(default)]
    silent: bool,
    decks: Option<Vec<decks::Snapshot>>,
    #[serde(default)]
    catalog: bool,
}

fn authorized(app: &App, user: &str, headers: &HeaderMap) -> bool {
    let Some(expected) = app.config.users.get(user).and_then(|u| u.token.as_deref()) else {
        return false;
    };
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    !expected.is_empty()
        && given.len() == expected.len()
        && given
            .bytes()
            .zip(expected.bytes())
            .fold(0, |acc, (a, b)| acc | (a ^ b))
            == 0
}

async fn upload(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(upload): Json<Upload>,
) -> Result<Json<Profile>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if upload.reviews.len() > MAX_PENDING || upload.deleted.len() > MAX_PENDING {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    if upload
        .decks
        .as_ref()
        .is_some_and(|decks| decks.len() > decks::MAX_DECKS)
    {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    if !decks::valid_clock(upload.clock)
        || upload
            .decks
            .as_ref()
            .is_some_and(|decks| !decks::valid_snapshots(decks))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let reviews: Vec<Review> = upload.reviews.into_iter().filter(|r| r.kind < 4).collect();
    let store_error = |e: Error| {
        eprintln!("upload for {user} failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    };
    let mut store = app.store.lock().unwrap();
    let silent = upload.silent || !store.is_known(&user).map_err(store_error)?;
    store
        .upsert(&user, &reviews, &upload.deleted, upload.clock)
        .map_err(store_error)?;
    if let Some(decks) = &upload.decks {
        store
            .record_decks(
                &user,
                &app.display(&user),
                decks,
                upload.clock,
                silent,
                now_ms(),
            )
            .map_err(store_error)?;
        if upload.catalog {
            store.prune_decks(&user, decks).map_err(store_error)?;
        }
    }
    let player = load_player(&store, &user).map_err(store_error)?;
    app.players.write().unwrap().insert(user.clone(), player);
    let profile = app.profile(&user).ok_or(StatusCode::NOT_FOUND)?;
    if silent {
        for event in &profile.events {
            store.mark_seen(&user, &event.key).map_err(store_error)?;
        }
    }
    Ok(Json(profile))
}

fn deck_settings(app: &App, store: &Store, user: &str) -> Result<decks::Settings, Error> {
    let mut users: BTreeSet<String> = app.config.users.keys().cloned().collect();
    let ntfy_enabled = app
        .config
        .ntfy
        .as_deref()
        .is_some_and(|base| !base.trim().is_empty());
    users.retain(|candidate| {
        candidate != user
            && app.config.users.get(candidate).is_some_and(|config| {
                config
                    .token
                    .as_deref()
                    .is_some_and(|token| !token.is_empty())
                    || (ntfy_enabled
                        && config
                            .ntfy_topic
                            .as_deref()
                            .is_some_and(|topic| !topic.trim().is_empty()))
            })
    });
    Ok(decks::Settings {
        decks: store.decks(user)?,
        recipients: users
            .into_iter()
            .map(|user| decks::Recipient {
                display: app.display(&user),
                user,
            })
            .collect(),
    })
}

async fn get_decks(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Json<decks::Settings>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    deck_settings(&app, &app.store.lock().unwrap(), &user)
        .map(Json)
        .map_err(|e| {
            eprintln!("deck settings for {user} failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn set_decks(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(update): Json<decks::SettingsUpdate>,
) -> Result<Json<decks::Settings>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if update.decks.len() > decks::MAX_DECKS {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let store_error = |e: Error| {
        eprintln!("deck settings for {user} failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    };
    let mut store = app.store.lock().unwrap();
    let current = deck_settings(&app, &store, &user).map_err(store_error)?;
    if !decks::valid_preferences(&update.decks, &current.decks, &current.recipients) {
        return Err(StatusCode::BAD_REQUEST);
    }
    store
        .set_deck_preferences(&user, &update.decks)
        .map_err(store_error)?;
    deck_settings(&app, &store, &user)
        .map(Json)
        .map_err(store_error)
}

async fn notifications(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<decks::Notification>>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    app.store
        .lock()
        .unwrap()
        .notifications(&user, now_ms())
        .map(Json)
        .map_err(|e| {
            eprintln!("notifications for {user} failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

#[derive(Serialize)]
struct WeekInfo {
    ends_at: i64,
    timezone: String,
}

async fn week_info(State(app): State<Arc<App>>) -> Json<WeekInfo> {
    Json(WeekInfo {
        ends_at: app.week.end_after(now_ms()),
        timezone: app.config.week_timezone.clone(),
    })
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
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

fn push(config: &Config, user: &str, title: &str, body: &str, tag: &str) {
    let (Some(base), Some(topic)) = (
        config.ntfy.as_deref(),
        config.users.get(user).and_then(|u| u.ntfy_topic.as_deref()),
    ) else {
        return;
    };
    let mut request = ureq::post(&format!("{}/{topic}", base.trim_end_matches('/')))
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .header("Title", title)
        .header("Tags", tag);
    if let Some(url) = &config.public_url {
        request = request.header("Click", format!("{}/#{user}", url.trim_end_matches('/')));
    }
    if let Err(e) = request.send(body) {
        eprintln!("ntfy push for {user} failed: {e}");
    }
}

fn load_player(store: &Store, user: &str) -> Result<Player, Error> {
    Ok(Player {
        reviews: store.reviews(user)?,
        clock: store.clock(user)?,
    })
}

fn sync_users(base: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().join("collection.anki2").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

fn import(app: &App, store: &mut Store, base: &Path) -> Result<(), Error> {
    for user in sync_users(base) {
        let first_import = !store.is_known(&user)?;
        let changed = match store.ingest(base, &user) {
            Ok(changed) => changed,
            Err(e) => {
                eprintln!("ingest for {user} failed: {e}");
                continue;
            }
        };
        if changed {
            let player = load_player(store, &user)?;
            app.players.write().unwrap().insert(user.clone(), player);
        }
        if first_import && let Some(profile) = app.profile(&user) {
            for event in &profile.events {
                store.mark_seen(&user, &event.key)?;
            }
        }
    }
    Ok(())
}

fn tick(app: &App) -> Result<(), Error> {
    let mut store = app.store.lock().unwrap();
    if let Some(base) = &app.config.sync_base {
        import(app, &mut store, base)?;
    }
    let users: Vec<String> = app.players.read().unwrap().keys().cloned().collect();
    for user in users {
        let Some(profile) = app.profile(&user) else {
            continue;
        };
        let mut fresh = Vec::new();
        for event in &profile.events {
            if store.mark_seen(&user, &event.key)? {
                fresh.push(event);
            }
        }
        if fresh.len() > MAX_SEPARATE_PUSHES {
            let titles: Vec<&str> = fresh.iter().map(|e| e.title.as_str()).collect();
            let title = format!("{} new unlocks", fresh.len());
            push(&app.config, &user, &title, &titles.join(", "), "tada");
        } else {
            for event in fresh {
                push(&app.config, &user, &event.title, &event.body, "tada");
            }
        }
        if profile.at_risk
            && profile.local_hour >= app.config.remind_hour
            && store.mark_seen(&user, &format!("risk:{}", profile.day))?
        {
            let body = format!(
                "Your {} day streak ends tonight. {}",
                profile.streak,
                if profile.freezes > 0 {
                    "A freeze would cover you, but why spend it?"
                } else {
                    "No freezes left."
                }
            );
            push(&app.config, &user, "Streak at risk", &body, "fire");
        }
    }
    let deliveries = store.take_deck_deliveries(now_ms())?;
    drop(store);
    for delivery in deliveries {
        push(
            &app.config,
            &delivery.user,
            &delivery.notification.title,
            &delivery.notification.body,
            "white_check_mark",
        );
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let path = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("ANKIQUEST_CONFIG").ok())
        .unwrap_or_else(|| "ankiquest.json".into());
    let mut config: Config = serde_json::from_slice(
        &std::fs::read(&path).map_err(|e| format!("cannot read config {path}: {e}"))?,
    )?;

    for (name, user) in &mut config.users {
        if let Some(file) = &user.token_file {
            let token = std::fs::read_to_string(file)
                .map_err(|e| format!("cannot read token for {name}: {e}"))?;
            user.token = Some(token.trim().to_string());
        }
    }

    let store = Store::open(&config.state_dir)?;
    let mut players = HashMap::new();
    for user in store.users()? {
        players.insert(user.clone(), load_player(&store, &user)?);
    }
    let week = Week {
        tz: config
            .week_timezone
            .parse()
            .map_err(|e| format!("week_timezone {:?}: {e}", config.week_timezone))?,
        rollover_hour: config.week_rollover_hour,
    };
    if week.rollover_hour > 23 {
        return Err("week_rollover_hour must be between 0 and 23".into());
    }
    let app = Arc::new(App {
        week,
        config,
        players: RwLock::new(players),
        store: Mutex::new(store),
    });

    let worker = app.clone();
    std::thread::spawn(move || {
        loop {
            if let Err(e) = tick(&worker) {
                eprintln!("tick failed: {e}");
            }
            std::thread::sleep(POLL);
        }
    });

    let router = Router::new()
        .route("/", get(index))
        .route("/manifest.webmanifest", get(manifest))
        .route("/icon.svg", get(icon))
        .route("/api/leaderboard", get(leaderboard))
        .route("/api/week", get(week_info))
        .route("/api/profile/{user}", get(profile))
        .route("/api/preview/{user}", post(preview))
        .route("/api/reviews/{user}", post(upload))
        .route("/api/decks/{user}", get(get_decks).post(set_decks))
        .route("/api/notifications/{user}", get(notifications))
        .with_state(app.clone());

    let listener = tokio::net::TcpListener::bind(&app.config.addr).await?;
    println!("ankiquest listening on http://{}", app.config.addr);
    axum::serve(listener, router).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Arc<App>, PathBuf) {
        let (store, path) = decks::tests::temporary_store();
        let config: Config = serde_json::from_value(serde_json::json!({
            "users": {
                "cerro": {"display": "Cerro", "token": "cerro-secret"},
                "hill": {"display": "Hill", "token": "hill-secret"},
                "friend": {"token": "friend-secret"}
            }
        }))
        .unwrap();
        (
            Arc::new(App {
                config,
                week: Week::default(),
                store: Mutex::new(store),
                players: RwLock::new(HashMap::new()),
            }),
            path,
        )
    }

    fn headers(user: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {user}-secret").parse().unwrap(),
        );
        headers
    }

    fn sample_upload(remaining: u64, silent: bool) -> Upload {
        Upload {
            reviews: vec![],
            deleted: vec![],
            clock: Clock::default(),
            silent,
            catalog: false,
            decks: Some(vec![decks::Snapshot {
                id: "1".into(),
                name: "Spanish".into(),
                remaining,
                reviewed_today: 3,
                day: Clock::default().day(now_ms()),
            }]),
        }
    }

    fn preferences(recipients: &[&str]) -> decks::SettingsUpdate {
        decks::SettingsUpdate {
            decks: vec![decks::Preference {
                id: "1".into(),
                enabled: true,
                recipients: recipients.iter().map(|r| (*r).into()).collect(),
            }],
        }
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

    #[test]
    fn recipient_choices_require_a_configured_delivery_channel() {
        let (mut app, path) = fixture();
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some("http://localhost:9999".into());
        config.users.insert(
            "push-only".into(),
            UserConfig {
                ntfy_topic: Some("friend-topic".into()),
                ..UserConfig::default()
            },
        );
        config.users.insert(
            "empty-token".into(),
            UserConfig {
                token: Some(String::new()),
                ..UserConfig::default()
            },
        );
        config
            .users
            .insert("display-only".into(), UserConfig::default());
        config.users.insert(
            "empty-topic".into(),
            UserConfig {
                ntfy_topic: Some(String::new()),
                ..UserConfig::default()
            },
        );
        {
            let mut store = app.store.lock().unwrap();
            store
                .upsert("imported", &[], &[], Clock::default())
                .unwrap();
            let settings = deck_settings(&app, &store, "cerro").unwrap();
            assert_eq!(
                settings
                    .recipients
                    .iter()
                    .map(|r| r.user.as_str())
                    .collect::<Vec<_>>(),
                vec!["friend", "hill", "push-only"]
            );
            assert!(settings.decks.is_empty());
        }
        Arc::get_mut(&mut app).unwrap().config.ntfy = None;
        assert_eq!(
            deck_settings(&app, &app.store.lock().unwrap(), "cerro")
                .unwrap()
                .recipients
                .iter()
                .map(|r| r.user.as_str())
                .collect::<Vec<_>>(),
            vec!["friend", "hill"]
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn all_private_handlers_reject_missing_and_other_users_tokens() {
        let (app, path) = fixture();
        for auth in [HeaderMap::new(), headers("hill")] {
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
                notifications(State(app.clone()), UrlPath("cerro".into()), auth.clone())
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
        let inbox = notifications(State(app.clone()), UrlPath("hill".into()), headers("hill"))
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
            notifications(
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

    #[tokio::test]
    async fn rejects_invalid_updates_without_changing_settings() {
        let (app, path) = fixture();
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(2, false)),
        )
        .await
        .unwrap();
        for update in [
            preferences(&["cerro"]),
            preferences(&["unknown"]),
            preferences(&[]),
            preferences(&["hill", "hill"]),
            decks::SettingsUpdate {
                decks: vec![decks::Preference {
                    id: "missing".into(),
                    enabled: false,
                    recipients: vec![],
                }],
            },
            decks::SettingsUpdate {
                decks: vec![
                    decks::Preference {
                        id: "1".into(),
                        enabled: true,
                        recipients: vec!["hill".into()],
                    },
                    decks::Preference {
                        id: "1".into(),
                        enabled: false,
                        recipients: vec![],
                    },
                ],
            },
        ] {
            assert_eq!(
                set_decks(
                    State(app.clone()),
                    UrlPath("cerro".into()),
                    headers("cerro"),
                    Json(update)
                )
                .await
                .unwrap_err(),
                StatusCode::BAD_REQUEST
            );
        }
        let settings = get_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
        )
        .await
        .unwrap()
        .0;
        assert!(!settings.decks[0].enabled);
        assert!(settings.decks[0].recipients.is_empty());
        let mut bad = sample_upload(0, false);
        bad.clock.offset_west_min = i64::MAX;
        assert_eq!(
            upload(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(bad)
            )
            .await
            .unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        let mut duplicate = sample_upload(0, false);
        let repeated = duplicate.decks.as_ref().unwrap()[0].clone();
        duplicate.decks.as_mut().unwrap().push(repeated);
        assert_eq!(
            upload(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(duplicate)
            )
            .await
            .unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        let mut oversized = sample_upload(0, false);
        let deck = oversized.decks.as_ref().unwrap()[0].clone();
        oversized.decks = Some(vec![deck; decks::MAX_DECKS + 1]);
        assert_eq!(
            upload(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(oversized)
            )
            .await
            .unwrap_err(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn legacy_and_silent_uploads_and_previews_do_not_publish_completion() {
        let (app, path) = fixture();
        let legacy: Upload = serde_json::from_value(serde_json::json!({
            "reviews": [], "clock": {"offset_west_min": 0, "rollover_hour": 4}
        }))
        .unwrap();
        assert!(legacy.decks.is_none());
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(legacy),
        )
        .await
        .unwrap();
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(2, false)),
        )
        .await
        .unwrap();
        let _ = set_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(preferences(&["hill"])),
        )
        .await
        .unwrap();
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(0, true)),
        )
        .await
        .unwrap();
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(0, false)),
        )
        .await
        .unwrap();
        let _ = preview(
            State(app.clone()),
            UrlPath("cerro".into()),
            Json(Pending { reviews: vec![] }),
        )
        .await
        .unwrap();
        assert!(
            notifications(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0
                .is_empty()
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
