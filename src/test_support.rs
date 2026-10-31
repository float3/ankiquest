//! Fixtures shared by the handler tests of several modules, and what those tests use.

use crate::Upload;
use crate::UploadResponse;
pub(crate) use crate::app::{App, Player, next_revision, now_ms};
pub(crate) use crate::config::Config;
pub(crate) use crate::game::{self, Clock, ClockHistory, Review, Week};
pub(crate) use crate::store::Store;
use crate::upload;
pub(crate) use crate::{access, decks, notifications};
pub(crate) use axum::Json;
pub(crate) use axum::body::Bytes;
pub(crate) use axum::extract::{Path as UrlPath, Query, State};
pub(crate) use axum::http::{HeaderMap, StatusCode, header};
pub(crate) use std::collections::HashMap;
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::sync::{Arc, Mutex, RwLock};
pub(crate) use std::time::Duration;

pub(crate) fn fixture() -> (Arc<App>, PathBuf) {
    let (store, path) = decks::tests::temporary_store();
    let config: Config = serde_json::from_value(serde_json::json!({
        "users": {
            "cerro": {"display": "Cerro", "token": "cerro-secret"},
            "hill": {"display": "Hill", "token": "hill-secret"},
            "friend": {"token": "friend-secret"}
        }
    }))
    .unwrap();
    (
        Arc::new(App {
            config,
            access: access::Access::default(),
            accounts: Default::default(),
            week: Week::default(),
            store: Mutex::new(store),
            players: RwLock::new(HashMap::new()),
            standings: Default::default(),
        }),
        path,
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
