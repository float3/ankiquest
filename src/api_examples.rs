//! Keeps `docs/api/examples` equal to what the real handlers answer. Run with
//! `UPDATE_API_EXAMPLES=1` to rewrite the files instead of comparing.
//!
//! The clock is pinned to [`NOW_MS`]. Random values are replaced before comparing, numbered in
//! the order they appear: device tokens by `example-token-N`, win link tokens by
//! `example-link-N` and device ids by `example-device-N`.

use crate::app::App;
use crate::game::{Clock, Event, Week};
use axum::body::{Body, to_bytes};
use axum::extract::connect_info::MockConnectInfo;
use axum::http::Request;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

const PASSWORD: &str = "long enough pw";

/// Wednesday 28 October 2026, 10:30 UTC.
const NOW_MS: i64 = 1_793_183_400_000;

struct Fixture {
    app: Arc<App>,
    path: PathBuf,
    secrets: Vec<(String, String)>,
}

impl Fixture {
    fn new() -> Self {
        let (store, path) = crate::decks::tests::temporary_store();
        let config = serde_json::from_value(json!({
            "registration": true,
            "public_url": "https://anki.example.test",
            "users": {
                "hill": {"display": "Hill", "token": "hill-secret"},
                "cerro": {"display": "Cerro", "token": "cerro-secret"},
                "friend": {"display": "Friend", "token": "friend-secret"}
            }
        }))
        .unwrap();
        let accounts = crate::accounts::Directory::load(&store.conn).unwrap();
        crate::app::fix_now_ms(Some(NOW_MS));
        Self {
            app: Arc::new(App {
                config,
                access: crate::access::Access::default(),
                accounts,
                week: Week::default(),
                store: Mutex::new(store),
                players: RwLock::new(HashMap::new()),
                standings: Default::default(),
            }),
            path,
            secrets: Vec::new(),
        }
    }

    async fn call(&self, method: &str, uri: &str, token: Option<&str>, body: Value) -> Value {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("accept-language", "en");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let body = if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        };
        let response = crate::router(self.app.clone())
            .layer(MockConnectInfo(
                "192.0.2.1:40000".parse::<SocketAddr>().unwrap(),
            ))
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        assert!(
            status.is_success(),
            "{method} {uri}: {status} {}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice(&bytes).unwrap()
    }

    fn reviews(&self, count: i64) -> Value {
        let day = Clock::default().day(NOW_MS);
        let first = NOW_MS - 20 * 60_000;
        let reviews: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "id": first + index * 1_000,
                    "cid": 1_000 + index,
                    "last_ivl": 1,
                    "ivl": 3,
                    "time_ms": 8_000,
                    "kind": 1
                })
            })
            .collect();
        json!({
            "reviews": reviews,
            "clock": {"offset_west_min": 0, "rollover_hour": 4},
            "decks": [
                {"id": "1", "name": "Spanish", "remaining": 12, "reviewed_today": count, "day": day},
                {"id": "2", "name": "Spanish::Verbs", "remaining": 0, "reviewed_today": 0, "day": day}
            ]
        })
    }

    fn celebrate(&self, user: &str, key: &str, title: &str, body: &str) -> i64 {
        let mut store = self.app.store.lock().unwrap();
        store.mark_seen(user, "examples:first-run").unwrap();
        store
            .queue_moments(
                user,
                &[Event {
                    key: key.into(),
                    title: title.into(),
                    body: body.into(),
                }],
                Clock::default().day(NOW_MS),
                NOW_MS,
                false,
                &json!({"streak": 1, "level": 2, "reviews_today": 5, "minutes_today": 1}),
            )
            .unwrap()[0]
    }

    fn notification(&self, user: &str, kind: &str) -> i64 {
        self.app
            .store
            .lock()
            .unwrap()
            .notifications(user, NOW_MS)
            .unwrap()
            .into_iter()
            .find(|notice| notice.kind == kind)
            .unwrap()
            .id
    }

    fn remember_links(&mut self) {
        let store = self.app.store.lock().unwrap();
        let mut stmt = store
            .conn
            .prepare("select link_token from wins where link_token is not null")
            .unwrap();
        let tokens: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        drop(stmt);
        drop(store);
        for token in tokens {
            self.keep_secret(&Value::from(token), "example-link");
        }
    }

    fn stable(&self, value: Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(key, value)| (key, self.stable(value)))
                    .collect(),
            ),
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|item| self.stable(item)).collect())
            }
            Value::String(mut text) => {
                for (secret, placeholder) in &self.secrets {
                    text = text.replace(secret.as_str(), placeholder);
                }
                Value::String(text)
            }
            other => other,
        }
    }

    fn keep_secret(&mut self, secret: &Value, kind: &str) {
        let count = self
            .secrets
            .iter()
            .filter(|(_, p)| p.starts_with(kind))
            .count();
        self.secrets.push((
            secret.as_str().unwrap().to_string(),
            format!("{kind}-{}", count + 1),
        ));
    }
}

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/api/examples")
}

#[tokio::test]
async fn examples_match_the_handlers() {
    let mut f = Fixture::new();
    let mut examples: Vec<(&str, Value)> = Vec::new();

    examples.push((
        "capabilities",
        f.call("GET", "/api/capabilities", None, Value::Null).await,
    ));

    let signed_up = f
        .call(
            "POST",
            "/api/accounts",
            None,
            json!({"user": "ana", "password": PASSWORD, "display": "Ana", "device": "Pixel 8"}),
        )
        .await;
    let signed_in = f
        .call(
            "POST",
            "/api/accounts/tokens",
            None,
            json!({"user": "ana", "password": PASSWORD, "device": "Pixel 9"}),
        )
        .await;
    let ana = signed_in["token"].as_str().unwrap().to_string();
    f.keep_secret(&signed_up["token"], "example-token");
    f.keep_secret(&signed_in["token"], "example-token");
    examples.push(("sign_up", signed_up));
    examples.push(("sign_in", signed_in));
    let devices = f
        .call("GET", "/api/devices/ana", Some(&ana), Value::Null)
        .await;
    for device in devices["devices"].as_array().unwrap() {
        f.keep_secret(&device["id"], "example-device");
    }
    examples.push(("devices", devices));

    let upload = f
        .call(
            "POST",
            "/api/reviews/hill",
            Some("hill-secret"),
            f.reviews(5),
        )
        .await;
    examples.push(("upload", upload));
    f.call(
        "POST",
        "/api/reviews/cerro",
        Some("cerro-secret"),
        f.reviews(3),
    )
    .await;

    f.call(
        "PUT",
        "/api/goal/hill",
        Some("hill-secret"),
        json!({"mode": "backlog", "value": 14}),
    )
    .await;

    let goal_key = format!("goal:{}", Clock::default().day(NOW_MS));
    let hill_win = f.celebrate(
        "hill",
        &goal_key,
        "Daily goal reached!",
        "5 cards today in 0 minutes.",
    );
    let cerro_win = f.celebrate("cerro", "streak:1:7", "7 day streak!", "A week in a row.");
    examples.push((
        "share_to_friends",
        f.call(
            "POST",
            "/api/share/hill",
            Some("hill-secret"),
            json!({"win": hill_win, "to": "friends"}),
        )
        .await,
    ));
    f.call(
        "POST",
        "/api/share/cerro",
        Some("cerro-secret"),
        json!({"win": cerro_win, "to": "friends"}),
    )
    .await;
    let cerro_share = f.notification("hill", "share");
    examples.push((
        "react",
        f.call(
            "POST",
            "/api/react/hill",
            Some("hill-secret"),
            json!({"notification": cerro_share}),
        )
        .await,
    ));
    let hill_share = f.notification("cerro", "share");
    f.call(
        "POST",
        "/api/react/cerro",
        Some("cerro-secret"),
        json!({"notification": hill_share}),
    )
    .await;
    f.remember_links();

    for (name, method, uri, body) in [
        ("profile", "GET", "/api/profile/hill", Value::Null),
        ("leaderboard", "GET", "/api/leaderboard", Value::Null),
        (
            "rank",
            "POST",
            "/api/rank/hill",
            json!({"previous": ["cerro", "hill"]}),
        ),
        (
            "notifications",
            "GET",
            "/api/notifications/hill",
            Value::Null,
        ),
        ("goal", "GET", "/api/goal/hill", Value::Null),
        ("sharing", "GET", "/api/sharing/hill", Value::Null),
        ("wins", "GET", "/api/wins/hill", Value::Null),
        ("decks", "GET", "/api/decks/hill", Value::Null),
        ("companion", "GET", "/api/companion/hill", Value::Null),
        (
            "streak_freezes",
            "GET",
            "/api/streak-freezes/hill",
            Value::Null,
        ),
    ] {
        examples.push((name, f.call(method, uri, Some("hill-secret"), body).await));
    }

    let update = std::env::var_os("UPDATE_API_EXAMPLES").is_some_and(|v| v == "1");
    let dir = examples_dir();
    let mut stale = Vec::new();
    for (name, value) in examples {
        let file = dir.join(format!("{name}.json"));
        let text = serde_json::to_string_pretty(&f.stable(value)).unwrap() + "\n";
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&file, text).unwrap();
        } else if std::fs::read_to_string(&file)
            .map(|saved| saved.replace("\r\n", "\n"))
            .ok()
            != Some(text)
        {
            stale.push(format!("docs/api/examples/{name}.json"));
        }
    }
    crate::app::fix_now_ms(None);
    drop(f.app);
    std::fs::remove_dir_all(&f.path).unwrap();
    assert!(
        stale.is_empty(),
        "{} no longer match the handlers. If the change is intended, regenerate them with \
         `UPDATE_API_EXAMPLES=1 cargo test api_examples` and commit the result.",
        stale.join(", ")
    );
}
