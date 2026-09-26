use crate::decks::Outgoing;
use crate::store::{Error, Store};
use crate::{App, authorized, now_ms, store_error};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize)]
struct Friend {
    user: String,
    display: String,
    enabled: bool,
    sent_today: bool,
}

#[derive(Debug, Serialize)]
struct Settings {
    receiving: bool,
    friends: Vec<Friend>,
}

fn key(sender: &str, day: i64) -> String {
    format!("friend-nudge:{day}:{sender}")
}

fn sent(store: &Store, sender: &str, recipient: &str, day: i64) -> Result<bool, Error> {
    Ok(store.conn.query_row(
        "select exists(select 1 from seen where user=?1 and key=?2)",
        params![recipient, key(sender, day)],
        |row| row.get(0),
    )?)
}

fn send(
    store: &mut Store,
    sender: &str,
    display: &str,
    recipient: &str,
    now: i64,
) -> Result<StatusCode, Error> {
    if !store.nudges_enabled(recipient)? {
        return Ok(StatusCode::FORBIDDEN);
    }
    let day = store.clock(sender)?.day(now);
    if sent(store, sender, recipient, day)? {
        return Ok(StatusCode::CONFLICT);
    }
    // Called under the App store lock; the limit and delivery commit together.
    store.send_once(
        &Outgoing {
            to: recipient,
            from: sender,
            title: "Time for Anki?",
            body: &format!("{display} is cheering you on. Make a little time for Anki today!"),
            kind: "nudge",
        },
        &key(sender, day),
        day,
        now,
        false,
    )?;
    Ok(StatusCode::OK)
}

async fn settings(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Settings>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let store = app.store.lock().unwrap();
    let day = store.clock(&user).map_err(store_error)?.day(now_ms());
    let mut friends = Vec::new();
    for recipient in app
        .config
        .users
        .keys()
        .filter(|recipient| **recipient != user)
    {
        friends.push(Friend {
            user: recipient.clone(),
            display: app.display(recipient),
            enabled: store.nudges_enabled(recipient).map_err(store_error)?,
            sent_today: sent(&store, &user, recipient, day).map_err(store_error)?,
        });
    }
    friends.sort_by(|a, b| a.display.cmp(&b.display).then(a.user.cmp(&b.user)));
    Ok(Json(Settings {
        receiving: store.nudges_enabled(&user).map_err(store_error)?,
        friends,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    recipient: String,
}

async fn nudge(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if request.recipient == user || !app.config.users.contains_key(&request.recipient) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let result = send(
        &mut app.store.lock().unwrap(),
        &user,
        &app.display(&user),
        &request.recipient,
        now_ms(),
    )
    .map_err(store_error)?;
    if result != StatusCode::OK {
        return Err(result);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receiving {
    enabled: bool,
}

async fn receiving(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Receiving>,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    app.store
        .lock()
        .unwrap()
        .set_nudges(&user, request.enabled)
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/friend-nudges.js", get(script))
        .route("/api/friend-nudges/{user}", get(settings).post(nudge))
        .route("/api/friend-nudges/{user}/receiving", post(receiving))
}

async fn script() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/javascript; charset=utf-8",
        )],
        include_str!("../static/friend-nudges.js"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use std::sync::{Mutex, RwLock};
    use tower::ServiceExt;

    #[tokio::test]
    async fn sender_routes_require_owner_auth_and_reject_self_or_unknown_recipients() {
        let (store, path) = crate::decks::tests::temporary_store();
        let config = serde_json::from_value(serde_json::json!({"users": {
            "cerro":{"token":"cerro-secret"},"hill":{"token":"hill-secret"}
        }}))
        .unwrap();
        let app = Arc::new(App {
            access: crate::access::Access::default(),
            config,
            week: crate::game::Week::default(),
            store: Mutex::new(store),
            players: RwLock::new(Default::default()),
        });
        for (owner, token, recipient, expected) in [
            ("cerro", "", "hill", StatusCode::UNAUTHORIZED),
            ("cerro", "hill-secret", "hill", StatusCode::UNAUTHORIZED),
            ("cerro", "cerro-secret", "cerro", StatusCode::BAD_REQUEST),
            ("cerro", "cerro-secret", "missing", StatusCode::BAD_REQUEST),
            ("cerro", "cerro-secret", "hill", StatusCode::FORBIDDEN),
        ] {
            let response = routes()
                .with_state(app.clone())
                .oneshot(
                    HttpRequest::post(format!("/api/friend-nudges/{owner}"))
                        .header("Authorization", format!("Bearer {token}"))
                        .header("Content-Type", "application/json")
                        .body(Body::from(
                            serde_json::json!({"recipient":recipient}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        app.store.lock().unwrap().set_nudges("hill", true).unwrap();
        for expected in [StatusCode::NO_CONTENT, StatusCode::CONFLICT] {
            let response = routes()
                .with_state(app.clone())
                .oneshot(
                    HttpRequest::post("/api/friend-nudges/cerro")
                        .header("Authorization", "Bearer cerro-secret")
                        .header("Content-Type", "application/json")
                        .body(Body::from(r#"{"recipient":"hill"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn nudges_require_opt_in_and_limit_each_friend_until_the_next_anki_day() {
        let (mut store, path) = crate::decks::tests::temporary_store();
        let now = 1_800_000_000_000;
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now).unwrap(),
            StatusCode::FORBIDDEN
        );
        assert!(
            !sent(
                &store,
                "cerro",
                "hill",
                store.clock("cerro").unwrap().day(now)
            )
            .unwrap()
        );
        store.set_nudges("hill", true).unwrap();
        store.set_nudges("friend", true).unwrap();
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now).unwrap(),
            StatusCode::OK
        );
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now + 1000).unwrap(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "friend", now).unwrap(),
            StatusCode::OK
        );
        assert_eq!(
            send(&mut store, "friend", "Friend", "hill", now).unwrap(),
            StatusCode::OK
        );
        let inbox = store.notifications("hill", now).unwrap();
        assert_eq!(inbox.len(), 2);
        assert_eq!(inbox[0].kind, "nudge");
        assert!(!inbox[0].sender.is_empty());
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now).unwrap(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now + 86_400_000).unwrap(),
            StatusCode::OK
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
}
