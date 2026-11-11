//! What this server supports, so clients can switch features on without waiting for a deploy.

use crate::app::App;
use axum::extract::State;
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
    "app_sign_in",
    "cookie_session",
    "deck_share",
    "devices",
    "global_leaderboard_hide",
    "goal_backlog_days",
    "mute",
    "reactions",
    "review_offsets",
    "share_cards",
    "wins",
];

/// The configured main address, trimmed and without a trailing slash, so clients that saved
/// another address for this server can move to it.
fn public_url(app: &App) -> Option<String> {
    let url = app
        .config
        .public_url
        .as_deref()?
        .trim()
        .trim_end_matches('/');
    (!url.is_empty()).then(|| url.to_string())
}

async fn capabilities(State(app): State<Arc<App>>) -> impl IntoResponse {
    let mut body = json!({"version": 1, "features": FEATURES});
    if let Some(url) = public_url(&app) {
        body["public_url"] = url.into();
    }
    ([(header::CACHE_CONTROL, "max-age=300")], Json(body))
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new().route(PATH, get(capabilities))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::cleanup;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn capabilities_are_public_cached_and_list_stable_names() {
        let (app, path) = crate::test_support::app_with(json!({
            "private_site": true,
            "users": {"alice": {"token": "alice-token"}}
        }));
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
        assert!(body.get("public_url").is_none());
        cleanup(app, path);
    }

    async fn public_url_answer(config: serde_json::Value) -> serde_json::Value {
        let (app, path) = crate::test_support::app_with(config);
        let response = crate::router(app.clone())
            .oneshot(Request::get(PATH).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        cleanup(app, path);
        body.get("public_url").cloned().unwrap_or_default()
    }

    #[tokio::test]
    async fn the_configured_main_address_is_named_without_a_trailing_slash() {
        assert_eq!(
            public_url_answer(json!({"public_url": " https://anki.example.test/ "})).await,
            "https://anki.example.test"
        );
        assert_eq!(
            public_url_answer(json!({"public_url": "https://anki.example.test"})).await,
            "https://anki.example.test"
        );
        assert!(
            public_url_answer(json!({"public_url": "  "}))
                .await
                .is_null()
        );
        assert!(public_url_answer(json!({})).await.is_null());
    }
}
