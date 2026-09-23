mod access;
mod avatars;
mod challenges;
mod competition;
mod decks;
mod freezes;
mod game;
mod reminders;
mod store;
mod subscriptions;

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Datelike;
use game::{Clock, Periods, Profile, Records, Review, Week};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use store::{Error, Store};

const POLL: Duration = Duration::from_secs(20);
const MAX_PENDING: usize = 5000;
const PUSH_BUDGET: Duration = Duration::from_secs(20);
/// How many people a record names: the holder and whoever came closest.
const PODIUM: usize = 3;

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
    #[serde(default, rename = "remind_hour")]
    legacy_remind_hour: Option<i64>,
    public_url: Option<String>,
    #[serde(default)]
    private_site: bool,
    site_password_file: Option<PathBuf>,
    #[serde(default)]
    site_trust_proxy: bool,
    #[serde(default = "default_week_timezone")]
    week_timezone: String,
    #[serde(default = "default_week_rollover_hour")]
    week_rollover_hour: u32,
    #[serde(default)]
    competition_start_date: Option<String>,
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

struct Player {
    reviews: Vec<Review>,
    clock: Clock,
    freeze_policy: game::FreezePolicy,
}

struct App {
    config: Config,
    access: access::Access,
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
        Some(game::compute_with_freezes(
            user,
            &self.display(user),
            &merged,
            &player.clock,
            &self.week,
            now_ms(),
            &player.freeze_policy,
        ))
    }

    fn profiles(&self) -> Vec<Profile> {
        let users: Vec<String> = self.players.read().unwrap().keys().cloned().collect();
        users.iter().filter_map(|u| self.profile(u)).collect()
    }

    fn participants(&self) -> Vec<competition::Participant> {
        let players = self.players.read().unwrap();
        let users: BTreeSet<_> = self
            .config
            .users
            .keys()
            .chain(players.keys())
            .cloned()
            .collect();
        users
            .into_iter()
            .map(|user| {
                let player = players.get(&user);
                competition::Participant {
                    display: self.display(&user),
                    reviews: player.map_or_else(Vec::new, |p| p.reviews.clone()),
                    clock: player.map_or_else(Clock::default, |p| p.clock),
                    freeze_policy: player
                        .map_or_else(game::FreezePolicy::default, |p| p.freeze_policy.clone()),
                    user,
                }
            })
            .collect()
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
    xp: u64,
    period: String,
    periods: Periods,
    streak: u64,
    today_reviews: u64,
}

#[derive(Deserialize)]
struct BoardQuery {
    period: Option<String>,
}

async fn leaderboard(
    State(app): State<Arc<App>>,
    Query(query): Query<BoardQuery>,
) -> Json<Vec<Standing>> {
    let period = query
        .period
        .filter(|name| Periods::NAMES.contains(&name.as_str()))
        .unwrap_or_else(|| "week".into());
    let mut standings: Vec<Standing> = app
        .profiles()
        .into_iter()
        .map(|p| Standing {
            user: p.user,
            display: p.display,
            level: p.level,
            xp_total: p.xp_total,
            week_xp: p.week_xp,
            xp: p.periods.get(&period),
            period: period.clone(),
            periods: p.periods,
            streak: p.streak,
            today_reviews: p.today.reviews,
        })
        .collect();
    standings.sort_by_key(|s| std::cmp::Reverse((s.xp, s.xp_total)));
    Json(standings)
}

async fn profile(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
) -> Result<Json<Profile>, StatusCode> {
    app.profile(&user).map(Json).ok_or(StatusCode::NOT_FOUND)
}

#[derive(Debug, Serialize)]
struct FreezeSettings {
    enabled: bool,
    freezes: u32,
    capacity: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FreezeUpdate {
    enabled: bool,
}

fn freeze_settings(app: &App, store: &Store, user: &str) -> Result<FreezeSettings, Error> {
    let history = store.freeze_preferences(user)?;
    let profile = app.profile(user);
    Ok(FreezeSettings {
        enabled: history.last().is_some_and(|change| change.enabled),
        freezes: profile.map_or(0, |profile| profile.stored_freezes),
        capacity: game::MAX_FREEZES,
    })
}

async fn get_freezes(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Json<FreezeSettings>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    freeze_settings(&app, &app.store.lock().unwrap(), &user)
        .map(Json)
        .map_err(|e| {
            eprintln!("streak freeze settings for {user} failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn set_freezes(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(update): Json<FreezeUpdate>,
) -> Result<Json<FreezeSettings>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let store_error = |e: Error| {
        eprintln!("streak freeze settings for {user} failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    };
    let mut store = app.store.lock().unwrap();
    store
        .set_freezes_enabled(&user, update.enabled, now_ms())
        .map_err(store_error)?;
    // Cache the timeline with the reviews: profile/preview are read-only and are
    // also called while the database lock is held by uploads and imports.
    let known = app.players.read().unwrap().contains_key(&user);
    if known {
        let player = load_player(&store, &user).map_err(store_error)?;
        app.players.write().unwrap().insert(user.clone(), player);
    }
    freeze_settings(&app, &store, &user)
        .map(Json)
        .map_err(store_error)
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
    access::authorized(app, user, headers)
}

async fn upload(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(upload): Json<Upload>,
) -> Result<Json<UploadResponse>, StatusCode> {
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
    let mut announced = Vec::new();
    if let Some(decks) = &upload.decks {
        announced = store
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
    challenges::refresh(&mut store, &app.participants(), now_ms()).map_err(store_error)?;
    let profile = app.profile(&user).ok_or(StatusCode::NOT_FOUND)?;
    if silent {
        for event in &profile.events {
            store.mark_seen(&user, &event.key).map_err(store_error)?;
        }
    }
    Ok(Json(UploadResponse { profile, announced }))
}

fn deck_settings(app: &App, store: &Store, user: &str) -> Result<decks::Settings, Error> {
    let mut users: BTreeSet<String> = app.config.users.keys().cloned().collect();
    let ntfy_enabled = app
        .config
        .ntfy
        .as_deref()
        .is_some_and(|base| !base.trim().is_empty());
    let unsubscribed = store.unsubscribed_recipients(user)?;
    users.retain(|candidate| {
        candidate != user
            && !unsubscribed.contains(candidate)
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
        nudges: store.nudges_enabled(user)?,
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
    if let Some(nudges) = update.nudges {
        store.set_nudges(&user, nudges).map_err(store_error)?;
    }
    deck_settings(&app, &store, &user)
        .map(Json)
        .map_err(store_error)
}

fn store_error(error: Error) -> StatusCode {
    eprintln!("community storage failed: {error}");
    StatusCode::INTERNAL_SERVER_ERROR
}

#[derive(Debug, Serialize)]
struct IncomingPreferences {
    #[serde(flatten)]
    settings: decks::IncomingSettings,
    senders: Vec<decks::Recipient>,
    unsubscribed_senders: Vec<String>,
    sharing_senders: Vec<String>,
}

fn incoming_preferences(
    app: &App,
    store: &Store,
    user: &str,
) -> Result<IncomingPreferences, Error> {
    let mut users: BTreeSet<String> = app.config.users.keys().cloned().collect();
    users.extend(store.known_incoming_senders(user)?);
    let unsubscribed_senders = store.unsubscribed_senders(user)?;
    users.extend(unsubscribed_senders.iter().cloned());
    users.remove(user);
    let mut senders: Vec<_> = users
        .into_iter()
        .map(|user| decks::Recipient {
            display: app.display(&user),
            user,
        })
        .collect();
    senders.sort_by(|a, b| a.display.cmp(&b.display).then_with(|| a.user.cmp(&b.user)));
    Ok(IncomingPreferences {
        settings: store.incoming_settings(user)?,
        senders,
        unsubscribed_senders,
        sharing_senders: store.sharing_senders(user)?,
    })
}

async fn get_incoming_preferences(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Json<IncomingPreferences>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    incoming_preferences(&app, &app.store.lock().unwrap(), &user)
        .map(Json)
        .map_err(store_error)
}

async fn set_incoming_preferences(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(update): Json<decks::IncomingSettings>,
) -> Result<Json<IncomingPreferences>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !decks::valid_incoming_settings(&update) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut store = app.store.lock().unwrap();
    let current = incoming_preferences(&app, &store, &user).map_err(store_error)?;
    if update
        .muted_senders
        .iter()
        .any(|user| !current.senders.iter().any(|sender| sender.user == *user))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    store
        .set_incoming_settings(&user, &update)
        .map_err(store_error)?;
    incoming_preferences(&app, &store, &user)
        .map(Json)
        .map_err(store_error)
}

async fn set_deck_subscriptions(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(update): Json<subscriptions::Settings>,
) -> Result<Json<IncomingPreferences>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !update.valid(&user) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut store = app.store.lock().unwrap();
    let current = incoming_preferences(&app, &store, &user).map_err(store_error)?;
    if update
        .unsubscribed_senders
        .iter()
        .any(|sender| !current.senders.iter().any(|person| &person.user == sender))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    store
        .set_deck_subscriptions(&user, &update)
        .map_err(store_error)?;
    incoming_preferences(&app, &store, &user)
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
    let mut store = app.store.lock().unwrap();
    let now = now_ms();
    challenges::refresh(&mut store, &app.participants(), now).map_err(store_error)?;
    let profile = app.profile(&user);
    let clock = app.players.read().unwrap().get(&user).map(|p| p.clock);
    if let (Some(profile), Some(clock)) = (&profile, &clock) {
        reminders::cancel_stale(&mut store, &user, profile, clock, &app.week, now)
            .map_err(store_error)?;
    }
    let mut result = Vec::new();
    for mut notice in store.notifications(&user, now).map_err(store_error)? {
        if notice.kind.starts_with("reminder_") {
            let (Some(profile), Some(clock)) = (&profile, &clock) else {
                continue;
            };
            if reminders::delivery_guard(&store, &user, &notice, profile, clock, &app.week, now)
                .map_err(store_error)?
                != reminders::DeliveryGuard::Deliver
            {
                continue;
            }
            notice.body = reminders::current_body(&notice, profile, clock, &app.week, now);
        }
        result.push(notice);
    }
    Ok(Json(result))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ActivityQuery {
    days: Option<u32>,
    before: Option<i64>,
    limit: Option<u32>,
}

async fn activity(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Query(query): Query<ActivityQuery>,
) -> Result<Json<decks::Activity>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let days = query.days.unwrap_or(decks::ACTIVITY_DAYS);
    let limit = query.limit.unwrap_or(100);
    if !matches!(days, 30 | 90)
        || !(1..=200).contains(&limit)
        || query.before.is_some_and(|id| id <= 0)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut store = app.store.lock().unwrap();
    let now = now_ms();
    challenges::refresh(&mut store, &app.participants(), now).map_err(store_error)?;
    store
        .activity(&user, now, days, query.before, limit)
        .map(Json)
        .map_err(store_error)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivityRead {
    ids: Vec<i64>,
}

async fn read_activity(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(request): Json<ActivityRead>,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if request.ids.len() > 200 || request.ids.iter().any(|id| *id <= 0) {
        return Err(StatusCode::BAD_REQUEST);
    }
    app.store
        .lock()
        .unwrap()
        .read_activity(&user, &request.ids, now_ms())
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
struct RecordHolder {
    user: String,
    display: String,
    value: u64,
    detail: u64,
    at: i64,
}

#[derive(Debug, Serialize)]
struct RecordBoard {
    window: String,
    unit: &'static str,
    holders: Vec<RecordHolder>,
}

fn podium(
    window: &str,
    unit: &'static str,
    profiles: &[Profile],
    of: impl Fn(&Profile) -> (u64, u64, i64),
) -> RecordBoard {
    let mut holders: Vec<RecordHolder> = profiles
        .iter()
        .map(|player| (player, of(player)))
        .filter(|(_, (value, _, _))| *value > 0)
        .map(|(player, (value, detail, at))| RecordHolder {
            user: player.user.clone(),
            display: player.display.clone(),
            value,
            detail,
            at,
        })
        .collect();
    holders.sort_by_key(|holder| std::cmp::Reverse((holder.value, holder.detail)));
    holders.truncate(PODIUM);
    RecordBoard {
        window: window.to_string(),
        unit,
        holders,
    }
}

/// The best hour, day, week, month and year anyone here has ever had, plus the
/// longest streak and the most days studied. Each names whoever came closest,
/// so a near miss is visible rather than hidden.
async fn records(State(app): State<Arc<App>>) -> Json<Vec<RecordBoard>> {
    let profiles = app.profiles();
    let mut board: Vec<RecordBoard> = Records::NAMES
        .iter()
        .map(|window| {
            podium(window, "xp", &profiles, |player| {
                let record = player.records.get(window);
                (record.xp, record.reviews, record.at)
            })
        })
        .collect();
    board.push(podium("streak", "days", &profiles, |player| {
        (
            player.lifetime.best_streak,
            0,
            player.lifetime.best_streak_at,
        )
    }));
    board.push(podium("days", "days", &profiles, |player| {
        (player.lifetime.days_active, 0, player.lifetime.first_day_at)
    }));
    Json(board)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyRequest {
    notification: i64,
    message: String,
}

#[derive(Debug, Serialize)]
struct ReplyResponse {
    sent_to: String,
}

/// Answers a deck completion, so finishing a deck starts a conversation.
async fn reply(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(request): Json<ReplyRequest>,
) -> Result<Json<ReplyResponse>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let message = request.message.trim();
    if !decks::valid_message(message) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let sender = app
        .store
        .lock()
        .unwrap()
        .reply(
            &user,
            &app.display(&user),
            request.notification,
            message,
            now_ms(),
        )
        .map_err(|e| {
            eprintln!("reply from {user} failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(ReplyResponse {
        sent_to: app.display(&sender),
    }))
}

/// The player's profile, plus any deck completion this upload just announced.
#[derive(Debug, Serialize)]
struct UploadResponse {
    #[serde(flatten)]
    profile: Profile,
    announced: Vec<decks::Announcement>,
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

async fn community_page() -> Html<&'static str> {
    Html(include_str!("../static/community.html"))
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
) -> Result<Json<serde_json::Value>, StatusCode> {
    let now = now_ms();
    let date = app.week.competition_date(now);
    let year = query.year.unwrap_or(date.year());
    let month = query.month.unwrap_or(date.month());
    if !(1970..=2100).contains(&year) || !(1..=12).contains(&month) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut store = app.store.lock().unwrap();
    if let Some(base) = &app.config.sync_base {
        import(&app, &mut store, base).map_err(|error| {
            eprintln!("community sync import incomplete: {error}");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    }
    let participants = app.participants();
    refresh_community(&app, &mut store, &participants, now).map_err(store_error)?;
    competition::dashboard(&store, &participants, &app.week, now, year, month)
        .map(Json)
        .map_err(store_error)
}

async fn winners(State(app): State<Arc<App>>) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut store = app.store.lock().unwrap();
    if let Some(base) = &app.config.sync_base {
        import(&app, &mut store, base).map_err(|error| {
            eprintln!("winner history sync import incomplete: {error}");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    }
    let participants = app.participants();
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
}

type ChallengeFailure = (StatusCode, Json<serde_json::Value>);

fn challenge_error(error: challenges::Error) -> ChallengeFailure {
    let (status, message) = match error {
        challenges::Error::Invalid(message) => (StatusCode::BAD_REQUEST, message),
        challenges::Error::Forbidden => (StatusCode::FORBIDDEN, "This challenge is private."),
        challenges::Error::NotFound => (StatusCode::NOT_FOUND, "Challenge not found."),
        challenges::Error::Storage(error) => {
            eprintln!("challenge storage failed: {error}");
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
    let mut recipients: Vec<_> = app
        .config
        .users
        .iter()
        .filter(|(name, config)| {
            name.as_str() != user
                && config
                    .token
                    .as_deref()
                    .is_some_and(|token| !token.is_empty())
        })
        .map(|(name, _)| decks::Recipient {
            user: name.clone(),
            display: app.display(name),
        })
        .collect();
    recipients.sort_by(|a, b| a.display.cmp(&b.display).then_with(|| a.user.cmp(&b.user)));
    Ok(ChallengeList {
        challenges: challenges::list(store, user, players, now_ms())?,
        recipients,
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
    let eligible = app
        .config
        .users
        .iter()
        .filter(|(_, config)| config.token.as_deref().is_some_and(|t| !t.is_empty()))
        .map(|(name, _)| name.clone())
        .collect();
    let mut store = app.store.lock().unwrap();
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

fn push_destination<'a>(config: &'a Config, user: &str) -> Option<(&'a str, &'a str)> {
    let base = config.ntfy.as_deref()?.trim();
    let topic = config.users.get(user)?.ntfy_topic.as_deref()?.trim();
    (!base.is_empty() && !topic.is_empty()).then_some((base, topic))
}

fn push(
    config: &Config,
    user: &str,
    title: &str,
    body: &str,
    tag: &str,
    priority: &str,
    timeout: Duration,
) -> Result<(), Error> {
    let (base, topic) = push_destination(config, user).ok_or("no ntfy destination")?;
    let mut request = ureq::post(&format!("{}/{topic}", base.trim_end_matches('/')))
        .config()
        .timeout_global(Some(timeout))
        .build()
        .header("Title", title)
        .header("Priority", priority)
        .header("Tags", tag);
    if let Some(url) = &config.public_url {
        request = request.header("Click", format!("{}/#{user}", url.trim_end_matches('/')));
    }
    let response = request.send(body)?;
    if !response.status().is_success() {
        return Err(format!("ntfy returned {}", response.status()).into());
    }
    Ok(())
}

fn load_player(store: &Store, user: &str) -> Result<Player, Error> {
    Ok(Player {
        reviews: store.reviews(user)?,
        clock: store.clock(user)?,
        freeze_policy: store.freeze_policy(user)?,
    })
}

fn sync_users(base: &Path) -> Result<Vec<String>, Error> {
    let mut users = Vec::new();
    let mut errors = Vec::new();
    for entry in std::fs::read_dir(base)? {
        let result = (|| -> Result<Option<String>, Error> {
            let entry = entry?;
            if !std::fs::metadata(entry.path())?.is_dir() {
                return Ok(None);
            }
            let collection = entry.path().join("collection.anki2");
            match std::fs::metadata(&collection) {
                Ok(metadata) if metadata.is_file() => entry
                    .file_name()
                    .into_string()
                    .map(Some)
                    .map_err(|name| format!("invalid sync user name: {name:?}").into()),
                Ok(_) => Err(format!("{} is not a collection file", collection.display()).into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(format!("{}: {error}", collection.display()).into()),
            }
        })();
        match result {
            Ok(Some(user)) => users.push(user),
            Ok(None) => {}
            Err(error) => errors.push(error.to_string()),
        }
    }
    if errors.is_empty() {
        Ok(users)
    } else {
        Err(errors.join("; ").into())
    }
}

fn import(app: &App, store: &mut Store, base: &Path) -> Result<(), Error> {
    let mut errors = Vec::new();
    for user in sync_users(base)? {
        let result = (|| -> Result<(), Error> {
            let first_import = !store.is_known(&user)?;
            if store.ingest(base, &user)? {
                let player = load_player(store, &user)?;
                app.players.write().unwrap().insert(user.clone(), player);
            }
            if first_import && let Some(profile) = app.profile(&user) {
                for event in &profile.events {
                    store.mark_seen(&user, &event.key)?;
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(format!("ingest for {user} failed: {error}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; ").into())
    }
}

fn streak_warning_body(profile: &Profile) -> String {
    format!(
        "Your {} day streak ends tonight. {}",
        profile.streak,
        if !profile.freezes_enabled {
            "Streak freezes are turned off."
        } else if profile.freezes > 0 {
            "A freeze would cover you, but why spend it?"
        } else {
            "No freezes left."
        }
    )
}

fn tick(app: &App) -> Result<(), Error> {
    let mut store = app.store.lock().unwrap();
    let import_complete =
        app.config
            .sync_base
            .as_ref()
            .is_none_or(|base| match import(app, &mut store, base) {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("competition refresh deferred until sync import succeeds: {error}");
                    false
                }
            });
    let participants = app.participants();
    let now = now_ms();
    if import_complete {
        refresh_community(app, &mut store, &participants, now)?;
    }
    challenges::refresh(&mut store, &participants, now)?;
    let users: Vec<String> = app.players.read().unwrap().keys().cloned().collect();
    // One snapshot of the standings, so a nudge knows who is just out of reach.
    let ranking = app.profiles();
    for user in users {
        let Some(profile) = app.profile(&user) else {
            continue;
        };
        let push_enabled = push_destination(&app.config, &user).is_some();
        store.queue_events(&user, &profile.events, profile.day, now_ms(), push_enabled)?;
        if store.nudges_enabled(&user)? {
            let gap = |other: &Profile| (other.display.clone(), other.week_xp);
            let ahead = ranking
                .iter()
                .filter(|other| other.week_xp > profile.week_xp)
                .min_by_key(|other| other.week_xp)
                .map(gap);
            for nudge in game::nudges(&profile, ahead.as_ref().map(|(w, xp)| (w.as_str(), *xp))) {
                store.send_once(
                    &decks::Outgoing {
                        to: &user,
                        from: "",
                        title: &nudge.title,
                        body: &nudge.body,
                        kind: "nudge",
                    },
                    &nudge.key,
                    profile.day,
                    now_ms(),
                    false,
                )?;
            }
        }
        if let Some(participant) = participants.iter().find(|p| p.user == user) {
            let recap = competition::weekly_recap(&store, &user, &app.week, now)?;
            reminders::tick(
                &mut store,
                &user,
                &profile,
                &participant.clock,
                &app.week,
                now,
                recap,
            )?;
        }
    }
    drop(store);
    deliver_notifications(app, PUSH_BUDGET)
}

fn deliver_notifications(app: &App, budget: Duration) -> Result<(), Error> {
    let deliveries = app.store.lock().unwrap().take_deck_deliveries(now_ms())?;
    let started = std::time::Instant::now();
    for delivery in deliveries {
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        let id = delivery.notification.id;
        let mut reminder_body = None;
        if delivery.notification.kind.starts_with("reminder_") {
            let mut store = app.store.lock().unwrap();
            let now = now_ms();
            let profile = app.profile(&delivery.user);
            let clock = app
                .players
                .read()
                .unwrap()
                .get(&delivery.user)
                .map(|p| p.clock);
            let guard = match (profile, clock) {
                (Some(profile), Some(clock)) => {
                    reminder_body = Some(reminders::current_body(
                        &delivery.notification,
                        &profile,
                        &clock,
                        &app.week,
                        now,
                    ));
                    reminders::delivery_guard(
                        &store,
                        &delivery.user,
                        &delivery.notification,
                        &profile,
                        &clock,
                        &app.week,
                        now,
                    )?
                }
                _ => reminders::DeliveryGuard::Cancel,
            };
            match guard {
                reminders::DeliveryGuard::Cancel => {
                    reminders::cancel(&mut store, id)?;
                    continue;
                }
                reminders::DeliveryGuard::Defer => {
                    store.defer_push(id, now)?;
                    continue;
                }
                reminders::DeliveryGuard::Deliver => {}
            }
        }
        let current_warning = if delivery.notification.kind == "risk" {
            match app.profile(&delivery.user) {
                Some(profile) if profile.day == delivery.notification.day && profile.at_risk => {
                    Some(streak_warning_body(&profile))
                }
                _ => {
                    app.store.lock().unwrap().cancel_streak_warning(id)?;
                    continue;
                }
            }
        } else {
            reminder_body
        };
        if push_destination(&app.config, &delivery.user).is_none() {
            app.store.lock().unwrap().defer_push(id, now_ms())?;
            continue;
        }
        if !app.store.lock().unwrap().begin_push(id, now_ms())? {
            continue;
        }
        let result = push(
            &app.config,
            &delivery.user,
            &delivery.notification.title,
            current_warning
                .as_deref()
                .unwrap_or(&delivery.notification.body),
            match delivery.notification.kind.as_str() {
                "event" => "tada",
                "risk" | "reminder_urgent" => "fire",
                _ => "white_check_mark",
            },
            if delivery.notification.kind.starts_with("reminder_")
                && delivery.notification.kind != "reminder_urgent"
            {
                "default"
            } else {
                "high"
            },
            remaining.min(Duration::from_secs(10)),
        );
        app.store
            .lock()
            .unwrap()
            .finish_push(id, result.is_ok(), now_ms())?;
        if let Err(e) = result {
            eprintln!(
                "ntfy push for {} failed; queued for retry: {e}",
                delivery.user
            );
        }
    }
    Ok(())
}

const USAGE: &str =
    "usage: ankiquest [config.json] [message <player> <text> [--from <player>] [--title <text>]]";

#[derive(Debug, PartialEq)]
struct Message {
    to: String,
    text: String,
    from: Option<String>,
    title: Option<String>,
}

/// Splits `[config] [message ...]` so the same binary serves and speaks.
fn parse_args(args: &[String]) -> Result<(Option<String>, Option<Message>), Error> {
    let (path, rest) = match args.first().map(String::as_str) {
        Some("message") => (None, args),
        Some(first) if !first.starts_with('-') => (Some(first.to_string()), &args[1..]),
        Some(_) => return Err(USAGE.into()),
        None => return Ok((None, None)),
    };
    let rest = match rest.first().map(String::as_str) {
        Some("message") => &rest[1..],
        Some(_) => return Err(USAGE.into()),
        None => return Ok((path, None)),
    };

    let mut positional = Vec::new();
    let mut from = None;
    let mut title = None;
    let mut rest = rest.iter();
    while let Some(argument) = rest.next() {
        let mut value = |name: &str| {
            rest.next()
                .cloned()
                .ok_or_else(|| Error::from(format!("{name} needs a value")))
        };
        match argument.as_str() {
            "--from" => from = Some(value("--from")?),
            "--title" => title = Some(value("--title")?),
            other if other.starts_with("--") => {
                return Err(format!("unknown option {other}").into());
            }
            other => positional.push(other.to_string()),
        }
    }
    let [to, text] = positional.as_slice() else {
        return Err(USAGE.into());
    };
    Ok((
        path,
        Some(Message {
            to: to.clone(),
            text: text.trim().to_string(),
            from,
            title,
        }),
    ))
}

/// Delivers a message written by hand on the server, replyable when it says who sent it.
fn send_message(config: &Config, message: &Message) -> Result<(), Error> {
    let known = |user: &str| config.users.contains_key(user);
    if !known(&message.to) {
        return Err(format!("{} is not a configured player", message.to).into());
    }
    if let Some(from) = message.from.as_deref().filter(|from| !known(from)) {
        return Err(format!("{from} is not a configured player").into());
    }
    if !decks::valid_message(&message.text) {
        return Err(format!(
            "the message must be 1 to {} characters on a single line",
            decks::MAX_MESSAGE
        )
        .into());
    }
    let title = match (&message.title, &message.from) {
        (Some(title), _) => title.clone(),
        (None, Some(from)) => format!("\u{1f4ac} {}", display_of(config, from)),
        (None, None) => "ankiquest".to_string(),
    };
    let mut store = Store::open(&config.state_dir)?;
    let now = now_ms();
    let day = store.clock(&message.to)?.day(now);
    let id = store.send(
        &decks::Outgoing {
            to: &message.to,
            from: message.from.as_deref().unwrap_or(""),
            title: &title,
            body: &message.text,
            kind: "message",
        },
        day,
        now,
    )?;
    println!("delivered to {} as notification {id}", message.to);
    Ok(())
}

fn display_of(config: &Config, user: &str) -> String {
    config
        .users
        .get(user)
        .and_then(|u| u.display.clone())
        .unwrap_or_else(|| user.to_string())
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
    let (given, message) = parse_args(&args)?;
    let path = given
        .or_else(|| std::env::var("ANKIQUEST_CONFIG").ok())
        .unwrap_or_else(|| "ankiquest.json".into());
    let mut config: Config = serde_json::from_slice(
        &std::fs::read(&path).map_err(|e| format!("cannot read config {path}: {e}"))?,
    )?;

    if config.legacy_remind_hour.is_some() {
        eprintln!("remind_hour is superseded by personal reminder preferences at /community.");
    }
    if let Some(date) = &config.competition_start_date {
        let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|_| "competition_start_date must be YYYY-MM-DD")?;
        if date.year() < 1970
            || date
                > chrono::DateTime::from_timestamp_millis(now_ms())
                    .unwrap()
                    .date_naive()
        {
            return Err("competition_start_date must be between 1970 and today".into());
        }
    }

    // Sending needs no tokens, and the running service holds the only readable copy.
    if let Some(message) = message {
        return send_message(&config, &message);
    }

    for (name, user) in &mut config.users {
        if let Some(file) = &user.token_file {
            let token = std::fs::read_to_string(file)
                .map_err(|e| format!("cannot read token for {name}: {e}"))?;
            user.token = Some(token.trim().to_string());
        }
    }

    let access = access::Access::from_config(&config)?;
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
        access,
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
        .merge(avatars::routes())
        .route("/", get(index))
        .route("/records", get(index))
        .route("/community", get(community_page))
        .route("/{period}", get(period_page))
        .route("/manifest.webmanifest", get(manifest))
        .route("/icon.svg", get(icon))
        .route("/site.css", get(access::site_css))
        .route("/site.js", get(access::site_js))
        .route("/login", get(access::login))
        .route("/auth/status", get(access::status))
        .route(
            "/auth/session",
            post(access::session).layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route("/auth/logout", post(access::logout))
        .route("/api/leaderboard", get(leaderboard))
        .route("/api/records", get(records))
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
        .route("/api/week", get(week_info))
        .route("/api/profile/{user}", get(profile))
        .route("/api/preview/{user}", post(preview))
        .route("/api/reviews/{user}", post(upload))
        .route("/api/decks/{user}", get(get_decks).post(set_decks))
        .route(
            "/api/streak-freezes/{user}",
            get(get_freezes).post(set_freezes),
        )
        .route("/api/notifications/{user}", get(notifications))
        .route(
            "/api/notification-preferences/{user}",
            get(get_incoming_preferences).post(set_incoming_preferences),
        )
        .route("/api/activity/{user}", get(activity))
        .route(
            "/api/deck-subscriptions/{user}",
            get(get_incoming_preferences).post(set_deck_subscriptions),
        )
        .route("/api/activity/{user}/read", post(read_activity))
        .route("/api/reply/{user}", post(reply))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            access::gate,
        ))
        .with_state(app)
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
                access: access::Access::default(),
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
        let board = community_dashboard(State(app.clone()), Query(CommunityQuery::default()))
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
                })
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
                 time integer, type integer, ease integer);",
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        for review in reviews {
            tx.execute(
                "insert into revlog values (?1,?2,?3,?4,?5,3)",
                rusqlite::params![
                    review.id,
                    review.cid,
                    review.last_ivl,
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

        let board = community_dashboard(State(app.clone()), community_query(day))
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
        let board = community_dashboard(State(app.clone()), community_query(day))
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
            community_dashboard(State(app.clone()), community_query(day))
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
        let board = community_dashboard(State(app.clone()), community_query(day))
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
            community_dashboard(State(app.clone()), Query(CommunityQuery::default()))
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

    #[tokio::test]
    async fn streak_freeze_settings_require_the_players_own_token() {
        let (app, path) = fixture();
        for headers in [HeaderMap::new(), headers("hill"), headers("unknown")] {
            assert_eq!(
                get_freezes(State(app.clone()), UrlPath("cerro".into()), headers.clone())
                    .await
                    .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                set_freezes(
                    State(app.clone()),
                    UrlPath("cerro".into()),
                    headers,
                    Json(FreezeUpdate { enabled: true }),
                )
                .await
                .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert!(
            app.store
                .lock()
                .unwrap()
                .freeze_preferences("cerro")
                .unwrap()
                .is_empty()
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn streak_freezes_start_empty_can_be_enabled_before_upload_and_reload() {
        let (app, path) = fixture();
        let settings = get_freezes(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
        )
        .await
        .unwrap()
        .0;
        assert!(!settings.enabled);
        assert_eq!((settings.freezes, settings.capacity), (0, 3));
        for enabled in [true, true, false, true] {
            let settings = set_freezes(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(FreezeUpdate { enabled }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(settings.enabled, enabled);
            assert_eq!(settings.freezes, 0);
        }
        assert!(
            app.players.read().unwrap().is_empty(),
            "settings do not add an empty leaderboard entry"
        );
        let other = get_freezes(State(app.clone()), UrlPath("hill".into()), headers("hill"))
            .await
            .unwrap()
            .0;
        assert!(!other.enabled);
        drop(app);
        let store = Store::open(&path).unwrap();
        let history = store.freeze_preferences("cerro").unwrap();
        assert_eq!(
            history.len(),
            3,
            "repeated saves cannot change the activation time"
        );
        assert!(history.last().unwrap().enabled);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn streak_freeze_updates_cannot_set_a_balance_or_omit_opt_in() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({"enabled": null}),
            serde_json::json!({"enabled": "true"}),
            serde_json::json!({"enabled": true, "freezes": 3}),
        ] {
            assert!(serde_json::from_value::<FreezeUpdate>(value).is_err());
        }
    }

    #[tokio::test]
    async fn streak_freeze_previews_do_not_bank_rewards_and_uploads_only_earn_once() {
        let (app, path) = fixture();
        let clock = Clock::default();
        let day = clock.day(now_ms()) - 1;
        let start = day * 86_400_000;
        // Enough morning reviews, time, combo and two sessions for any initial quests.
        let reviews: Vec<Review> = (0..60)
            .map(|i| Review {
                id: start + 6 * 3_600_000 + i * 10_000 + if i >= 30 { 600_000 } else { 0 },
                cid: i,
                last_ivl: 30,
                time_ms: 10_000,
                kind: 0,
            })
            .collect();
        {
            let mut store = app.store.lock().unwrap();
            store.upsert("cerro", &[], &[], clock).unwrap();
            store.set_freezes_enabled("cerro", true, start).unwrap();
            app.players
                .write()
                .unwrap()
                .insert("cerro".into(), load_player(&store, "cerro").unwrap());
        }
        assert_eq!(app.profile("cerro").unwrap().freezes, 0);
        let projected = preview(
            State(app.clone()),
            UrlPath("cerro".into()),
            Json(Pending {
                reviews: reviews.clone(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(projected.freezes, 1);
        assert_eq!(
            app.profile("cerro").unwrap().freezes,
            0,
            "preview never saves reviews or a reward"
        );
        for _ in 0..2 {
            let response = upload(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(Upload {
                    reviews: reviews.clone(),
                    deleted: vec![],
                    clock,
                    silent: true,
                    decks: None,
                    catalog: false,
                }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(response.profile.freezes, 1);
            assert!(response.profile.freezes_enabled);
        }
        let paused = set_freezes(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(FreezeUpdate { enabled: false }),
        )
        .await
        .unwrap()
        .0;
        assert!(!paused.enabled);
        assert_eq!(paused.freezes, 1);
        assert!(
            !app.profile("cerro").unwrap().freezes_enabled,
            "cached profiles see the new setting immediately"
        );
        assert_eq!(
            app.profile("cerro").unwrap().freezes,
            0,
            "older clients only see available protection"
        );
        assert_eq!(app.profile("cerro").unwrap().stored_freezes, 1);
        let config = app.config.clone();
        drop(app);
        let store = Store::open(&path).unwrap();
        let player = load_player(&store, "cerro").unwrap();
        let reloaded = App {
            config,
            access: access::Access::default(),
            week: Week::default(),
            store: Mutex::new(store),
            players: RwLock::new(HashMap::from([("cerro".into(), player)])),
        };
        assert_eq!(reloaded.profile("cerro").unwrap().freezes, 0);
        assert_eq!(reloaded.profile("cerro").unwrap().stored_freezes, 1);
        assert!(!reloaded.profile("cerro").unwrap().freezes_enabled);
        drop(reloaded);
        std::fs::remove_dir_all(path).unwrap();
    }

    fn queued_message(app: &App, user: &str) -> i64 {
        app.store
            .lock()
            .unwrap()
            .send(
                &decks::Outgoing {
                    to: user,
                    from: "",
                    title: "Hello",
                    body: "Do not lose this",
                    kind: "message",
                },
                Clock::default().day(now_ms()),
                now_ms(),
            )
            .unwrap()
    }

    fn was_pushed(app: &App, id: i64) -> bool {
        app.store
            .lock()
            .unwrap()
            .conn
            .query_row(
                "select pushed from notifications where id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn local_push(status: u16) -> (String, std::thread::JoinHandle<String>) {
        local_push_with_check(status, || {})
    }

    fn local_push_with_check(
        status: u16,
        check: impl FnOnce() + Send + 'static,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "no push request arrived"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("accept push: {e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 2048];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            check();
            let _ = write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            String::from_utf8(request).unwrap()
        });
        (address, server)
    }

    #[test]
    fn missing_push_configuration_does_not_consume_a_queued_message() {
        let (app, path) = fixture();
        let id = queued_message(&app, "hill");
        tick(&app).unwrap();
        assert!(
            !was_pushed(&app, id),
            "an inbox-only delivery has not been pushed"
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn stale_streak_warnings_are_cancelled_after_rollover_or_studying() {
        for studied in [false, true] {
            let (app, path) = fixture();
            let now = now_ms();
            let day = Clock::default().day(now);
            let review = Review {
                id: if studied { now } else { now - 86_400_000 },
                cid: 1,
                last_ivl: 30,
                time_ms: 5000,
                kind: 1,
            };
            {
                let mut store = app.store.lock().unwrap();
                store
                    .upsert("hill", &[review], &[], Clock::default())
                    .unwrap();
                app.players
                    .write()
                    .unwrap()
                    .insert("hill".into(), load_player(&store, "hill").unwrap());
                store
                    .send_once(
                        &decks::Outgoing {
                            to: "hill",
                            from: "",
                            title: "Streak at risk",
                            body: "Your streak ends tonight",
                            kind: "risk",
                        },
                        "risk:test",
                        if studied { day } else { day - 1 },
                        now,
                        true,
                    )
                    .unwrap();
            }
            deliver_notifications(&app, PUSH_BUDGET).unwrap();
            assert_eq!(
                app.store
                    .lock()
                    .unwrap()
                    .conn
                    .query_row(
                        "select count(*) from notifications where kind = 'risk'",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                0,
                "an obsolete warning must not be sent on a later retry"
            );
            assert!(
                !app.store
                    .lock()
                    .unwrap()
                    .mark_seen("hill", "risk:test")
                    .unwrap(),
                "cancelling a stale warning must not generate it again"
            );
            drop(app);
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[tokio::test]
    async fn queued_streak_warnings_follow_freeze_setting_changes_before_retry() {
        for enabled in [false, true] {
            let (mut app, path) = fixture();
            let (base, failed) = local_push(503);
            let config = &mut Arc::get_mut(&mut app).unwrap().config;
            config.ntfy = Some(base);
            config.users.get_mut("hill").unwrap().ntfy_topic = Some("hill-topic".into());
            let clock = Clock::default();
            let day = clock.day(now_ms());
            let start = (day - 1) * 86_400_000;
            let reviews: Vec<_> = (0..60)
                .map(|i| Review {
                    id: start + 6 * 3_600_000 + i * 10_000 + if i >= 30 { 600_000 } else { 0 },
                    cid: i,
                    last_ivl: 30,
                    time_ms: 10_000,
                    kind: 0,
                })
                .collect();
            {
                let mut store = app.store.lock().unwrap();
                store.upsert("hill", &reviews, &[], clock).unwrap();
                store.set_freezes_enabled("hill", true, start).unwrap();
                app.players
                    .write()
                    .unwrap()
                    .insert("hill".into(), load_player(&store, "hill").unwrap());
            }
            // Both directions retain one earned freeze: only its availability changes.
            let settings = set_freezes(
                State(app.clone()),
                UrlPath("hill".into()),
                headers("hill"),
                Json(FreezeUpdate { enabled: !enabled }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(settings.enabled, !enabled);
            let before = app.profile("hill").unwrap();
            assert_eq!(before.stored_freezes, 1);
            assert_eq!(before.freezes_enabled, !enabled);
            assert!(before.at_risk);
            let old_tail = if enabled {
                "Streak freezes are turned off."
            } else {
                "A freeze would cover you, but why spend it?"
            };
            let old_body = format!("Your {} day streak ends tonight. {old_tail}", before.streak);
            let id = {
                let mut store = app.store.lock().unwrap();
                store
                    .send_once(
                        &decks::Outgoing {
                            to: "hill",
                            from: "",
                            title: "Streak at risk",
                            body: &old_body,
                            kind: "risk",
                        },
                        &format!("risk:{day}"),
                        day,
                        now_ms(),
                        true,
                    )
                    .unwrap();
                store.conn.last_insert_rowid()
            };
            deliver_notifications(&app, PUSH_BUDGET).unwrap();
            assert!(failed.join().unwrap().ends_with(&old_body));
            assert!(!was_pushed(&app, id));

            let settings = set_freezes(
                State(app.clone()),
                UrlPath("hill".into()),
                headers("hill"),
                Json(FreezeUpdate { enabled }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(settings.enabled, enabled);
            assert_eq!(settings.freezes, 1);
            let current = app.profile("hill").unwrap();
            assert_eq!(current.stored_freezes, 1);
            assert_eq!(current.freezes, u32::from(enabled));
            let (base, success) = local_push(200);
            Arc::get_mut(&mut app).unwrap().config.ntfy = Some(base);
            app.store
                .lock()
                .unwrap()
                .conn
                .execute("update notifications set retry_at = 0 where id = ?1", [id])
                .unwrap();
            deliver_notifications(&app, PUSH_BUDGET).unwrap();
            let request = success.join().unwrap();
            let expected_tail = if enabled {
                "A freeze would cover you, but why spend it?"
            } else {
                "Streak freezes are turned off."
            };
            assert!(
                request.ends_with(expected_tail),
                "retried warning must describe the current setting (enabled={enabled})"
            );
            assert!(!request.ends_with(old_tail));
            assert!(was_pushed(&app, id));
            assert!(
                app.store
                    .lock()
                    .unwrap()
                    .notifications("hill", now_ms())
                    .unwrap()
                    .is_empty()
            );
            drop(app);
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[tokio::test]
    async fn disabling_nudges_cancels_failed_pushes_without_losing_inbox_history() {
        for enable_again in [false, true] {
            let (mut app, path) = fixture();
            let (base, failed) = local_push(503);
            let config = &mut Arc::get_mut(&mut app).unwrap().config;
            config.ntfy = Some(base);
            config.users.get_mut("hill").unwrap().ntfy_topic = Some("hill-topic".into());
            let id = {
                let mut store = app.store.lock().unwrap();
                store.set_nudges("hill", true).unwrap();
                store
                    .send_once(
                        &decks::Outgoing {
                            to: "hill",
                            from: "",
                            title: "Almost there",
                            body: "Level 2 is 90 XP away.",
                            kind: "nudge",
                        },
                        "nudge:test",
                        Clock::default().day(now_ms()),
                        now_ms(),
                        false,
                    )
                    .unwrap();
                store.conn.last_insert_rowid()
            };
            deliver_notifications(&app, PUSH_BUDGET).unwrap();
            assert!(failed.join().unwrap().ends_with("Level 2 is 90 XP away."));
            assert!(!was_pushed(&app, id));
            let settings = set_decks(
                State(app.clone()),
                UrlPath("hill".into()),
                headers("hill"),
                Json(decks::SettingsUpdate {
                    decks: vec![],
                    nudges: Some(false),
                }),
            )
            .await
            .unwrap()
            .0;
            assert!(!settings.nudges);
            if enable_again {
                let settings = set_decks(
                    State(app.clone()),
                    UrlPath("hill".into()),
                    headers("hill"),
                    Json(decks::SettingsUpdate {
                        decks: vec![],
                        nudges: Some(true),
                    }),
                )
                .await
                .unwrap()
                .0;
                assert!(settings.nudges);
            }
            drop(app);
            let mut store = Store::open(&path).unwrap();
            let retry_at = now_ms() + 60_000;
            assert!(
                store.take_deck_deliveries(retry_at).unwrap().is_empty(),
                "opting out must cancel a failed nudge even after restart and re-enabling"
            );
            assert!(!store.begin_push(id, retry_at).unwrap());
            let inbox = store.notifications("hill", retry_at).unwrap();
            assert_eq!(inbox.len(), 1);
            assert_eq!(inbox[0].id, id);
            assert_eq!(inbox[0].body, "Level 2 is 90 XP away.");
            assert!(!store.mark_seen("hill", "nudge:test").unwrap());
            drop(store);
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn failed_http_push_is_not_acknowledged() {
        let (mut app, path) = fixture();
        let (base, server) = local_push(500);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("hill").unwrap().ntfy_topic = Some("hill-topic".into());
        let id = queued_message(&app, "hill");
        tick(&app).unwrap();
        assert!(server.join().unwrap().starts_with("POST /hill-topic "));
        assert!(
            !was_pushed(&app, id),
            "an HTTP error must leave the message retryable"
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn ntfy_pushes_request_high_priority_for_vibration_and_pop_up_alerts() {
        let (mut app, path) = fixture();
        let (base, server) = local_push(200);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("hill").unwrap().ntfy_topic = Some("hill-topic".into());
        let id = queued_message(&app, "hill");
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        let request = server.join().unwrap();
        assert!(
            request
                .lines()
                .any(|line| line.eq_ignore_ascii_case("priority: high")),
            "ntfy needs high priority to request vibration and a pop-up"
        );
        assert!(was_pushed(&app, id));
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn successful_retry_acknowledges_the_same_message_without_holding_the_store_lock() {
        let (mut app, path) = fixture();
        let (base, failed) = local_push(503);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("hill").unwrap().ntfy_topic = Some("hill-topic".into());
        let id = queued_message(&app, "hill");
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        failed.join().unwrap();
        assert!(!was_pushed(&app, id));
        let app_for_check = Arc::new(Mutex::new(std::sync::Weak::<App>::new()));
        let check = app_for_check.clone();
        let (base, success) = local_push_with_check(200, move || {
            let app = check.lock().unwrap().upgrade().unwrap();
            assert!(
                app.store.try_lock().is_ok(),
                "HTTP must not block uploads or inbox reads"
            );
        });
        Arc::get_mut(&mut app).unwrap().config.ntfy = Some(base);
        *app_for_check.lock().unwrap() = Arc::downgrade(&app);
        app.store
            .lock()
            .unwrap()
            .conn
            .execute("update notifications set retry_at = 0 where id = ?1", [id])
            .unwrap();
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        let request = success.join().unwrap();
        assert!(request.starts_with("POST /hill-topic "));
        assert!(request.ends_with("Do not lose this"));
        assert!(was_pushed(&app, id));
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        let store = app.store.lock().unwrap();
        assert_eq!(store.notifications("hill", now_ms()).unwrap()[0].id, id);
        assert_eq!(
            store
                .conn
                .query_row(
                    "select push_attempts from notifications where id = ?1",
                    [id],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        drop(store);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn unconfigured_front_rows_do_not_block_a_configured_recipient() {
        let (mut app, path) = fixture();
        let (base, server) = local_push(200);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("friend").unwrap().ntfy_topic = Some("friend-topic".into());
        for _ in 0..101 {
            queued_message(&app, "hill");
        }
        let id = queued_message(&app, "friend");
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        assert!(server.join().unwrap().starts_with("POST /friend-topic "));
        assert!(was_pushed(&app, id));
        assert_eq!(app.store.lock().unwrap().conn.query_row("select sum(push_attempts + pushed) from notifications where recipient = 'hill'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn network_budget_leaves_unattempted_messages_ready_for_the_next_tick() {
        let (mut app, path) = fixture();
        let (base, server) =
            local_push_with_check(200, || std::thread::sleep(Duration::from_millis(500)));
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        for user in ["hill", "friend"] {
            config.users.get_mut(user).unwrap().ntfy_topic = Some(user.into());
        }
        let first = queued_message(&app, "hill");
        let second = queued_message(&app, "friend");
        deliver_notifications(&app, Duration::ZERO).unwrap();
        let start = std::time::Instant::now();
        deliver_notifications(&app, Duration::from_millis(100)).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "the request timeout respects the remaining budget"
        );
        server.join().unwrap();
        assert!(!was_pushed(&app, first));
        let store = app.store.lock().unwrap();
        assert_eq!(
            store
                .conn
                .query_row(
                    "select push_attempts + retry_at from notifications where id = ?1",
                    [second],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0,
            "skipped messages must not be leased or marked delivered"
        );
        drop(store);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn failed_server_game_events_remain_durable_without_duplicating_the_inbox() {
        let (mut app, path) = fixture();
        let (base, server) = local_push(500);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("cerro").unwrap().ntfy_topic = Some("cerro-topic".into());
        let now = now_ms();
        let reviews: Vec<_> = (0..100)
            .map(|i| Review {
                id: now - (100 - i) * 10_000,
                cid: i,
                last_ivl: 30,
                time_ms: 5000,
                kind: 0,
            })
            .collect();
        {
            let mut store = app.store.lock().unwrap();
            store
                .upsert("cerro", &reviews, &[], Clock::default())
                .unwrap();
            app.players
                .write()
                .unwrap()
                .insert("cerro".into(), load_player(&store, "cerro").unwrap());
        }
        assert!(!app.profile("cerro").unwrap().events.is_empty());
        tick(&app).unwrap();
        server.join().unwrap();
        let count: i64 = app
            .store
            .lock()
            .unwrap()
            .conn
            .query_row(
                "select count(*) from notifications where recipient = 'cerro' and pushed = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(count > 0, "failed server unlocks need a durable retry");
        tick(&app).unwrap();
        assert!(
            app.store
                .lock()
                .unwrap()
                .notifications("cerro", now_ms())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            app.store
                .lock()
                .unwrap()
                .conn
                .query_row(
                    "select count(*) from notifications where recipient = 'cerro'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            count
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
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
            nudges: None,
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
                notifications(State(app.clone()), UrlPath("cerro".into()), auth.clone())
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
    async fn recipient_unsubscribe_removes_sender_selections_and_only_recipient_can_restore() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let (app, path) = fixture();
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
            Json(preferences(&["hill", "friend"])),
        )
        .await
        .unwrap();
        let save = |owner: &str, unsubscribed: Vec<&str>| {
            Request::post("/api/deck-subscriptions/hill")
                .header("authorization", format!("Bearer {owner}-secret"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"enabled":true,"unsubscribed_senders":unsubscribed})
                        .to_string(),
                ))
                .unwrap()
        };
        let response = router(app.clone())
            .oneshot(save("hill", vec!["cerro"]))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let settings = get_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(settings.decks[0].recipients, ["friend"]);
        assert!(
            !settings
                .recipients
                .iter()
                .any(|person| person.user == "hill")
        );
        assert_eq!(
            set_decks(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(preferences(&["hill", "friend"]))
            )
            .await
            .unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            router(app.clone())
                .oneshot(save("cerro", vec![]))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            router(app.clone())
                .oneshot(save("hill", vec![]))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.store.lock().unwrap().decks("cerro").unwrap()[0].recipients,
            ["friend", "hill"]
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn subscription_api_requires_owner_validates_senders_and_protects_cookie_writes() {
        use axum::body::{Body, to_bytes};
        use axum::http::Request;
        use tower::ServiceExt;

        let (app, path) = fixture();
        let read = || {
            Request::get("/api/deck-subscriptions/hill")
                .body(Body::empty())
                .unwrap()
        };
        for token in [None, Some("cerro-secret")] {
            let mut request = read();
            if let Some(token) = token {
                request
                    .headers_mut()
                    .insert("authorization", format!("Bearer {token}").parse().unwrap());
            }
            assert_eq!(
                router(app.clone()).oneshot(request).await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }
        for (body, status) in [
            (
                serde_json::json!({"enabled":true,"unsubscribed_senders":["hill"]}),
                StatusCode::BAD_REQUEST,
            ),
            (
                serde_json::json!({"enabled":true,"unsubscribed_senders":["unknown"]}),
                StatusCode::BAD_REQUEST,
            ),
            (
                serde_json::json!({"enabled":true,"unsubscribed_senders":["cerro","cerro"]}),
                StatusCode::BAD_REQUEST,
            ),
            (
                serde_json::json!({"enabled":true,"unsubscribed_senders":["bad\nname"]}),
                StatusCode::BAD_REQUEST,
            ),
            (
                serde_json::json!({"enabled":true,"unsubscribed_senders":(0..101).map(|i| format!("sender{i}")).collect::<Vec<_>>()}),
                StatusCode::BAD_REQUEST,
            ),
            (
                serde_json::json!({"enabled":true}),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                serde_json::json!({"enabled":true,"unsubscribed_senders":[],"recipient":"cerro"}),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            let request = Request::post("/api/deck-subscriptions/hill")
                .header("authorization", "Bearer hill-secret")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            assert_eq!(
                router(app.clone()).oneshot(request).await.unwrap().status(),
                status
            );
        }
        let session = access::session(
            State(app.clone()),
            axum::extract::ConnectInfo("127.0.0.1:40123".parse().unwrap()),
            headers("hill"),
            axum::body::Bytes::new(),
        )
        .await;
        assert_eq!(session.status(), StatusCode::NO_CONTENT);
        let cookie = session.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        for csrf in [false, true] {
            let mut request = Request::post("/api/deck-subscriptions/hill")
                .header("cookie", cookie)
                .header("content-type", "application/json");
            if csrf {
                request = request.header("x-ankiquest-csrf", "1");
            }
            let response = router(app.clone())
                .oneshot(
                    request
                        .body(Body::from(
                            r#"{"enabled":true,"unsubscribed_senders":["cerro"]}"#,
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if csrf {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            if csrf {
                let body: serde_json::Value =
                    serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap())
                        .unwrap();
                assert_eq!(body["unsubscribed_senders"], serde_json::json!(["cerro"]));
                assert_eq!(body["sharing_senders"], serde_json::json!([]));
                assert!(!body.to_string().contains("deck_id"));
            }
        }
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn incoming_preferences_are_private_validated_and_available_before_studying() {
        let (app, path) = fixture();
        let settings =
            get_incoming_preferences(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0;
        assert!(settings.settings.enabled);
        assert!(settings.settings.muted_senders.is_empty());
        assert_eq!(
            settings
                .senders
                .iter()
                .map(|p| p.user.as_str())
                .collect::<Vec<_>>(),
            vec!["cerro", "friend"]
        );
        assert!(app.profile("hill").is_none());
        for muted_senders in [
            vec!["hill".into()],
            vec!["unknown".into()],
            vec!["cerro".into(), "cerro".into()],
        ] {
            assert_eq!(
                set_incoming_preferences(
                    State(app.clone()),
                    UrlPath("hill".into()),
                    headers("hill"),
                    Json(decks::IncomingSettings {
                        enabled: false,
                        muted_senders
                    })
                )
                .await
                .unwrap_err(),
                StatusCode::BAD_REQUEST
            );
        }
        assert!(
            app.store
                .lock()
                .unwrap()
                .incoming_settings("hill")
                .unwrap()
                .enabled
        );
        assert!(
            serde_json::from_value::<decks::IncomingSettings>(serde_json::json!({
                "enabled":false,"muted_senders":[],"user":"cerro"
            }))
            .is_err()
        );
        {
            let mut store = app.store.lock().unwrap();
            store
                .send(
                    &decks::Outgoing {
                        to: "hill",
                        from: "former-player",
                        title: "Deck complete",
                        body: "Private deck name",
                        kind: "completion",
                    },
                    0,
                    now_ms(),
                )
                .unwrap();
        }
        let saved = set_incoming_preferences(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(decks::IncomingSettings {
                enabled: false,
                muted_senders: vec!["former-player".into()],
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(!saved.settings.enabled);
        assert_eq!(saved.settings.muted_senders, vec!["former-player"]);
        assert!(
            !serde_json::to_string(&saved)
                .unwrap()
                .contains("Private deck name")
        );
        assert!(
            app.store
                .lock()
                .unwrap()
                .incoming_settings("cerro")
                .unwrap()
                .enabled
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn recipient_mutes_change_upload_delivery_without_changing_outgoing_settings() {
        let (app, path) = fixture();
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
            Json(preferences(&["hill", "friend"])),
        )
        .await
        .unwrap();
        let _ = set_incoming_preferences(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(decks::IncomingSettings {
                enabled: true,
                muted_senders: vec!["cerro".into()],
            }),
        )
        .await
        .unwrap();
        let completion = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(0, false)),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(completion.announced.len(), 1);
        assert_eq!(completion.announced[0].recipients, 1);
        assert!(
            notifications(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0
                .is_empty()
        );
        assert_eq!(
            notifications(
                State(app.clone()),
                UrlPath("friend".into()),
                headers("friend")
            )
            .await
            .unwrap()
            .0
            .len(),
            1
        );
        let outgoing = get_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(outgoing.decks[0].recipients, vec!["friend", "hill"]);
        let _ = set_incoming_preferences(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(decks::IncomingSettings {
                enabled: true,
                muted_senders: vec![],
            }),
        )
        .await
        .unwrap();
        assert!(
            upload(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(sample_upload(0, false))
            )
            .await
            .unwrap()
            .0
            .announced
            .is_empty()
        );
        assert!(
            notifications(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0
                .is_empty()
        );
        // Old clients save only outgoing preferences; recipient choices survive.
        let _ = set_incoming_preferences(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(decks::IncomingSettings {
                enabled: false,
                muted_senders: vec!["cerro".into()],
            }),
        )
        .await
        .unwrap();
        let _ = set_decks(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(decks::SettingsUpdate {
                decks: vec![],
                nudges: Some(true),
            }),
        )
        .await
        .unwrap();
        let settings =
            get_incoming_preferences(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0;
        assert!(!settings.settings.enabled);
        assert_eq!(settings.settings.muted_senders, vec!["cerro"]);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn the_upload_reports_what_it_just_announced() {
        let (app, path) = fixture();
        let send = |body: Upload| {
            let app = app.clone();
            async move {
                upload(
                    State(app),
                    UrlPath("cerro".into()),
                    headers("cerro"),
                    Json(body),
                )
                .await
                .unwrap()
                .0
            }
        };
        assert!(send(sample_upload(2, false)).await.announced.is_empty());
        let _ = set_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(preferences(&["hill"])),
        )
        .await
        .unwrap();

        let announced = send(sample_upload(0, false)).await.announced;
        assert_eq!(announced.len(), 1);
        assert_eq!(announced[0].deck, "Spanish");
        assert_eq!(announced[0].recipients, 1);
        assert!(
            send(sample_upload(0, false)).await.announced.is_empty(),
            "a deck is only announced once a day"
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    fn words(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    fn studied(user: &str, count: i64, day: i64) -> Upload {
        Upload {
            reviews: (0..count)
                .map(|index| Review {
                    id: day * 86_400_000 + 12 * 3_600_000 + index * 60_000 + user.len() as i64,
                    cid: index + 1,
                    last_ivl: 30,
                    time_ms: 5_000,
                    kind: 1,
                })
                .collect(),
            deleted: vec![],
            clock: Clock::default(),
            silent: true,
            catalog: false,
            decks: None,
        }
    }

    #[tokio::test]
    async fn the_records_board_names_whoever_had_the_best_window() {
        let (app, path) = fixture();
        let day = Clock::default().day(now_ms());
        for (user, count) in [("cerro", 30), ("hill", 8)] {
            let _ = upload(
                State(app.clone()),
                UrlPath(user.into()),
                headers(user),
                Json(studied(user, count, day)),
            )
            .await
            .unwrap();
        }
        let board = records(State(app.clone())).await.0;
        assert_eq!(
            board.iter().map(|r| r.window.as_str()).collect::<Vec<_>>(),
            [Records::NAMES.as_slice(), &["streak", "days"]].concat()
        );
        for window in board.iter().filter(|window| window.unit == "xp") {
            assert_eq!(
                window
                    .holders
                    .iter()
                    .map(|h| h.user.as_str())
                    .collect::<Vec<_>>(),
                vec!["cerro", "hill"],
                "{} lists the holder and whoever came closest",
                window.window
            );
            assert_eq!(window.holders[0].display, "Cerro");
            assert!(window.holders[0].value > window.holders[1].value);
            assert!(window.holders[1].detail > 0);
        }
        assert_eq!(
            board[0].holders[0].detail, 30,
            "all thirty land inside one hour"
        );
        let lifetime: Vec<&RecordBoard> = board
            .iter()
            .filter(|window| window.unit == "days")
            .collect();
        assert_eq!(
            lifetime
                .iter()
                .map(|w| w.window.as_str())
                .collect::<Vec<_>>(),
            vec!["streak", "days"]
        );
        for window in lifetime {
            assert_eq!(
                window.holders.len(),
                2,
                "{} names everyone who has studied",
                window.window
            );
            assert!(
                window
                    .holders
                    .iter()
                    .all(|holder| holder.value == 1 && holder.detail == 0)
            );
            assert!(window.holders.iter().all(|holder| holder.at > 0));
        }
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn nudges_are_off_until_asked_for_and_then_stay_on() {
        let (app, path) = fixture();
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(2, false)),
        )
        .await
        .unwrap();
        let save = |nudges: Option<bool>| {
            let app = app.clone();
            async move {
                let mut update = preferences(&["hill"]);
                update.nudges = nudges;
                set_decks(
                    State(app),
                    UrlPath("cerro".into()),
                    headers("cerro"),
                    Json(update),
                )
                .await
                .unwrap()
                .0
            }
        };
        assert!(!save(None).await.nudges, "off until someone asks");
        assert!(save(Some(true)).await.nudges);
        assert!(save(None).await.nudges, "an older client leaves it alone");
        assert!(!save(Some(false)).await.nudges);
        assert!(
            !get_decks(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0
                .nudges,
            "the setting belongs to one player"
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn the_message_command_reads_a_config_and_its_options_in_any_order() {
        assert_eq!(parse_args(&words(&[])).unwrap(), (None, None));
        let (path, nothing) = parse_args(&words(&["/etc/ankiquest.json"])).unwrap();
        assert_eq!(path.as_deref(), Some("/etc/ankiquest.json"));
        assert_eq!(nothing, None);

        let (path, message) = parse_args(&words(&[
            "/etc/ankiquest.json",
            "message",
            "--from",
            "cerro",
            "aldanita",
            "  keep going  ",
            "--title",
            "Hi",
        ]))
        .unwrap();
        assert_eq!(path.as_deref(), Some("/etc/ankiquest.json"));
        assert_eq!(
            message,
            Some(Message {
                to: "aldanita".into(),
                text: "keep going".into(),
                from: Some("cerro".into()),
                title: Some("Hi".into()),
            })
        );

        let (path, message) = parse_args(&words(&["message", "aldanita", "hello"])).unwrap();
        assert_eq!(path, None, "the config then comes from the environment");
        assert_eq!(message.unwrap().from, None);

        for bad in [
            vec!["message"],
            vec!["message", "aldanita"],
            vec!["message", "aldanita", "hello", "spare"],
            vec!["message", "aldanita", "hello", "--from"],
            vec!["message", "aldanita", "hello", "--shout"],
            vec!["--help"],
            vec!["/etc/ankiquest.json", "serve"],
        ] {
            assert!(
                parse_args(&words(&bad)).is_err(),
                "{bad:?} should not parse"
            );
        }
    }

    #[tokio::test]
    async fn a_message_written_on_the_server_lands_in_the_inbox_and_can_be_answered() {
        let (app, path) = fixture();
        let config: Config = serde_json::from_value(serde_json::json!({
            "state_dir": path,
            "users": {
                "cerro": {"display": "Cerro", "token": "cerro-secret"},
                "hill": {"display": "Hill", "token": "hill-secret"},
            }
        }))
        .unwrap();
        let note = |to: &str, from: Option<&str>| Message {
            to: to.into(),
            text: "you're doing great, keep going".into(),
            from: from.map(str::to_string),
            title: None,
        };
        send_message(&config, &note("hill", Some("cerro"))).unwrap();

        let inbox = notifications(State(app.clone()), UrlPath("hill".into()), headers("hill"))
            .await
            .unwrap()
            .0;
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].title, "\u{1f4ac} Cerro");
        assert_eq!(inbox[0].kind, "message", "written by hand, not by the game");
        assert_eq!(inbox[0].body, "you're doing great, keep going");
        assert_eq!(inbox[0].sender, "cerro");
        assert_eq!(
            reply(
                State(app.clone()),
                UrlPath("hill".into()),
                headers("hill"),
                Json(ReplyRequest {
                    notification: inbox[0].id,
                    message: "thank you!".into()
                })
            )
            .await
            .unwrap()
            .0
            .sent_to,
            "Cerro"
        );

        send_message(&config, &note("hill", None)).unwrap();
        let anonymous = notifications(State(app.clone()), UrlPath("hill".into()), headers("hill"))
            .await
            .unwrap()
            .0
            .pop()
            .unwrap();
        assert_eq!(anonymous.title, "ankiquest");
        assert!(anonymous.sender.is_empty(), "nobody to answer");

        assert!(send_message(&config, &note("nobody", None)).is_err());
        assert!(send_message(&config, &note("hill", Some("nobody"))).is_err());
        for text in ["", "   ", "two\nlines", &"x".repeat(decks::MAX_MESSAGE + 1)] {
            let mut empty = note("hill", None);
            empty.text = text.into();
            assert!(
                send_message(&config, &empty).is_err(),
                "{text:?} is not a message"
            );
        }
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn a_completion_can_be_answered_once_and_the_answer_answered_back() {
        let (app, path) = fixture();
        let post = |app: Arc<App>, body: Upload| async move {
            let _ = upload(
                State(app),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(body),
            )
            .await
            .unwrap();
        };
        let inbox = |app: Arc<App>, user: &'static str| async move {
            notifications(State(app), UrlPath(user.into()), headers(user))
                .await
                .unwrap()
                .0
        };
        let say = |app: Arc<App>, user: &'static str, notification: i64, message: &str| {
            let message = message.to_string();
            async move {
                reply(
                    State(app),
                    UrlPath(user.into()),
                    headers(user),
                    Json(ReplyRequest {
                        notification,
                        message,
                    }),
                )
                .await
            }
        };

        post(app.clone(), sample_upload(2, false)).await;
        let _ = set_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(preferences(&["hill"])),
        )
        .await
        .unwrap();
        post(app.clone(), sample_upload(0, false)).await;

        let completion = inbox(app.clone(), "hill").await.remove(0);
        assert_eq!(completion.sender, "cerro");
        assert!(!completion.replied);
        let id = completion.id;
        assert_eq!(
            say(app.clone(), "hill", id, "  Good job!  ")
                .await
                .unwrap()
                .0
                .sent_to,
            "Cerro"
        );
        assert!(
            inbox(app.clone(), "hill").await[0].replied,
            "a reply is only sent once"
        );

        let answer = inbox(app.clone(), "cerro").await.remove(0);
        assert_eq!(answer.title, "\u{1f4ac} Hill");
        assert_eq!(answer.body, "Good job!");
        assert_eq!(answer.sender, "hill");
        assert_eq!(
            say(app.clone(), "cerro", answer.id, "thanks!")
                .await
                .unwrap()
                .0
                .sent_to,
            "Hill"
        );
        assert_eq!(inbox(app.clone(), "hill").await[1].body, "thanks!");

        for (user, notification, message) in [
            ("hill", id, "already answered"),
            ("friend", id, "not my notification"),
            ("cerro", answer.id, "answered by me"),
            ("hill", id + 999, "no such notification"),
        ] {
            assert_eq!(
                say(app.clone(), user, notification, message)
                    .await
                    .unwrap_err(),
                StatusCode::NOT_FOUND
            );
        }
        for message in [
            "",
            "   ",
            "line\nbreak",
            &"x".repeat(decks::MAX_MESSAGE + 1),
        ] {
            assert_eq!(
                say(app.clone(), "hill", id, message).await.unwrap_err(),
                StatusCode::BAD_REQUEST
            );
        }
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
                nudges: None,
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
                nudges: None,
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
