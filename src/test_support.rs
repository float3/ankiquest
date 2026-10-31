//! Fixtures shared by the handler tests of several modules.

use crate::app::{App, Player, next_revision, now_ms};
use crate::config::Config;
use crate::game::{self, Clock, ClockHistory, Review, Week};
use crate::{access, decks};
use axum::http::{HeaderMap, header};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

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
