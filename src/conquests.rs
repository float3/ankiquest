//! Hard cards learned at last: a page to share each one, and telling friends about it.
//! Only what [`Conquest`] carries is shared, never the card itself, unless its owner
//! chooses to add a label saying what the card was.
use crate::decks::Outgoing;
use crate::game::{self, Conquest};
use crate::store::Error;
use crate::{App, access, authorized, now_ms, store_error};
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

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists conquest_labels (
             user text not null,
             at integer not null,
             label text not null,
             primary key (user, at)
         ) without rowid;",
    )?;
    Ok(())
}

fn label_of(conn: &Connection, user: &str, at: i64) -> rusqlite::Result<Option<String>> {
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
}

#[derive(Debug, Serialize)]
pub(crate) struct Told {
    pub(crate) told: usize,
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
    let label = label_of(&app.store.lock().unwrap().conn, &user, at).map_err(store_error)?;
    let (told, friends) = if authorized(&app, &user, &headers) {
        let store = app.store.lock().unwrap();
        let told: bool = store
            .conn
            .query_row(
                "select exists (select 1 from seen where user = ?1 and key = ?2)",
                [user.as_str(), &told_key(at)],
                |r| r.get(0),
            )
            .map_err(store_error)?;
        let friends = crate::social::friends(&store.conn, &user)
            .map_err(store_error)?
            .len();
        (Some(told), Some(friends))
    } else {
        (None, None)
    };
    Ok(Json(Shared {
        display: app.display(&user),
        user,
        conquest,
        label,
        told,
        friends,
    }))
}

/// Sends the conquest to each friend's activity, once, however often it is asked.
pub(crate) async fn tell(
    State(app): State<Arc<App>>,
    Path((user, at)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Result<Json<Told>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
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
    for friend in &friends {
        store
            .send_once(
                &Outgoing {
                    to: friend,
                    from: &user,
                    title: &title,
                    body: &body,
                    kind: "conquest",
                },
                &format!("conquest:{user}:{at}"),
                day,
                now,
                false,
            )
            .map_err(store_error)?;
    }
    store.mark_seen(&user, &told_key(at)).map_err(store_error)?;
    Ok(Json(Told {
        told: friends.len(),
    }))
}

/// Says what the card was. Only its owner can, and only for a card they conquered.
pub(crate) async fn set_label(
    State(app): State<Arc<App>>,
    Path((user, at)): Path<(String, i64)>,
    headers: HeaderMap,
    Json(update): Json<LabelUpdate>,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
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
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
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
        .route("/api/conquests/{user}/{at}/tell", post(tell))
        .route(
            "/api/conquests/{user}/{at}/label",
            post(set_label).delete(remove_label),
        )
}
