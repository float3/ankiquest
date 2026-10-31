//! Public, embedded mascot art. Only these fixed assets are served before sign-in.
use crate::app::App;
use axum::{
    Router,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use std::sync::Arc;

pub(crate) fn public_asset(path: &str) -> bool {
    matches!(
        path,
        "/aki/welcome.png"
            | "/aki/review.png"
            | "/aki/celebrate.png"
            | "/aki/streak.png"
            | "/aki/freeze.png"
            | "/aki/winner.png"
            | "/aki/face.png"
            | "/ankilope/welcome.png"
            | "/ankilope/study.png"
            | "/ankilope/celebrate.png"
            | "/ankilope/face.png"
    )
}

async fn asset(Path(name): Path<String>) -> Response {
    let bytes: &'static [u8] = match name.as_str() {
        "welcome.png" => include_bytes!("../static/aki/welcome.png"),
        "review.png" => include_bytes!("../static/aki/review.png"),
        "celebrate.png" => include_bytes!("../static/aki/celebrate.png"),
        "streak.png" => include_bytes!("../static/aki/streak.png"),
        "freeze.png" => include_bytes!("../static/aki/freeze.png"),
        "winner.png" => include_bytes!("../static/aki/winner.png"),
        "face.png" => include_bytes!("../static/aki/face.png"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response()
}

async fn ankilope_asset(Path(name): Path<String>) -> Response {
    let bytes: &'static [u8] = match name.as_str() {
        "welcome.png" => include_bytes!("../static/ankilope/welcome.png"),
        "study.png" => include_bytes!("../static/ankilope/study.png"),
        "celebrate.png" => include_bytes!("../static/ankilope/celebrate.png"),
        "face.png" => include_bytes!("../static/ankilope/face.png"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response()
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/aki/{name}", get(asset))
        .route("/ankilope/{name}", get(ankilope_asset))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn only_named_images_are_public_and_have_png_content() {
        for name in [
            "welcome",
            "review",
            "celebrate",
            "streak",
            "freeze",
            "winner",
            "face",
        ] {
            assert!(public_asset(&format!("/aki/{name}.png")));
            let response = asset(Path(format!("{name}.png"))).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
            let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
                .await
                .unwrap();
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        }
        for name in ["config.json", "../config.json", "FACE.png", "unknown.png"] {
            assert!(!public_asset(&format!("/aki/{name}")));
            assert_eq!(
                asset(Path(name.into())).await.status(),
                StatusCode::NOT_FOUND
            );
        }
        for name in ["welcome", "study", "celebrate", "face"] {
            assert!(public_asset(&format!("/ankilope/{name}.png")));
            let response = ankilope_asset(Path(format!("{name}.png"))).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        }
        assert!(!public_asset("/ankilope/config.json"));
        assert_eq!(
            ankilope_asset(Path("unknown.png".into())).await.status(),
            StatusCode::NOT_FOUND
        );
    }
}
