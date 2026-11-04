//! Keeps `docs/api/examples` equal to what the real handlers answer. Run with
//! `UPDATE_API_EXAMPLES=1` to rewrite the files instead of comparing.
//!
//! The clock is pinned to [`NOW_MS`]. Random values are replaced before comparing, numbered in
//! the order they appear: device tokens by `example-token-N`, win link tokens by
//! `example-link-N` and device ids by `example-device-N`.

use crate::app::App;
use crate::game::{Clock, Event};
use axum::body::{Body, to_bytes};
use axum::http::Request;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

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
        let (store, path) = crate::test_support::temporary_store();
        let config = serde_json::from_value(json!({
            "registration": true,
            "public_url": "https://anki.example.test",
            "oauth": {
                "google": {"client_id": "example-google", "client_secret": "example"},
                "github": {"client_id": "example-github", "client_secret": "example"}
            },
            "users": {
                "hill": {"display": "Hill", "token": "hill-secret"},
                "cerro": {"display": "Cerro", "token": "cerro-secret"},
                "friend": {"display": "Friend", "token": "friend-secret"}
            }
        }))
        .unwrap();
        crate::app::fix_now_ms(Some(NOW_MS));
        Self {
            app: Arc::new(crate::test_support::app_from(store, config)),
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
        let response = crate::test_support::route(&self.app, request.body(body).unwrap()).await;
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
                true,
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
            // Number links in the order they were made; the unique index would
            // otherwise return them sorted by their random tokens.
            .prepare("select link_token from wins where link_token is not null order by id")
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
    examples.push((
        "share_to_link",
        f.call(
            "POST",
            "/api/share/hill",
            Some("hill-secret"),
            json!({"win": hill_win, "to": "link"}),
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
    // A celebration that speaks to the player, so the inbox shows its first-person share words.
    f.celebrate(
        "hill",
        "level:3",
        "Level 3",
        "You reached level 3 with 400 XP.",
    );
    f.remember_links();

    for (name, method, uri, body) in [
        ("profile", "GET", "/api/profile/hill", Value::Null),
        ("leaderboard", "GET", "/api/leaderboard", Value::Null),
        (
            "rank",
            "POST",
            "/api/rank/hill",
            json!({"previous": ["cerro", "hill"], "week": 1_792_987_200_000_i64}),
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
        // Last, so cerro is not muted for the examples above.
        (
            "mute",
            "POST",
            "/api/social/hill",
            json!({"action": "mute", "user": "cerro"}),
        ),
    ] {
        examples.push((name, f.call(method, uri, Some("hill-secret"), body).await));
    }

    // Friend goals come last, so their invitations change no example above. They start an hour
    // ago, so the reviews uploaded above count: one goal in progress, one reached, one ended early
    // and one invitation waiting for Hill.
    crate::app::fix_now_ms(Some(NOW_MS - 60 * 60_000));
    let goal = |title: &str, kind: &str, cooperative: bool, target: u64, to: &[&str], key: &str| {
        json!({
            "title": title,
            "kind": kind,
            "cooperative": cooperative,
            "target": target,
            "duration_days": 7,
            "recipients": to,
            "request_id": key
        })
    };
    let ids = |list: &Value| -> Vec<i64> {
        list["challenges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|challenge| challenge["id"].as_i64().unwrap())
            .collect()
    };
    let created = f
        .call(
            "POST",
            "/api/community/challenges/hill",
            Some("hill-secret"),
            goal(
                "Three study days together",
                "study_days",
                false,
                3,
                &["cerro", "friend"],
                "example-request-1",
            ),
        )
        .await;
    let mine = ids(&created)[0];
    examples.push(("challenge_create", created));
    let together = f
        .call(
            "POST",
            "/api/community/challenges/cerro",
            Some("cerro-secret"),
            goal(
                "Spanish sprint",
                "reviews",
                true,
                5,
                &["hill"],
                "example-request-2",
            ),
        )
        .await;
    let sprint = ids(&together).into_iter().max().unwrap();
    f.call(
        "POST",
        &format!("/api/community/challenges/cerro/{mine}"),
        Some("cerro-secret"),
        json!({"action": "accept"}),
    )
    .await;
    f.call(
        "POST",
        &format!("/api/community/challenges/hill/{sprint}"),
        Some("hill-secret"),
        json!({"action": "accept"}),
    )
    .await;
    let early = f
        .call(
            "POST",
            "/api/community/challenges/hill",
            Some("hill-secret"),
            goal(
                "Weekend reviews",
                "reviews",
                false,
                40,
                &["cerro"],
                "example-request-3",
            ),
        )
        .await;
    let early = ids(&early).into_iter().max().unwrap();
    f.call(
        "POST",
        &format!("/api/community/challenges/hill/{early}"),
        Some("hill-secret"),
        json!({"action": "cancel"}),
    )
    .await;
    let invited = f
        .call(
            "POST",
            "/api/community/challenges/friend",
            Some("friend-secret"),
            goal(
                "Fifty reviews each",
                "reviews",
                false,
                50,
                &["hill"],
                "example-request-4",
            ),
        )
        .await;
    let invited = ids(&invited).into_iter().max().unwrap();
    crate::app::fix_now_ms(Some(NOW_MS));
    examples.push((
        "challenges",
        f.call(
            "GET",
            "/api/community/challenges/hill",
            Some("hill-secret"),
            Value::Null,
        )
        .await,
    ));
    examples.push((
        "challenge_action",
        f.call(
            "POST",
            &format!("/api/community/challenges/hill/{invited}"),
            Some("hill-secret"),
            json!({"action": "accept"}),
        )
        .await,
    ));

    // After the inbox example, whose row ids would otherwise shift.
    examples.push((
        "deck_share",
        f.call(
            "POST",
            "/api/decks/hill/share",
            Some("hill-secret"),
            json!({"id": "1", "name": "Spanish", "recipients": ["cerro"]}),
        )
        .await,
    ));

    // Account settings come last, since they change how Ana signs in. Ana linked GitHub.
    f.app
        .store
        .lock()
        .unwrap()
        .conn
        .execute(
            "insert into sign_in_links (provider, subject, user, created_at) values ('github', 'example-subject', 'ana', ?1)",
            [NOW_MS],
        )
        .unwrap();
    examples.push((
        "sign_in_methods",
        f.call("GET", "/api/sign-in/ana", Some(&ana), Value::Null)
            .await,
    ));
    examples.push((
        "account",
        f.call("GET", "/api/accounts/ana", Some(&ana), Value::Null)
            .await,
    ));
    examples.push((
        "password_change",
        f.call(
            "POST",
            "/api/sign-in/ana",
            Some(&ana),
            json!({"action": "password", "current": PASSWORD, "password": "an even longer pw", "sign_out_others": false}),
        )
        .await,
    ));
    examples.push((
        "devices_sign_out",
        f.call(
            "POST",
            "/api/devices/ana",
            Some(&ana),
            json!({"action": "sign_out_others"}),
        )
        .await,
    ));

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
