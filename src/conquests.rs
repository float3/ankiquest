//! Hard cards learned at last: a page to share each one, and telling friends about it.
//! Only what [`Conquest`] carries is shared, never the card itself.
use crate::decks::Outgoing;
use crate::game::{self, Conquest};
use crate::{App, access, authorized, now_ms, store_error};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub(crate) struct Shared {
    user: String,
    display: String,
    pub(crate) conquest: Conquest,
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
    let body = format!(
        "They forgot it {} times and learned it anyway.",
        conquest.lapses
    );
    let now = now_ms();
    let mut store = app.store.lock().unwrap();
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

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/conquered/{user}/{at}", get(page))
        .route("/api/conquests/{user}/{at}", get(show))
        .route("/api/conquests/{user}/{at}/tell", post(tell))
}
