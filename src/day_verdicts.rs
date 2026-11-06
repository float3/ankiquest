//! Golden cases for the one day verdict a client may work out itself: today's, with reviews it
//! has not uploaded yet added to what the server last said.
//!
//! The server judges every day (`streak_state`, `today`, the heatmap's `state`), and clients show
//! that. Right after answering offline a client still wants to show today's progress, so it adds the
//! review time of its unsent reviews to `today.time_ms` and compares the sum with
//! `today.streak_minutes`, exactly as the server will once they arrive. These cases are worked out by
//! the server's own [`compute_with_rules`] and written to `tests/fixtures/day_verdicts.json`, which the
//! clients test their copy of the rule against. Regenerate with
//! `UPDATE_FIXTURES=1 cargo test day_verdicts`.

use crate::game::{
    Clock, ClockHistory, DAY_MS, FreezePolicy, Review, StreakChange, StreakRule, Week,
    compute_with_rules, date_string,
};
use serde_json::{Value, json};
use std::path::PathBuf;

const NOON: i64 = 12 * 3_600_000;
const MINUTE: i64 = 60_000;
/// Today, as a day number: 2026-11-06 in UTC.
const TODAY: i64 = 20_398;

struct Case {
    name: &'static str,
    offset_west_min: i64,
    rollover_hour: i64,
    streak_minutes: u32,
    /// Days an admin protected, relative to today (-1 is yesterday).
    granted: &'static [i64],
    /// Reviews the server has, as (ms after today's noon UTC, time taken).
    uploaded: &'static [(i64, i64)],
    /// Reviews the client holds that are not uploaded, the same way. Ids at or before the
    /// last uploaded review are ones the client sends again; the server already has them.
    pending: &'static [(i64, i64)],
    /// Ms after today's noon UTC.
    now: i64,
}

const CASES: &[Case] = &[
    Case {
        name: "unsent reviews reach the minimum",
        offset_west_min: 0,
        rollover_hour: 4,
        streak_minutes: 10,
        granted: &[],
        uploaded: &[(-DAY_MS, 600_000), (0, 360_000)],
        pending: &[(MINUTE * 10, 300_000)],
        now: MINUTE * 20,
    },
    Case {
        name: "unsent reviews stay short of the minimum",
        offset_west_min: 0,
        rollover_hour: 4,
        streak_minutes: 10,
        granted: &[],
        uploaded: &[(-DAY_MS, 600_000), (0, 360_000)],
        pending: &[(MINUTE * 10, 180_000)],
        now: MINUTE * 20,
    },
    Case {
        name: "one millisecond short, then exactly the minimum",
        offset_west_min: 0,
        rollover_hour: 4,
        streak_minutes: 10,
        granted: &[],
        uploaded: &[(0, 599_999)],
        pending: &[(MINUTE * 15, 1)],
        now: MINUTE * 20,
    },
    Case {
        name: "one millisecond short with nothing unsent",
        offset_west_min: 0,
        rollover_hour: 4,
        streak_minutes: 10,
        granted: &[],
        uploaded: &[(-DAY_MS, 600_000), (0, 599_999)],
        pending: &[],
        now: MINUTE * 20,
    },
    Case {
        name: "nothing uploaded today, unsent reviews meet the minimum",
        offset_west_min: 0,
        rollover_hour: 4,
        streak_minutes: 10,
        granted: &[],
        uploaded: &[(-DAY_MS, 600_000)],
        pending: &[(0, 300_000), (MINUTE, 300_000)],
        now: MINUTE * 20,
    },
    Case {
        name: "already studied stays studied without a longer streak",
        offset_west_min: 0,
        rollover_hour: 4,
        streak_minutes: 10,
        granted: &[],
        uploaded: &[(-DAY_MS, 600_000), (0, 660_000)],
        pending: &[(MINUTE * 15, 30_000)],
        now: MINUTE * 20,
    },
    Case {
        name: "a frozen yesterday stays protected while today is short",
        offset_west_min: 0,
        rollover_hour: 4,
        streak_minutes: 10,
        granted: &[-1],
        uploaded: &[(-2 * DAY_MS, 600_000), (0, 120_000)],
        pending: &[(MINUTE * 5, 120_000)],
        now: MINUTE * 20,
    },
    Case {
        name: "a frozen yesterday and an unsent minimum today",
        offset_west_min: 0,
        rollover_hour: 4,
        streak_minutes: 10,
        granted: &[-1],
        uploaded: &[(-2 * DAY_MS, 600_000), (0, 120_000)],
        pending: &[(MINUTE * 5, 480_000)],
        now: MINUTE * 20,
    },
    Case {
        name: "reviews sent again are not counted twice",
        offset_west_min: 0,
        rollover_hour: 4,
        streak_minutes: 10,
        granted: &[],
        uploaded: &[(-DAY_MS, 600_000), (0, 360_000)],
        pending: &[(0, 360_000), (MINUTE * 10, 180_000)],
        now: MINUTE * 20,
    },
    Case {
        name: "an unsent review before the rollover belongs to yesterday",
        offset_west_min: -120,
        rollover_hour: 4,
        streak_minutes: 10,
        granted: &[],
        // 01:30 UTC is 03:30 in UTC+2, before the 04:00 rollover.
        uploaded: &[(-2 * DAY_MS, 600_000), (-DAY_MS - 3 * 3_600_000, 600_000)],
        pending: &[(-NOON + 90 * MINUTE, 600_000), (0, 120_000)],
        now: MINUTE * 20,
    },
    Case {
        name: "the day turns over at the player's rollover, not midnight",
        offset_west_min: 300,
        rollover_hour: 2,
        streak_minutes: 5,
        granted: &[],
        // 07:30 UTC is 02:30 in UTC-5, just after the 02:00 rollover.
        uploaded: &[
            (-DAY_MS, 300_000),
            (-NOON + 7 * 3_600_000 + 30 * MINUTE, 120_000),
        ],
        pending: &[(-NOON + 7 * 3_600_000 + 40 * MINUTE, 180_000)],
        now: 0,
    },
    Case {
        name: "with no minimum any review counts",
        offset_west_min: 0,
        rollover_hour: 4,
        streak_minutes: 0,
        granted: &[],
        uploaded: &[(-DAY_MS, 5_000)],
        pending: &[(0, 4_000)],
        now: MINUTE * 20,
    },
    Case {
        name: "a new streak starts at one",
        offset_west_min: 0,
        rollover_hour: 4,
        streak_minutes: 10,
        granted: &[],
        uploaded: &[(-3 * DAY_MS, 600_000)],
        pending: &[(0, 600_000)],
        now: MINUTE * 20,
    },
    Case {
        name: "a profile from a day that has ended says nothing about today",
        offset_west_min: 0,
        rollover_hour: 4,
        streak_minutes: 10,
        granted: &[],
        uploaded: &[(-DAY_MS, 600_000)],
        pending: &[(0, 600_000)],
        // The server's answer is from yesterday evening; now is today.
        now: MINUTE * 20,
    },
];

fn review(at: i64, time_ms: i64) -> Review {
    let id = TODAY * DAY_MS + NOON + at;
    Review {
        id,
        cid: id,
        last_ivl: 0,
        ivl: Some(1),
        time_ms,
        kind: 1,
    }
}

/// What a client reads of the server's profile to judge today.
fn server_view(profile: &Value) -> Value {
    let today = &profile["today"];
    json!({
        "streak": profile["streak"],
        "streak_state": profile["streak_state"],
        "day_ends_at": profile["day_ends_at"],
        "last_review_id": profile["last_review_id"],
        "today": {
            "starts_at": today["starts_at"],
            "reviews": today["reviews"],
            "time_ms": today["time_ms"],
            "minutes": today["minutes"],
            "streak_minutes": today["streak_minutes"],
        },
    })
}

/// The rule a client follows, as `docs/mobile-api.md` states it. Null when the server's answer is
/// from a day that has ended. A field the client cannot know is null: the streak, and while today
/// is short its state, once an unsent review belongs to an earlier day.
fn client(server: &Value, pending: &[Value], now: i64) -> Value {
    let today = &server["today"];
    let starts = today["starts_at"].as_i64().unwrap();
    let ends = server["day_ends_at"].as_i64().unwrap();
    if now >= ends {
        return Value::Null;
    }
    let last = server["last_review_id"].as_i64().unwrap();
    let unsent: Vec<&Value> = pending
        .iter()
        .filter(|r| r["id"].as_i64().unwrap() > last)
        .collect();
    let counted: Vec<&&Value> = unsent
        .iter()
        .filter(|r| (starts..ends).contains(&r["id"].as_i64().unwrap()))
        .collect();
    let earlier = unsent.iter().any(|r| r["id"].as_i64().unwrap() < starts);
    let reviews = today["reviews"].as_u64().unwrap() + counted.len() as u64;
    let time_ms = today["time_ms"].as_i64().unwrap()
        + counted
            .iter()
            .map(|r| r["time_ms"].as_i64().unwrap())
            .sum::<i64>();
    let needed = today["streak_minutes"].as_i64().unwrap();
    let was = server["streak_state"].as_str().unwrap();
    let met = was == "studied" || (reviews > 0 && time_ms >= needed * MINUTE);
    let streak = server["streak"].as_u64().unwrap() + u64::from(met && was != "studied");
    json!({
        "met": met,
        "streak_state": if met { json!("studied") } else if earlier { Value::Null } else { json!(was) },
        "streak": if earlier { Value::Null } else { json!(streak) },
        "reviews": reviews,
        "time_ms": time_ms,
        "minutes": time_ms.max(0) / MINUTE,
        "streak_minutes": needed,
        "unsent": counted.len(),
    })
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/day_verdicts.json")
}

#[test]
fn day_verdicts_match_the_server() {
    let mut cases = Vec::new();
    for case in CASES {
        let clock = ClockHistory::single(Clock {
            offset_west_min: case.offset_west_min,
            rollover_hour: case.rollover_hour,
        });
        let rule = StreakRule {
            changes: vec![StreakChange {
                at: 0,
                minutes: case.streak_minutes,
            }],
            players: Default::default(),
        };
        let now = TODAY * DAY_MS + NOON + case.now;
        let stale = case.name.contains("has ended");
        let policy = FreezePolicy {
            granted: case
                .granted
                .iter()
                .map(|d| date_string(clock.day(now) + d))
                .collect(),
            ..FreezePolicy::default()
        };
        let profile = |reviews: &[Review], at: i64| {
            serde_json::to_value(compute_with_rules(
                "player",
                "Player",
                reviews,
                &clock,
                &Week::default(),
                at,
                &policy,
                &rule,
            ))
            .unwrap()
        };
        let uploaded: Vec<Review> = case.uploaded.iter().map(|(a, t)| review(*a, *t)).collect();
        let pending: Vec<Review> = case.pending.iter().map(|(a, t)| review(*a, *t)).collect();
        let answered = if stale { now - NOON } else { now };
        let server = server_view(&profile(&uploaded, answered));
        let pending_json: Vec<Value> = pending
            .iter()
            .map(|r| json!({"id": r.id, "time_ms": r.time_ms}))
            .collect();
        let verdict = client(&server, &pending_json, now);

        // The client's rule must say what the server says once everything is uploaded.
        let mut all = uploaded.clone();
        all.extend(pending.iter().filter(|r| !uploaded.contains(r)).copied());
        all.sort_by_key(|r| r.id);
        let full = profile(&all, now);
        if stale {
            assert!(verdict.is_null(), "{}", case.name);
        } else {
            let today = &full["today"];
            assert_eq!(
                verdict["met"],
                json!(full["streak_state"] == "studied"),
                "{}",
                case.name
            );
            for field in ["reviews", "time_ms", "minutes", "streak_minutes"] {
                assert_eq!(verdict[field], today[field], "{}: {field}", case.name);
            }
            for field in ["streak", "streak_state"] {
                if !verdict[field].is_null() {
                    assert_eq!(verdict[field], full[field], "{}: {field}", case.name);
                }
            }
        }
        cases.push(json!({
            "name": case.name,
            "now_ms": now,
            "server": server,
            "pending": pending_json,
            "expected": verdict,
        }));
    }
    let text = serde_json::to_string_pretty(&json!({
        "_about": "Today's streak verdict with reviews not uploaded yet, worked out by the server. \
                   Written by src/day_verdicts.rs (UPDATE_FIXTURES=1 cargo test day_verdicts); \
                   the Android app keeps a copy in AnkiDroid/src/test/resources/ankiquest/day_verdicts.json \
                   and CI there fails when it drifts from this file.",
        "cases": cases,
    }))
    .unwrap()
        + "\n";
    let path = fixture_path();
    if std::env::var_os("UPDATE_FIXTURES").is_some_and(|v| v == "1") {
        std::fs::write(&path, &text).unwrap();
    } else {
        let saved = std::fs::read_to_string(&path)
            .map(|s| s.replace("\r\n", "\n"))
            .unwrap_or_default();
        assert!(
            saved == text,
            "tests/fixtures/day_verdicts.json no longer matches the server. If the change is \
             intended, regenerate it with `UPDATE_FIXTURES=1 cargo test day_verdicts` and commit it."
        );
    }
}
