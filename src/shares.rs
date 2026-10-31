//! Sharing a win with friends. A win, a celebration, a finished challenge, a weekly recap or a
//! top-three place on this week's board reaches everyone in the player's circle as a
//! notification they can reply to and cheer, with the same topic so it shows the same art.
//! A win can also leave AnkiQuest as a link to its card.

use crate::app::{App, now_ms};
use crate::notifications::{Outgoing, insert_notification};
use crate::store::{Error, Store, clock_in};
use crate::wins::{self, Win as Moment};
use crate::{access::authorized, boards::standings, web_util::base_url, web_util::store_error};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

/// Notification kinds a player may pass on to friends.
const SHAREABLE: [&str; 3] = ["celebration", "challenge_complete", "reminder_weekly_recap"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    /// One of the player's own notifications.
    notification: Option<i64>,
    /// The player's current place on this week's board, 1 to 3.
    place: Option<usize>,
    /// One of the player's wins.
    win: Option<i64>,
    /// Where the win goes: "friends" or "link".
    to: Option<String>,
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
                        what: format!("{}: “{}”", r.get::<_, String>(0)?, r.get::<_, String>(1)?),
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
    let title = format!("🎉 {display} shared a win");
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
    automatic: bool,
    now: i64,
) -> Result<usize, Error> {
    if wins.is_empty() {
        return Ok(0);
    }
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
            (
                format!("🎉 {display} shared a win"),
                format!("{}: “{}”", win.title, win.body),
            )
        } else {
            (
                format!("🎉 {display}: {} wins", fresh.len()),
                fresh
                    .iter()
                    .map(|win| win.title.as_str())
                    .collect::<Vec<_>>()
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
            "update notifications set topic = ?2, win_id = ?3 where id = ?1",
            params![id, first.topic(), first.id],
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
    send_wins(store, user, &app.display(user), &friends, &wins, true, now)
}

/// The text a player pastes along with a win's link.
fn invitation(win: &Moment, url: &str, language: &str) -> String {
    crate::i18n::substitute(
        &crate::i18n::text("{0}: {1} Join me on AnkiQuest: {2}", language),
        &[
            &crate::i18n::system(&win.title, language),
            &crate::i18n::system(&win.body, language),
            url,
        ],
    )
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
    let mut store = app.store.lock().unwrap();
    let token = wins::ensure_link(&store.conn, user, id)
        .map_err(store_error)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let win = wins::win(&store.conn, user, id).ok_or(StatusCode::NOT_FOUND)?;
    let link = wins::link(&base_url(app, headers), &token);
    if to == "link" {
        let language = crate::i18n::owned(headers, &store, user).map_err(store_error)?;
        let text = invitation(&win, &link.url, &language);
        return Ok(json!(Linked {
            url: link.url,
            image: link.image,
            text,
        }));
    }
    let friends = friends(app, &store.conn, user).map_err(store_error)?;
    let display = app.display(user);
    let recipients = send_wins(
        &mut store,
        user,
        &display,
        &friends,
        &[win],
        false,
        now_ms(),
    )
    .map_err(store_error)?;
    Ok(json!({ "recipients": recipients, "link": link }))
}

async fn share(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Result<Json<Value>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let win = match (request.notification, request.place, request.win, request.to) {
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
                what: format!("#{place} on this week's leaderboard"),
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
                title: &format!("🎉 {display} cheered"),
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
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
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
        .route("/api/react/{user}", post(react_to))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Event;
    use crate::test_support::{fixture, headers};
    use crate::wins::tests::moment;

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
        let (mut store, _path) = crate::decks::tests::temporary_store();
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
        let (mut store, _path) = crate::decks::tests::temporary_store();
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
        let (mut store, _path) = crate::decks::tests::temporary_store();
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
                false,
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
        let (mut store, _path) = crate::decks::tests::temporary_store();
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
            send_wins(&mut store, "hill", "Hill", &friends, &both, true, 3_000).unwrap(),
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
            send_wins(&mut store, "hill", "Hill", &friends, &third, true, 4_000).unwrap(),
            0,
            "the cap counts rows, and one was sent today"
        );
        assert_eq!(
            send_wins(&mut store, "hill", "Hill", &friends, &third, false, 4_000).unwrap(),
            1,
            "a win over the cap can still be shared by hand"
        );
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
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn a_cheer_reaches_the_sender_once_and_can_be_answered() {
        let (mut store, _path) = crate::decks::tests::temporary_store();
        let id = moment(&store, "hill", "level:7");
        let win = wins::win(&store.conn, "hill", id).unwrap();
        send_wins(
            &mut store,
            "hill",
            "Hill",
            &strings(&["cerro"]),
            &[win],
            false,
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
        let (mut store, _path) = crate::decks::tests::temporary_store();
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
}
