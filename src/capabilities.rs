//! What this server supports, so clients can switch features on without waiting for a deploy.

use crate::app::App;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use std::sync::Arc;

pub(crate) const PATH: &str = "/api/capabilities";

/// Stable names of the features clients may need to detect. Names are never removed or renamed;
/// new ones are added as features land.
pub(crate) const FEATURES: &[&str] = &[
    "activity",
    "cookie_session",
    "devices",
    "goal_backlog_days",
    "reactions",
    "share_cards",
    "wins",
];

async fn capabilities() -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "max-age=300")],
        Json(json!({"version": 1, "features": FEATURES})),
    )
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new().route(PATH, get(capabilities))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn capabilities_are_public_cached_and_list_stable_names() {
        let (store, path) = crate::decks::tests::temporary_store();
        let config = serde_json::from_value(json!({
            "private_site": true,
            "users": {"alice": {"token": "alice-token"}}
        }))
        .unwrap();
        let app = Arc::new(App {
            config,
            access: crate::access::Access::default(),
            accounts: Default::default(),
            week: crate::game::Week::default(),
            store: std::sync::Mutex::new(store),
            players: Default::default(),
            standings: Default::default(),
        });
        let response = crate::router(app.clone())
            .oneshot(Request::get(PATH).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "max-age=300");
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["version"], 1);
        let names: Vec<&str> = body["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap())
            .collect();
        assert_eq!(names, FEATURES);
        for name in [
            "cookie_session",
            "devices",
            "wins",
            "goal_backlog_days",
            "share_cards",
        ] {
            assert!(names.contains(&name), "{name}");
        }
        for name in names {
            assert!(
                name.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{name} is snake_case"
            );
        }
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
