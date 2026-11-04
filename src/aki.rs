//! Public, embedded mascot art. Only these fixed assets are served before sign-in.
use crate::app::App;
use axum::{
    Router,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use std::sync::Arc;

/// A file in static/, served at the same path.
macro_rules! png {
    ($path:literal) => {
        (
            concat!("/", $path),
            include_bytes!(concat!("../static/", $path)),
        )
    };
}

/// Every companion picture, by the path it is served at. `face-192.png` and `face-32.png` are
/// small copies of `face.png` for icons, which pages draw at 25 to 72 pixels.
const ASSETS: &[(&str, &[u8])] = &[
    png!("aki/welcome.png"),
    png!("aki/review.png"),
    png!("aki/celebrate.png"),
    png!("aki/streak.png"),
    png!("aki/freeze.png"),
    png!("aki/winner.png"),
    png!("aki/face.png"),
    png!("aki/face-192.png"),
    png!("aki/face-32.png"),
    png!("ankilope/welcome.png"),
    png!("ankilope/study.png"),
    png!("ankilope/celebrate.png"),
    png!("ankilope/face.png"),
    png!("ankilope/face-192.png"),
];

pub(crate) fn public_asset(path: &str) -> bool {
    ASSETS.iter().any(|(served, _)| *served == path)
}

async fn asset(uri: Uri) -> Response {
    let Some((_, bytes)) = ASSETS.iter().find(|(served, _)| *served == uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        *bytes,
    )
        .into_response()
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/aki/{name}", get(asset))
        .route("/ankilope/{name}", get(asset))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(path: &str) -> Response {
        asset(path.parse().unwrap()).await
    }

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
            "face-192",
            "face-32",
        ] {
            let path = format!("/aki/{name}.png");
            assert!(public_asset(&path));
            let response = get(&path).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
            let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
                .await
                .unwrap();
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        }
        for name in ["config.json", "..%2Fconfig.json", "FACE.png", "unknown.png"] {
            let path = format!("/aki/{name}");
            assert!(!public_asset(&path));
            assert_eq!(get(&path).await.status(), StatusCode::NOT_FOUND);
        }
        for name in ["welcome", "study", "celebrate", "face", "face-192"] {
            let path = format!("/ankilope/{name}.png");
            assert!(public_asset(&path));
            let response = get(&path).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        }
        assert!(!public_asset("/ankilope/config.json"));
        assert!(!public_asset("/ankilope/winner.png"));
        assert_eq!(
            get("/ankilope/unknown.png").await.status(),
            StatusCode::NOT_FOUND
        );
    }
}
