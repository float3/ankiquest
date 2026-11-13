//! Sharing a win with friends. A win, a celebration, a finished challenge, a weekly recap or a
//! top-three place on this week's board reaches everyone in the player's circle as a
//! notification they can reply to and cheer, with the same topic so it shows the same art.
//! A win can also leave AnkiQuest as a link to its card. Any moment that is a win can go to
//! chosen friends and groups with a note, and be taken back for a day.

use crate::access::require_owner;
use crate::app::{App, now_ms};
use crate::conquests::{ConquestWin, UNDO_MS, conquest_win};
use crate::notifications::{Outgoing, insert_notification};
use crate::store::{Error, Store, clock_in};
use crate::wins::{self, Win as Moment};
use crate::{boards::standings, web_util::base_url, web_util::store_error};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, post};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Notification kinds a player may pass on to friends.
const SHAREABLE: [&str; 3] = ["celebration", "challenge_complete", "reminder_weekly_recap"];

/// Most friends one share names, as for a finished deck.
const MAX_FRIENDS: usize = 100;

/// Most groups one share names: every group a player can be in.
const MAX_GROUPS: usize = crate::social::MAX_GROUPS as usize;

/// Long enough for a sentence, short enough to stay a note.
const MAX_NOTE: usize = 140;

/// Longest moment key a share names; real keys are far shorter.
const MAX_KEY: usize = 128;

/// The `to` that sends a moment to everyone in the player's circle.
const CIRCLE: &str = "circle";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    /// One of the player's own notifications.
    notification: Option<i64>,
    /// The player's current place on this week's board, 1 to 3.
    place: Option<usize>,
    /// One of the player's wins.
    win: Option<i64>,
    /// Where it goes: "friends" or "link" with `win`; "circle" or chosen people with `moment`.
    to: Option<To>,
    /// A moment to send to chosen people or the circle.
    moment: Option<MomentRef>,
    /// The player's own words to go with `moment`.
    note: Option<String>,
}

/// Who a share goes to: a word, or the friends and groups the player picked.
#[derive(Deserialize)]
#[serde(untagged)]
enum To {
    Word(String),
    Chosen(Chosen),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Chosen {
    #[serde(default)]
    friends: Vec<String>,
    #[serde(default)]
    groups: Vec<i64>,
}

/// A shareable moment: a win by id or event key, one of the player's own rows that is about a
/// win, or a conquered card by its `at`.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum MomentRef {
    Win(i64),
    Key(String),
    Notification(i64),
    Conquest(i64),
}

/// What a moment reference names.
enum Found {
    Win(i64),
    /// Nothing of the player's.
    Missing,
    /// Something real with no win to share yet: an older row, or a conquest the worker has not
    /// announced.
    Unkept,
}

/// What sending a moment by hand answers.
#[derive(Debug, Serialize)]
struct MomentShared {
    /// Recipients who got it now; those who already had it or muted the player are left out.
    recipients: usize,
    link: wins::Link,
    /// The win that was sent, to take it back with `DELETE /api/share/{user}/{win}`.
    win: i64,
    /// Until when (ms) it can be taken back, when it reached anyone.
    #[serde(skip_serializing_if = "Option::is_none")]
    undo_until: Option<i64>,
}

#[derive(Debug, Serialize)]
struct Retracted {
    /// The rows taken back, which their recipients had not read or answered.
    retracted: usize,
}

/// How wins leave the player.
#[derive(Clone, Copy)]
pub(crate) enum Sending<'a> {
    /// By a sharing policy, within the daily cap.
    Automatic,
    /// By hand, with the player's note when they wrote one.
    ByHand(Option<&'a str>),
}

#[derive(Debug, Serialize)]
struct Shared {
    /// Friends who got it now; those who already had it or muted the player are left out.
    recipients: usize,
}

#[derive(Debug, Serialize)]
struct Linked {
    url: String,
    image: String,
    text: String,
}

struct Win {
    what: String,
    topic: Option<String>,
    key: String,
}

/// One of the player's shareable notifications: the win it is about, or a quote of it.
enum Notice {
    Win(i64),
    Quote(Win),
}

fn notification_win(store: &Store, user: &str, id: i64) -> Result<Option<Notice>, Error> {
    Ok(store
        .conn
        .query_row(
            "select title, body, kind, topic, win_id from notifications
             where id = ?1 and recipient = ?2 and push_only = 0",
            params![id, user],
            |r| {
                let kind: String = r.get(2)?;
                if !SHAREABLE.contains(&kind.as_str()) {
                    return Ok(None);
                }
                Ok(Some(match r.get(4)? {
                    Some(win) => Notice::Win(win),
                    None => Notice::Quote(Win {
                        what: {
                            let (title, body) =
                                wins::own_words(&r.get::<_, String>(0)?, &r.get::<_, String>(1)?);
                            format!("{title}: “{body}”")
                        },
                        topic: r.get(3)?,
                        key: format!("notification:{id}"),
                    }),
                }))
            },
        )
        .optional()?
        .flatten())
}

fn muted(conn: &Connection, recipient: &str, sender: &str) -> Result<bool, Error> {
    Ok(conn.query_row(
        &format!("select {}", crate::decks::sender_muted_sql("?1", "?2")),
        params![recipient, sender],
        |r| r.get(0),
    )?)
}

/// Everyone in `user`'s circle the server knows, other than `user`.
pub(crate) fn friends(app: &App, conn: &Connection, user: &str) -> rusqlite::Result<Vec<String>> {
    Ok(app
        .circle(conn, user)?
        .into_iter()
        .filter(|friend| *friend != user && app.is_user(friend))
        .collect())
}

fn send(
    store: &mut Store,
    sender: &str,
    display: &str,
    friends: &[String],
    win: &Win,
    now: i64,
) -> Result<usize, Error> {
    let day = store.clock(sender)?.day(now);
    let title = crate::i18n::english("🎉 {0} shared a win", &[display]);
    let mut sent = 0;
    for friend in friends {
        if muted(&store.conn, friend, sender)? {
            continue;
        }
        let key = format!("share:{sender}:{}", win.key);
        if store.is_seen(friend, &key)? {
            continue;
        }
        store.send_once_about(
            &Outgoing {
                to: friend,
                from: sender,
                title: &title,
                body: &win.what,
                kind: "share",
            },
            win.topic.as_deref(),
            &key,
            day,
            now,
            false,
        )?;
        sent += 1;
    }
    Ok(sent)
}

/// Sends `wins` to each friend who has not had them, as one `share` row per friend: the win
/// itself, or a digest of several. Automatic rows stop at the sender's daily cap per friend.
/// Returns how many friends got a row.
pub(crate) fn send_wins(
    store: &mut Store,
    sender: &str,
    display: &str,
    friends: &[String],
    wins: &[Moment],
    sending: Sending,
    now: i64,
) -> Result<usize, Error> {
    if wins.is_empty() {
        return Ok(0);
    }
    let (automatic, note) = match sending {
        Sending::Automatic => (true, None),
        Sending::ByHand(note) => (false, note),
    };
    let day = store.clock(sender)?.day(now);
    let cap = wins::sharing(&store.conn, sender)?.daily_cap;
    let tx = store.conn.transaction()?;
    let mut sent = 0;
    for friend in friends {
        if muted(&tx, friend, sender)? {
            continue;
        }
        let mut fresh = Vec::new();
        for win in wins {
            if !tx.query_row(
                "select exists (select 1 from win_deliveries where win = ?1 and recipient = ?2)",
                params![win.id, friend],
                |r| r.get::<_, bool>(0),
            )? {
                fresh.push(win);
            }
        }
        let Some(first) = fresh.first() else {
            continue;
        };
        if automatic {
            let today: u32 = tx.query_row(
                "select count(distinct d.notification) from win_deliveries d
                 join wins w on w.id = d.win
                 where w.user = ?1 and d.recipient = ?2 and d.day = ?3 and d.automatic = 1",
                params![sender, friend, day],
                |r| r.get(0),
            )?;
            if today >= cap {
                continue;
            }
        }
        let (title, body) = if let [win] = fresh.as_slice() {
            let (title, body) = wins::shared_words(&tx, win)?;
            (
                crate::i18n::english("🎉 {0} shared a win", &[display]),
                format!("{title}: “{body}”"),
            )
        } else {
            (
                crate::i18n::english("🎉 {0}: {1} wins", &[display, &fresh.len().to_string()]),
                fresh
                    .iter()
                    .map(|win| Ok(wins::shared_words(&tx, win)?.0))
                    .collect::<Result<Vec<_>, Error>>()?
                    .join(" · "),
            )
        };
        let id = insert_notification(
            &tx,
            &Outgoing {
                to: friend,
                from: sender,
                title: &title,
                body: &body,
                kind: "share",
            },
            day,
            now,
            false,
        )?;
        tx.execute(
            "update notifications set topic = ?2, win_id = ?3, note = ?4 where id = ?1",
            params![id, first.topic(), first.id, note],
        )?;
        for win in &fresh {
            tx.execute(
                "insert into win_deliveries (win, recipient, notification, day, automatic)
                 values (?1, ?2, ?3, ?4, ?5)",
                params![win.id, friend, id, day, automatic],
            )?;
            tx.execute(
                "update wins set shared_friends = shared_friends + 1 where id = ?1",
                [win.id],
            )?;
        }
        sent += 1;
    }
    tx.commit()?;
    Ok(sent)
}

/// Gives one of `user`'s own shareable rows what sharing it would tell others, in `language`:
/// its win's shared words, or the row itself in the player's own words. Call it before the
/// row is translated, while its texts are still the stored English ones.
pub(crate) fn share_words(
    conn: &Connection,
    user: &str,
    notice: &mut crate::notifications::Notification,
    language: &str,
) -> rusqlite::Result<()> {
    if !SHAREABLE.contains(&notice.kind.as_str()) {
        return Ok(());
    }
    let (title, body) = match notice.win.and_then(|id| wins::win(conn, user, id)) {
        Some(win) => wins::shared_words(conn, &win)?,
        None => wins::own_words(&notice.title, &notice.body),
    };
    if (title.as_str(), body.as_str()) != (notice.title.as_str(), notice.body.as_str()) {
        notice.share_title = Some(crate::i18n::system_text(&title, language));
        notice.share_body = Some(crate::i18n::system_text(&body, language));
    }
    Ok(())
}

/// Words a share or cheer about wins again from the wins themselves, in `language`: their
/// stored English titles are translated one by one, never read back out of the joined text.
pub(crate) fn localize(
    conn: &Connection,
    notice: &mut crate::notifications::Notification,
    language: &str,
) -> rusqlite::Result<()> {
    if crate::i18n::normalize(language) == "en" {
        return Ok(());
    }
    let system = |value: &str| crate::i18n::system_text(value, language);
    match notice.kind.as_str() {
        "share" => {
            // In the sharer's own words, as the English row was written.
            let wins = wins::delivered(conn, notice.id)?
                .iter()
                .map(|win| wins::shared_words(conn, win))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            match wins.as_slice() {
                [] => {}
                [(title, body)] => notice.body = format!("{}: “{}”", system(title), system(body)),
                many => {
                    notice.body = many
                        .iter()
                        .map(|(title, _)| system(title))
                        .collect::<Vec<_>>()
                        .join(" · ")
                }
            }
        }
        "reaction" => {
            if let Some(title) = notice
                .win
                .map(|win| {
                    conn.query_row("select title from wins where id = ?1", [win], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()
                })
                .transpose()?
                .flatten()
            {
                notice.body = format!("“{}”", system(&title));
            }
        }
        _ => {}
    }
    Ok(())
}

/// Sends `user`'s new automatic wins to their friends, each with its link; once per tick.
pub(crate) fn share_automatically(
    app: &App,
    store: &mut Store,
    user: &str,
    now: i64,
) -> Result<usize, Error> {
    let mut wins = wins::take_automatic(&store.conn, user)?;
    if wins.is_empty() {
        return Ok(0);
    }
    for win in &mut wins {
        win.link_token = wins::ensure_link(&store.conn, user, win.id)?;
    }
    let friends = friends(app, &store.conn, user)?;
    send_wins(
        store,
        user,
        &app.display(user),
        &friends,
        &wins,
        Sending::Automatic,
        now,
    )
}

/// The text a player pastes along with a win's link.
fn invitation(
    conn: &Connection,
    win: &Moment,
    url: &str,
    language: &str,
) -> rusqlite::Result<String> {
    let (title, body) = wins::shared_words(conn, win)?;
    Ok(crate::i18n::substitute(
        &crate::i18n::text("{0}: {1} Join me on AnkiQuest: {2}", language),
        &[
            &crate::i18n::system_text(&title, language),
            &crate::i18n::system_text(&body, language),
            url,
        ],
    ))
}

fn share_win(
    app: &App,
    user: &str,
    id: i64,
    to: &str,
    headers: &HeaderMap,
) -> Result<Value, StatusCode> {
    if !matches!(to, "friends" | "link") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let store = app.store.lock().unwrap();
    let token = wins::ensure_link(&store.conn, user, id)
        .map_err(store_error)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let win = wins::win(&store.conn, user, id).ok_or(StatusCode::NOT_FOUND)?;
    let link = wins::link(&base_url(app, headers), &token);
    if to == "link" {
        let language = crate::i18n::owned(headers, &store, user).map_err(store_error)?;
        let text = invitation(&store.conn, &win, &link.url, &language).map_err(store_error)?;
        return Ok(json!(Linked {
            url: link.url,
            image: link.image,
            text,
        }));
    }
    let friends = friends(app, &store.conn, user).map_err(store_error)?;
    drop(store);
    let (recipients, link) = deliver(app, user, id, &friends, None, headers, now_ms())?;
    Ok(json!({ "recipients": recipients, "link": link }))
}

/// Sends `user`'s win `id` by hand to `friends`, with `note`. Answers how many got it now, and
/// its link, which friends see the card through.
fn deliver(
    app: &App,
    user: &str,
    id: i64,
    friends: &[String],
    note: Option<&str>,
    headers: &HeaderMap,
    now: i64,
) -> Result<(usize, wins::Link), StatusCode> {
    let mut store = app.store.lock().unwrap();
    let token = wins::ensure_link(&store.conn, user, id)
        .map_err(store_error)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let win = wins::win(&store.conn, user, id).ok_or(StatusCode::NOT_FOUND)?;
    let link = wins::link(&base_url(app, headers), &token);
    let display = app.display(user);
    let sent = send_wins(
        &mut store,
        user,
        &display,
        friends,
        &[win],
        Sending::ByHand(note),
        now,
    )
    .map_err(store_error)?;
    Ok((sent, link))
}

/// The win `moment` names among `user`'s own.
fn resolve(app: &App, user: &str, moment: &MomentRef) -> Result<Found, Error> {
    let found = |id: Option<i64>| id.map_or(Found::Missing, Found::Win);
    let store = app.store.lock().unwrap();
    Ok(match moment {
        MomentRef::Win(id) => found(wins::win(&store.conn, user, *id).map(|win| win.id)),
        MomentRef::Key(key) => found(wins::id_of(&store.conn, user, key)?),
        MomentRef::Notification(id) => match notification_win(&store, user, *id)? {
            Some(Notice::Win(win)) => Found::Win(win),
            Some(Notice::Quote(_)) => Found::Unkept,
            None => Found::Missing,
        },
        MomentRef::Conquest(at) => {
            // The conquest reads the store itself.
            drop(store);
            match conquest_win(app, user, *at)? {
                ConquestWin::Kept(id) => Found::Win(id),
                ConquestWin::Pending => Found::Unkept,
                ConquestWin::Missing => Found::Missing,
            }
        }
    })
}

/// Whether a share names its people within the limits: someone, nobody twice.
fn valid_chosen(chosen: &Chosen) -> bool {
    let friends: BTreeSet<&String> = chosen.friends.iter().collect();
    let groups: BTreeSet<&i64> = chosen.groups.iter().collect();
    !(chosen.friends.is_empty() && chosen.groups.is_empty())
        && chosen.friends.len() <= MAX_FRIENDS
        && chosen.groups.len() <= MAX_GROUPS
        && friends.len() == chosen.friends.len()
        && groups.len() == chosen.groups.len()
}

/// Who `to` reaches for `user`, each once, without `user`; `None` when it is malformed or names
/// someone outside the player's circle or a group they are not in.
fn recipients(
    app: &App,
    conn: &Connection,
    user: &str,
    to: &To,
) -> rusqlite::Result<Option<Vec<String>>> {
    let circle = friends(app, conn, user)?;
    let chosen = match to {
        To::Word(word) if word == CIRCLE => return Ok(Some(circle)),
        To::Word(_) => return Ok(None),
        To::Chosen(chosen) => chosen,
    };
    if !valid_chosen(chosen) || !chosen.friends.iter().all(|f| circle.contains(f)) {
        return Ok(None);
    }

    // Group members the server knows, as for the circle.
    let mut reached: BTreeSet<String> = chosen.friends.iter().cloned().collect();
    for &group in &chosen.groups {
        if !crate::social::is_member(conn, group, user)? {
            return Ok(None);
        }
        reached.extend(
            crate::social::group_members(conn, group)?
                .into_iter()
                .filter(|member| member != user && app.is_user(member)),
        );
    }
    Ok(Some(reached.into_iter().collect()))
}

/// Sends a moment by hand to `to`, with the player's `note`.
fn share_moment(
    app: &App,
    user: &str,
    moment: &MomentRef,
    to: &To,
    note: Option<&str>,
    headers: &HeaderMap,
) -> Result<Value, StatusCode> {
    // Malformed parts first, then what they name.
    let note = note
        .map(|note| crate::web_util::clean_line(note, MAX_NOTE).ok_or(StatusCode::BAD_REQUEST))
        .transpose()?;
    if matches!(moment, MomentRef::Key(key) if key.len() > MAX_KEY) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let recipients = recipients(app, &app.store.lock().unwrap().conn, user, to)
        .map_err(store_error)?
        .ok_or(StatusCode::BAD_REQUEST)?;
    let win = match resolve(app, user, moment).map_err(store_error)? {
        Found::Win(id) => id,
        Found::Missing => return Err(StatusCode::NOT_FOUND),
        Found::Unkept => return Err(StatusCode::CONFLICT),
    };

    let now = now_ms();
    let (sent, link) = deliver(app, user, win, &recipients, note.as_deref(), headers, now)?;
    Ok(json!(MomentShared {
        recipients: sent,
        link,
        win,
        undo_until: (sent > 0).then_some(now + UNDO_MS),
    }))
}

async fn share(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Result<Json<Value>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    if let Some(moment) = &request.moment {
        let (None, None, None, Some(to)) = (
            request.notification,
            request.place,
            request.win,
            &request.to,
        ) else {
            return Err(StatusCode::BAD_REQUEST);
        };
        return share_moment(&app, &user, moment, to, request.note.as_deref(), &headers).map(Json);
    }

    // The bodies from before moments: no note, and `to` only as a word.
    let to = match request.to {
        None => None,
        Some(To::Word(word)) => Some(word),
        Some(To::Chosen(_)) => return Err(StatusCode::BAD_REQUEST),
    };
    if request.note.is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let win = match (request.notification, request.place, request.win, to) {
        (None, None, Some(id), Some(to)) => {
            return share_win(&app, &user, id, &to, &headers).map(Json);
        }
        (Some(id), None, None, None) => {
            let notice = notification_win(&app.store.lock().unwrap(), &user, id)
                .map_err(store_error)?
                .ok_or(StatusCode::NOT_FOUND)?;
            match notice {
                Notice::Win(win) => {
                    return share_win(&app, &user, win, "friends", &headers).map(Json);
                }
                Notice::Quote(win) => win,
            }
        }
        (None, Some(place @ 1..=3), None, None) => {
            let circle = app
                .circle(&app.store.lock().unwrap().conn, &user)
                .map_err(store_error)?;
            let board = standings(&app, &circle, None);
            let here = board.iter().position(|s| s.user == user);
            if here != Some(place - 1) || board[place - 1].week_xp == 0 {
                return Err(StatusCode::CONFLICT);
            }
            let day = app
                .store
                .lock()
                .unwrap()
                .clock(&user)
                .map_err(store_error)?
                .day(now_ms());
            Win {
                what: crate::i18n::english(
                    "#{0} on this week's leaderboard",
                    &[&place.to_string()],
                ),
                topic: Some(format!("place:{place}")),
                key: format!("place:{place}:{day}"),
            }
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let friends = friends(&app, &app.store.lock().unwrap().conn, &user).map_err(store_error)?;
    let recipients = send(
        &mut app.store.lock().unwrap(),
        &user,
        &app.display(&user),
        &friends,
        &win,
        now_ms(),
    )
    .map_err(store_error)?;
    Ok(Json(json!(Shared { recipients })))
}

/// One hand-sent row of a win, as taking it back sees it.
struct SentRow {
    notification: i64,
    recipient: String,
    /// Ms.
    sent_at: i64,
    /// Unread, not cheered or answered, and about this win alone.
    untouched: bool,
}

fn sent_rows(conn: &Connection, win: i64) -> rusqlite::Result<Vec<SentRow>> {
    conn.prepare(
        "select d.notification, d.recipient, n.created_at,
                n.read_at is null and n.replied = 0
                and not exists (select 1 from reactions r where r.notification = n.id)
                and (select count(*) from win_deliveries o where o.notification = n.id) = 1
         from win_deliveries d join notifications n on n.id = d.notification
         where d.win = ?1 and d.automatic = 0",
    )?
    .query_map([win], |r| {
        Ok(SentRow {
            notification: r.get(0)?,
            recipient: r.get(1)?,
            sent_at: r.get(2)?,
            untouched: r.get(3)?,
        })
    })?
    .collect()
}

/// Takes back win `win` from friends it was sent to by hand within [`UNDO_MS`], who have not
/// read, cheered or answered it: their row goes, and the win can reach them again. Answers
/// how many rows went; `None` when it was sent by hand only longer ago than that.
fn retract(conn: &mut Connection, win: i64, now: i64) -> Result<Option<usize>, Error> {
    let tx = conn.transaction()?;
    let rows = sent_rows(&tx, win)?;
    let recent: Vec<&SentRow> = rows.iter().filter(|r| now < r.sent_at + UNDO_MS).collect();
    if recent.is_empty() && !rows.is_empty() {
        return Ok(None);
    }

    // Each row goes with its delivery, so the win counts as not sent to that friend.
    let mut retracted = 0;
    for row in recent.into_iter().filter(|row| row.untouched) {
        tx.execute(
            "delete from notifications where id = ?1",
            [row.notification],
        )?;
        tx.execute(
            "delete from win_deliveries where win = ?1 and recipient = ?2",
            params![win, row.recipient],
        )?;
        tx.execute(
            "update wins set shared_friends = max(shared_friends - 1, 0) where id = ?1",
            [win],
        )?;
        retracted += 1;
    }
    tx.commit()?;
    Ok(Some(retracted))
}

/// Takes a moment sent by hand back; see [`retract`]. Too late is `409`.
async fn unshare(
    State(app): State<Arc<App>>,
    Path((user, win)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Result<Json<Retracted>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let mut store = app.store.lock().unwrap();
    wins::win(&store.conn, &user, win).ok_or(StatusCode::NOT_FOUND)?;
    let retracted = retract(&mut store.conn, win, now_ms())
        .map_err(store_error)?
        .ok_or(StatusCode::CONFLICT)?;
    Ok(Json(Retracted { retracted }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reaction {
    notification: i64,
}

#[derive(Debug, Serialize)]
struct Reacted {
    already: bool,
}

/// `user` cheers a `share` or `completion` row in their inbox, and its sender hears about it
/// once. Answers whether they already had; `None` when there is no such row to cheer.
pub(crate) fn react(
    conn: &mut Connection,
    user: &str,
    display: &str,
    id: i64,
    now: i64,
) -> Result<Option<bool>, Error> {
    let tx = conn.transaction()?;
    let row: Option<(String, String, Option<i64>, Option<String>)> = tx
        .query_row(
            "select n.sender, n.title, n.win_id, (select w.title from wins w where w.id = n.win_id)
             from notifications n where n.id = ?1 and n.recipient = ?2 and n.push_only = 0
             and n.kind in ('share', 'completion') and n.sender not in ('', ?2)",
            params![id, user],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((sender, title, win, win_title)) = row else {
        return Ok(None);
    };
    if tx.execute(
        "insert or ignore into reactions (reactor, notification, sender, created_at)
         values (?1, ?2, ?3, ?4)",
        params![user, id, sender, now],
    )? == 0
    {
        return Ok(Some(true));
    }
    if !muted(&tx, &sender, user)? {
        let cheer = insert_notification(
            &tx,
            &Outgoing {
                to: &sender,
                from: user,
                title: &crate::i18n::english("🎉 {0} cheered", &[display]),
                body: &format!("“{}”", win_title.unwrap_or(title)),
                kind: "reaction",
            },
            clock_in(&tx, &sender)?.day(now),
            now,
            false,
        )?;
        if let Some(win) = win {
            wins::attach(&tx, cheer, win)?;
        }
    }
    tx.commit()?;
    Ok(Some(false))
}

async fn react_to(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(reaction): Json<Reaction>,
) -> Result<Json<Reacted>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let display = app.display(&user);
    let already = react(
        &mut app.store.lock().unwrap().conn,
        &user,
        &display,
        reaction.notification,
        now_ms(),
    )
    .map_err(store_error)?
    .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(Reacted { already }))
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/share/{user}", post(share))
        .route("/api/share/{user}/{win}", delete(unshare))
        .route("/api/react/{user}", post(react_to))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Event;
    use crate::notifications::Notification;
    use crate::test_support::cleanup;
    use crate::test_support::{call, fixture, headers, sign_up};
    use crate::wins::tests::moment;
    use std::path::PathBuf;

    fn win_for(store: &mut Store, user: &str) -> i64 {
        let event = |key: &str| Event {
            key: key.into(),
            title: format!("Title {key}"),
            body: "Body".into(),
        };
        store
            .queue_events(user, &[event("achievement:first")], 1, 1_000, false)
            .unwrap();
        store
            .queue_events(user, &[event("streak:1:30")], 1, 2_000, false)
            .unwrap();
        store.notifications(user, 2_000).unwrap()[0].id
    }

    fn quote(store: &Store, user: &str, id: i64) -> Option<Win> {
        match notification_win(store, user, id).unwrap()? {
            Notice::Quote(win) => Some(win),
            Notice::Win(_) => None,
        }
    }

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_celebration_is_shared_as_its_win_and_older_rows_as_a_quote() {
        let (mut store, _path) = crate::test_support::temporary_store();
        let id = win_for(&mut store, "hill");
        let win = store.notifications("hill", 2_000).unwrap()[0].win.unwrap();
        assert!(matches!(
            notification_win(&store, "hill", id).unwrap(),
            Some(Notice::Win(found)) if found == win
        ));
        store
            .conn
            .execute("update notifications set win_id = null", [])
            .unwrap();
        let quoted = quote(&store, "hill", id).unwrap();
        assert_eq!(quoted.topic.as_deref(), Some("streak"));
        let friends = strings(&["cerro", "aldanita"]);
        assert_eq!(
            send(&mut store, "hill", "Hill", &friends, &quoted, 3_000).unwrap(),
            2
        );
        assert_eq!(
            send(&mut store, "hill", "Hill", &friends, &quoted, 4_000).unwrap(),
            0,
            "a win reaches each friend once"
        );
        let got = &store.notifications("cerro", 4_000).unwrap()[0];
        assert_eq!(got.kind, "share");
        assert_eq!(got.sender, "hill");
        assert_eq!(got.title, "🎉 Hill shared a win");
        assert_eq!(got.body, "Title streak:1:30: “Body”");
        assert_eq!(got.topic.as_deref(), Some("streak"));
    }

    #[test]
    fn only_your_own_wins_can_be_shared() {
        let (mut store, _path) = crate::test_support::temporary_store();
        let id = win_for(&mut store, "hill");
        assert!(notification_win(&store, "cerro", id).unwrap().is_none());
        store
            .conn
            .execute(
                "update notifications set kind = 'nudge' where id = ?1",
                [id],
            )
            .unwrap();
        assert!(notification_win(&store, "hill", id).unwrap().is_none());
    }

    #[test]
    fn a_win_reaches_each_friend_once_with_its_topic_and_card() {
        let (mut store, _path) = crate::test_support::temporary_store();
        let id = moment(&store, "hill", "streak:1:30");
        let token = wins::ensure_link(&store.conn, "hill", id).unwrap().unwrap();
        let win = wins::win(&store.conn, "hill", id).unwrap();
        store
            .conn
            .execute(
                "insert into incoming_muted_senders (recipient, sender) values ('aldanita', 'hill')",
                [],
            )
            .unwrap();
        let friends = strings(&["cerro", "aldanita"]);
        let share = |store: &mut Store| {
            send_wins(
                store,
                "hill",
                "Hill",
                &friends,
                std::slice::from_ref(&win),
                Sending::ByHand(None),
                3_000,
            )
            .unwrap()
        };
        assert_eq!(share(&mut store), 1);
        assert_eq!(share(&mut store), 0, "each win reaches each friend once");
        assert!(store.notifications("aldanita", 3_000).unwrap().is_empty());
        let mut got = store.notifications("cerro", 3_000).unwrap().remove(0);
        assert_eq!(
            (got.kind.as_str(), got.sender.as_str(), got.title.as_str()),
            ("share", "hill", "🎉 Hill shared a win")
        );
        assert_eq!(got.body, "Title streak:1:30: “Body”");
        assert_eq!(got.topic.as_deref(), Some("streak"));
        assert_eq!(got.win, Some(id));
        wins::decorate(&mut got, "https://aq.example");
        assert_eq!(got.image, Some(format!("https://aq.example/w/{token}.png")));
        assert_eq!(
            wins::win(&store.conn, "hill", id).unwrap().shared_friends,
            1
        );
        wins::revoke_link(&store.conn, "hill", id).unwrap();
        let mut revoked = store.notifications("cerro", 3_000).unwrap().remove(0);
        wins::decorate(&mut revoked, "https://aq.example");
        assert_eq!(revoked.image, None);
    }

    #[test]
    fn automatic_wins_arrive_as_one_digest_within_the_daily_cap() {
        let (mut store, _path) = crate::test_support::temporary_store();
        wins::update_sharing(
            &store.conn,
            "hill",
            wins::SharingUpdate {
                policies: Default::default(),
                daily_cap: Some(1),
            },
        )
        .unwrap();
        let first = moment(&store, "hill", "level:4");
        let second = moment(&store, "hill", "achievement:x");
        let both = [
            wins::win(&store.conn, "hill", first).unwrap(),
            wins::win(&store.conn, "hill", second).unwrap(),
        ];
        let friends = strings(&["cerro"]);
        assert_eq!(
            send_wins(
                &mut store,
                "hill",
                "Hill",
                &friends,
                &both,
                Sending::Automatic,
                3_000
            )
            .unwrap(),
            1
        );
        let got = store.notifications("cerro", 3_000).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "🎉 Hill: 2 wins");
        assert_eq!(got[0].body, "Title level:4 · Title achievement:x");
        assert_eq!(got[0].topic.as_deref(), Some("level"));
        assert_eq!(got[0].win, Some(first));
        let third = [wins::win(&store.conn, "hill", moment(&store, "hill", "level:5")).unwrap()];
        assert_eq!(
            send_wins(
                &mut store,
                "hill",
                "Hill",
                &friends,
                &third,
                Sending::Automatic,
                4_000
            )
            .unwrap(),
            0,
            "the cap counts rows, and one was sent today"
        );
        assert_eq!(
            send_wins(
                &mut store,
                "hill",
                "Hill",
                &friends,
                &third,
                Sending::ByHand(None),
                4_000
            )
            .unwrap(),
            1,
            "a win over the cap can still be shared by hand"
        );
    }

    #[test]
    fn the_players_own_rows_carry_what_sharing_them_would_say() {
        let (store, _path) = crate::test_support::temporary_store();
        let celebrate = |title: &str, body: &str, kind: &str| {
            insert_notification(
                &store.conn,
                &Outgoing {
                    to: "hill",
                    from: "",
                    title,
                    body,
                    kind,
                },
                20_000,
                1_000,
                false,
            )
            .unwrap()
        };
        let level = celebrate("Level 4", "You reached level 4 with 900 XP.", "celebration");
        let win = wins::create(
            &store.conn,
            "hill",
            "level:4",
            20_000,
            1_000,
            "Level 4",
            "You reached level 4 with 900 XP.",
            &json!({}),
        )
        .unwrap()
        .unwrap();
        wins::attach(&store.conn, level, win).unwrap();
        celebrate(
            "Streak secured!",
            "Today counts toward your 5-day streak.",
            "celebration",
        );
        celebrate(
            "Daily goal reached!",
            "120 cards today in 35 minutes.",
            "celebration",
        );
        celebrate("Hi", "You there?", "message");
        let rows = |language: &str| {
            let mut rows = store.notifications("hill", 2_000).unwrap();
            rows.sort_by_key(|row| row.id);
            for row in &mut rows {
                share_words(&store.conn, "hill", row, language).unwrap();
            }
            rows.into_iter()
                .map(|row| (row.share_title, row.share_body))
                .collect::<Vec<_>>()
        };
        let some = |title: &str, body: &str| (Some(title.to_string()), Some(body.to_string()));
        assert_eq!(
            rows("en"),
            [
                some("Level 4", "I reached level 4 with 900 XP."),
                some("Streak secured!", "Today counts toward my 5-day streak."),
                (None, None),
                (None, None),
            ],
            "a neutral row and a message carry nothing"
        );
        assert_eq!(
            rows("pt")[..2],
            [
                some("Nível 4", "Cheguei ao nível 4 com 900 XP."),
                some(
                    "Sequência garantida!",
                    "Hoje conta para a minha sequência de 5 dias."
                ),
            ]
        );
    }

    #[test]
    fn shared_wins_are_worded_from_the_wins_in_the_readers_language() {
        let (mut store, _path) = crate::test_support::temporary_store();
        let win = |store: &Store, key: &str, title: &str, body: &str| {
            let id = wins::create(
                &store.conn,
                "hill",
                key,
                20_000,
                1_000,
                title,
                body,
                &json!({}),
            )
            .unwrap()
            .unwrap();
            wins::win(&store.conn, "hill", id).unwrap()
        };
        let level = win(
            &store,
            "level:4",
            "Level 4",
            "You reached level 4 with 900 XP.",
        );
        let secured = win(
            &store,
            "secured:1",
            "Streak secured!",
            "Today counts toward your 5-day streak.",
        );
        let friends = strings(&["cerro", "aldanita"]);
        send_wins(
            &mut store,
            "hill",
            "Hill",
            &friends[..1],
            &[level.clone(), secured],
            Sending::ByHand(None),
            3_000,
        )
        .unwrap();
        send_wins(
            &mut store,
            "hill",
            "Hill",
            &friends[1..],
            &[level],
            Sending::ByHand(None),
            3_000,
        )
        .unwrap();
        let mut digest = store.notifications("cerro", 3_000).unwrap().remove(0);
        crate::i18n::notification(&mut digest, "es", "Hill");
        localize(&store.conn, &mut digest, "es").unwrap();
        assert_eq!(digest.title, "🎉 Hill: 2 logros");
        assert_eq!(digest.body, "Nivel 4 · ¡Racha asegurada!");
        let mut single = store.notifications("aldanita", 3_000).unwrap().remove(0);
        crate::i18n::notification(&mut single, "de", "Hill");
        localize(&store.conn, &mut single, "de").unwrap();
        assert_eq!(single.title, "🎉 Hill hat einen Erfolg geteilt");
        assert_eq!(
            single.body, "Level 4: “Ich habe Level 4 mit 900 XP erreicht.”",
            "a friend reads it in the sharer's words"
        );
        let mut english = store.notifications("aldanita", 3_000).unwrap().remove(0);
        localize(&store.conn, &mut english, "en").unwrap();
        assert_eq!(english.body, "Level 4: “I reached level 4 with 900 XP.”");

        assert_eq!(
            react(&mut store.conn, "aldanita", "Aldana", single.id, 4_000).unwrap(),
            Some(false)
        );
        let mut cheer = store.notifications("hill", 4_000).unwrap().remove(0);
        crate::i18n::notification(&mut cheer, "fr", "Aldana");
        localize(&store.conn, &mut cheer, "fr").unwrap();
        assert_eq!(cheer.title, "🎉 Aldana t’a applaudi");
        assert_eq!(cheer.body, "“Niveau 4”");
        let mut english = store.notifications("hill", 4_000).unwrap().remove(0);
        localize(&store.conn, &mut english, "en").unwrap();
        assert_eq!(english.body, "“Level 4”");
    }

    #[tokio::test]
    async fn the_share_endpoint_sends_wins_to_friends_or_makes_a_link() {
        let (app, path) = fixture();
        let id = moment(&app.store.lock().unwrap(), "hill", "level:7");
        let ask = |body: Value, token: &str| {
            share(
                State(app.clone()),
                Path("hill".into()),
                headers(token),
                Json(serde_json::from_value(body).unwrap()),
            )
        };
        let Json(sent) = ask(json!({"win": id, "to": "friends"}), "hill")
            .await
            .unwrap();
        assert_eq!(sent["recipients"], 2, "cerro and friend");
        assert!(sent["link"]["image"].as_str().unwrap().ends_with(".png"));
        assert_eq!(
            sent.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["link", "recipients"],
            "the old body answers as it always did"
        );
        let Json(again) = ask(json!({"win": id, "to": "friends"}), "hill")
            .await
            .unwrap();
        assert_eq!(again["recipients"], 0);
        let Json(link) = ask(json!({"win": id, "to": "link"}), "hill").await.unwrap();
        let token = app
            .store
            .lock()
            .unwrap()
            .conn
            .query_row("select link_token from wins where id = ?1", [id], |r| {
                r.get::<_, String>(0)
            })
            .unwrap();
        assert_eq!(link["url"], format!("/w/{token}"));
        assert_eq!(link["image"], format!("/w/{token}.png"));
        assert_eq!(
            link["text"],
            format!("Title level:7: Body Join me on AnkiQuest: /w/{token}")
        );
        assert_eq!(
            ask(json!({"win": id, "to": "everyone"}), "hill")
                .await
                .err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            ask(json!({"win": id + 1, "to": "link"}), "hill")
                .await
                .err(),
            Some(StatusCode::NOT_FOUND)
        );
        assert_eq!(
            ask(json!({"win": id}), "hill").await.err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            ask(json!({"win": id, "to": "link"}), "cerro").await.err(),
            Some(StatusCode::UNAUTHORIZED)
        );
        cleanup(app, path);
    }

    #[test]
    fn a_cheer_reaches_the_sender_once_and_can_be_answered() {
        let (mut store, _path) = crate::test_support::temporary_store();
        let id = moment(&store, "hill", "level:7");
        let win = wins::win(&store.conn, "hill", id).unwrap();
        send_wins(
            &mut store,
            "hill",
            "Hill",
            &strings(&["cerro"]),
            &[win],
            Sending::ByHand(None),
            3_000,
        )
        .unwrap();
        let row = store.notifications("cerro", 3_000).unwrap()[0].id;
        assert_eq!(
            react(&mut store.conn, "hill", "Hill", row, 4_000).unwrap(),
            None
        );
        assert_eq!(
            react(&mut store.conn, "cerro", "Cerro", row, 4_000).unwrap(),
            Some(false)
        );
        assert_eq!(
            react(&mut store.conn, "cerro", "Cerro", row, 5_000).unwrap(),
            Some(true)
        );
        assert!(store.notifications("cerro", 5_000).unwrap()[0].reacted);
        let cheers = store.notifications("hill", 5_000).unwrap();
        assert_eq!(cheers.len(), 1);
        assert_eq!(
            (cheers[0].kind.as_str(), cheers[0].sender.as_str()),
            ("reaction", "cerro")
        );
        assert_eq!(cheers[0].title, "🎉 Cerro cheered");
        assert_eq!(cheers[0].body, "“Title level:7”");
        assert!(!cheers[0].reacted);
        assert_eq!(
            store
                .reply("hill", "Hill", cheers[0].id, "thanks!", 6_000)
                .unwrap()
                .as_deref(),
            Some("cerro")
        );
    }

    #[test]
    fn a_friend_who_muted_you_is_skipped() {
        let (mut store, _path) = crate::test_support::temporary_store();
        let id = win_for(&mut store, "hill");
        store
            .conn
            .execute("update notifications set win_id = null", [])
            .unwrap();
        let win = quote(&store, "hill", id).unwrap();
        store
            .conn
            .execute(
                "insert into incoming_muted_senders (recipient, sender) values ('cerro', 'hill')",
                [],
            )
            .unwrap();
        assert_eq!(
            send(
                &mut store,
                "hill",
                "Hill",
                &strings(&["cerro"]),
                &win,
                3_000
            )
            .unwrap(),
            0
        );
        assert!(store.notifications("cerro", 3_000).unwrap().is_empty());
    }

    /// Hill, Cerro and Friend share the original community; accounts can sign up.
    fn moments_app() -> (Arc<App>, PathBuf) {
        crate::test_support::app_with(json!({
            "registration": true,
            "users": {
                "cerro": {"display": "Cerro", "token": "cerro-secret"},
                "hill": {"display": "Hill", "token": "hill-secret"},
                "friend": {"token": "friend-secret"}
            }
        }))
    }

    async fn share_as_hill(app: &Arc<App>, body: Value) -> (StatusCode, Value) {
        call(app, "POST", "/api/share/hill", "hill-secret", body).await
    }

    fn inbox(app: &App, user: &str) -> Vec<Notification> {
        app.store
            .lock()
            .unwrap()
            .notifications(user, now_ms())
            .unwrap()
    }

    fn sql(app: &App, statement: &str) {
        app.store
            .lock()
            .unwrap()
            .conn
            .execute_batch(statement)
            .unwrap();
    }

    #[tokio::test]
    async fn a_moment_reaches_only_the_chosen_friends_with_the_note() {
        let (app, path) = moments_app();
        let id = moment(&app.store.lock().unwrap(), "hill", "level:7");
        let before = now_ms();
        let (status, sent) = share_as_hill(
            &app,
            json!({
                "moment": {"win": id},
                "to": {"friends": ["cerro"]},
                "note": "  Finally\n level 7 "
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(sent["recipients"], 1);
        assert_eq!(sent["win"], id);
        assert!(sent["link"]["image"].as_str().unwrap().ends_with(".png"));
        assert!(sent["undo_until"].as_i64().unwrap() >= before + UNDO_MS);
        let got = inbox(&app, "cerro").remove(0);
        assert_eq!(
            (got.kind.as_str(), got.win, got.note.as_deref()),
            ("share", Some(id), Some("Finally level 7"))
        );
        assert_eq!(
            serde_json::to_value(&got).unwrap()["note"],
            "Finally level 7"
        );
        assert!(inbox(&app, "friend").is_empty(), "Friend was not chosen");

        // The circle then reaches everyone else, and nobody twice.
        let circle = json!({"moment": {"key": "level:7"}, "to": "circle"});
        let (_, rest) = share_as_hill(&app, circle.clone()).await;
        assert_eq!(rest["recipients"], 1);
        assert_eq!(inbox(&app, "friend")[0].note, None);
        assert_eq!(inbox(&app, "cerro").len(), 1);
        let (_, again) = share_as_hill(&app, circle).await;
        assert_eq!(again["recipients"], 0);
        assert!(again.get("undo_until").is_none(), "nothing to take back");
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_moment_goes_only_to_people_the_player_can_reach() {
        let (app, path) = moments_app();
        let id = moment(&app.store.lock().unwrap(), "hill", "level:7");
        sign_up(&app, "ana").await;
        let crowd: Vec<String> = (0..=MAX_FRIENDS).map(|i| format!("p{i}")).collect();
        let to_win = |to: Value| json!({"moment": {"win": id}, "to": to});
        let with_note = |note: String| json!({"moment": {"win": id}, "to": "circle", "note": note});
        for body in [
            to_win(json!({"friends": ["ana"]})),
            to_win(json!({"friends": ["nobody"]})),
            to_win(json!({"friends": ["cerro", "cerro"]})),
            to_win(json!({"friends": crowd})),
            to_win(json!({})),
            to_win(json!({"friends": [], "groups": []})),
            to_win(json!({"groups": [999]})),
            to_win(json!("everyone")),
            to_win(json!("friends")),
            json!({"moment": {"win": id}}),
            json!({"moment": {"win": id}, "win": id, "to": "circle"}),
            json!({"moment": {"key": "k".repeat(MAX_KEY + 1)}, "to": "circle"}),
            with_note("x".repeat(MAX_NOTE + 1)),
            with_note(" \n ".into()),
            json!({"win": id, "to": {"friends": ["cerro"]}}),
            json!({"win": id, "to": "friends", "note": "hi"}),
        ] {
            assert_eq!(
                share_as_hill(&app, body.clone()).await.0,
                StatusCode::BAD_REQUEST,
                "{body}"
            );
        }
        for moment in [
            json!({"win": id + 1}),
            json!({"key": "level:99"}),
            json!({"notification": 12345}),
            json!({"conquest": 5}),
        ] {
            let body = json!({"moment": moment, "to": "circle"});
            assert_eq!(
                share_as_hill(&app, body.clone()).await.0,
                StatusCode::NOT_FOUND,
                "{body}"
            );
        }
        assert!(inbox(&app, "cerro").is_empty());
        assert!(inbox(&app, "ana").is_empty());
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_row_names_its_win_and_an_older_row_has_none_to_send() {
        let (app, path) = moments_app();
        let row = win_for(&mut app.store.lock().unwrap(), "hill");
        let win = app
            .store
            .lock()
            .unwrap()
            .notifications("hill", 2_000)
            .unwrap()[0]
            .win
            .unwrap();
        let body = json!({"moment": {"notification": row}, "to": {"friends": ["friend"]}});
        let (status, sent) = share_as_hill(&app, body.clone()).await;
        assert_eq!((status, sent["win"].as_i64()), (StatusCode::OK, Some(win)));
        assert_eq!(inbox(&app, "friend")[0].win, Some(win));
        sql(&app, "update notifications set win_id = null");
        assert_eq!(share_as_hill(&app, body).await.0, StatusCode::CONFLICT);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_group_reaches_each_member_once() {
        let (app, path) = moments_app();
        let id = moment(&app.store.lock().unwrap(), "hill", "level:7");
        sign_up(&app, "ana").await;
        sign_up(&app, "bea").await;
        // Hill, Ana and Cerro are in the club; Bea has a group of her own.
        sql(
            &app,
            "insert into groups (id, name, invite, created_by, created_at)
                 values (1, 'Club', 'one', 'hill', 0), (2, 'Bea', 'two', 'bea', 0);
             insert into group_members (group_id, user, owner, joined_at)
                 values (1, 'hill', 1, 0), (1, 'ana', 0, 0), (1, 'cerro', 0, 0), (2, 'bea', 1, 0);",
        );
        let to = |to: Value| json!({"moment": {"win": id}, "to": to});
        assert_eq!(
            share_as_hill(&app, to(json!({"groups": [2]}))).await.0,
            StatusCode::BAD_REQUEST,
            "a group the player is not in"
        );
        let (status, sent) =
            share_as_hill(&app, to(json!({"friends": ["cerro"], "groups": [1]}))).await;
        assert_eq!(
            (status, sent["recipients"].as_u64()),
            (StatusCode::OK, Some(2))
        );
        assert_eq!(inbox(&app, "ana").len(), 1);
        assert_eq!(
            inbox(&app, "cerro").len(),
            1,
            "named and in the group, told once"
        );
        assert!(inbox(&app, "friend").is_empty() && inbox(&app, "bea").is_empty());
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_chosen_friend_who_muted_the_player_is_skipped() {
        let (app, path) = moments_app();
        let id = moment(&app.store.lock().unwrap(), "hill", "level:7");
        sql(
            &app,
            "insert into muted_people (recipient, sender, created_at) values ('cerro', 'hill', 0)",
        );
        let body = json!({"moment": {"win": id}, "to": {"friends": ["cerro", "friend"]}});
        let (_, sent) = share_as_hill(&app, body).await;
        assert_eq!(sent["recipients"], 1);
        assert!(inbox(&app, "cerro").is_empty());
        assert_eq!(inbox(&app, "friend").len(), 1);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_moment_sent_by_hand_can_be_taken_back_for_a_day() {
        let (app, path) = moments_app();
        let id = moment(&app.store.lock().unwrap(), "hill", "level:7");
        let undo = |user: &str, token: &str| {
            let path = format!("/api/share/{user}/{id}");
            let token = token.to_string();
            let app = app.clone();
            async move { call(&app, "DELETE", &path, &token, Value::Null).await }
        };
        let send_to = |friends: Value| json!({"moment": {"win": id}, "to": {"friends": friends}});
        assert_eq!(
            undo("hill", "hill-secret").await,
            (StatusCode::OK, json!({"retracted": 0})),
            "nothing sent, nothing to take back"
        );

        // A row Friend read stays; Cerro's goes, and Cerro can be sent it again.
        share_as_hill(&app, send_to(json!(["cerro", "friend"]))).await;
        sql(
            &app,
            "update notifications set read_at = 1 where recipient = 'friend'",
        );
        assert_eq!(undo("hill", "hill-secret").await.1["retracted"], 1);
        assert!(inbox(&app, "cerro").is_empty());
        assert_eq!(inbox(&app, "friend").len(), 1);
        let shared = |app: &App| wins::win(&app.store.lock().unwrap().conn, "hill", id).unwrap();
        assert_eq!(shared(&app).shared_friends, 1);
        let (_, again) = share_as_hill(&app, send_to(json!(["cerro"]))).await;
        assert_eq!(again["recipients"], 1);

        // A cheered row stays too.
        let row = inbox(&app, "cerro")[0].id;
        react(
            &mut app.store.lock().unwrap().conn,
            "cerro",
            "Cerro",
            row,
            now_ms(),
        )
        .unwrap();
        assert_eq!(undo("hill", "hill-secret").await.1["retracted"], 0);
        assert_eq!(inbox(&app, "cerro").len(), 1);

        // Only the owner can, and a day later nobody can.
        assert_eq!(
            undo("hill", "cerro-secret").await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(undo("cerro", "cerro-secret").await.0, StatusCode::NOT_FOUND);
        sql(
            &app,
            &format!(
                "update notifications set created_at = created_at - {UNDO_MS} where sender = 'hill'"
            ),
        );
        assert_eq!(undo("hill", "hill-secret").await.0, StatusCode::CONFLICT);
        cleanup(app, path);
    }

    #[test]
    fn automatic_deliveries_are_not_taken_back() {
        let (mut store, _path) = crate::test_support::temporary_store();
        let id = moment(&store, "hill", "level:7");
        let win = wins::win(&store.conn, "hill", id).unwrap();
        let friends = strings(&["cerro"]);
        send_wins(
            &mut store,
            "hill",
            "Hill",
            &friends,
            &[win],
            Sending::Automatic,
            3_000,
        )
        .unwrap();
        assert_eq!(retract(&mut store.conn, id, 4_000).unwrap(), Some(0));
        assert_eq!(store.notifications("cerro", 4_000).unwrap().len(), 1);
    }
}
