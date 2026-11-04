//! Fixtures shared by the handler tests of several modules, and what those tests use.

pub(crate) use crate::app::{App, Player, next_revision, now_ms};
pub(crate) use crate::config::Config;
pub(crate) use crate::game::{self, Clock, ClockHistory, Review, Week};
pub(crate) use crate::store::Store;
use crate::study::{Upload, UploadResponse, upload};
pub(crate) use crate::{access, decks, notifications};
pub(crate) use axum::Json;
pub(crate) use axum::extract::{Path as UrlPath, State};
pub(crate) use axum::http::{HeaderMap, StatusCode, header};
pub(crate) use std::collections::HashMap;
pub(crate) use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
pub(crate) use std::sync::{Arc, Mutex, RwLock};
pub(crate) use std::time::Duration;

/// A store in a new temporary directory; [`cleanup`] removes it.
pub(crate) fn temporary_store() -> (Store, PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "ankiquest-test-{}-{}-{}",
        std::process::id(),
        now_ms(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    (Store::open(&path).unwrap(), path)
}

/// Drops whatever holds the store, then removes its directory.
pub(crate) fn cleanup<T>(owner: T, path: impl AsRef<Path>) {
    drop(owner);
    std::fs::remove_dir_all(path).unwrap();
}

/// An app over `store` with `config`: accounts loaded from the store, no players loaded.
pub(crate) fn app_from(store: Store, config: Config) -> App {
    App {
        accounts: crate::accounts::Directory::load(&store.conn).unwrap(),
        config,
        access: access::Access::default(),
        week: Week::default(),
        store: Mutex::new(store),
        players: RwLock::new(HashMap::new()),
        standings: Default::default(),
        heartbeat: Default::default(),
    }
}

/// An app with the JSON `config` over a new temporary store.
pub(crate) fn app_with(config: serde_json::Value) -> (Arc<App>, PathBuf) {
    let (store, path) = temporary_store();
    (
        Arc::new(app_from(store, serde_json::from_value(config).unwrap())),
        path,
    )
}

pub(crate) fn fixture() -> (Arc<App>, PathBuf) {
    app_with(serde_json::json!({
        "users": {
            "cerro": {"display": "Cerro", "token": "cerro-secret"},
            "hill": {"display": "Hill", "token": "hill-secret"},
            "friend": {"token": "friend-secret"}
        }
    }))
}

/// `request` through the whole router, from `peer` unless it names its own `ConnectInfo`.
pub(crate) async fn route_from(
    app: &Arc<App>,
    request: axum::http::Request<axum::body::Body>,
    peer: &str,
) -> axum::response::Response {
    use tower::ServiceExt;
    crate::router(app.clone())
        .layer(axum::extract::connect_info::MockConnectInfo(
            peer.parse::<std::net::SocketAddr>().unwrap(),
        ))
        .oneshot(request)
        .await
        .unwrap()
}

pub(crate) async fn route(
    app: &Arc<App>,
    request: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    route_from(app, request, "192.0.2.1:40000").await
}

/// A response's status and JSON body, or null when the body is not JSON.
pub(crate) async fn json_response(
    response: axum::response::Response,
) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

pub(crate) fn headers(user: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {user}-secret").parse().unwrap(),
    );
    headers
}

pub(crate) fn reviews_at(first: i64, count: i64) -> Vec<Review> {
    (0..count)
        .map(|index| Review {
            id: first + index * 1000,
            cid: 1_000 + index,
            last_ivl: 1,
            ivl: None,
            time_ms: 8_000,
            kind: 1,
        })
        .collect()
}

pub(crate) fn with_reviews(app: &App, user: &str, count: i64) {
    app.players.write().unwrap().insert(
        user.into(),
        Player {
            reviews: Arc::new(reviews_at(now_ms() - 600_000, count)),
            clock: Clock::default(),
            clock_history: ClockHistory::default(),
            freeze_policy: game::FreezePolicy::default(),
            revision: next_revision(),
        },
    );
}

pub(crate) fn sample_upload(remaining: u64, silent: bool) -> Upload {
    Upload {
        reviews: vec![],
        deleted: vec![],
        clock: Clock::default(),
        silent,
        catalog: false,
        decks: Some(vec![decks::Snapshot {
            id: "1".into(),
            name: "Spanish".into(),
            remaining,
            reviewed_today: 3,
            day: Clock::default().day(now_ms()),
        }]),
    }
}

pub(crate) fn preferences(recipients: &[&str]) -> decks::SettingsUpdate {
    decks::SettingsUpdate {
        decks: vec![decks::Preference {
            id: "1".into(),
            enabled: true,
            recipients: recipients.iter().map(|r| (*r).into()).collect(),
        }],
        nudges: None,
        celebrations: None,
    }
}

pub(crate) fn plain_upload(reviews: Vec<Review>) -> Upload {
    Upload {
        reviews,
        deleted: vec![],
        clock: Clock::default(),
        silent: false,
        catalog: false,
        decks: None,
    }
}

pub(crate) async fn upload_as(app: &Arc<App>, user: &str, body: Upload) -> UploadResponse {
    upload(
        State(app.clone()),
        UrlPath(user.into()),
        headers(user),
        Json(body),
    )
    .await
    .unwrap()
    .0
}
