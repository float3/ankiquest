//! Keeps `docs/api/examples` equal to what the real handlers answer. Run with
//! `UPDATE_API_EXAMPLES=1` to rewrite the files instead of comparing.
//!
//! The clock is pinned to [`NOW_MS`]. Random values are replaced before comparing, numbered in
//! the order they appear: device tokens by `example-token-N`, win link tokens by
//! `example-link-N`, device ids by `example-device-N`, group invite codes by `example-invite-N`
//! and app sign-in tickets by `example-ticket-N`.

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
        let app = Arc::new(crate::test_support::app_from(store, config));
        // Players who have used the app, so the deck recipient picker offers them.
        crate::test_support::used_app(&app, &["cerro", "friend"]);
        Self {
            app,
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
            "leaderboard_day",
            "GET",
            "/api/leaderboard?period=day",
            Value::Null,
        ),
        ("week", "GET", "/api/week", Value::Null),
        // Another player's profile, as the leaderboard opens it: no `goal`.
        ("player_profile", "GET", "/api/profile/cerro", Value::Null),
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

    // Records and study history come last: Hill's earlier study days, uploaded here, change no
    // example above. Two and three days ago, so the history has more than today.
    let earlier: Vec<Value> = [(3, 4), (2, 6)]
        .iter()
        .flat_map(|&(days_ago, count)| {
            let first = NOW_MS - days_ago * 86_400_000 - 60 * 60_000;
            (0..count).map(move |index| {
                json!({
                    "id": first + index * 1_000,
                    "cid": 2_000 + days_ago * 100 + index,
                    "last_ivl": 1,
                    "ivl": 3,
                    "time_ms": 9_000,
                    "kind": 1
                })
            })
        })
        .collect();
    f.call(
        "POST",
        "/api/reviews/hill",
        Some("hill-secret"),
        json!({"reviews": earlier, "clock": {"offset_west_min": 0, "rollover_hour": 4}}),
    )
    .await;
    examples.push((
        "records",
        f.call("GET", "/api/records", Some("hill-secret"), Value::Null)
            .await,
    ));
    examples.push((
        "study",
        f.call("GET", "/api/study/hill", Some("hill-secret"), Value::Null)
            .await,
    ));
    // Account settings come after the examples above, since they change how Ana signs in. Ana linked GitHub.
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

    let browser = f
        .call(
            "POST",
            "/api/sign-in/ana",
            Some(&ana),
            json!({"action": "browser", "provider": "github", "purpose": "reauth"}),
        )
        .await;
    let ticket = browser["url"].as_str().unwrap().split("ticket=").nth(1);
    f.keep_secret(&Value::from(ticket.unwrap()), "example-ticket");
    examples.push(("sign_in_browser", browser));

    // Community reminders, the competition calendar and a conquered card, which the app's own
    // screens read. Hill keeps the defaults until saving his choice.
    examples.push((
        "reminders",
        f.call(
            "GET",
            "/api/community/reminders/hill",
            Some("hill-secret"),
            Value::Null,
        )
        .await,
    ));
    examples.push((
        "reminders_save",
        f.call(
            "POST",
            "/api/community/reminders/hill",
            Some("hill-secret"),
            json!({
                "gentle_daily": true,
                "urgent_streak": true,
                "freeze_used": false,
                "freeze_refill": false,
                "milestone": true,
                "weekly_closing": false,
                "weekly_recap": true,
                "reminder_hour": 19,
                "quiet_start": 22,
                "quiet_end": 8,
                "daily_limit": 2
            }),
        )
        .await,
    ));
    examples.push((
        "community",
        f.call(
            "GET",
            "/api/community?year=2026&month=10",
            None,
            Value::Null,
        )
        .await,
    ));
    // One card Hill forgot four times over the past weeks before it earned a 25-day interval
    // yesterday: a review failed into relearning, then the relearning step, four times. He says
    // what the card was.
    let answers: Vec<(i64, i64, i64)> = std::iter::once((0, 0, 1))
        .chain((0..4).flat_map(|_| [(1, 3, -600), (2, -600, 1)]))
        .chain([(1, 1, 8), (1, 8, 25)])
        .collect();
    let first = NOW_MS - answers.len() as i64 * 86_400_000;
    let card: Vec<Value> = answers
        .iter()
        .enumerate()
        .map(|(index, &(kind, last_ivl, ivl))| {
            json!({
                "id": first + index as i64 * 86_400_000,
                "cid": 9_000,
                "last_ivl": last_ivl,
                "ivl": ivl,
                "time_ms": 7_000,
                "kind": kind
            })
        })
        .collect();
    let conquered = first + (answers.len() as i64 - 1) * 86_400_000;
    f.call(
        "POST",
        "/api/reviews/hill",
        Some("hill-secret"),
        json!({"reviews": card, "clock": {"offset_west_min": 0, "rollover_hour": 4}}),
    )
    .await;
    f.app
        .store
        .lock()
        .unwrap()
        .conn
        .execute(
            "insert into conquest_labels (user, at, label) values ('hill', ?1, 'el murciélago')",
            [conquered],
        )
        .unwrap();
    // The worker keeps the conquest as a win, whose card the owner can share.
    crate::wins::create(
        &f.app.store.lock().unwrap().conn,
        "hill",
        &format!("conquered:{conquered}"),
        Clock::default().day(NOW_MS),
        NOW_MS,
        "Card conquered",
        "A card you forgot 4 times now stays with you for weeks.",
        &json!({}),
    )
    .unwrap();
    examples.push((
        "conquest",
        f.call(
            "GET",
            &format!("/api/conquests/hill/{conquered}"),
            Some("hill-secret"),
            Value::Null,
        )
        .await,
    ));
    examples.push((
        "conquest_tell",
        f.call(
            "POST",
            &format!("/api/conquests/hill/{conquered}/tell"),
            Some("hill-secret"),
            json!({}),
        )
        .await,
    ));
    // Within a day, and until a friend cheers it, telling friends can be taken back.
    examples.push((
        "conquest_untell",
        f.call(
            "DELETE",
            &format!("/api/conquests/hill/{conquered}/tell"),
            Some("hill-secret"),
            Value::Null,
        )
        .await,
    ));

    // The global leaderboard: profiles are public and listed by default, but Ana made hers
    // private. With no friends yet she sees public standings and her own full row, which nobody
    // else sees until she turns the global leaderboard on. Then Hill sees her there as a
    // stranger: name, level, XP and streak only.
    f.call("POST", "/api/reviews/ana", Some(&ana), f.reviews(2))
        .await;
    f.call(
        "POST",
        "/api/social/ana",
        Some(&ana),
        json!({"action": "visibility", "public": false}),
    )
    .await;
    examples.push((
        "leaderboard_global_hidden",
        f.call(
            "GET",
            "/api/leaderboard?scope=global",
            Some(&ana),
            Value::Null,
        )
        .await,
    ));
    examples.push((
        "global_leaderboard_on",
        f.call(
            "POST",
            "/api/decks/ana",
            Some(&ana),
            json!({"decks": [], "global_leaderboard": true}),
        )
        .await,
    ));
    examples.push((
        "leaderboard_global",
        f.call(
            "GET",
            "/api/leaderboard?scope=global&period=day",
            Some("hill-secret"),
            Value::Null,
        )
        .await,
    ));

    // Friends and groups come after everything above, whose circles they would otherwise change.
    // Hill is friends with Friend, waits for Cerro, has a request from Ana, owns a group Ana joined
    // and belongs to one of Friend's.
    let befriend = |user: &str| json!({"action": "befriend", "user": user});
    for (user, token, other) in [
        ("hill", "hill-secret", "friend"),
        ("friend", "friend-secret", "hill"),
        ("hill", "hill-secret", "cerro"),
        ("ana", ana.as_str(), "hill"),
    ] {
        f.call(
            "POST",
            &format!("/api/social/{user}"),
            Some(token),
            befriend(other),
        )
        .await;
    }
    let created = f
        .call(
            "POST",
            "/api/social/hill",
            Some("hill-secret"),
            json!({"action": "create_group", "name": "Spanish club"}),
        )
        .await;
    let invite = created["groups"][0]["invite"].clone();
    f.keep_secret(&invite, "example-invite");
    let invite = invite.as_str().unwrap().to_string();
    examples.push(("group_create", created));
    f.call(
        "POST",
        "/api/social/ana",
        Some(&ana),
        json!({"action": "join", "invite": invite}),
    )
    .await;
    let owls = f
        .call(
            "POST",
            "/api/social/friend",
            Some("friend-secret"),
            json!({"action": "create_group", "name": "Night owls"}),
        )
        .await;
    f.call(
        "POST",
        "/api/social/hill",
        Some("hill-secret"),
        json!({"action": "join", "invite": owls["groups"][0]["invite"]}),
    )
    .await;
    examples.push((
        "social",
        f.call("GET", "/api/social/hill", Some("hill-secret"), Value::Null)
            .await,
    ));
    examples.push((
        "invite",
        f.call("GET", &format!("/api/invites/{invite}"), None, Value::Null)
            .await,
    ));
    examples.push((
        "social_change",
        f.call(
            "POST",
            "/api/social/hill",
            Some("hill-secret"),
            befriend("ana"),
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
