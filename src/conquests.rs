//! Hard cards learned at last: a page to share each one, and telling friends about it, which
//! the owner can take back for a day, from friends who have not read it yet.
//! Only what [`Conquest`] carries is shared, never the card itself, unless its owner
//! chooses to add a label saying what the card was. The label also appears on the card and
//! page of the matching `conquered` win (see `wins::shared_words`).
use crate::access::require_owner;
use crate::game::{self, Conquest};
use crate::notifications::Outgoing;
use crate::store::Error;
use crate::{access, access::authorized, app::App, app::now_ms, web_util::store_error};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Long enough for a word with its translation, short enough to stay a label.
pub const MAX_LABEL: usize = 120;

/// How long after telling friends the owner can take it back.
pub const UNDO_MS: i64 = 24 * 60 * 60 * 1000;

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists conquest_labels (
             user text not null,
             at integer not null,
             label text not null,
             primary key (user, at)
         ) without rowid;
         create table if not exists conquest_tells (
             user text not null,
             at integer not null,
             told_at integer not null,
             primary key (user, at)
         ) without rowid;
         create table if not exists conquest_tell_rows (
             user text not null,
             at integer not null,
             notification integer not null,
             primary key (user, at, notification)
         ) without rowid;",
    )?;
    Ok(())
}

pub(crate) fn label_of(conn: &Connection, user: &str, at: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "select label from conquest_labels where user = ?1 and at = ?2",
        params![user, at],
        |r| r.get(0),
    )
    .optional()
}

/// One line of plain text, or `None` when nothing printable is left.
fn clean_label(raw: &str) -> Option<String> {
    let label = raw
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!label.is_empty() && label.chars().count() <= MAX_LABEL).then_some(label)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LabelUpdate {
    pub(crate) label: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct Shared {
    user: String,
    display: String,
    pub(crate) conquest: Conquest,
    /// What the card was, when its owner chose to say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    /// Present for the owner only: whether friends have been told, and how many there are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) told: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) friends: Option<usize>,
    /// For the owner only, once the worker has kept the conquest as a win: its id, so the
    /// app can share the win's card (`POST /api/share/{user}` with `to: "link"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) win: Option<i64>,
    /// For the owner only, while telling friends can still be taken back: until when (ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) undo_until: Option<i64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Told {
    pub(crate) told: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct Untold {
    /// The friends whose unread row was taken back.
    pub(crate) retracted: usize,
}

/// Until when the owner can take back telling friends about the card conquered at `at`:
/// a day after telling them, unless a friend has cheered or answered it since.
fn undo_until(conn: &Connection, user: &str, at: i64, now: i64) -> rusqlite::Result<Option<i64>> {
    let Some(told_at) = conn
        .query_row(
            "select told_at from conquest_tells where user = ?1 and at = ?2",
            params![user, at],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let answered: bool = conn.query_row(
        "select exists (select 1 from conquest_tell_rows t join notifications n on n.id = t.notification
             where t.user = ?1 and t.at = ?2 and (n.replied = 1
                 or exists (select 1 from reactions r where r.notification = n.id)))",
        params![user, at],
        |r| r.get(0),
    )?;
    let until = told_at + UNDO_MS;
    Ok((!answered && now < until).then_some(until))
}

fn told_key(at: i64) -> String {
    format!("conquest-told:{at}")
}

fn find(app: &App, user: &str, at: i64) -> Option<Conquest> {
    let players = app.players.read().unwrap();
    let reviews = players.get(user)?.reviews.clone();
    drop(players);
    game::conquests(&reviews, now_ms())
        .into_iter()
        .find(|c| c.at == at)
}

async fn page() -> Html<&'static str> {
    Html(web!("conquered.html"))
}

pub(crate) async fn show(
    State(app): State<Arc<App>>,
    Path((user, at)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Result<Json<Shared>, StatusCode> {
    let viewer = access::viewer(&app, &headers);
    if !app
        .visible(&app.store.lock().unwrap().conn, &viewer, &user)
        .map_err(store_error)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let conquest = find(&app, &user, at).ok_or(StatusCode::NOT_FOUND)?;
    let label = {
        let conn = &app.store.lock().unwrap().conn;
        // Profiles became public by default after some labels were written for friends only.
        if app
            .visible_by_choice(conn, &viewer, &user)
            .map_err(store_error)?
        {
            label_of(conn, &user, at).map_err(store_error)?
        } else {
            None
        }
    };
    let (told, friends, win, undo) = if authorized(&app, &user, &headers) {
        let store = app.store.lock().unwrap();
        let told = store.is_seen(&user, &told_key(at)).map_err(store_error)?;
        let friends = crate::social::friends(&store.conn, &user)
            .map_err(store_error)?
            .len();
        let win = own_win(&store, &user, &conquest).map_err(store_error)?;
        let undo = undo_until(&store.conn, &user, at, now_ms()).map_err(store_error)?;
        (Some(told), Some(friends), win, undo)
    } else {
        (None, None, None, None)
    };
    Ok(Json(Shared {
        display: app.display(&user),
        user,
        conquest,
        label,
        told,
        friends,
        win,
        undo_until: undo,
    }))
}

/// The owner's win for `conquest`, so its card can be shared. A conquest the server already
/// announced without keeping a win, from before wins were kept, gets one now, quietly and with
/// no stats of a day long gone. One not announced yet waits for the worker, which keeps it.
fn own_win(
    store: &crate::store::Store,
    user: &str,
    conquest: &Conquest,
) -> Result<Option<i64>, Error> {
    let event = game::conquest_event(conquest);
    if let Some(id) = crate::wins::id_of(&store.conn, user, &event.key)? {
        return Ok(Some(id));
    }
    if !store.is_seen(user, &event.key)? {
        return Ok(None);
    }
    let day = store.clock(user)?.day(conquest.at);
    Ok(crate::wins::keep(
        &store.conn,
        user,
        &event.key,
        day,
        conquest.at,
        &event.title,
        &event.body,
        &serde_json::json!({}),
    )?)
}

/// Sends the conquest to each friend's activity, once, however often it is asked.
pub(crate) async fn tell(
    State(app): State<Arc<App>>,
    Path((user, at)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Result<Json<Told>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let conquest = find(&app, &user, at).ok_or(StatusCode::NOT_FOUND)?;
    let display = app.display(&user);
    let title = if conquest.leech {
        format!("{display} tamed a leech")
    } else {
        format!("{display} conquered a hard card")
    };
    let now = now_ms();
    let mut store = app.store.lock().unwrap();
    let body = match label_of(&store.conn, &user, at).map_err(store_error)? {
        Some(label) => format!(
            "They forgot “{label}” {} times and learned it anyway.",
            conquest.lapses
        ),
        None => format!(
            "They forgot it {} times and learned it anyway.",
            conquest.lapses
        ),
    };
    let day = store.clock(&user).map_err(store_error)?.day(now);
    let friends = crate::social::friends(&store.conn, &user).map_err(store_error)?;
    let tx = store.conn.transaction().map_err(store_error)?;
    for friend in &friends {
        if crate::social::muted(&tx, friend, &user).map_err(store_error)? {
            continue;
        }
        // Once per friend, as `Store::send_once`, keeping the row so it can be taken back.
        if tx
            .execute(
                "insert or ignore into seen (user, key) values (?1, ?2)",
                params![friend, friend_key(&user, at)],
            )
            .map_err(store_error)?
            == 0
        {
            continue;
        }
        let id = crate::notifications::insert_notification(
            &tx,
            &Outgoing {
                to: friend,
                from: &user,
                title: &title,
                body: &body,
                kind: "conquest",
            },
            day,
            now,
            false,
        )
        .map_err(store_error)?;
        tx.execute(
            "insert or ignore into conquest_tell_rows (user, at, notification) values (?1, ?2, ?3)",
            params![user, at, id],
        )
        .map_err(store_error)?;
    }
    if tx
        .execute(
            "insert or ignore into seen (user, key) values (?1, ?2)",
            params![user, told_key(at)],
        )
        .map_err(store_error)?
        > 0
    {
        tx.execute(
            "insert or replace into conquest_tells (user, at, told_at) values (?1, ?2, ?3)",
            params![user, at, now],
        )
        .map_err(store_error)?;
    }
    tx.commit().map_err(store_error)?;
    Ok(Json(Told {
        told: friends.len(),
    }))
}

fn friend_key(user: &str, at: i64) -> String {
    format!("conquest:{user}:{at}")
}

/// Takes telling friends back, within [`UNDO_MS`] and until a friend cheered or answered it:
/// rows friends have not read yet leave their activity, and the owner can tell them again.
/// Rows already read stay, and those friends are not told twice. Asking again, or about a
/// card friends were never told of, takes nothing back; too late is `409`.
pub(crate) async fn untell(
    State(app): State<Arc<App>>,
    Path((user, at)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Result<Json<Untold>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    find(&app, &user, at).ok_or(StatusCode::NOT_FOUND)?;
    let mut store = app.store.lock().unwrap();
    if !store.is_seen(&user, &told_key(at)).map_err(store_error)? {
        return Ok(Json(Untold { retracted: 0 }));
    }
    if undo_until(&store.conn, &user, at, now_ms())
        .map_err(store_error)?
        .is_none()
    {
        return Err(StatusCode::CONFLICT);
    }
    let tx = store.conn.transaction().map_err(store_error)?;
    let unread: Vec<(i64, String)> = tx
        .prepare(
            "select n.id, n.recipient from conquest_tell_rows t join notifications n on n.id = t.notification
             where t.user = ?1 and t.at = ?2 and n.read_at is null",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![user, at], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect()
        })
        .map_err(store_error)?;
    for (id, recipient) in &unread {
        tx.execute("delete from notifications where id = ?1", [id])
            .map_err(store_error)?;
        tx.execute(
            "delete from seen where user = ?1 and key = ?2",
            params![recipient, friend_key(&user, at)],
        )
        .map_err(store_error)?;
    }
    for sql in [
        "delete from conquest_tell_rows where user = ?1 and at = ?2",
        "delete from conquest_tells where user = ?1 and at = ?2",
    ] {
        tx.execute(sql, params![user, at]).map_err(store_error)?;
    }
    tx.execute(
        "delete from seen where user = ?1 and key = ?2",
        params![user, told_key(at)],
    )
    .map_err(store_error)?;
    tx.commit().map_err(store_error)?;
    Ok(Json(Untold {
        retracted: unread.len(),
    }))
}

/// Says what the card was. Only its owner can, and only for a card they conquered.
pub(crate) async fn set_label(
    State(app): State<Arc<App>>,
    Path((user, at)): Path<(String, i64)>,
    headers: HeaderMap,
    Json(update): Json<LabelUpdate>,
) -> Result<StatusCode, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let label = clean_label(&update.label).ok_or(StatusCode::BAD_REQUEST)?;
    find(&app, &user, at).ok_or(StatusCode::NOT_FOUND)?;
    app.store
        .lock()
        .unwrap()
        .conn
        .execute(
            "insert into conquest_labels (user, at, label) values (?1, ?2, ?3)
             on conflict (user, at) do update set label = excluded.label",
            params![user, at, label],
        )
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Takes the label back. Friends already told keep the message they received.
pub(crate) async fn remove_label(
    State(app): State<Arc<App>>,
    Path((user, at)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    require_owner(&app, &user, &headers)?;
    app.store
        .lock()
        .unwrap()
        .conn
        .execute(
            "delete from conquest_labels where user = ?1 and at = ?2",
            params![user, at],
        )
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/conquered/{user}/{at}", get(page))
        .route("/api/conquests/{user}/{at}", get(show))
        .route("/api/conquests/{user}/{at}/tell", post(tell).delete(untell))
        .route(
            "/api/conquests/{user}/{at}/label",
            post(set_label).delete(remove_label),
        )
}
