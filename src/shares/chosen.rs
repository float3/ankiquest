//! Explicit recipients and private notes share the existing win-delivery ledger.

use super::*;
use crate::social;
use crate::web_util::{base64url, random_bytes};
use std::collections::BTreeSet;

const MAX_NOTE: usize = 280;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    moment: Reference,
    to: Destination,
    #[serde(default)]
    note: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Reference {
    Win(i64),
    Notification(i64),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Destination {
    Circle(String),
    Selected(Selection),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    #[serde(default)]
    friends: Vec<String>,
    #[serde(default)]
    groups: Vec<i64>,
}

pub(super) fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists moment_shares (
             receipt text primary key,
             sender text not null,
             created_at integer not null,
             note text not null,
             undone integer not null default 0
         ) without rowid;
         create table if not exists moment_share_rows (
             receipt text not null,
             notification integer primary key
         );
         create index if not exists moment_share_receipt on moment_share_rows (receipt);",
    )?;
    Ok(())
}

fn recipients(
    app: &App,
    conn: &Connection,
    user: &str,
    destination: Destination,
) -> Result<Vec<String>, StatusCode> {
    let allowed: BTreeSet<String> = friends(app, conn, user)
        .map_err(store_error)?
        .into_iter()
        .collect();
    let selection = match destination {
        Destination::Circle(mode) if mode == "circle" => return Ok(allowed.into_iter().collect()),
        Destination::Circle(_) => return Err(StatusCode::BAD_REQUEST),
        Destination::Selected(selection) => selection,
    };
    if selection.friends.len() > social::MAX_MEMBERS as usize
        || selection.groups.len() > social::MAX_GROUPS as usize
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut chosen = BTreeSet::new();
    for friend in selection.friends {
        if !allowed.contains(&friend) {
            return Err(StatusCode::FORBIDDEN);
        }
        chosen.insert(friend);
    }
    for group in selection.groups {
        if !social::is_member(conn, group, user).map_err(store_error)? {
            return Err(StatusCode::FORBIDDEN);
        }
        chosen.extend(social::group_members(conn, group).map_err(store_error)?);
    }
    chosen.remove(user);
    chosen.retain(|friend| allowed.contains(friend));
    if chosen.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(chosen.into_iter().collect())
}

fn clean_note(raw: &str) -> Result<String, StatusCode> {
    let note = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if note.chars().count() > MAX_NOTE || note.chars().any(char::is_control) {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(note)
}

pub(super) fn share(
    app: &App,
    user: &str,
    request: Request,
    headers: &HeaderMap,
) -> Result<Value, StatusCode> {
    let mut store = app.store.lock().unwrap();
    let id = match request.moment {
        Reference::Win(id) => id,
        Reference::Notification(id) => match notification_win(&store, user, id)
            .map_err(store_error)?
            .ok_or(StatusCode::NOT_FOUND)?
        {
            Notice::Win(id) => id,
            Notice::Quote(_) => return Err(StatusCode::NOT_FOUND),
        },
    };
    let win = wins::win(&store.conn, user, id).ok_or(StatusCode::NOT_FOUND)?;
    // Validate the whole selection before creating a public link or delivering any row.
    let recipients = recipients(app, &store.conn, user, request.to)?;
    let note = clean_note(&request.note)?;
    let token = wins::ensure_link(&store.conn, user, id)
        .map_err(store_error)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let link = wins::link(&base_url(app, headers), &token);
    let receipt = base64url(&random_bytes("share randomness unavailable").map_err(store_error)?);
    let now = now_ms();
    let sent = deliver(
        &mut store,
        user,
        &app.display(user),
        &recipients,
        &[win],
        Delivery::Chosen {
            receipt: &receipt,
            note: &note,
        },
        now,
    )
    .map_err(store_error)?;
    Ok(json!({
        "recipients": sent,
        "link": link,
        "receipt": (sent > 0).then_some(receipt),
        "undo_until": (sent > 0).then_some(now + crate::conquests::UNDO_MS),
    }))
}

/// Read notes from delivery metadata, never from the publicly shareable win.
pub(super) fn note(conn: &Connection, notification: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "select s.note from moment_share_rows r join moment_shares s on s.receipt = r.receipt
         where r.notification = ?1 and s.note != ''",
        [notification],
        |row| row.get(0),
    )
    .optional()
}

pub(super) async fn undo(
    State(app): State<Arc<App>>,
    Path((user, receipt)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let mut store = app.store.lock().unwrap();
    let tx = store.conn.transaction().map_err(store_error)?;
    let (created_at, undone): (i64, bool) = tx
        .query_row(
            "select created_at, undone from moment_shares where receipt = ?1 and sender = ?2",
            params![receipt, user],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(store_error)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if undone {
        return Ok(Json(json!({"retracted": 0})));
    }
    if now_ms() >= created_at + crate::conquests::UNDO_MS {
        return Err(StatusCode::CONFLICT);
    }
    let answered: bool = tx.query_row(
        "select exists (select 1 from moment_share_rows r join notifications n on n.id = r.notification
         where r.receipt = ?1 and (n.replied = 1 or exists
         (select 1 from reactions x where x.notification = n.id)))",
        [&receipt], |row| row.get(0),
    ).map_err(store_error)?;
    if answered {
        return Err(StatusCode::CONFLICT);
    }
    let unread: Vec<i64> = tx
        .prepare(
            "select n.id from moment_share_rows r join notifications n on n.id = r.notification
         where r.receipt = ?1 and n.sender = ?2 and n.read_at is null",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![receipt, user], |row| row.get(0))?
                .collect()
        })
        .map_err(store_error)?;
    for id in &unread {
        tx.execute(
            "update wins set shared_friends = max(0, shared_friends - 1)
             where id in (select win from win_deliveries where notification = ?1)",
            [id],
        )
        .map_err(store_error)?;
        for sql in [
            "delete from win_deliveries where notification = ?1",
            "delete from moment_share_rows where notification = ?1",
            "delete from notifications where id = ?1",
        ] {
            tx.execute(sql, [id]).map_err(store_error)?;
        }
    }
    // Read rows retain their notes and delivery keys; a later share cannot send them twice.
    tx.execute(
        "update moment_shares set undone = 1 where receipt = ?1",
        [&receipt],
    )
    .map_err(store_error)?;
    tx.commit().map_err(store_error)?;
    Ok(Json(json!({"retracted": unread.len()})))
}
