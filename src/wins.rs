//! Wins: every celebratory moment the server notices, kept per player so it can reach
//! friends in AnkiQuest or go anywhere as a card behind an unguessable link. Each kind has
//! a sharing policy the player chooses. See `docs/sharing.md`.

use crate::access::authorized;
use crate::app::App;
use crate::game::Profile;
use crate::notifications::Notification;
use crate::store::Error;
use crate::web_util::{base_url, base64url, random_bytes, store_error};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Every kind of win with its default policy.
pub(crate) const KINDS: [(&str, &str); 10] = [
    ("daily_goal", "ask"),
    ("streak_secured", "private"),
    ("streak", "ask"),
    ("level", "ask"),
    ("achievement", "ask"),
    ("record", "ask"),
    ("quest", "private"),
    ("conquered", "ask"),
    ("place", "ask"),
    ("challenge", "ask"),
];

const POLICIES: [&str; 3] = ["private", "ask", "auto"];
const DEFAULT_CAP: u32 = 3;

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists wins (
             id integer primary key,
             user text not null,
             kind text not null,
             key text not null,
             day integer not null,
             created_at integer not null,
             title text not null,
             body text not null,
             stats text not null,
             art text not null,
             shared_friends integer not null default 0,
             link_token text unique,
             unique (user, key)
         );
         create table if not exists win_deliveries (
             win integer not null,
             recipient text not null,
             notification integer not null,
             day integer not null,
             automatic integer not null,
             primary key (win, recipient)
         ) without rowid;
         create index if not exists win_deliveries_recipient on win_deliveries (recipient, day);
         create table if not exists win_auto_queue (win integer primary key);
         create table if not exists sharing_settings (
             user text primary key,
             policies text not null default '{}',
             daily_cap integer not null default 3
         ) without rowid;
         create table if not exists reactions (
             reactor text not null,
             notification integer not null,
             sender text not null,
             created_at integer not null,
             primary key (reactor, notification)
         ) without rowid;",
    )?;
    Ok(())
}

/// The kind of win an event key starts, if it is one.
pub(crate) fn kind_of(key: &str) -> Option<&'static str> {
    Some(match key.split_once(':')?.0 {
        "goal" => "daily_goal",
        "secured" => "streak_secured",
        "streak" => "streak",
        "level" => "level",
        "achievement" => "achievement",
        "record" => "record",
        "quest" => "quest",
        "conquered" => "conquered",
        "place" => "place",
        "challenge" => "challenge",
        _ => return None,
    })
}

/// The companion pose a card of `kind` shows. Ankilope only has `celebrate`.
pub(crate) fn art(kind: &str, companion: &str) -> &'static str {
    let pose = match kind {
        "streak" | "streak_secured" => "streak",
        "place" | "record" => "winner",
        _ => "celebrate",
    };
    if companion == "ankilope" {
        "celebrate"
    } else {
        pose
    }
}

/// What every win records about the player at that moment.
pub(crate) fn stats(profile: &Profile) -> Value {
    json!({
        "streak": profile.streak,
        "level": profile.level,
        "reviews_today": profile.today.reviews,
        "minutes_today": profile.today.minutes,
    })
}

/// One stored win.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Win {
    pub(crate) id: i64,
    pub(crate) user: String,
    pub(crate) kind: String,
    pub(crate) key: String,
    pub(crate) day: i64,
    /// Milliseconds.
    pub(crate) created_at: i64,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) stats: Value,
    pub(crate) art: String,
    /// How many friends it reached.
    pub(crate) shared_friends: i64,
    pub(crate) link_token: Option<String>,
}

impl Win {
    /// The topic a notification about this win carries, so clients show the same art.
    pub(crate) fn topic(&self) -> String {
        match (self.kind.as_str(), self.stats["place"].as_u64()) {
            ("place", Some(place)) => format!("place:{place}"),
            _ => self
                .key
                .split_once(':')
                .map_or(&*self.key, |(t, _)| t)
                .into(),
        }
    }
}

const COLUMNS: &str =
    "id, user, kind, key, day, created_at, title, body, stats, art, shared_friends, link_token";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Win> {
    let stats: String = r.get(8)?;
    Ok(Win {
        id: r.get(0)?,
        user: r.get(1)?,
        kind: r.get(2)?,
        key: r.get(3)?,
        day: r.get(4)?,
        created_at: r.get(5)?,
        title: r.get(6)?,
        body: r.get(7)?,
        stats: serde_json::from_str(&stats).unwrap_or(Value::Null),
        art: r.get(9)?,
        shared_friends: r.get(10)?,
        link_token: r.get(11)?,
    })
}

fn find(conn: &Connection, user: &str, id: i64) -> rusqlite::Result<Option<Win>> {
    conn.query_row(
        &format!("select {COLUMNS} from wins where id = ?1 and user = ?2"),
        params![id, user],
        row,
    )
    .optional()
}

/// `user`'s win `id`, or `None` when it is someone else's or gone.
pub(crate) fn win(conn: &Connection, user: &str, id: i64) -> Option<Win> {
    find(conn, user, id).unwrap_or_else(|error| {
        log_error!("reading win {id} failed: {error}");
        None
    })
}

/// The win a public link points to, with its owner, while the link exists.
#[allow(dead_code)]
pub(crate) fn by_token(conn: &Connection, token: &str) -> Option<(String, Win)> {
    conn.query_row(
        &format!("select {COLUMNS} from wins where link_token = ?1"),
        [token],
        row,
    )
    .optional()
    .unwrap_or_else(|error| {
        log_error!("reading a win link failed: {error}");
        None
    })
    .map(|win| (win.user.clone(), win))
}

/// The link token of `user`'s win `id`, made once; `None` when the win is not theirs.
pub(crate) fn ensure_link(
    conn: &Connection,
    user: &str,
    id: i64,
) -> rusqlite::Result<Option<String>> {
    let Some(win) = find(conn, user, id)? else {
        return Ok(None);
    };
    if let Some(token) = win.link_token {
        return Ok(Some(token));
    }
    let bytes = random_bytes("link randomness unavailable")
        .map_err(rusqlite::Error::ToSqlConversionFailure)?;
    let token = base64url(&bytes[..18]);
    conn.execute(
        "update wins set link_token = ?3 where id = ?1 and user = ?2",
        params![id, user, token],
    )?;
    Ok(Some(token))
}

/// Forgets the link of `user`'s win `id`; `false` when the win is not theirs.
pub(crate) fn revoke_link(conn: &Connection, user: &str, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "update wins set link_token = null where id = ?1 and user = ?2",
        params![id, user],
    )? > 0)
}

fn companion(conn: &Connection, user: &str) -> rusqlite::Result<String> {
    Ok(conn
        .query_row(
            "select companion from companion_preferences where user = ?1",
            [user],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_else(|| crate::companions::DEFAULT.into()))
}

/// A win for the moment `key`, once per player and key. Returns its id when it is new.
/// A win whose policy is `auto` waits for the next tick to reach friends.
#[allow(clippy::too_many_arguments)]
pub(crate) fn create(
    conn: &Connection,
    user: &str,
    key: &str,
    day: i64,
    now_ms: i64,
    title: &str,
    body: &str,
    stats: &Value,
) -> rusqlite::Result<Option<i64>> {
    let Some(kind) = kind_of(key) else {
        return Ok(None);
    };
    let art = art(kind, &companion(conn, user)?);
    if conn.execute(
        "insert or ignore into wins (user, kind, key, day, created_at, title, body, stats, art)
         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            user,
            kind,
            key,
            day,
            now_ms,
            title,
            body,
            stats.to_string(),
            art
        ],
    )? == 0
    {
        return Ok(None);
    }
    let id = conn.last_insert_rowid();
    if policy(conn, user, kind)? == "auto" {
        conn.execute("insert into win_auto_queue (win) values (?1)", [id])?;
    }
    Ok(Some(id))
}

/// The id of `user`'s win for `key`, if there is one.
pub(crate) fn id_of(conn: &Connection, user: &str, key: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "select id from wins where user = ?1 and key = ?2",
        params![user, key],
        |r| r.get(0),
    )
    .optional()
}

/// Marks notification `id` as being about win `win`.
pub(crate) fn attach(conn: &Connection, id: i64, win: i64) -> rusqlite::Result<()> {
    conn.execute(
        "update notifications set win_id = ?2 where id = ?1",
        params![id, win],
    )?;
    Ok(())
}

/// Marks the weekly recap that announced a place, sent after `since`, as being about its win.
pub(crate) fn attach_recap(
    conn: &Connection,
    user: &str,
    win: i64,
    topic: &str,
    since: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "update notifications set win_id = ?2 where recipient = ?1 and kind = 'reminder_weekly_recap'
         and topic = ?3 and win_id is null and created_at >= ?4",
        params![user, win, topic, since],
    )?;
    Ok(())
}

/// Takes `user`'s automatic wins that have not reached friends yet, oldest first.
pub(crate) fn take_automatic(conn: &Connection, user: &str) -> rusqlite::Result<Vec<Win>> {
    let wins = conn
        .prepare(&format!(
            "select {COLUMNS} from wins where user = ?1
             and id in (select win from win_auto_queue) order by id"
        ))?
        .query_map([user], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for win in &wins {
        conn.execute("delete from win_auto_queue where win = ?1", [win.id])?;
    }
    Ok(wins)
}

/// The player's sharing settings: a policy for every kind, and the daily cap.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Sharing {
    pub(crate) policies: BTreeMap<String, String>,
    pub(crate) daily_cap: u32,
}

pub(crate) fn sharing(conn: &Connection, user: &str) -> rusqlite::Result<Sharing> {
    let (stored, daily_cap): (String, u32) = conn
        .query_row(
            "select policies, daily_cap from sharing_settings where user = ?1",
            [user],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or_else(|| ("{}".into(), DEFAULT_CAP));
    let chosen: BTreeMap<String, String> = serde_json::from_str(&stored).unwrap_or_default();
    let policies = KINDS
        .iter()
        .map(|(kind, default)| {
            let policy = chosen
                .get(*kind)
                .filter(|policy| POLICIES.contains(&policy.as_str()))
                .map_or(*default, String::as_str);
            (kind.to_string(), policy.to_string())
        })
        .collect();
    Ok(Sharing {
        policies,
        daily_cap,
    })
}

pub(crate) fn policy(conn: &Connection, user: &str, kind: &str) -> rusqlite::Result<String> {
    Ok(sharing(conn, user)?
        .policies
        .remove(kind)
        .unwrap_or_else(|| "private".into()))
}

/// The public page and image of a link, under `base`.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Link {
    pub(crate) url: String,
    pub(crate) image: String,
}

pub(crate) fn link(base: &str, token: &str) -> Link {
    Link {
        url: format!("{base}/w/{token}"),
        image: format!("{base}/w/{token}.png"),
    }
}

/// Gives a friend's `share` row the card image of its win, while the link exists.
pub(crate) fn decorate(notice: &mut Notification, base: &str) {
    if notice.kind == "share"
        && let Some(token) = &notice.link_token
    {
        notice.image = Some(link(base, token).image);
    }
}

#[derive(Debug, Serialize)]
struct Listed {
    id: i64,
    kind: String,
    key: String,
    day: i64,
    /// Seconds, like the inbox.
    created_at: i64,
    title: String,
    body: String,
    stats: Value,
    art: String,
    policy: String,
    shared_with_friends: bool,
    link: Option<Link>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    limit: Option<u32>,
    before: Option<i64>,
}

fn list(
    conn: &Connection,
    user: &str,
    limit: u32,
    before: Option<i64>,
) -> rusqlite::Result<Vec<Win>> {
    conn.prepare(&format!(
        "select {COLUMNS} from wins where user = ?1 and (?2 is null or id < ?2)
         order by id desc limit ?3"
    ))?
    .query_map(params![user, before, limit], row)?
    .collect()
}

async fn get_wins(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    Query(page): Query<Page>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let limit = page.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) || page.before.is_some_and(|id| id <= 0) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let base = base_url(&app, &headers);
    let store = app.store.lock().unwrap();
    let language = crate::i18n::owned(&headers, &store, &user).map_err(store_error)?;
    let policies = sharing(&store.conn, &user).map_err(store_error)?.policies;
    let wins = list(&store.conn, &user, limit, page.before).map_err(store_error)?;
    let wins: Vec<Listed> = wins
        .into_iter()
        .map(|win| Listed {
            policy: policies.get(&win.kind).cloned().unwrap_or_default(),
            link: win.link_token.as_deref().map(|token| link(&base, token)),
            id: win.id,
            created_at: win.created_at / 1000,
            title: crate::i18n::system(&win.title, &language),
            body: crate::i18n::system(&win.body, &language),
            kind: win.kind,
            key: win.key,
            day: win.day,
            stats: win.stats,
            art: win.art,
            shared_with_friends: win.shared_friends > 0,
        })
        .collect();
    Ok(Json(json!({ "wins": wins })))
}

async fn delete_link(
    State(app): State<Arc<App>>,
    Path((user, id)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let store = app.store.lock().unwrap();
    if revoke_link(&store.conn, &user, id).map_err(store_error)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn get_sharing(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Sharing>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let store = app.store.lock().unwrap();
    Ok(Json(sharing(&store.conn, &user).map_err(store_error)?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SharingUpdate {
    #[serde(default)]
    pub(crate) policies: BTreeMap<String, String>,
    pub(crate) daily_cap: Option<u32>,
}

/// Applies `update` over the current settings; `None` when a kind, policy or cap is invalid.
pub(crate) fn update_sharing(
    conn: &Connection,
    user: &str,
    update: SharingUpdate,
) -> rusqlite::Result<Option<Sharing>> {
    let valid = update.policies.iter().all(|(kind, policy)| {
        KINDS.iter().any(|(known, _)| known == kind) && POLICIES.contains(&policy.as_str())
    }) && update.daily_cap.is_none_or(|cap| (1..=10).contains(&cap));
    if !valid {
        return Ok(None);
    }
    let mut current = sharing(conn, user)?;
    current.policies.extend(update.policies);
    if let Some(cap) = update.daily_cap {
        current.daily_cap = cap;
    }
    conn.execute(
        "insert into sharing_settings (user, policies, daily_cap) values (?1, ?2, ?3)
         on conflict (user) do update set policies = excluded.policies, daily_cap = excluded.daily_cap",
        params![
            user,
            serde_json::to_string(&current.policies).unwrap_or_default(),
            current.daily_cap
        ],
    )?;
    Ok(Some(current))
}

async fn set_sharing(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(update): Json<SharingUpdate>,
) -> Result<Json<Sharing>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let store = app.store.lock().unwrap();
    update_sharing(&store.conn, &user, update)
        .map_err(store_error)?
        .map(Json)
        .ok_or(StatusCode::BAD_REQUEST)
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/wins/{user}", get(get_wins))
        .route("/api/wins/{user}/{id}/link", delete(delete_link))
        .route("/api/sharing/{user}", get(get_sharing).put(set_sharing))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_support::*;

    pub(crate) fn moment(store: &Store, user: &str, key: &str) -> i64 {
        create(
            &store.conn,
            user,
            key,
            20_000,
            1_000,
            &format!("Title {key}"),
            "Body",
            &json!({"streak": 3}),
        )
        .unwrap()
        .unwrap()
    }

    #[test]
    fn moments_map_to_kinds_and_art() {
        for (key, kind, pose) in [
            ("goal:1", "daily_goal", "celebrate"),
            ("secured:1", "streak_secured", "streak"),
            ("streak:1:30", "streak", "streak"),
            ("level:5", "level", "celebrate"),
            ("achievement:x", "achievement", "celebrate"),
            ("record:day:1", "record", "winner"),
            ("quest:1:0", "quest", "celebrate"),
            ("conquered:7", "conquered", "celebrate"),
            ("place:1:2", "place", "winner"),
            ("challenge:4", "challenge", "celebrate"),
        ] {
            assert_eq!(kind_of(key), Some(kind), "{key}");
            assert_eq!(art(kind, "aki"), pose);
            assert_eq!(art(kind, "ankilope"), "celebrate");
            assert!(KINDS.iter().any(|(known, _)| *known == kind));
        }
        assert_eq!(kind_of("nudge:place:1"), None);
        assert_eq!(kind_of("reminder:daily:1"), None);
    }

    #[test]
    fn a_win_is_made_once_and_its_link_is_unguessable_reusable_and_revocable() {
        let (store, path) = decks::tests::temporary_store();
        store.set_companion("hill", "ankilope").unwrap();
        let id = moment(&store, "hill", "record:day:20000");
        assert_eq!(
            create(
                &store.conn,
                "hill",
                "record:day:20000",
                1,
                1,
                "t",
                "b",
                &json!({})
            )
            .unwrap(),
            None
        );
        let found = win(&store.conn, "hill", id).unwrap();
        assert_eq!(found.kind, "record");
        assert_eq!(found.art, "celebrate");
        assert_eq!(found.stats["streak"], 3);
        assert!(win(&store.conn, "cerro", id).is_none());
        assert_eq!(ensure_link(&store.conn, "cerro", id).unwrap(), None);
        let token = ensure_link(&store.conn, "hill", id).unwrap().unwrap();
        assert!(token.len() >= 24, "{token}");
        assert_eq!(
            ensure_link(&store.conn, "hill", id).unwrap(),
            Some(token.clone())
        );
        let (owner, linked) = by_token(&store.conn, &token).unwrap();
        assert_eq!((owner.as_str(), linked.id), ("hill", id));
        let other = moment(&store, "hill", "level:3");
        assert_ne!(
            ensure_link(&store.conn, "hill", other).unwrap(),
            Some(token.clone())
        );
        assert!(!revoke_link(&store.conn, "cerro", id).unwrap());
        assert!(revoke_link(&store.conn, "hill", id).unwrap());
        assert!(by_token(&store.conn, &token).is_none());
        assert!(
            create(&store.conn, "hill", "nudge:x", 1, 1, "t", "b", &json!({}))
                .unwrap()
                .is_none()
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn sharing_settings_default_merge_and_reject_unknowns() {
        let (store, path) = decks::tests::temporary_store();
        let defaults = sharing(&store.conn, "hill").unwrap();
        assert_eq!(defaults.daily_cap, 3);
        assert_eq!(defaults.policies.len(), KINDS.len());
        assert_eq!(defaults.policies["quest"], "private");
        assert_eq!(defaults.policies["daily_goal"], "ask");
        let update = |policies: &[(&str, &str)], daily_cap| SharingUpdate {
            policies: policies
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            daily_cap,
        };
        let changed = update_sharing(&store.conn, "hill", update(&[("level", "auto")], Some(5)))
            .unwrap()
            .unwrap();
        assert_eq!(changed.policies["level"], "auto");
        assert_eq!(changed.policies["streak"], "ask");
        let kept = update_sharing(&store.conn, "hill", update(&[("quest", "ask")], None))
            .unwrap()
            .unwrap();
        assert_eq!(
            (kept.policies["level"].as_str(), kept.daily_cap),
            ("auto", 5)
        );
        assert_eq!(sharing(&store.conn, "hill").unwrap(), kept);
        for bad in [
            update(&[("deck", "auto")], None),
            update(&[("level", "loud")], None),
            update(&[], Some(0)),
            update(&[], Some(11)),
        ] {
            assert!(update_sharing(&store.conn, "hill", bad).unwrap().is_none());
        }
        assert_eq!(sharing(&store.conn, "cerro").unwrap(), defaults);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn automatic_wins_wait_in_a_queue_taken_once() {
        let (store, path) = decks::tests::temporary_store();
        update_sharing(
            &store.conn,
            "hill",
            SharingUpdate {
                policies: [("level".to_string(), "auto".to_string())].into(),
                daily_cap: None,
            },
        )
        .unwrap();
        let auto = moment(&store, "hill", "level:4");
        moment(&store, "hill", "achievement:x");
        let taken = take_automatic(&store.conn, "hill").unwrap();
        assert_eq!(taken.iter().map(|w| w.id).collect::<Vec<_>>(), [auto]);
        assert!(take_automatic(&store.conn, "hill").unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn the_list_is_private_newest_first_and_pages() {
        let (app, path) = fixture();
        let ids: Vec<i64> = {
            let store = app.store.lock().unwrap();
            ["level:2", "level:3", "level:4"]
                .iter()
                .map(|key| moment(&store, "hill", key))
                .collect()
        };
        let page = |user: &str, limit, before| {
            let app = app.clone();
            let user = user.to_string();
            async move {
                get_wins(
                    State(app),
                    UrlPath(user.clone()),
                    Query(Page { limit, before }),
                    headers(&user),
                )
                .await
                .map(|Json(page)| page["wins"].as_array().unwrap().clone())
            }
        };
        let id = |win: &Value| win["id"].as_i64().unwrap();
        let all = page("hill", None, None).await.unwrap();
        assert_eq!(all.iter().map(id).rev().collect::<Vec<_>>(), ids);
        assert_eq!(all[0]["policy"], "ask");
        assert_eq!(all[0]["created_at"], 1);
        assert_eq!(all[0]["link"], Value::Null);
        assert_eq!(all[0]["shared_with_friends"], false);
        let older = page("hill", Some(1), Some(ids[2])).await.unwrap();
        assert_eq!(older.iter().map(id).collect::<Vec<_>>(), [ids[1]]);
        assert!(page("cerro", None, None).await.unwrap().is_empty());
        assert_eq!(
            page("hill", Some(0), None).await.err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            get_wins(
                State(app.clone()),
                UrlPath("hill".into()),
                Query(Page {
                    limit: None,
                    before: None
                }),
                headers("cerro"),
            )
            .await
            .err(),
            Some(StatusCode::UNAUTHORIZED)
        );
        app.store
            .lock()
            .unwrap()
            .conn
            .execute("update wins set link_token = 'tok' where id = ?1", [ids[2]])
            .unwrap();
        let linked = page("hill", Some(1), None).await.unwrap();
        assert_eq!(linked[0]["link"]["image"], "/w/tok.png");
        let revoke = |user: &str, id| {
            delete_link(
                State(app.clone()),
                UrlPath((user.to_string(), id)),
                headers(user),
            )
        };
        assert_eq!(
            revoke("cerro", ids[2]).await.err(),
            Some(StatusCode::NOT_FOUND)
        );
        assert_eq!(
            revoke("hill", ids[2]).await.unwrap(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            page("hill", Some(1), None).await.unwrap()[0]["link"],
            Value::Null
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
