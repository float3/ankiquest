//! Opt-in reminders, using the same durable inbox and retry queue as deck notices.
use crate::decks::{Notification, Outgoing};
use crate::game::{Clock, MAX_FREEZES, Profile, Week};
use crate::store::{Error, Store};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub use crate::competition::WeeklyRecap as Recap;

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub gentle_daily: bool,
    pub urgent_streak: bool,
    pub freeze_used: bool,
    pub freeze_refill: bool,
    pub milestone: bool,
    pub weekly_closing: bool,
    pub weekly_recap: bool,
    pub reminder_hour: u8,
    pub quiet_start: u8,
    pub quiet_end: u8,
    pub daily_limit: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            gentle_daily: false,
            urgent_streak: false,
            freeze_used: false,
            freeze_refill: false,
            milestone: false,
            weekly_closing: false,
            weekly_recap: false,
            reminder_hour: 20,
            quiet_start: 22,
            quiet_end: 9,
            daily_limit: 2,
        }
    }
}

impl Settings {
    pub fn valid(&self) -> bool {
        self.reminder_hour < 24
            && self.quiet_start < 24
            && self.quiet_end < 24
            && (1..=5).contains(&self.daily_limit)
    }

    /// Matching endpoints explicitly disable quiet hours.
    pub fn quiet(&self, hour: i64) -> bool {
        let (start, end) = (i64::from(self.quiet_start), i64::from(self.quiet_end));
        if start < end {
            (start..end).contains(&hour)
        } else if start > end {
            hour >= start || hour < end
        } else {
            false
        }
    }

    fn enabled(&self, kind: &str) -> bool {
        match kind {
            "reminder_daily" => self.gentle_daily,
            "reminder_urgent" => self.urgent_streak,
            "reminder_freeze_used" => self.freeze_used,
            "reminder_freeze_refill" => self.freeze_refill,
            "reminder_milestone" => self.milestone,
            "reminder_weekly_closing" => self.weekly_closing,
            "reminder_weekly_recap" => self.weekly_recap,
            _ => false,
        }
    }
}

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists reminder_settings (
             user text primary key,
             settings text not null
         ) without rowid;
         create table if not exists reminder_baselines (
             user text not null,
             kind text not null,
             since integer not null,
             primary key (user, kind)
         ) without rowid;",
    )?;
    // An old global warning may be waiting on a failed push when the upgraded
    // server starts. It must not bypass the new opt-ins or quiet hours.
    conn.execute(
        "update notifications set pushed = 1, push_only = 1 where kind = 'risk' and pushed = 0",
        [],
    )?;
    Ok(())
}

pub fn settings(store: &Store, user: &str) -> Result<Settings, Error> {
    let saved: Option<String> = store
        .conn
        .query_row(
            "select settings from reminder_settings where user = ?1",
            [user],
            |row| row.get(0),
        )
        .optional()?;
    saved.map_or_else(
        || Ok(Settings::default()),
        |saved| Ok(serde_json::from_str(&saved)?),
    )
}

/// Full replacement, after authentication by the API handler. No historical
/// freeze or recap is announced when its switch is enabled for the first time.
pub fn save_settings(store: &mut Store, user: &str, value: &Settings) -> Result<(), Error> {
    save_settings_at(store, user, value, crate::now_ms())
}

fn save_settings_at(
    store: &mut Store,
    user: &str,
    value: &Settings,
    now_ms: i64,
) -> Result<(), Error> {
    if !value.valid() {
        return Err("hours must be 0–23 and daily limit must be 1–5".into());
    }
    let previous = settings(store, user)?;
    if previous == *value {
        store.conn.execute(
            "update notifications set pushed = 1, push_only = 1 where recipient = ?1 and kind = 'risk'", [user],
        )?;
        return Ok(());
    }
    let tx = store.conn.transaction()?;
    tx.execute(
        "insert into reminder_settings (user, settings) values (?1, ?2)
         on conflict(user) do update set settings = excluded.settings",
        params![user, serde_json::to_string(value)?],
    )?;
    for (kind, before, after) in [
        ("freeze_used", previous.freeze_used, value.freeze_used),
        ("weekly_recap", previous.weekly_recap, value.weekly_recap),
    ] {
        if before != after {
            if after {
                // Record the user's opt-in time, not the next worker tick:
                // a rollover or server restart between them must not swallow
                // a freeze or recap that happened after the switch was saved.
                tx.execute(
                    "insert into reminder_baselines (user,kind,since) values (?1,?2,?3)
                     on conflict(user,kind) do update set since=excluded.since",
                    params![user, kind, now_ms],
                )?;
            } else {
                tx.execute(
                    "delete from reminder_baselines where user = ?1 and kind = ?2",
                    params![user, kind],
                )?;
            }
        }
    }
    // Retain still-enabled notices and their retry state when unrelated settings
    // change. Delivery rechecks their timing and relevance. Canceled rows remain
    // budget receipts, so toggling preferences cannot reset the daily limit.
    for kind in [
        "risk",
        "reminder_daily",
        "reminder_urgent",
        "reminder_freeze_used",
        "reminder_freeze_refill",
        "reminder_milestone",
        "reminder_weekly_closing",
        "reminder_weekly_recap",
    ] {
        if !value.enabled(kind) {
            tx.execute(
                "update notifications set pushed = 1, push_only = 1
                 where recipient = ?1 and kind = ?2",
                params![user, kind],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn baseline(store: &Store, user: &str, kind: &str, enabled: bool, now: i64) -> Result<i64, Error> {
    if !enabled {
        store.conn.execute(
            "delete from reminder_baselines where user = ?1 and kind = ?2",
            params![user, kind],
        )?;
        return Ok(now);
    }
    store.conn.execute(
        "insert or ignore into reminder_baselines (user, kind, since) values (?1, ?2, ?3)",
        params![user, kind, now],
    )?;
    Ok(store.conn.query_row(
        "select since from reminder_baselines where user = ?1 and kind = ?2",
        params![user, kind],
        |row| row.get(0),
    )?)
}

fn local_midnight(clock: &Clock, now: i64) -> i64 {
    let offset = clock.offset_west_min * 60_000;
    (now - offset).div_euclid(DAY) * DAY + offset
}

fn deadline(clock: &Clock, now: i64) -> i64 {
    (clock.day(now) + 1) * DAY + clock.rollover_hour * HOUR + clock.offset_west_min * 60_000
}

/// Place the preferred wall-clock hour inside the current Anki day. Hours
/// before rollover belong to its final morning, rather than its opening day.
fn reminder_due(prefs: &Settings, clock: &Clock, now: i64) -> bool {
    let day_start = deadline(clock, now) - DAY;
    let scheduled =
        day_start + (i64::from(prefs.reminder_hour) - clock.rollover_hour).rem_euclid(24) * HOUR;
    now >= scheduled
}

/// Warn in the last two hours, or the final waking hour when quiet time covers
/// the cutoff. A 04:00 Anki cutoff and 22:00–09:00 quiet time warns at 21:00.
fn urgent_window(prefs: &Settings, clock: &Clock, now: i64) -> bool {
    deadline_window(prefs, clock, now, deadline(clock, now))
}

fn deadline_window(prefs: &Settings, clock: &Clock, now: i64, cutoff: i64) -> bool {
    if prefs.quiet(clock.hour(now)) {
        return false;
    }
    if cutoff - now <= 2 * HOUR {
        return true;
    }
    if prefs.quiet_start == prefs.quiet_end {
        return false;
    }
    let mut quiet_at = local_midnight(clock, now) + i64::from(prefs.quiet_start) * HOUR;
    if quiet_at <= now {
        quiet_at += DAY;
    }
    let quiet_length =
        (i64::from(prefs.quiet_end) - i64::from(prefs.quiet_start)).rem_euclid(24) * HOUR;
    quiet_at - now <= HOUR && quiet_at < cutoff && quiet_at + quiet_length >= cutoff
}

fn unprotected(profile: &Profile) -> bool {
    profile.at_risk && profile.today.reviews == 0 && profile.streak > 0 && profile.freezes == 0
}

fn refill_due(profile: &Profile) -> bool {
    profile.freezes_enabled
        && profile.stored_freezes < MAX_FREEZES
        && !profile.freeze_earned_today
        && profile.quests.iter().any(|quest| !quest.done)
}

fn next_milestone(profile: &Profile) -> Option<u64> {
    let next = profile.streak + 1;
    (profile.today.reviews == 0
        && ([3, 7, 14, 30, 50, 100, 200, 365, 500, 730, 1000].contains(&next)
            || (next > 1000 && next.is_multiple_of(100))))
    .then_some(next)
}

fn seen(store: &Store, user: &str, key: &str) -> Result<bool, Error> {
    Ok(store
        .conn
        .query_row(
            "select 1 from seen where user = ?1 and key = ?2",
            params![user, key],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn used_budget(store: &Store, user: &str, clock: &Clock, now: i64) -> Result<u8, Error> {
    let start = local_midnight(clock, now);
    let count: i64 = store.conn.query_row(
        "select count(*) from notifications where recipient = ?1
         and kind glob 'reminder_*' and created_at >= ?2 and created_at < ?3",
        params![user, start, start + DAY],
        |row| row.get(0),
    )?;
    Ok(count.min(255) as u8)
}

struct Candidate {
    kind: &'static str,
    key: String,
    title: &'static str,
    body: String,
    day: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryGuard {
    Deliver,
    Defer,
    Cancel,
}

/// Check immediately before a push retry. `created_at` from the public inbox is
/// in seconds, so read the stored millisecond timestamp instead of trusting it.
pub fn delivery_guard(
    store: &Store,
    user: &str,
    notice: &Notification,
    profile: &Profile,
    clock: &Clock,
    week: &Week,
    now: i64,
) -> Result<DeliveryGuard, Error> {
    let prefs = settings(store, user)?;
    if !prefs.enabled(&notice.kind) {
        return Ok(DeliveryGuard::Cancel);
    }
    let created: Option<i64> = store.conn.query_row(
        "select created_at from notifications where id = ?1 and recipient = ?2 and push_only = 0",
        params![notice.id, user], |row| row.get(0),
    ).optional()?;
    let Some(created) = created else {
        return Ok(DeliveryGuard::Cancel);
    };
    // Old retries must not wake somebody on a different day or consume today's
    // entire push allowance alongside newly generated notices.
    if local_midnight(clock, created) != local_midnight(clock, now) {
        return Ok(DeliveryGuard::Cancel);
    }
    let valid = match notice.kind.as_str() {
        "reminder_daily" => notice.day == profile.day && profile.today.reviews == 0,
        "reminder_urgent" => notice.day == profile.day && unprotected(profile),
        "reminder_milestone" => notice.day == profile.day && next_milestone(profile).is_some(),
        "reminder_freeze_refill" => notice.day == profile.day && refill_due(profile),
        "reminder_freeze_used" => profile
            .history
            .iter()
            .any(|day| day.frozen && clock.day(day.at) == notice.day),
        "reminder_weekly_closing" => {
            week.end_after(created) == week.end_after(now) && profile.week_xp > 0
        }
        "reminder_weekly_recap" => week.end_after(created) == week.end_after(now),
        _ => false,
    };
    if !valid {
        return Ok(DeliveryGuard::Cancel);
    }
    if prefs.quiet(clock.hour(now)) {
        return Ok(DeliveryGuard::Defer);
    }
    if matches!(
        notice.kind.as_str(),
        "reminder_daily" | "reminder_milestone" | "reminder_freeze_refill"
    ) && !reminder_due(&prefs, clock, now)
    {
        return Ok(DeliveryGuard::Defer);
    }
    if notice.kind == "reminder_urgent" && !urgent_window(&prefs, clock, now) {
        return Ok(DeliveryGuard::Cancel);
    }
    if notice.kind == "reminder_weekly_closing"
        && !deadline_window(&prefs, clock, now, week.end_after(now))
    {
        return Ok(DeliveryGuard::Cancel);
    }
    Ok(DeliveryGuard::Deliver)
}

/// Refresh time-sensitive wording immediately before returning or delivering
/// an existing notice, including when freeze protection was changed meanwhile.
pub fn current_body(
    notice: &Notification,
    profile: &Profile,
    clock: &Clock,
    week: &Week,
    now: i64,
) -> String {
    match notice.kind.as_str() {
        "reminder_urgent" => urgent_body(profile, clock, now),
        "reminder_weekly_closing" => closing_body(profile, week, now),
        _ => notice.body.clone(),
    }
}

fn urgent_body(profile: &Profile, clock: &Clock, now: i64) -> String {
    let hours = (deadline(clock, now) - now + HOUR - 1) / HOUR;
    format!(
        "Your {}-day streak ends in {hours} {} at your {:02}:00 Anki cutoff. {}",
        profile.streak,
        if hours == 1 { "hour" } else { "hours" },
        clock.rollover_hour,
        if profile.freezes_enabled {
            "No freezes available."
        } else {
            "Streak protection is off."
        }
    )
}

fn closing_body(profile: &Profile, week: &Week, now: i64) -> String {
    let hours = (week.end_after(now) - now + HOUR - 1) / HOUR;
    format!(
        "The shared leaderboard week closes in {hours} {} ({}, {:02}:00). You have {} XP this week.",
        if hours == 1 { "hour" } else { "hours" },
        week.tz,
        week.rollover_hour,
        profile.week_xp
    )
}

pub fn cancel(store: &mut Store, id: i64) -> Result<(), Error> {
    store.conn.execute(
        "update notifications set pushed = 1, push_only = 1 where id = ?1 and kind glob 'reminder_*'", [id],
    )?;
    Ok(())
}

/// Call on sync/tick before serving the inbox as well as before push delivery.
pub fn cancel_stale(
    store: &mut Store,
    user: &str,
    profile: &Profile,
    clock: &Clock,
    week: &Week,
    now: i64,
) -> Result<(), Error> {
    let notices = store.notifications(user, now)?;
    for notice in notices
        .iter()
        .filter(|notice| notice.kind.starts_with("reminder_"))
    {
        if delivery_guard(store, user, notice, profile, clock, week, now)? == DeliveryGuard::Cancel
        {
            cancel(store, notice.id)?;
        }
    }
    Ok(())
}

pub fn tick(
    store: &mut Store,
    user: &str,
    profile: &Profile,
    clock: &Clock,
    week: &Week,
    now_ms: i64,
    previous_week: Option<Recap>,
) -> Result<(), Error> {
    let prefs = settings(store, user)?;
    let freeze_since = baseline(store, user, "freeze_used", prefs.freeze_used, now_ms)?;
    let recap_since = baseline(store, user, "weekly_recap", prefs.weekly_recap, now_ms)?;
    cancel_stale(store, user, profile, clock, week, now_ms)?;
    if prefs.quiet(clock.hour(now_ms)) {
        return Ok(());
    }
    let day = profile.day;
    let urgent_key = format!("reminder:urgent:{day}");
    let urgent_seen = seen(store, user, &urgent_key)?;
    let due = reminder_due(&prefs, clock, now_ms);
    let mut candidates = Vec::new();
    if prefs.urgent_streak && unprotected(profile) && urgent_window(&prefs, clock, now_ms) {
        candidates.push(Candidate {
            kind: "reminder_urgent",
            key: urgent_key.clone(),
            title: "Your streak needs a review",
            body: urgent_body(profile, clock, now_ms),
            day,
        });
    }
    if prefs.freeze_used
        && let Some(frozen) = profile.history.iter().rev().find(|item| {
            item.frozen
                && item.at + DAY > freeze_since
                && item.at + DAY <= now_ms
                && now_ms - (item.at + DAY) < 2 * DAY
        })
    {
        candidates.push(Candidate {
            kind: "reminder_freeze_used",
            key: format!("reminder:freeze_used:{}", frozen.date),
            title: "A freeze protected your streak",
            body: format!(
                "A freeze covered {}. You have {} {} left{}.",
                frozen.date,
                profile.stored_freezes,
                if profile.stored_freezes == 1 {
                    "freeze"
                } else {
                    "freezes"
                },
                if profile.freezes_enabled {
                    ""
                } else {
                    " in storage; streak protection is now off"
                }
            ),
            day: clock.day(frozen.at),
        });
    }
    let week_end = week.end_after(now_ms);
    if prefs.weekly_closing
        && profile.week_xp > 0
        && deadline_window(&prefs, clock, now_ms, week_end)
    {
        candidates.push(Candidate {
            kind: "reminder_weekly_closing",
            key: format!("reminder:weekly_closing:{week_end}"),
            title: "The weekly competition closes soon",
            body: closing_body(profile, week, now_ms),
            day,
        });
    }
    if due && !urgent_seen {
        if prefs.milestone
            && let Some(next) = next_milestone(profile)
        {
            candidates.push(Candidate {
                kind: "reminder_milestone",
                key: format!("reminder:milestone:{day}"),
                title: "A streak milestone is one study day away",
                body: format!("Study today to reach {next} days in your streak."),
                day,
            });
        }
        if prefs.gentle_daily
            && profile.today.reviews == 0
            && !candidates
                .iter()
                .any(|candidate| candidate.kind == "reminder_milestone")
        {
            candidates.push(Candidate {
                kind: "reminder_daily",
                key: format!("reminder:daily:{day}"),
                title: "Time for a few reviews?",
                body: if profile.streak > 0 {
                    format!(
                        "You haven't studied today. Your {}-day streak is still waiting.",
                        profile.streak
                    )
                } else {
                    "A few reviews today can start your next streak.".into()
                },
                day,
            });
        }
    }
    if due && prefs.freeze_refill && refill_due(profile) {
        let remaining = profile.quests.iter().filter(|quest| !quest.done).count();
        candidates.push(Candidate {
            kind: "reminder_freeze_refill", key: format!("reminder:freeze_refill:{day}"), title: "Earn another streak freeze",
            body: format!("Finish your {remaining} remaining daily {} to earn another freeze. You have {} of {MAX_FREEZES} stored.",
                if remaining == 1 { "quest" } else { "quests" }, profile.stored_freezes), day,
        });
    }
    if prefs.weekly_recap
        && let Some(recap) = previous_week.filter(|recap| {
            recap.week_end > recap_since && recap.week_end <= now_ms && now_ms < recap.valid_until
        })
    {
        let placing = recap.rank.map_or_else(
            || "No ranked result".into(),
            |rank| format!("Place #{rank}"),
        );
        let improvement = match recap.improvement_xp.cmp(&0) {
            std::cmp::Ordering::Greater => {
                format!("{} more XP than the previous week", recap.improvement_xp)
            }
            std::cmp::Ordering::Less => format!(
                "{} fewer XP than the previous week",
                recap.improvement_xp.unsigned_abs()
            ),
            std::cmp::Ordering::Equal => "the same XP as the previous week".into(),
        };
        candidates.push(Candidate {
            kind: "reminder_weekly_recap",
            key: format!("reminder:weekly_recap:{}", recap.week_start),
            title: "Your weekly study recap",
            body: format!(
                "{} study days, {} XP, {} daily wins. {placing}; {improvement}.",
                recap.days_studied, recap.xp, recap.daily_wins
            ),
            day,
        });
    }
    for candidate in candidates {
        if seen(store, user, &candidate.key)? {
            continue;
        }
        let already_urgent = seen(store, user, &urgent_key)?;
        if already_urgent && matches!(candidate.kind, "reminder_daily" | "reminder_milestone") {
            continue;
        }
        let used = used_budget(store, user, clock, now_ms)?;
        let reserve = prefs.urgent_streak
            && unprotected(profile)
            && !already_urgent
            && candidate.kind != "reminder_urgent";
        let allowance = prefs.daily_limit.saturating_sub(u8::from(reserve));
        if used >= allowance {
            continue;
        }
        store.send_once(
            &Outgoing {
                to: user,
                from: "",
                title: candidate.title,
                body: &candidate.body,
                kind: candidate.kind,
            },
            &candidate.key,
            candidate.day,
            now_ms,
            false,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{self, FreezePolicy, HistoryDay, Review};

    const DATE: i64 = 20_000;
    fn at(hour: i64) -> i64 {
        DATE * DAY + hour * HOUR
    }

    fn store() -> (Store, std::path::PathBuf) {
        let (store, path) = crate::decks::tests::temporary_store();
        initialize(&store.conn).unwrap();
        (store, path)
    }

    // These fixtures use a simulated calendar; record settings at its baseline
    // instead of the host's wall clock. Event-boundary tests pass explicit times.
    fn save_settings(store: &mut Store, user: &str, value: &Settings) -> Result<(), Error> {
        super::save_settings_at(store, user, value, at(20))
    }

    fn profile(clock: &Clock, now: i64) -> Profile {
        game::compute_with_freezes(
            "hill",
            "Hill",
            &[Review {
                id: now - DAY,
                cid: 1,
                last_ivl: 1,
                ivl: None,
                time_ms: 10_000,
                kind: 1,
            }],
            clock,
            &Week::default(),
            now,
            &FreezePolicy::default(),
        )
    }

    fn run(store: &mut Store, profile: &Profile, clock: &Clock, now: i64) {
        tick(store, "hill", profile, clock, &Week::default(), now, None).unwrap();
    }

    fn cleanup(store: Store, path: std::path::PathBuf) {
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn reminders_are_individually_opt_in_and_settings_validate() {
        let (mut store, path) = store();
        let defaults = settings(&store, "hill").unwrap();
        assert_eq!(defaults, Settings::default());
        assert_eq!(
            (
                defaults.reminder_hour,
                defaults.quiet_start,
                defaults.quiet_end,
                defaults.daily_limit
            ),
            (20, 22, 9, 2)
        );
        assert!(serde_json::from_str::<Settings>(r#"{"unknown": true}"#).is_err());
        run(
            &mut store,
            &profile(&Clock::default(), at(21)),
            &Clock::default(),
            at(21),
        );
        assert!(store.notifications("hill", at(21)).unwrap().is_empty());
        let bad = Settings {
            daily_limit: 0,
            ..defaults.clone()
        };
        assert!(save_settings(&mut store, "hill", &bad).is_err());
        let enabled = Settings {
            gentle_daily: true,
            ..defaults
        };
        save_settings(&mut store, "hill", &enabled).unwrap();
        run(
            &mut store,
            &profile(&Clock::default(), at(20)),
            &Clock::default(),
            at(20),
        );
        assert_eq!(store.notifications("hill", at(20)).unwrap().len(), 1);
        assert!(!settings(&store, "stranger").unwrap().gentle_daily);
        cleanup(store, path);
    }

    #[test]
    fn reopening_retires_legacy_risk_pushes_without_losing_deduplication() {
        let (mut store, path) = store();
        let notice = Outgoing {
            to: "hill",
            from: "",
            title: "Streak at risk",
            body: "Old global warning",
            kind: "risk",
        };
        store
            .send_once(&notice, "risk:legacy", DATE, at(20), true)
            .unwrap();
        assert_eq!(store.take_deck_deliveries(at(20)).unwrap().len(), 1);
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(store.take_deck_deliveries(at(20)).unwrap().is_empty());
        assert!(seen(&store, "hill", "risk:legacy").unwrap());
        assert!(store.notifications("hill", at(20)).unwrap().is_empty());
        cleanup(store, path);
    }

    #[test]
    fn saving_even_unchanged_opt_out_retires_only_that_players_legacy_warnings() {
        let (mut store, path) = store();
        for user in ["hill", "friend"] {
            store
                .send_once(
                    &Outgoing {
                        to: user,
                        from: "",
                        title: "Streak at risk",
                        body: "Old global warning",
                        kind: "risk",
                    },
                    "risk:legacy",
                    DATE,
                    at(20),
                    true,
                )
                .unwrap();
        }
        save_settings(&mut store, "hill", &Settings::default()).unwrap();
        let pending = store.take_deck_deliveries(at(20)).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].user, "friend");
        assert!(seen(&store, "hill", "risk:legacy").unwrap());
        cleanup(store, path);
    }

    #[test]
    fn unrelated_settings_preserve_failed_urgent_delivery_and_its_budget() {
        let (mut store, path) = store();
        let clock = Clock::default();
        let now = at(21);
        let p = profile(&clock, now);
        let mut prefs = Settings {
            urgent_streak: true,
            ..Settings::default()
        };
        save_settings(&mut store, "hill", &prefs).unwrap();
        run(&mut store, &p, &clock, now);
        let notice = store.notifications("hill", now).unwrap().remove(0);
        assert_eq!(notice.kind, "reminder_urgent");
        assert!(store.begin_push(notice.id, now).unwrap());
        store.finish_push(notice.id, false, now).unwrap();

        prefs.weekly_recap = true;
        super::save_settings_at(&mut store, "hill", &prefs, now + 1000).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        let retry_at = now + 20_000;
        let inbox = store.notifications("hill", retry_at).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].id, notice.id);
        assert!(store.take_deck_deliveries(retry_at - 1).unwrap().is_empty());
        let pending = store.take_deck_deliveries(retry_at).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].notification.id, notice.id);
        assert_eq!(
            delivery_guard(
                &store,
                "hill",
                &notice,
                &p,
                &clock,
                &Week::default(),
                retry_at,
            )
            .unwrap(),
            DeliveryGuard::Deliver
        );
        run(&mut store, &p, &clock, retry_at);
        assert_eq!(store.notifications("hill", retry_at).unwrap().len(), 1);
        assert_eq!(used_budget(&store, "hill", &clock, retry_at).unwrap(), 1);

        prefs.quiet_start = 21;
        super::save_settings_at(&mut store, "hill", &prefs, retry_at).unwrap();
        assert_eq!(
            delivery_guard(
                &store,
                "hill",
                &notice,
                &p,
                &clock,
                &Week::default(),
                retry_at,
            )
            .unwrap(),
            DeliveryGuard::Defer,
            "retained notices still honor newly configured quiet hours"
        );

        prefs.urgent_streak = false;
        super::save_settings_at(&mut store, "hill", &prefs, retry_at).unwrap();
        assert!(store.notifications("hill", retry_at).unwrap().is_empty());
        assert!(store.take_deck_deliveries(retry_at).unwrap().is_empty());
        assert_eq!(used_budget(&store, "hill", &clock, retry_at).unwrap(), 1);
        assert!(seen(&store, "hill", &format!("reminder:urgent:{}", p.day)).unwrap());
        cleanup(store, path);
    }

    #[test]
    fn retained_daily_reminder_waits_for_a_later_preferred_hour() {
        let (mut store, path) = store();
        let clock = Clock::default();
        let p = profile(&clock, at(20));
        let mut prefs = Settings {
            gentle_daily: true,
            ..Settings::default()
        };
        save_settings(&mut store, "hill", &prefs).unwrap();
        run(&mut store, &p, &clock, at(20));
        let notice = store.notifications("hill", at(20)).unwrap().remove(0);
        prefs.reminder_hour = 21;
        super::save_settings_at(&mut store, "hill", &prefs, at(20) + 60_000).unwrap();
        for (now, expected) in [
            (at(20) + 60_000, DeliveryGuard::Defer),
            (at(21), DeliveryGuard::Deliver),
        ] {
            run(&mut store, &p, &clock, now);
            assert_eq!(
                delivery_guard(&store, "hill", &notice, &p, &clock, &Week::default(), now).unwrap(),
                expected
            );
            let inbox = store.notifications("hill", now).unwrap();
            assert_eq!(inbox.len(), 1);
            assert_eq!(inbox[0].id, notice.id);
            assert_eq!(used_budget(&store, "hill", &clock, now).unwrap(), 1);
        }
        cleanup(store, path);
    }

    #[test]
    fn quiet_hours_wrap_and_urgent_uses_last_allowed_window() {
        let prefs = Settings::default();
        let clock = Clock::default();
        for hour in [22, 23, 0, 8] {
            assert!(prefs.quiet(hour));
        }
        for hour in [9, 20, 21] {
            assert!(!prefs.quiet(hour));
        }
        assert!(!urgent_window(&prefs, &clock, at(20)));
        assert!(urgent_window(&prefs, &clock, at(21)));
        assert!(!urgent_window(&prefs, &clock, at(23)));
        let daytime = Settings {
            quiet_start: 9,
            quiet_end: 17,
            ..prefs.clone()
        };
        let morning_cutoff = Clock {
            rollover_hour: 10,
            ..clock
        };
        assert!(urgent_window(&daytime, &morning_cutoff, at(8)));
        assert!(daytime.quiet(10));
        assert!(!daytime.quiet(20));
        let chosen_overnight = Settings {
            quiet_start: 0,
            quiet_end: 0,
            ..prefs
        };
        assert!(urgent_window(&chosen_overnight, &clock, at(26)));
        let offset = Clock {
            offset_west_min: -120,
            ..clock
        };
        assert!(urgent_window(&Settings::default(), &offset, at(19)));
        let week_end = Week::default().end_after(at(20));
        assert!(deadline_window(
            &Settings::default(),
            &clock,
            week_end - 7 * HOUR,
            week_end
        ));
        assert!(!deadline_window(
            &Settings::default(),
            &clock,
            week_end - 6 * HOUR,
            week_end
        ));
        assert!(deadline_window(
            &Settings::default(),
            &offset,
            week_end - 9 * HOUR,
            week_end
        ));
    }

    #[test]
    fn reminder_before_rollover_waits_for_the_end_of_each_anki_day() {
        let (mut store, path) = store();
        let clock = Clock::default();
        let prefs = Settings {
            gentle_daily: true,
            reminder_hour: 1,
            quiet_start: 0,
            quiet_end: 0,
            ..Settings::default()
        };
        save_settings(&mut store, "hill", &prefs).unwrap();
        assert!(!reminder_due(&prefs, &clock, at(20)));
        assert!(reminder_due(&prefs, &clock, at(25)));
        run(&mut store, &profile(&clock, at(25)), &clock, at(25));
        assert_eq!(store.notifications("hill", at(25)).unwrap().len(), 1);
        run(&mut store, &profile(&clock, at(28)), &clock, at(28));
        run(&mut store, &profile(&clock, at(44)), &clock, at(44));
        assert_eq!(
            used_budget(&store, "hill", &clock, at(44)).unwrap(),
            1,
            "04:00 rollover must not trigger the next 01:00 reminder early"
        );
        assert!(store.notifications("hill", at(44)).unwrap().is_empty());
        run(&mut store, &profile(&clock, at(49)), &clock, at(49));
        let notice = store.notifications("hill", at(49)).unwrap().remove(0);
        assert_eq!(notice.day, clock.day(at(49)));
        let east = Clock {
            offset_west_min: -120,
            ..clock
        };
        assert!(
            reminder_due(&prefs, &east, at(23)),
            "01:00 local time is due"
        );
        assert!(
            !reminder_due(&prefs, &east, at(26)),
            "04:00 local rollover starts a new window"
        );
        cleanup(store, path);
    }

    #[test]
    fn freeze_baseline_is_the_saved_opt_in_even_if_worker_restarts_after_rollover() {
        let (mut store, path) = store();
        let clock = Clock::default();
        let mut prefs = Settings {
            freeze_used: true,
            ..Settings::default()
        };
        super::save_settings_at(&mut store, "hill", &prefs, at(27)).unwrap();
        // An unrelated settings edit after rollover must retain the opt-in time.
        prefs.reminder_hour = 19;
        super::save_settings_at(&mut store, "hill", &prefs, at(29)).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        let mut p = profile(&clock, at(33));
        p.history.push(HistoryDay {
            at: at(4),
            date: game::date_string(DATE),
            xp: 0,
            reviews: 0,
            time_ms: 0,
            new_cards: 0,
            streak: 1,
            frozen: true,
        });
        run(&mut store, &p, &clock, at(33));
        assert_eq!(
            baseline(&store, "hill", "freeze_used", true, at(33)).unwrap(),
            at(27)
        );
        let notices = store.notifications("hill", at(33)).unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, "reminder_freeze_used");
        // Re-enabling after a past event establishes a new backfill boundary.
        prefs.freeze_used = false;
        super::save_settings_at(&mut store, "hill", &prefs, at(34)).unwrap();
        prefs.freeze_used = true;
        super::save_settings_at(&mut store, "hill", &prefs, at(35)).unwrap();
        assert_eq!(
            baseline(&store, "hill", "freeze_used", true, at(36)).unwrap(),
            at(35)
        );
        cleanup(store, path);
    }

    #[test]
    fn recap_opted_in_before_week_end_survives_a_delayed_first_tick() {
        let (mut store, path) = store();
        let clock = Clock::default();
        let week = Week::default();
        let end = week.end_after(at(20));
        let prefs = Settings {
            weekly_recap: true,
            quiet_start: 0,
            quiet_end: 0,
            ..Settings::default()
        };
        super::save_settings_at(&mut store, "hill", &prefs, end - HOUR).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        let now = end + DAY + HOUR;
        let p = profile(&clock, now);
        let recap = Recap {
            week_start: end - 7 * DAY,
            week_end: end,
            valid_until: end + 7 * DAY,
            days_studied: 5,
            xp: 500,
            daily_wins: 2,
            rank: Some(2),
            improvement_xp: 100,
        };
        tick(&mut store, "hill", &p, &clock, &week, now, Some(recap)).unwrap();
        assert_eq!(
            store.notifications("hill", now).unwrap()[0].kind,
            "reminder_weekly_recap"
        );
        assert_eq!(
            baseline(&store, "hill", "weekly_recap", true, now).unwrap(),
            end - HOUR
        );
        cleanup(store, path);
    }

    #[test]
    fn recap_eligibility_uses_the_archive_clock_after_configuration_changes() {
        let (mut store, path) = store();
        let clock = Clock::default();
        let archive_week = Week::default();
        let current_week = Week {
            rollover_hour: 5,
            ..archive_week
        };
        let end = archive_week.end_after(at(20));
        let prefs = Settings {
            weekly_recap: true,
            quiet_start: 0,
            quiet_end: 0,
            ..Settings::default()
        };
        super::save_settings_at(&mut store, "hill", &prefs, end - HOUR).unwrap();
        let now = end + DAY + HOUR;
        let p = profile(&clock, now);
        let recap = Recap {
            week_start: end - 7 * DAY,
            week_end: end,
            valid_until: end + 7 * DAY,
            days_studied: 5,
            xp: 500,
            daily_wins: 2,
            rank: Some(2),
            improvement_xp: 100,
        };
        for _ in 0..2 {
            tick(
                &mut store,
                "hill",
                &p,
                &clock,
                &current_week,
                now,
                Some(recap.clone()),
            )
            .unwrap();
        }
        let notices = store.notifications("hill", now).unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, "reminder_weekly_recap");

        // A new recipient avoids the first player's deduplication key, proving
        // the archive deadline itself prevents a stale recap from being queued.
        super::save_settings_at(&mut store, "friend", &prefs, end - HOUR).unwrap();
        let expired = recap.valid_until;
        let p = profile(&clock, expired);
        tick(
            &mut store,
            "friend",
            &p,
            &clock,
            &current_week,
            expired,
            Some(recap),
        )
        .unwrap();
        assert!(store.notifications("friend", expired).unwrap().is_empty());
        cleanup(store, path);
    }

    #[test]
    fn urgent_requires_a_real_unprotected_streak_and_copy_distinguishes_paused_protection() {
        let (mut store, path) = store();
        let clock = Clock::default();
        save_settings(
            &mut store,
            "hill",
            &Settings {
                urgent_streak: true,
                ..Settings::default()
            },
        )
        .unwrap();
        let mut p = profile(&clock, at(21));
        p.freezes_enabled = true;
        p.freezes = 1;
        p.stored_freezes = 1;
        run(&mut store, &p, &clock, at(21));
        assert!(store.notifications("hill", at(21)).unwrap().is_empty());
        p.freezes = 0;
        p.freezes_enabled = false;
        p.today.reviews = 1;
        run(&mut store, &p, &clock, at(21));
        assert!(store.notifications("hill", at(21)).unwrap().is_empty());
        p.today.reviews = 0;
        p.streak = 0;
        run(&mut store, &p, &clock, at(21));
        assert!(
            store.notifications("hill", at(21)).unwrap().is_empty(),
            "an empty streak cannot be at risk"
        );
        p.streak = 1;
        run(&mut store, &p, &clock, at(21));
        let notices = store.notifications("hill", at(21)).unwrap();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].body.contains("protection is off"));
        assert!(!notices[0].body.contains("No freezes"));
        p.freezes_enabled = true;
        p.stored_freezes = 0;
        p.day += 1;
        run(&mut store, &p, &clock, at(45));
        let next = store.notifications("hill", at(45)).unwrap();
        assert_eq!(next.len(), 1);
        assert!(next[0].body.contains("No freezes available"));
        cleanup(store, path);
    }

    #[test]
    fn daily_budget_reserves_urgency_survives_restart_and_cancellation() {
        let (mut store, path) = store();
        let clock = Clock::default();
        let mut p = profile(&clock, at(20));
        p.streak = 6;
        let prefs = Settings {
            gentle_daily: true,
            milestone: true,
            urgent_streak: true,
            ..Settings::default()
        };
        save_settings(&mut store, "hill", &prefs).unwrap();
        run(&mut store, &p, &clock, at(20));
        let notices = store.notifications("hill", at(20)).unwrap();
        assert_eq!(
            notices.len(),
            1,
            "reserve the second slot for the later urgent warning"
        );
        assert_eq!(notices[0].kind, "reminder_milestone");
        cancel(&mut store, notices[0].id).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        initialize(&store.conn).unwrap();
        run(&mut store, &p, &clock, at(21));
        run(&mut store, &p, &clock, at(21));
        let notices = store.notifications("hill", at(21)).unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, "reminder_urgent");
        assert_eq!(used_budget(&store, "hill", &clock, at(21)).unwrap(), 2);
        cleanup(store, path);
    }

    #[test]
    fn delivery_rechecks_study_rollover_quiet_time_and_settings() {
        let (mut store, path) = store();
        let clock = Clock::default();
        let p = profile(&clock, at(20));
        save_settings(
            &mut store,
            "hill",
            &Settings {
                gentle_daily: true,
                ..Settings::default()
            },
        )
        .unwrap();
        run(&mut store, &p, &clock, at(20));
        let notice = store.notifications("hill", at(20)).unwrap().remove(0);
        let guard = |store: &Store, p: &Profile, now| {
            delivery_guard(store, "hill", &notice, p, &clock, &Week::default(), now).unwrap()
        };
        assert_eq!(guard(&store, &p, at(20)), DeliveryGuard::Deliver);
        store
            .conn
            .execute(
                "update notifications set pushed = 1 where id = ?1",
                [notice.id],
            )
            .unwrap();
        assert_eq!(
            guard(&store, &p, at(20)),
            DeliveryGuard::Deliver,
            "already pushed notices remain in the inbox while current"
        );
        assert_eq!(guard(&store, &p, at(22)), DeliveryGuard::Defer);
        let mut studied = p.clone();
        studied.today.reviews = 1;
        assert_eq!(guard(&store, &studied, at(21)), DeliveryGuard::Cancel);
        assert_eq!(guard(&store, &p, at(28)), DeliveryGuard::Cancel);
        save_settings(&mut store, "hill", &Settings::default()).unwrap();
        assert_eq!(guard(&store, &p, at(21)), DeliveryGuard::Cancel);
        assert!(store.notifications("hill", at(21)).unwrap().is_empty());
        cleanup(store, path);
    }

    #[test]
    fn refill_is_only_for_enabled_unfinished_earning_opportunities() {
        let (mut store, path) = store();
        let clock = Clock::default();
        let mut p = profile(&clock, at(20));
        p.freezes_enabled = false;
        save_settings(
            &mut store,
            "hill",
            &Settings {
                freeze_refill: true,
                ..Settings::default()
            },
        )
        .unwrap();
        run(&mut store, &p, &clock, at(20));
        assert!(store.notifications("hill", at(20)).unwrap().is_empty());
        p.freezes_enabled = true;
        p.stored_freezes = MAX_FREEZES;
        run(&mut store, &p, &clock, at(20));
        assert!(store.notifications("hill", at(20)).unwrap().is_empty());
        p.stored_freezes = 1;
        p.freeze_earned_today = true;
        run(&mut store, &p, &clock, at(20));
        assert!(store.notifications("hill", at(20)).unwrap().is_empty());
        p.freeze_earned_today = false;
        run(&mut store, &p, &clock, at(20));
        assert_eq!(
            store.notifications("hill", at(20)).unwrap()[0].kind,
            "reminder_freeze_refill"
        );
        p.quests.iter_mut().for_each(|quest| quest.done = true);
        cancel_stale(&mut store, "hill", &p, &clock, &Week::default(), at(21)).unwrap();
        assert!(store.notifications("hill", at(21)).unwrap().is_empty());
        cleanup(store, path);
    }

    #[test]
    fn freeze_used_suppresses_history_then_waits_until_waking_hours() {
        let (mut store, path) = store();
        let clock = Clock::default();
        let mut p = profile(&clock, at(20));
        p.history.push(HistoryDay {
            at: at(4) - DAY,
            date: game::date_string(DATE - 1),
            xp: 0,
            reviews: 0,
            time_ms: 0,
            new_cards: 0,
            streak: 1,
            frozen: true,
        });
        save_settings(
            &mut store,
            "hill",
            &Settings {
                freeze_used: true,
                ..Settings::default()
            },
        )
        .unwrap();
        run(&mut store, &p, &clock, at(20));
        assert!(
            store.notifications("hill", at(20)).unwrap().is_empty(),
            "no historical freeze notification on opt-in"
        );
        p.history.push(HistoryDay {
            at: at(4),
            date: game::date_string(DATE),
            xp: 0,
            reviews: 0,
            time_ms: 0,
            new_cards: 0,
            streak: 1,
            frozen: true,
        });
        p.day += 1;
        run(&mut store, &p, &clock, at(28));
        assert!(
            store.notifications("hill", at(28)).unwrap().is_empty(),
            "freeze consumed during quiet hours"
        );
        run(&mut store, &p, &clock, at(33));
        let notices = store.notifications("hill", at(33)).unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, "reminder_freeze_used");
        assert!(notices[0].body.contains(&game::date_string(DATE)));
        run(&mut store, &p, &clock, at(34));
        assert_eq!(store.notifications("hill", at(34)).unwrap().len(), 1);
        cleanup(store, path);
    }

    #[test]
    fn weekly_recap_is_once_for_newly_finished_week_and_closing_needs_activity() {
        let (mut store, path) = store();
        let clock = Clock::default();
        let week = Week::default();
        let end = week.end_after(at(20));
        let prefs = Settings {
            weekly_recap: true,
            weekly_closing: true,
            quiet_start: 0,
            quiet_end: 0,
            ..Settings::default()
        };
        save_settings(&mut store, "hill", &prefs).unwrap();
        let mut p = profile(&clock, at(20));
        let historical = Recap {
            week_start: end - 14 * DAY,
            week_end: end - 7 * DAY,
            valid_until: end,
            days_studied: 6,
            xp: 800,
            daily_wins: 2,
            rank: Some(1),
            improvement_xp: 50,
        };
        tick(
            &mut store,
            "hill",
            &p,
            &clock,
            &week,
            at(20),
            Some(historical),
        )
        .unwrap();
        assert!(
            store.notifications("hill", at(20)).unwrap().is_empty(),
            "no old recap when enabled"
        );
        p.week_xp = 0;
        run(&mut store, &p, &clock, end - HOUR);
        assert!(store.notifications("hill", end - HOUR).unwrap().is_empty());
        p.week_xp = 500;
        run(&mut store, &p, &clock, end - HOUR);
        assert_eq!(
            store.notifications("hill", end - HOUR).unwrap()[0].kind,
            "reminder_weekly_closing"
        );
        let recap = Recap {
            week_start: end - 7 * DAY,
            week_end: end,
            valid_until: end + 7 * DAY,
            days_studied: 5,
            xp: 500,
            daily_wins: 2,
            rank: Some(2),
            improvement_xp: 100,
        };
        let now = end + 5 * HOUR;
        p.day = clock.day(now);
        tick(
            &mut store,
            "hill",
            &p,
            &clock,
            &week,
            now,
            Some(recap.clone()),
        )
        .unwrap();
        tick(&mut store, "hill", &p, &clock, &week, now, Some(recap)).unwrap();
        let notices = store.notifications("hill", now).unwrap();
        assert_eq!(
            notices
                .iter()
                .filter(|notice| notice.kind == "reminder_weekly_recap")
                .count(),
            1
        );
        let recap = notices
            .iter()
            .find(|notice| notice.kind == "reminder_weekly_recap")
            .unwrap();
        assert!(recap.body.contains("5 study days, 500 XP, 2 daily wins"));
        assert!(recap.body.contains("100 more XP"));
        cleanup(store, path);
    }
}
