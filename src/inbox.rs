//! The inbox endpoints: notifications, the activity feed, what a player sent, and replies.

use crate::access::authorized;
use crate::app::{App, now_ms};
use crate::web_util::{failed_for, store_error};
use crate::{challenges, decks, i18n, notifications, reminders};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(crate) async fn get_notifications(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<notifications::Notification>>, StatusCode> {
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
    let language = i18n::owned(&headers, &store, &user).map_err(store_error)?;
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
        let sender_display = app.display(&notice.sender);
        i18n::notification(&mut notice, &language, &sender_display);
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
    category: Option<notifications::ActivityCategory>,
    unread_only: Option<bool>,
}

/// A page size of 1 to 200 that starts before a positive notification id, if any.
fn valid_page(limit: u32, before: Option<i64>) -> bool {
    (1..=200).contains(&limit) && before.is_none_or(|id| id > 0)
}

async fn activity(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Query(query): Query<ActivityQuery>,
) -> Result<Json<notifications::Activity>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let days = query.days.unwrap_or(notifications::ACTIVITY_DAYS);
    let limit = query.limit.unwrap_or(100);
    if !matches!(days, 30 | 90) || !valid_page(limit, query.before) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut store = app.store.lock().unwrap();
    let now = now_ms();
    challenges::refresh(&mut store, &app.participants(), now).map_err(store_error)?;
    let language = i18n::owned(&headers, &store, &user).map_err(store_error)?;
    let mut activity = store
        .activity_filtered(
            &user,
            now,
            notifications::ActivityOptions {
                days,
                before: query.before,
                limit,
                category: query.category,
                unread_only: query.unread_only.unwrap_or(false),
            },
        )
        .map_err(store_error)?;
    for notice in &mut activity.items {
        i18n::notification(notice, &language, &app.display(&notice.sender));
    }
    for notice in &mut activity.attention {
        i18n::notification(notice, &language, &app.display(&notice.sender));
    }
    Ok(Json(activity))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivityRead {
    #[serde(default)]
    ids: Vec<i64>,
    through: Option<i64>,
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
    if request.ids.len() > 200
        || request.ids.iter().any(|id| *id <= 0)
        || request
            .through
            .is_some_and(|id| id <= 0 || !request.ids.is_empty())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut store = app.store.lock().unwrap();
    match request.through {
        Some(through) => store.read_all_activity(&user, through, now_ms()),
        None => store.read_activity(&user, &request.ids, now_ms()),
    }
    .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SentQuery {
    before: Option<i64>,
    limit: Option<u32>,
}

async fn sent_activity(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Query(query): Query<SentQuery>,
) -> Result<Json<notifications::SentPage>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let limit = query.limit.unwrap_or(100);
    if !valid_page(limit, query.before) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let store = app.store.lock().unwrap();
    let language = i18n::owned(&headers, &store, &user).map_err(store_error)?;
    let mut page = store
        .sent(&user, now_ms(), query.before, limit)
        .map_err(store_error)?;
    for item in &mut page.items {
        i18n::notification(&mut item.notice, &language, &app.display(&user));
    }
    Ok(Json(page))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplyRequest {
    pub(crate) notification: i64,
    pub(crate) message: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReplyResponse {
    pub(crate) sent_to: String,
}

/// Answers a deck completion, so finishing a deck starts a conversation.
pub(crate) async fn reply(
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
        .map_err(failed_for("reply from", &user))?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(ReplyResponse {
        sent_to: app.display(&sender),
    }))
}
pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/notifications/{user}", get(get_notifications))
        .route("/api/activity/{user}", get(activity))
        .route("/api/activity/{user}/read", post(read_activity))
        .route("/api/activity/{user}/sent", get(sent_activity))
        .route("/api/reply/{user}", post(reply))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decks::set_decks;
    use crate::study::{Upload, upload};
    use crate::test_support::*;

    #[tokio::test]
    async fn spanish_inbox_preserves_names_decks_and_authored_messages() {
        let (app, _path) = fixture();
        let now = now_ms();
        {
            let mut store = app.store.lock().unwrap();
            for (kind, title, body) in [
                (
                    "completion",
                    "Deck complete",
                    "Cerro has finished their Friends::{0} studies for today.",
                ),
                ("message", "Deck complete", "Good job!"),
                ("reply", "Thanks!", "Good job!"),
                (
                    "nudge",
                    "Time for Anki?",
                    "Cerro is cheering you on. Make a little time for Anki today!",
                ),
            ] {
                store
                    .send(
                        &notifications::Outgoing {
                            to: "hill",
                            from: "cerro",
                            title,
                            body,
                            kind,
                        },
                        1,
                        now,
                    )
                    .unwrap();
            }
        }
        let mut owner = headers("hill");
        owner.insert(header::ACCEPT_LANGUAGE, "es-ES,es;q=0.9".parse().unwrap());
        let inbox = get_notifications(State(app.clone()), UrlPath("hill".into()), owner)
            .await
            .unwrap()
            .0;
        let completion = inbox
            .iter()
            .find(|notice| notice.kind == "completion")
            .unwrap();
        assert_eq!(completion.title, "Mazo completado");
        assert_eq!(
            completion.body,
            "Cerro ha terminado su estudio de Friends::{0} por hoy."
        );
        for notice in inbox
            .iter()
            .filter(|notice| matches!(notice.kind.as_str(), "message" | "reply"))
        {
            assert_eq!(notice.body, "Good job!");
        }
        assert!(
            inbox
                .iter()
                .find(|notice| notice.kind == "nudge")
                .unwrap()
                .body
                .starts_with("Cerro te anima.")
        );
        assert_eq!(app.store.lock().unwrap().language("hill").unwrap(), "es");
        let english = {
            let mut h = headers("hill");
            h.insert(header::ACCEPT_LANGUAGE, "en".parse().unwrap());
            h
        };
        let inbox = get_notifications(State(app.clone()), UrlPath("hill".into()), english)
            .await
            .unwrap()
            .0;
        assert!(
            inbox
                .iter()
                .find(|notice| notice.kind == "completion")
                .unwrap()
                .body
                .contains("has finished their Friends::{0}")
        );
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
            get_notifications(State(app), UrlPath(user.into()), headers(user))
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
}
