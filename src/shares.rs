//! Sharing a win with friends. A celebration, a finished challenge, a weekly recap or a
//! top-three place on this week's board reaches everyone in the player's circle as a
//! notification they can reply to, with the same topic so it shows the same art.

use crate::decks::Outgoing;
use crate::store::{Error, Store};
use crate::{App, authorized, now_ms, standings, store_error};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
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
}

#[derive(Debug, Serialize)]
struct Shared {
    /// Friends who got it now; those who already had it or muted the player are left out.
    recipients: usize,
}

struct Win {
    what: String,
    topic: Option<String>,
    key: String,
}

fn notification_win(store: &Store, user: &str, id: i64) -> Result<Option<Win>, Error> {
    let row: Option<(String, String, String, Option<String>)> = store
        .conn
        .query_row(
            "select title, body, kind, topic from notifications
             where id = ?1 and recipient = ?2 and push_only = 0",
            params![id, user],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    Ok(row
        .filter(|(_, _, kind, _)| SHAREABLE.contains(&kind.as_str()))
        .map(|(title, body, _, topic)| Win {
            what: format!("{title}: “{body}”"),
            topic,
            key: format!("notification:{id}"),
        }))
}

fn muted(store: &Store, recipient: &str, sender: &str) -> Result<bool, Error> {
    Ok(store.conn.query_row(
        "select exists(select 1 from (select recipient, sender from incoming_muted_senders
                       union all select recipient, sender from sender_unsubscriptions)
                       where recipient = ?1 and sender = ?2)",
        params![recipient, sender],
        |r| r.get(0),
    )?)
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
        if muted(store, friend, sender)? {
            continue;
        }
        let key = format!("share:{sender}:{}", win.key);
        let before = store.conn.query_row(
            "select exists(select 1 from seen where user = ?1 and key = ?2)",
            params![friend, key],
            |r| r.get::<_, bool>(0),
        )?;
        if before {
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

async fn share(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Result<Json<Shared>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let circle = app
        .circle(&app.store.lock().unwrap().conn, &user)
        .map_err(store_error)?;
    let win = match (request.notification, request.place) {
        (Some(id), None) => notification_win(&app.store.lock().unwrap(), &user, id)
            .map_err(store_error)?
            .ok_or(StatusCode::NOT_FOUND)?,
        (None, Some(place @ 1..=3)) => {
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
    let friends: Vec<String> = circle
        .into_iter()
        .filter(|friend| *friend != user && app.is_user(friend))
        .collect();
    let recipients = send(
        &mut app.store.lock().unwrap(),
        &user,
        &app.display(&user),
        &friends,
        &win,
        now_ms(),
    )
    .map_err(store_error)?;
    Ok(Json(Shared { recipients }))
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/share/{user}", post(share))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Event;

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

    #[test]
    fn friends_get_a_win_once_with_its_topic_and_can_reply() {
        let (mut store, _path) = crate::decks::tests::temporary_store();
        let id = win_for(&mut store, "hill");
        let win = notification_win(&store, "hill", id).unwrap().unwrap();
        assert_eq!(win.topic.as_deref(), Some("streak"));
        let friends = ["cerro".to_string(), "aldanita".to_string()];
        assert_eq!(
            send(&mut store, "hill", "Hill", &friends, &win, 3_000).unwrap(),
            2
        );
        assert_eq!(
            send(&mut store, "hill", "Hill", &friends, &win, 4_000).unwrap(),
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
    fn a_friend_who_muted_you_is_skipped() {
        let (mut store, _path) = crate::decks::tests::temporary_store();
        let id = win_for(&mut store, "hill");
        let win = notification_win(&store, "hill", id).unwrap().unwrap();
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
                &["cerro".to_string()],
                &win,
                3_000
            )
            .unwrap(),
            0
        );
        assert!(store.notifications("cerro", 3_000).unwrap().is_empty());
    }
}
