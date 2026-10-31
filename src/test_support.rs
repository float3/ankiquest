//! Fixtures shared by the handler tests of several modules.

use crate::{App, Config, Week, access, decks};
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
