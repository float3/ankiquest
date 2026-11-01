//! The dev console for the players listed in `devs`: announcements to everyone or to
//! chosen groups, aggregate statistics, and recent server errors. Statistics are
//! counts across players, never anything about one player's cards.

use crate::decks::valid_message;
use crate::game::{DAY_MS, Review};
use crate::notifications::{Outgoing, insert_notification};
use crate::store::{Error, Store};
use crate::{access, app::App, app::now_ms, web_util::store_error};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Html;
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// Days of history the statistics cover, today included.
pub const DAYS: i64 = 30;
/// How long a new account has to be before it counts towards retention.
const RETENTION_MS: i64 = 7 * DAY_MS;
pub const MAX_TITLE: usize = 80;
const MAX_GROUPS: usize = 100;
const LISTED: i64 = 50;

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists announcements (
             id integer primary key autoincrement,
             sender text not null,
             title text not null,
             body text not null,
             groups text not null,
             recipients integer not null,
             created_at integer not null
         );",
    )?;
    Ok(())
}

impl App {
    pub fn is_dev(&self, user: &str) -> bool {
        self.config.devs.iter().any(|dev| dev == user)
    }
}

/// 401 without the player's own credentials, 403 when they are not a dev.
fn check(app: &App, user: &str, headers: &HeaderMap) -> Result<(), StatusCode> {
    if !access::authorized(app, user, headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !app.is_dev(user) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Who an announcement reaches: everyone the server knows, or the members of the
/// given groups. `None` when the groups are empty, too many, or one does not exist.
pub fn recipients(
    app: &App,
    conn: &Connection,
    groups: Option<&[i64]>,
) -> rusqlite::Result<Option<BTreeSet<String>>> {
    let known = app.known_users();
    let Some(groups) = groups else {
        return Ok(Some(known));
    };
    if groups.is_empty() || groups.len() > MAX_GROUPS {
        return Ok(None);
    }
    let mut users = BTreeSet::new();
    for &group in groups {
        if !conn
            .prepare("select 1 from groups where id = ?1")?
            .exists([group])?
        {
            return Ok(None);
        }
        users.extend(
            crate::social::group_members(conn, group)?
                .into_iter()
                .filter(|user| known.contains(user)),
        );
    }
    Ok(Some(users))
}

/// Puts the announcement in every recipient's inbox, where the delivery loop pushes
/// it like any other message, and records it. Returns the announcement's id.
pub fn announce(
    store: &mut Store,
    sender: &str,
    title: &str,
    body: &str,
    groups: &[i64],
    recipients: &BTreeSet<String>,
    now: i64,
) -> Result<i64, Error> {
    let mut days = Vec::with_capacity(recipients.len());
    for user in recipients {
        days.push((user, store.clock(user)?.day(now)));
    }
    let tx = store.conn.transaction()?;
    for (user, day) in days {
        insert_notification(
            &tx,
            &Outgoing {
                to: user,
                from: "",
                title,
                body,
                kind: "message",
            },
            day,
            now,
            false,
        )?;
    }
    tx.execute(
        "insert into announcements (sender, title, body, groups, recipients, created_at)
         values (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            sender,
            title,
            body,
            serde_json::to_string(groups)?,
            recipients.len() as i64,
            now
        ],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(id)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub title: String,
    pub body: String,
    /// Absent for everyone.
    #[serde(default)]
    pub groups: Option<Vec<i64>>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct GroupRef {
    pub id: i64,
    /// `None` once the group is gone.
    pub name: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Announcement {
    pub id: i64,
    pub sender: String,
    pub title: String,
    pub body: String,
    /// Empty when it went to everyone.
    pub groups: Vec<GroupRef>,
    pub recipients: i64,
    pub created_at: i64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct GroupSummary {
    pub id: i64,
    pub name: String,
    pub members: i64,
}

pub fn groups(conn: &Connection) -> rusqlite::Result<Vec<GroupSummary>> {
    conn.prepare(
        "select g.id, g.name, count(m.user) from groups g
         left join group_members m on m.group_id = g.id
         group by g.id order by g.name collate nocase, g.id",
    )?
    .query_map([], |r| {
        Ok(GroupSummary {
            id: r.get(0)?,
            name: r.get(1)?,
            members: r.get(2)?,
        })
    })?
    .collect()
}

pub fn announcements(conn: &Connection) -> Result<Vec<Announcement>, Error> {
    let rows: Vec<(i64, String, String, String, String, i64, i64)> = conn
        .prepare(
            "select id, sender, title, body, groups, recipients, created_at
             from announcements order by id desc limit ?1",
        )?
        .query_map([LISTED], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    let mut name = conn.prepare("select name from groups where id = ?1")?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, sender, title, body, groups, recipients, created_at) in rows {
        let ids: Vec<i64> = serde_json::from_str(&groups)?;
        let mut groups = Vec::with_capacity(ids.len());
        for id in ids {
            let mut names = name.query_map([id], |r| r.get(0))?;
            groups.push(GroupRef {
                id,
                name: names.next().transpose()?,
            });
        }
        out.push(Announcement {
            id,
            sender,
            title,
            body,
            groups,
            recipients,
            created_at,
        });
    }
    Ok(out)
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Players {
    pub total: usize,
    pub configured: usize,
    pub accounts: usize,
    /// Players who only come from the sync folder.
    pub synced: usize,
}

/// Distinct players with a review in the last 24 hours, 7 days and 30 days.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Active {
    pub day: usize,
    pub week: usize,
    pub month: usize,
}

/// One UTC day.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Day {
    /// Days since 1970-01-01 UTC.
    pub day: i64,
    pub reviews: usize,
    pub players: usize,
    pub signups: usize,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Groups {
    pub count: usize,
    /// Distinct players in at least one group.
    pub members: usize,
    /// Members per group, largest first.
    pub sizes: Vec<i64>,
    pub friendships: i64,
}

/// Accounts older than a week, and how many of them reviewed in the last week.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Retention {
    pub eligible: usize,
    pub retained: usize,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Stats {
    pub players: Players,
    pub active: Active,
    /// The last `DAYS` days, oldest first.
    pub days: Vec<Day>,
    pub groups: Groups,
    pub retention: Retention,
    pub generated_at: i64,
}

/// What review history says: active counts, the per-day series without sign-ups,
/// and each player's latest review. Reviews are in id order.
pub fn activity<'a>(
    histories: impl IntoIterator<Item = (&'a str, &'a [Review])>,
    now: i64,
) -> (Active, Vec<Day>, HashMap<&'a str, i64>) {
    let today = now.div_euclid(DAY_MS);
    let first = today - DAYS + 1;
    let mut per_day: BTreeMap<i64, (usize, BTreeSet<&str>)> = BTreeMap::new();
    let mut active = Active::default();
    let mut latest = HashMap::new();
    for (user, reviews) in histories {
        let Some(last) = reviews.last() else {
            continue;
        };
        latest.insert(user, last.id);
        let age = now - last.id;
        active.day += usize::from(age < DAY_MS);
        active.week += usize::from(age < 7 * DAY_MS);
        active.month += usize::from(age < DAYS * DAY_MS);
        let start = reviews.partition_point(|r| r.id < first * DAY_MS);
        for review in &reviews[start..] {
            let entry = per_day.entry(review.id.div_euclid(DAY_MS)).or_default();
            entry.0 += 1;
            entry.1.insert(user);
        }
    }
    let days = (first..=today)
        .map(|day| {
            let (reviews, players) = per_day.remove(&day).unwrap_or_default();
            Day {
                day,
                reviews,
                players: players.len(),
                signups: 0,
            }
        })
        .collect();
    (active, days, latest)
}

pub fn stats(app: &App, now: i64) -> Result<Stats, Error> {
    let known = app.known_users();
    let account_users = app.accounts.users();
    let configured = app.config.users.len();
    let players = Players {
        total: known.len(),
        configured,
        accounts: account_users.len(),
        synced: known
            .iter()
            .filter(|user| !app.config.users.contains_key(*user) && !app.accounts.contains(user))
            .count(),
    };
    let histories: Vec<(String, Arc<Vec<Review>>)> = app
        .players
        .read()
        .unwrap()
        .iter()
        .map(|(user, player)| (user.clone(), player.reviews.clone()))
        .collect();
    let (active, mut days, latest) = activity(
        histories
            .iter()
            .map(|(user, reviews)| (user.as_str(), reviews.as_slice())),
        now,
    );
    let store = app.store.lock().unwrap();
    let conn = &store.conn;
    let first = days.first().map_or(0, |day| day.day);
    let signups: HashMap<i64, i64> = conn
        .prepare(
            "select created_at / ?2, count(*) from accounts where created_at >= ?1 * ?2 group by 1",
        )?
        .query_map(params![first, DAY_MS], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for day in &mut days {
        day.signups = signups.get(&day.day).map_or(0, |&n| n as usize);
    }
    let created: Vec<(String, i64)> = conn
        .prepare("select user, created_at from accounts")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut retention = Retention::default();
    for (user, created_at) in &created {
        if now - created_at >= RETENTION_MS {
            retention.eligible += 1;
            if latest
                .get(user.as_str())
                .is_some_and(|last| now - last < RETENTION_MS)
            {
                retention.retained += 1;
            }
        }
    }
    let sizes: Vec<i64> = conn
        .prepare(
            "select count(m.user) as size from groups g
             left join group_members m on m.group_id = g.id group by g.id order by size desc",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let groups = Groups {
        count: sizes.len(),
        members: conn.query_row("select count(distinct user) from group_members", [], |r| {
            r.get::<_, i64>(0)
        })? as usize,
        sizes,
        friendships: conn.query_row(
            "select count(*) from friendships where accepted = 1",
            [],
            |r| r.get(0),
        )?,
    };
    Ok(Stats {
        players,
        active,
        days,
        groups,
        retention,
        generated_at: now,
    })
}

#[derive(Debug, Serialize, PartialEq)]
pub struct LogEntry {
    pub at: i64,
    pub user: Option<String>,
    pub message: String,
}

/// The latest errors from the server log, newest first.
pub fn errors(conn: &Connection, since: i64) -> rusqlite::Result<Vec<LogEntry>> {
    conn.prepare(
        "select at, user, message from server_log where level = 'error' and at >= ?1
         order by id desc limit ?2",
    )?
    .query_map(params![since, LISTED], |r| {
        Ok(LogEntry {
            at: r.get(0)?,
            user: r.get(1)?,
            message: r.get(2)?,
        })
    })?
    .collect()
}

async fn page() -> Html<&'static str> {
    Html(web!("dev.html"))
}

async fn get_stats(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Stats>, StatusCode> {
    check(&app, &user, &headers)?;
    stats(&app, now_ms()).map(Json).map_err(store_error)
}

async fn get_groups(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<GroupSummary>>, StatusCode> {
    check(&app, &user, &headers)?;
    groups(&app.store.lock().unwrap().conn)
        .map(Json)
        .map_err(store_error)
}

async fn get_errors(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<LogEntry>>, StatusCode> {
    check(&app, &user, &headers)?;
    errors(&app.store.lock().unwrap().conn, now_ms() - 7 * DAY_MS)
        .map(Json)
        .map_err(store_error)
}

async fn list(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<Announcement>>, StatusCode> {
    check(&app, &user, &headers)?;
    announcements(&app.store.lock().unwrap().conn)
        .map(Json)
        .map_err(store_error)
}

#[derive(Debug, Serialize)]
struct Sent {
    id: i64,
    recipients: usize,
}

async fn send(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(draft): Json<Draft>,
) -> Result<Json<Sent>, StatusCode> {
    check(&app, &user, &headers)?;
    let title = draft.title.trim();
    let body = draft.body.trim();
    if title.is_empty()
        || title.chars().count() > MAX_TITLE
        || title.chars().any(char::is_control)
        || !valid_message(body)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut store = app.store.lock().unwrap();
    let Some(recipients) =
        recipients(&app, &store.conn, draft.groups.as_deref()).map_err(store_error)?
    else {
        return Err(StatusCode::BAD_REQUEST);
    };
    if recipients.is_empty() {
        return Err(StatusCode::CONFLICT);
    }
    let groups = draft.groups.unwrap_or_default();
    let id = announce(
        &mut store,
        &user,
        title,
        body,
        &groups,
        &recipients,
        now_ms(),
    )
    .map_err(store_error)?;
    drop(store);
    let audience = if groups.is_empty() {
        "everyone".to_string()
    } else {
        format!("groups {groups:?}")
    };
    log_info!(
        user: user,
        "announcement {id} to {audience} ({} players): {title}",
        recipients.len()
    );
    Ok(Json(Sent {
        id,
        recipients: recipients.len(),
    }))
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/dev", get(page))
        .route("/api/dev/{user}/stats", get(get_stats))
        .route("/api/dev/{user}/groups", get(get_groups))
        .route("/api/dev/{user}/errors", get(get_errors))
        .route("/api/dev/{user}/announcements", get(list).post(send))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Clock, ClockHistory, FreezePolicy};
    use axum::body::{Body, to_bytes};
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::Request;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::{Mutex, RwLock};
    use tower::ServiceExt;

    const NOW: i64 = 1_790_000_000_000;

    fn app_with(store: Store) -> App {
        let config = serde_json::from_value(serde_json::json!({
            "registration": true,
            "devs": ["hill", "nobody"],
            "users": {
                "hill": {"token": "hill-token"},
                "cerro": {"token": "cerro-token"},
                "quiet": {}
            }
        }))
        .unwrap();
        App {
            config,
            access: crate::access::Access::default(),
            accounts: crate::accounts::Directory::load(&store.conn).unwrap(),
            week: crate::game::Week::default(),
            store: Mutex::new(store),
            players: RwLock::new(HashMap::new()),
            standings: Default::default(),
            heartbeat: Default::default(),
        }
    }

    fn fixture() -> (Arc<App>, PathBuf) {
        let (store, path) = crate::decks::tests::temporary_store();
        (Arc::new(app_with(store)), path)
    }

    async fn call(
        app: &Arc<App>,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let request = request
            .body(Body::from(if method == "GET" {
                String::new()
            } else {
                body.to_string()
            }))
            .unwrap();
        let response = crate::router(app.clone())
            .layer(MockConnectInfo(
                "192.0.2.7:40000".parse::<SocketAddr>().unwrap(),
            ))
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    fn get<'a>(
        app: &'a Arc<App>,
        path: &'a str,
        token: Option<&'a str>,
    ) -> impl Future<Output = (StatusCode, serde_json::Value)> + 'a {
        call(app, "GET", path, token, serde_json::Value::Null)
    }

    fn group(conn: &Connection, name: &str, members: &[&str]) -> i64 {
        conn.execute(
            "insert into groups (name, invite, created_by, created_at) values (?1, ?1, ?2, 0)",
            params![name, members[0]],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        for member in members {
            conn.execute(
                "insert into group_members (group_id, user, joined_at) values (?1, ?2, 0)",
                params![id, member],
            )
            .unwrap();
        }
        id
    }

    fn inbox(app: &App) -> Vec<(String, String, String, String)> {
        app.store
            .lock()
            .unwrap()
            .conn
            .prepare(
                "select recipient, sender, kind, title from notifications order by recipient, id",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn review(id: i64) -> Review {
        Review {
            id,
            cid: 1,
            last_ivl: 0,
            ivl: Some(1),
            time_ms: 1_000,
            kind: 0,
        }
    }

    fn player(app: &App, user: &str, at: &[i64]) {
        app.players.write().unwrap().insert(
            user.into(),
            crate::app::Player {
                reviews: Arc::new(at.iter().copied().map(review).collect()),
                clock: Clock::default(),
                clock_history: ClockHistory::default(),
                freeze_policy: FreezePolicy::default(),
                revision: crate::app::next_revision(),
            },
        );
    }

    #[tokio::test]
    async fn only_devs_reach_the_console_with_their_own_credentials() {
        let (app, path) = fixture();
        for endpoint in ["stats", "groups", "errors", "announcements"] {
            let mine = format!("/api/dev/hill/{endpoint}");
            let theirs = format!("/api/dev/cerro/{endpoint}");
            assert_eq!(get(&app, &mine, None).await.0, StatusCode::UNAUTHORIZED);
            assert_eq!(
                get(&app, &mine, Some("cerro-token")).await.0,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(get(&app, &mine, Some("hill-token")).await.0, StatusCode::OK);
            assert_eq!(
                get(&app, &theirs, Some("cerro-token")).await.0,
                StatusCode::FORBIDDEN
            );
        }
        let draft = serde_json::json!({"title": "Hi", "body": "Hello"});
        let url = "/api/dev/cerro/announcements";
        assert_eq!(
            call(&app, "POST", url, Some("cerro-token"), draft).await.0,
            StatusCode::FORBIDDEN
        );
        assert!(inbox(&app).is_empty());
        assert_eq!(
            get(&app, "/auth/status", Some("hill-token")).await.1["dev"],
            true
        );
        let theirs = get(&app, "/auth/status", Some("cerro-token")).await.1;
        assert!(theirs.get("dev").is_none());
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn a_dev_name_cannot_be_taken_by_a_sign_up() {
        let (app, path) = fixture();
        let signup = |user: &str| serde_json::json!({"user": user, "password": "long enough pw"});
        assert_eq!(
            call(&app, "POST", "/api/accounts", None, signup("nobody"))
                .await
                .0,
            StatusCode::CONFLICT
        );
        assert!(
            call(&app, "POST", "/api/accounts", None, signup("somebody"))
                .await
                .0
                .is_success()
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn recipients_are_everyone_or_the_members_of_existing_groups() {
        let (app, path) = fixture();
        player(&app, "synced", &[]);
        let store = app.store.lock().unwrap();
        let conn = &store.conn;
        let book = group(conn, "Book club", &["hill", "cerro"]);
        let other = group(conn, "Other", &["quiet", "cerro", "gone"]);
        let names = |groups: Option<&[i64]>| {
            recipients(&app, conn, groups)
                .unwrap()
                .map(|set| set.into_iter().collect::<Vec<_>>())
        };
        assert_eq!(
            names(None),
            Some(vec![
                "cerro".into(),
                "hill".into(),
                "quiet".into(),
                "synced".into()
            ])
        );
        assert_eq!(
            names(Some(&[book])),
            Some(vec!["cerro".into(), "hill".into()])
        );
        assert_eq!(
            names(Some(&[book, other])),
            Some(vec!["cerro".into(), "hill".into(), "quiet".into()])
        );
        assert_eq!(names(Some(&[])), None);
        assert_eq!(names(Some(&[book, 999])), None);
        drop(store);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn an_announcement_lands_in_each_inbox_once_and_is_listed() {
        let (app, path) = fixture();
        let book = group(
            &app.store.lock().unwrap().conn,
            "Book club",
            &["hill", "cerro"],
        );
        let url = "/api/dev/hill/announcements";
        let draft = serde_json::json!({"title": " Update ", "body": "New quests this week", "groups": [book]});
        let (status, sent) = call(&app, "POST", url, Some("hill-token"), draft).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(sent["recipients"], 2);
        let expected = |to: &str| {
            (
                to.to_string(),
                String::new(),
                "message".to_string(),
                "Update".to_string(),
            )
        };
        assert_eq!(inbox(&app), vec![expected("cerro"), expected("hill")]);
        let items = get(&app, "/api/activity/cerro", Some("cerro-token"))
            .await
            .1;
        assert!(items.to_string().contains("New quests this week"));
        let draft = serde_json::json!({"title": "All", "body": "Everyone"});
        assert_eq!(
            call(&app, "POST", url, Some("hill-token"), draft).await.0,
            StatusCode::OK
        );
        assert_eq!(inbox(&app).len(), 5);
        for bad in [
            serde_json::json!({"title": "", "body": "x"}),
            serde_json::json!({"title": "x", "body": "two\nlines"}),
            serde_json::json!({"title": "x".repeat(MAX_TITLE + 1), "body": "x"}),
            serde_json::json!({"title": "x", "body": "x", "groups": []}),
            serde_json::json!({"title": "x", "body": "x", "groups": [999]}),
        ] {
            assert_eq!(
                call(&app, "POST", url, Some("hill-token"), bad).await.0,
                StatusCode::BAD_REQUEST
            );
        }
        assert_eq!(inbox(&app).len(), 5);
        let listed = get(&app, url, Some("hill-token")).await.1;
        assert_eq!(listed[0]["title"], "All");
        assert_eq!(listed[0]["groups"], serde_json::json!([]));
        assert_eq!(listed[0]["recipients"], 3);
        assert_eq!(
            listed[1]["groups"],
            serde_json::json!([{"id": book, "name": "Book club"}])
        );
        assert_eq!(listed[1]["sender"], "hill");
        let exported = crate::privacy::export(&app.store.lock().unwrap().conn, "hill").unwrap();
        assert_eq!(exported["announcements_sent"].as_array().unwrap().len(), 2);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn activity_counts_players_and_reviews_per_utc_day() {
        let today = NOW.div_euclid(DAY_MS) * DAY_MS;
        let hill: Vec<Review> = [
            today - 40 * DAY_MS,
            today - 2 * DAY_MS,
            today + 1,
            today + 2,
        ]
        .into_iter()
        .map(review)
        .collect();
        let cerro: Vec<Review> = [today - 10 * DAY_MS, today - 2 * DAY_MS + 5]
            .into_iter()
            .map(review)
            .collect();
        let (active, days, latest) = activity(
            [
                ("hill", hill.as_slice()),
                ("cerro", cerro.as_slice()),
                ("idle", &[][..]),
            ],
            NOW,
        );
        assert_eq!(
            active,
            Active {
                day: 1,
                week: 2,
                month: 2
            }
        );
        assert_eq!(days.len(), DAYS as usize);
        let at = |offset: i64| &days[(DAYS - 1 - offset) as usize];
        assert_eq!((at(0).reviews, at(0).players), (2, 1));
        assert_eq!((at(2).reviews, at(2).players), (2, 2));
        assert_eq!((at(10).reviews, at(10).players), (1, 1));
        assert_eq!(days.iter().map(|day| day.reviews).sum::<usize>(), 5);
        assert_eq!(latest.get("hill"), Some(&(today + 2)));
        assert!(!latest.contains_key("idle"));
    }

    #[test]
    fn stats_count_players_signups_groups_and_retention() {
        let (store, path) = crate::decks::tests::temporary_store();
        let now = now_ms();
        for (user, age) in [
            ("old", 20 * DAY_MS),
            ("older", 25 * DAY_MS),
            ("new", DAY_MS),
        ] {
            store
                .conn
                .execute(
                    "insert into accounts (user, display, password_hash, created_at)
                     values (?1, ?1, '', ?2)",
                    params![user, now - age],
                )
                .unwrap();
        }
        group(&store.conn, "A", &["hill", "cerro", "old"]);
        group(&store.conn, "B", &["hill"]);
        store
            .conn
            .execute(
                "insert into friendships (a, b, requested_by, accepted, created_at)
                 values ('cerro', 'hill', 'hill', 1, 0), ('cerro', 'quiet', 'quiet', 0, 0)",
                [],
            )
            .unwrap();
        let app = app_with(store);
        player(&app, "old", &[now - DAY_MS + 1000]);
        player(&app, "older", &[now - 9 * DAY_MS]);
        player(&app, "hill", &[now - 1000]);
        player(&app, "synced", &[]);
        let stats = stats(&app, now).unwrap();
        assert_eq!(
            stats.players,
            Players {
                total: 7,
                configured: 3,
                accounts: 3,
                synced: 1
            }
        );
        assert_eq!(
            stats.active,
            Active {
                day: 2,
                week: 2,
                month: 3
            }
        );
        assert_eq!(stats.days.iter().map(|day| day.signups).sum::<usize>(), 3);
        assert_eq!(
            stats.groups,
            Groups {
                count: 2,
                members: 3,
                sizes: vec![3, 1],
                friendships: 1
            }
        );
        assert_eq!(
            stats.retention,
            Retention {
                eligible: 2,
                retained: 1
            }
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
