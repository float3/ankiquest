//! Persistent competition history. Anki-day XP is credited to the shared date
//! containing that Anki day's start, matching game::Periods. The live leaderboard
//! keeps its existing scoring; this archive freezes each result after a late-sync
//! grace period and never rewrites a finalized winner.

use crate::game::{self, Clock, FreezePolicy, Week};
use crate::store::{Error, Store};
use chrono::{Datelike, Duration, NaiveDate};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const GRACE_MS: i64 = 86_400_000;

pub struct Participant {
    pub user: String,
    pub display: String,
    pub reviews: std::sync::Arc<Vec<game::Review>>,
    pub clock: Clock,
    pub freeze_policy: FreezePolicy,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Standing {
    pub user: String,
    pub display: String,
    pub xp: u64,
    pub reviews: u64,
    pub time_ms: i64,
    pub new_cards: u64,
    pub days_active: u64,
    pub best_streak: u64,
    pub freezes_used: u64,
}

impl Standing {
    fn add(&mut self, other: &Self) {
        self.xp += other.xp;
        self.reviews += other.reviews;
        self.time_ms += other.time_ms;
        self.new_cards += other.new_cards;
        self.days_active += other.days_active;
        self.best_streak = self.best_streak.max(other.best_streak);
        self.freezes_used += other.freezes_used;
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Period {
    pub key: String,
    pub start: String,
    /// Exclusive end date.
    pub end: String,
    pub kind: String,
    pub status: String,
    pub reconstructed: bool,
    pub partial: bool,
    pub winners: Vec<String>,
    pub winning_margin: u64,
    pub standings: Vec<Standing>,
}

impl Period {
    fn is_final(&self) -> bool {
        self.status == "final"
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Metadata {
    start_date: Option<String>,
    start_source: String,
    initialized_at: i64,
    time_zone: String,
    rollover_hour: u32,
}

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists competition_meta (
             id integer primary key check (id = 1),
             payload text not null
         );
         create table if not exists competition_periods (
             kind text not null,
             key text not null,
             finalized integer not null,
             payload text not null,
             primary key (kind, key)
         ) without rowid;",
    )?;
    Ok(())
}

fn read_metadata(conn: &Connection) -> Result<Option<Metadata>, Error> {
    let raw: Option<String> = conn
        .query_row("select payload from competition_meta where id=1", [], |r| {
            r.get(0)
        })
        .optional()?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
        .transpose()
}

fn read_periods(conn: &Connection) -> Result<Vec<Period>, Error> {
    let mut stmt = conn.prepare("select payload from competition_periods order by key, kind")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    rows.map(|raw| Ok(serde_json::from_str(&raw?)?)).collect()
}

fn read_period(conn: &Connection, kind: &str, key: &str) -> Result<Option<Period>, Error> {
    let raw: Option<String> = conn
        .query_row(
            "select payload from competition_periods where kind=?1 and key=?2",
            params![kind, key],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
        .transpose()
}

fn date(value: &str) -> Result<NaiveDate, Error> {
    Ok(NaiveDate::parse_from_str(value, "%Y-%m-%d")?)
}

fn monday(day: NaiveDate) -> NaiveDate {
    day - Duration::days(i64::from(day.weekday().num_days_from_monday()))
}

fn month_start(day: NaiveDate) -> NaiveDate {
    day.with_day(1).unwrap()
}

fn next_month(day: NaiveDate) -> NaiveDate {
    if day.month() == 12 {
        NaiveDate::from_ymd_opt(day.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(day.year(), day.month() + 1, 1).unwrap()
    }
}

fn make_period(
    kind: &str,
    dates: std::ops::Range<NaiveDate>,
    first: NaiveDate,
    standings: impl IntoIterator<Item = Standing>,
    week: &Week,
    now_ms: i64,
    initialized_at: i64,
) -> Period {
    let (start, end) = (dates.start, dates.end);
    let mut standings: Vec<_> = standings.into_iter().collect();
    standings.sort_by(|a, b| b.xp.cmp(&a.xp).then_with(|| a.user.cmp(&b.user)));
    let top = standings
        .iter()
        .filter(|s| s.reviews > 0)
        .map(|s| s.xp)
        .max();
    let winners: Vec<_> = top.map_or_else(Vec::new, |xp| {
        standings
            .iter()
            .filter(|s| s.reviews > 0 && s.xp == xp)
            .map(|s| s.user.clone())
            .collect()
    });
    let runner_up = standings
        .iter()
        .filter(|s| s.reviews > 0 && !winners.contains(&s.user))
        .map(|s| s.xp)
        .max()
        .unwrap_or(0);
    let closes = week.boundary_ms(end);
    Period {
        key: if kind == "month" {
            start.format("%Y-%m").to_string()
        } else {
            start.to_string()
        },
        start: start.to_string(),
        end: end.to_string(),
        kind: kind.into(),
        status: if now_ms >= closes + GRACE_MS {
            "final"
        } else if now_ms >= closes {
            "provisional"
        } else {
            "ongoing"
        }
        .into(),
        reconstructed: closes <= initialized_at,
        partial: start < first,
        winning_margin: if winners.len() == 1 {
            top.unwrap_or(0).saturating_sub(runner_up)
        } else {
            0
        },
        winners,
        standings,
    }
}

/// Refresh in one scoring pass per participant, plus linear aggregation by day.
/// The transaction protects metadata and all periods as one consistent snapshot.
pub fn refresh(
    store: &mut Store,
    players: &[Participant],
    week: &Week,
    now_ms: i64,
    start_date: Option<&str>,
) -> Result<(), Error> {
    let existing = read_metadata(&store.conn)?;
    let mut meta = existing.unwrap_or(Metadata {
        start_date: None,
        start_source: if start_date.is_some() {
            "configured"
        } else {
            "available_history"
        }
        .into(),
        initialized_at: now_ms,
        time_zone: week.tz.to_string(),
        rollover_hour: week.rollover_hour,
    });
    // The archive's time zone is fixed at creation. A later server config change
    // must not quietly re-label finalized dates or change old contest boundaries.
    let archive_week = Week {
        tz: meta.time_zone.parse()?,
        rollover_hour: meta.rollover_hour,
    };
    let today = archive_week.competition_date(now_ms);
    let mut credits: BTreeMap<NaiveDate, BTreeMap<String, Standing>> = BTreeMap::new();
    let mut joined = HashMap::new();
    let mut earliest = None;
    for player in players {
        // Exclude future-dated revlog rows from the archive.
        let reviews: Vec<_> = player
            .reviews
            .iter()
            .filter(|r| r.id <= now_ms)
            .copied()
            .collect();
        let profile = game::compute_with_freezes(
            &player.user,
            &player.display,
            &reviews,
            &player.clock,
            &archive_week,
            now_ms,
            &player.freeze_policy,
        );
        joined.insert(
            player.user.clone(),
            profile
                .history
                .iter()
                .find(|day| day.reviews > 0)
                .map_or(today, |day| archive_week.competition_date(day.at)),
        );
        for day in profile.history {
            let common_day = archive_week.competition_date(day.at);
            if common_day > today {
                continue;
            }
            if day.reviews > 0 {
                earliest =
                    Some(earliest.map_or(common_day, |first: NaiveDate| first.min(common_day)));
            }
            let standing = credits
                .entry(common_day)
                .or_default()
                .entry(player.user.clone())
                .or_insert_with(|| Standing {
                    user: player.user.clone(),
                    display: player.display.clone(),
                    ..Standing::default()
                });
            standing.xp += day.xp;
            standing.reviews += day.reviews;
            standing.time_ms += day.time_ms;
            standing.new_cards += day.new_cards;
            // At DST transitions two fixed-offset Anki-day starts can map to the
            // same competition date. Consistency still counts that date once.
            standing.days_active = u64::from(standing.reviews > 0);
            standing.best_streak = standing.best_streak.max(day.streak);
            standing.freezes_used += u64::from(day.frozen);
        }
    }
    let configured = start_date.map(date).transpose()?;
    let prior_start = meta.start_date.as_deref().map(date).transpose()?;
    let first = if meta.start_source == "configured" {
        prior_start.or(configured)
    } else {
        match (prior_start, earliest) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    };
    meta.start_date = first.map(|d| d.to_string());
    let old = read_periods(&store.conn)?;
    let finalized_days: HashMap<_, _> = old
        .into_iter()
        .filter(|p| p.kind == "day" && p.is_final())
        .map(|p| (p.key.clone(), p))
        .collect();
    let mut periods = Vec::new();
    if let Some(first) = first.filter(|first| *first <= today) {
        let mut day = first;
        while day <= today {
            let key = day.to_string();
            let p = if let Some(frozen) = finalized_days.get(&key) {
                frozen.clone()
            } else {
                let mut standings = credits.remove(&day).unwrap_or_default();
                for player in players {
                    if day < joined[&player.user] {
                        continue;
                    }
                    standings
                        .entry(player.user.clone())
                        .or_insert_with(|| Standing {
                            user: player.user.clone(),
                            display: player.display.clone(),
                            ..Standing::default()
                        });
                }
                make_period(
                    "day",
                    day..day + Duration::days(1),
                    first,
                    standings.into_values(),
                    &archive_week,
                    now_ms,
                    meta.initialized_at,
                )
            };
            periods.push(p);
            day += Duration::days(1);
        }
        let mut weeks: BTreeMap<NaiveDate, BTreeMap<String, Standing>> = BTreeMap::new();
        let mut months: BTreeMap<NaiveDate, BTreeMap<String, Standing>> = BTreeMap::new();
        let mut reconstructed_weeks = BTreeSet::new();
        let mut reconstructed_months = BTreeSet::new();
        for day in &periods {
            let start = date(&day.start)?;
            if day.reconstructed {
                reconstructed_weeks.insert(monday(start));
                reconstructed_months.insert(month_start(start));
            }
            for bucket in [
                weeks.entry(monday(start)).or_default(),
                months.entry(month_start(start)).or_default(),
            ] {
                for row in &day.standings {
                    bucket
                        .entry(row.user.clone())
                        .or_insert_with(|| Standing {
                            user: row.user.clone(),
                            display: row.display.clone(),
                            ..Standing::default()
                        })
                        .add(row);
                }
            }
        }
        for (start, rows) in weeks {
            let mut period = make_period(
                "week",
                start..start + Duration::days(7),
                first,
                rows.into_values(),
                &archive_week,
                now_ms,
                meta.initialized_at,
            );
            period.reconstructed |= reconstructed_weeks.contains(&start);
            periods.push(period);
        }
        for (start, rows) in months {
            let mut period = make_period(
                "month",
                start..next_month(start),
                first,
                rows.into_values(),
                &archive_week,
                now_ms,
                meta.initialized_at,
            );
            period.reconstructed |= reconstructed_months.contains(&start);
            periods.push(period);
        }
    }
    let tx = store.conn.transaction()?;
    tx.execute("insert into competition_meta (id,payload) values (1,?1) on conflict(id) do update set payload=excluded.payload", [serde_json::to_string(&meta)?])?;
    {
        let mut stmt = tx.prepare("insert into competition_periods (kind,key,finalized,payload) values (?1,?2,?3,?4) on conflict(kind,key) do update set finalized=excluded.finalized,payload=excluded.payload where competition_periods.finalized=0")?;
        for period in periods {
            stmt.execute(params![
                period.kind,
                period.key,
                period.is_final(),
                serde_json::to_string(&period)?
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct WeeklyRecap {
    pub week_start: i64,
    pub week_end: i64,
    /// End of the following week, using the persisted archive clock.
    pub valid_until: i64,
    pub days_studied: u64,
    pub xp: u64,
    pub daily_wins: u64,
    pub rank: Option<u64>,
    pub improvement_xp: i64,
}

pub fn weekly_recap(
    store: &Store,
    user: &str,
    week: &Week,
    now_ms: i64,
) -> Result<Option<WeeklyRecap>, Error> {
    let meta = read_metadata(&store.conn)?;
    let archive_week = meta
        .as_ref()
        .map(|m| {
            Ok::<_, Error>(Week {
                tz: m.time_zone.parse()?,
                rollover_hour: m.rollover_hour,
            })
        })
        .transpose()?
        .unwrap_or(*week);
    let end = monday(archive_week.competition_date(now_ms));
    let start = end - Duration::days(7);
    let previous = start - Duration::days(7);
    if now_ms < archive_week.boundary_ms(end) + GRACE_MS {
        return Ok(None);
    }
    let Some(latest) =
        read_period(&store.conn, "week", &start.to_string())?.filter(Period::is_final)
    else {
        return Ok(None);
    };
    let score = latest.standings.iter().find(|s| s.user == user);
    let prior = read_period(&store.conn, "week", &previous.to_string())?;
    let previous_xp = prior
        .as_ref()
        .and_then(|p| p.standings.iter().find(|s| s.user == user))
        .map_or(0, |s| s.xp);
    let mut daily_wins = 0;
    for offset in 0..7 {
        if read_period(
            &store.conn,
            "day",
            &(start + Duration::days(offset)).to_string(),
        )?
        .is_some_and(|period| won(&period, user))
        {
            daily_wins += 1;
        }
    }
    Ok(Some(WeeklyRecap {
        week_start: archive_week.boundary_ms(start),
        week_end: archive_week.boundary_ms(end),
        valid_until: archive_week.boundary_ms(end + Duration::days(7)),
        days_studied: score.map_or(0, |s| s.days_active),
        xp: score.map_or(0, |s| s.xp),
        daily_wins,
        rank: score.filter(|s| s.reviews > 0).map(|s| {
            1 + latest
                .standings
                .iter()
                .filter(|other| other.reviews > 0 && other.xp > s.xp)
                .count() as u64
        }),
        improvement_xp: score.map_or(0, |s| s.xp) as i64 - previous_xp as i64,
    }))
}

fn won(period: &Period, user: &str) -> bool {
    period.is_final() && period.winners.iter().any(|winner| winner == user)
}

#[derive(Default, Serialize)]
struct WinnerTotals {
    user: String,
    display: String,
    history_start: Option<String>,
    day_wins: u64,
    week_wins: u64,
    month_wins: u64,
}

impl WinnerTotals {
    fn add_win(&mut self, kind: &str) {
        match kind {
            "day" => self.day_wins += 1,
            "week" => self.week_wins += 1,
            "month" => self.month_wins += 1,
            _ => {}
        }
    }
}

/// Raw lifetime wins and a common comparison window for the current roster.
/// Both views use the original archived winners; this never rewrites a result.
pub fn winner_history(store: &Store, players: &[Participant]) -> Result<Value, Error> {
    let meta = read_metadata(&store.conn)?;
    let periods = read_periods(&store.conn)?;
    winner_history_from_periods(meta.as_ref(), players, &periods)
}

fn archived_history_starts(periods: &[Period]) -> BTreeMap<String, String> {
    let mut starts: BTreeMap<String, String> = BTreeMap::new();
    for period in periods.iter().filter(|p| p.kind == "day") {
        for row in period.standings.iter().filter(|row| row.reviews > 0) {
            starts
                .entry(row.user.clone())
                .and_modify(|start| {
                    if period.start < *start {
                        *start = period.start.clone();
                    }
                })
                .or_insert_with(|| period.start.clone());
        }
    }
    starts
}

fn has_full_daily_coverage(period: &Period, covered_days: &BTreeSet<&str>) -> Result<bool, Error> {
    let mut day = date(&period.start)?;
    let end = date(&period.end)?;
    while day < end {
        if !covered_days.contains(day.to_string().as_str()) {
            return Ok(false);
        }
        day += Duration::days(1);
    }
    Ok(true)
}

fn winner_history_from_periods(
    meta: Option<&Metadata>,
    players: &[Participant],
    periods: &[Period],
) -> Result<Value, Error> {
    let mut identities = BTreeMap::new();
    let history_starts = archived_history_starts(periods);
    for period in periods {
        for row in &period.standings {
            identities.insert(row.user.clone(), row.display.clone());
        }
    }
    let mut roster = BTreeSet::new();
    let mut waiting_players = Vec::new();
    for player in players {
        identities.insert(player.user.clone(), player.display.clone());
        if history_starts.contains_key(&player.user) {
            roster.insert(player.user.clone());
        } else {
            waiting_players.push(json!({"user":player.user,"display":player.display}));
        }
    }
    let shared_start = (roster.len() >= 2)
        .then(|| {
            roster
                .iter()
                .filter_map(|user| history_starts.get(user))
                .max()
        })
        .flatten();
    let totals = |user: &str, display: &str| WinnerTotals {
        user: user.into(),
        display: display.into(),
        history_start: history_starts.get(user).cloned(),
        ..WinnerTotals::default()
    };
    let mut lifetime: BTreeMap<_, _> = identities
        .iter()
        .map(|(user, display)| (user.clone(), totals(user, display)))
        .collect();
    let mut shared: BTreeMap<_, _> = roster
        .iter()
        .map(|user| (user.clone(), totals(user, &identities[user])))
        .collect();
    let roster_present = |period: &Period| {
        roster
            .iter()
            .all(|user| period.standings.iter().any(|row| row.user == *user))
    };
    // An aggregate row only proves that a player appeared at some point in the
    // week/month. Require a recorded row on every constituent day as well, so a
    // late import or a gap in archived membership cannot create a partial match.
    // A present zero-review row is a valid missed study day, not missing history.
    let covered_days: BTreeSet<_> = periods
        .iter()
        .filter(|p| p.kind == "day" && p.is_final() && !p.partial && roster_present(p))
        .map(|p| p.start.as_str())
        .collect();
    let mut shared_periods = [0u64; 3];
    for period in periods.iter().filter(|p| p.is_final()) {
        for winner in &period.winners {
            if let Some(total) = lifetime.get_mut(winner) {
                total.add_win(&period.kind);
            }
        }
        let Some(start) = shared_start else {
            continue;
        };
        if period.partial
            || period.start < *start
            || period.winners.is_empty()
            || !roster_present(period)
        {
            continue;
        }
        let slot = match period.kind.as_str() {
            "day" => 0,
            "week" => 1,
            "month" => 2,
            _ => continue,
        };
        if slot != 0 && !has_full_daily_coverage(period, &covered_days)? {
            continue;
        }
        shared_periods[slot] += 1;
        for winner in &period.winners {
            if let Some(total) = shared.get_mut(winner) {
                total.add_win(&period.kind);
            }
        }
    }
    Ok(json!({
        "meta": {
            "start_date":meta.and_then(|m| m.start_date.as_deref()),
            "start_source":meta.map_or("available_history", |m| m.start_source.as_str()),
            "time_zone":meta.map(|m| m.time_zone.as_str()),
            "rollover_hour":meta.map(|m| m.rollover_hour)
        },
        "shared": {
            "start_date":shared_start,
            "player_count":roster.len(),
            "periods":{"day":shared_periods[0],"week":shared_periods[1],"month":shared_periods[2]},
            "players":shared.into_values().collect::<Vec<_>>()
        },
        "lifetime":{"players":lifetime.into_values().collect::<Vec<_>>()},
        "waiting_players":waiting_players
    }))
}

fn trophy(id: &str, title: &str, date: &str) -> Value {
    json!({"id":id,"title":title,"date":date})
}

fn add_records(days: &[&Period]) -> Vec<Value> {
    let mut personal: HashMap<(String, &str), u64> = HashMap::new();
    let mut server: HashMap<&str, u64> = HashMap::new();
    let mut records = Vec::new();
    for day in days.iter().filter(|p| p.is_final()) {
        // Emit at most one shared server-record event per metric and date, with
        // the tied holders listed explicitly rather than inventing a tiebreaker.
        for metric in ["daily_xp", "daily_reviews", "streak"] {
            let value_of = |s: &Standing| match metric {
                "daily_xp" => s.xp,
                "daily_reviews" => s.reviews,
                _ => s.best_streak,
            };
            let eligible: Vec<_> = day.standings.iter().filter(|s| s.reviews > 0).collect();
            for row in &eligible {
                let value = value_of(row);
                let best = personal.entry((row.user.clone(), metric)).or_default();
                if value > *best {
                    *best = value;
                    records.push(json!({"scope":"personal","metric":metric,"date":day.start,"user":row.user,"display":row.display,"value":value,"reconstructed":day.reconstructed}));
                }
            }
            let value = eligible.iter().map(|s| value_of(s)).max().unwrap_or(0);
            let best = server.entry(metric).or_default();
            if value > *best {
                *best = value;
                for row in eligible.iter().filter(|s| value_of(s) == value) {
                    records.push(json!({"scope":"server","metric":metric,"date":day.start,"user":row.user,"display":row.display,"value":value,"reconstructed":day.reconstructed}));
                }
            }
        }
    }
    records
}

fn awards(days: &[&Period], weeks: &[&Period]) -> Vec<Value> {
    let mut awards = Vec::new();
    let mut last_active: HashMap<&str, NaiveDate> = HashMap::new();
    for day in days.iter().filter(|p| p.is_final()) {
        let current = date(&day.start).unwrap();
        for row in day.standings.iter().filter(|s| s.reviews > 0) {
            if let Some(previous) = last_active.insert(&row.user, current) {
                let missed = (current - previous).num_days() - 1;
                if missed >= 7 {
                    awards.push(json!({"kind":"comeback","title":format!("Welcome back after {missed} days"),"date":day.start,"user":row.user,"display":row.display,"value":missed,"reconstructed":day.reconstructed}));
                }
            }
        }
    }
    for week in weeks.iter().filter(|p| p.is_final() && !p.partial) {
        for row in week.standings.iter().filter(|s| s.days_active >= 7) {
            awards.push(json!({"kind":"consistency","title":"Seven days of studying","date":week.start,"user":row.user,"display":row.display,"value":row.days_active,"reconstructed":week.reconstructed}));
        }
        let previous_date = (date(&week.start).unwrap() - Duration::days(7)).to_string();
        let Some(previous) = weeks
            .iter()
            .find(|p| p.start == previous_date && p.is_final() && !p.partial)
        else {
            continue;
        };
        let changes: Vec<_> = week
            .standings
            .iter()
            .filter_map(|row| {
                let old = previous
                    .standings
                    .iter()
                    .find(|p| p.user == row.user)?
                    .reviews;
                // An archived empty week is a valid zero baseline. A newcomer
                // without a prior snapshot row was already excluded above.
                (row.reviews > old).then_some((row, old, row.reviews.saturating_sub(old)))
            })
            .collect();
        let best = changes.iter().map(|(_, _, gain)| *gain).max().unwrap_or(0);
        for (row, old, gain) in changes.into_iter().filter(|(_, _, gain)| *gain == best) {
            awards.push(json!({"kind":"most_improved","title":"Most improved this week","date":week.start,"user":row.user,"display":row.display,"value":gain,"previous":old,"reconstructed":week.reconstructed}));
        }
    }
    awards.sort_by(|a, b| a["date"].as_str().cmp(&b["date"].as_str()));
    awards
}

pub fn dashboard(
    store: &Store,
    players: &[Participant],
    week: &Week,
    now_ms: i64,
    year: i32,
    month: u32,
) -> Result<Value, Error> {
    let selected_month =
        NaiveDate::from_ymd_opt(year, month, 1).ok_or("Invalid calendar year or month")?;
    let selected_end = next_month(selected_month).to_string();
    let selected_start = selected_month.to_string();
    let year_start = format!("{year:04}-01-01");
    let year_end = format!("{:04}-01-01", year + 1);
    let meta = read_metadata(&store.conn)?.unwrap_or(Metadata {
        start_date: None,
        start_source: "available_history".into(),
        initialized_at: now_ms,
        time_zone: week.tz.to_string(),
        rollover_hour: week.rollover_hour,
    });
    let periods = read_periods(&store.conn)?;
    let days: Vec<_> = periods.iter().filter(|p| p.kind == "day").collect();
    let weeks: Vec<_> = periods.iter().filter(|p| p.kind == "week").collect();
    let seasons: Vec<_> = periods.iter().filter(|p| p.kind == "month").collect();
    let awards = awards(&days, &weeks);
    let records = add_records(&days);
    let mut identities: BTreeMap<String, String> = BTreeMap::new();
    for period in &days {
        for row in &period.standings {
            identities.insert(row.user.clone(), row.display.clone());
        }
    }
    for player in players {
        identities.insert(player.user.clone(), player.display.clone());
    }
    let mut summaries = Vec::new();
    for (user, display) in &identities {
        let wins: Vec<_> = days.iter().filter(|p| won(p, user)).collect();
        let week_wins: Vec<_> = weeks.iter().filter(|p| won(p, user)).collect();
        let season_wins: Vec<_> = seasons.iter().filter(|p| won(p, user)).collect();
        let mut longest = 0u64;
        let mut streak = 0u64;
        for day in days.iter().filter(|p| p.is_final()) {
            streak = if won(day, user) { streak + 1 } else { 0 };
            longest = longest.max(streak);
        }
        let mut trophies = Vec::new();
        for (count, id, title) in [
            (1, "first_daily_win", "First daily victory"),
            (7, "seven_daily_wins", "Seven daily victories"),
            (30, "thirty_daily_wins", "Thirty daily victories"),
        ] {
            if let Some(period) = wins.get(count - 1) {
                trophies.push(trophy(id, title, &period.start));
            }
        }
        if let Some(period) = week_wins.first() {
            trophies.push(trophy("first_weekly_win", "Weekly champion", &period.start));
        }
        if let Some(period) = season_wins.first() {
            trophies.push(trophy("first_season_win", "Season champion", &period.start));
        }
        for (kind, id, title) in [
            ("consistency", "consistent_week", "A week of consistency"),
            ("comeback", "comeback", "Back in the game"),
        ] {
            if let Some(award) = awards
                .iter()
                .find(|a| a["kind"] == kind && a["user"] == *user)
            {
                trophies.push(trophy(id, title, award["date"].as_str().unwrap()));
            }
        }
        let mut yearly = Standing::default();
        let mut months: BTreeMap<String, Standing> = BTreeMap::new();
        for day in days
            .iter()
            .filter(|p| p.start >= year_start && p.start < year_end)
        {
            if let Some(row) = day.standings.iter().find(|s| s.user == *user) {
                yearly.add(row);
                months.entry(day.start[..7].into()).or_default().add(row);
            }
        }
        let strongest = months
            .iter()
            .filter(|(_, s)| s.reviews > 0)
            .max_by(|(a, x), (b, y)| x.xp.cmp(&y.xp).then_with(|| b.cmp(a)))
            .map(|(month, _)| month);
        let yearly_trophies = trophies
            .iter()
            .filter(|t| {
                t["date"]
                    .as_str()
                    .is_some_and(|d| d >= year_start.as_str() && d < year_end.as_str())
            })
            .count();
        summaries.push(json!({
            "user":user,"display":display,"day_wins":wins.len(),"week_wins":week_wins.len(),"season_wins":season_wins.len(),
            "shared_wins":wins.iter().chain(week_wins.iter()).chain(season_wins.iter()).filter(|p| p.winners.len()>1).count(),
            "longest_winning_streak":longest,"trophies":trophies,
            "year_review":{"year":year,"xp":yearly.xp,"reviews":yearly.reviews,"study_days":yearly.days_active,
                "best_streak":yearly.best_streak,"strongest_month":strongest,"trophies":yearly_trophies,
                "days_won":wins.iter().filter(|p| p.start>=year_start && p.start<year_end).count(),
                "weeks_won":week_wins.iter().filter(|p| p.start>=year_start && p.start<year_end).count(),
                "months":months.iter().map(|(month,s)| json!({"month":month,"xp":s.xp,"reviews":s.reviews,"study_days":s.days_active})).collect::<Vec<_>>()}
        }));
    }
    let users: Vec<_> = identities.keys().collect();
    let history_starts = archived_history_starts(&periods);
    let mut head_to_head = Vec::new();
    for (index, a) in users.iter().enumerate() {
        for b in users.iter().skip(index + 1) {
            let pair_start = history_starts
                .get(*a)
                .zip(history_starts.get(*b))
                .map(|(a, b)| a.max(b));
            let covered_days: BTreeSet<_> = days
                .iter()
                .filter(|p| {
                    p.is_final()
                        && !p.partial
                        && p.standings.iter().any(|row| &row.user == *a)
                        && p.standings.iter().any(|row| &row.user == *b)
                })
                .map(|p| p.start.as_str())
                .collect();
            let count = |periods: &[&Period]| -> Result<[u64; 3], Error> {
                let mut score = [0u64; 3];
                for period in periods.iter().filter(|p| p.is_final()) {
                    if period.kind == "week" {
                        let Some(start) = pair_start else {
                            continue;
                        };
                        // Compare complete weeks for this pair, independently
                        // of when other community members started studying.
                        if period.partial
                            || period.start < *start
                            || !has_full_daily_coverage(period, &covered_days)?
                        {
                            continue;
                        }
                    }
                    let ra = period.standings.iter().find(|r| &r.user == *a);
                    let rb = period.standings.iter().find(|r| &r.user == *b);
                    // Include only periods both players were present in the
                    // historical snapshot; never invent losses before joining.
                    if let (Some(ra), Some(rb)) = (ra, rb) {
                        if ra.reviews == 0 && rb.reviews == 0 {
                            continue;
                        }
                        match ra.xp.cmp(&rb.xp) {
                            std::cmp::Ordering::Greater => score[0] += 1,
                            std::cmp::Ordering::Less => score[1] += 1,
                            std::cmp::Ordering::Equal => score[2] += 1,
                        }
                    }
                }
                Ok(score)
            };
            let d = count(&days)?;
            let w = count(&weeks)?;
            head_to_head.push(json!({"a":a,"b":b,"days_a":d[0],"days_b":d[1],"days_tied":d[2],"weeks_a":w[0],"weeks_b":w[1],"weeks_tied":w[2]}));
        }
    }
    let winner_history = winner_history_from_periods(Some(&meta), players, &periods)?;
    let mut metadata = serde_json::to_value(meta)?;
    metadata["year"] = json!(year);
    metadata["month"] = json!(month);
    metadata["grace_hours"] = json!(24);
    metadata["scoring_note"] = json!(
        "Each full Anki day's XP counts toward the shared competition date containing that Anki day's start, matching the weekly leaderboard. Results accept syncs for 24 hours after the common cutoff, then remain fixed. Later syncs still count on the live leaderboard. Reconstructed results use the review history available when archived; coverage begins with the earliest retained history and may predate this server."
    );
    Ok(json!({
        "meta":metadata,"players":summaries,
        "calendar":days.iter().filter(|p| p.start>=selected_start && p.start<selected_end).collect::<Vec<_>>(),
        "weeks":weeks.iter().filter(|p| p.end>year_start && p.start<year_end).collect::<Vec<_>>(),
        "seasons":seasons.iter().filter(|p| p.start>=year_start && p.start<year_end).collect::<Vec<_>>(),
        "awards":awards.into_iter().filter(|a| a["date"].as_str().is_some_and(|d| d>=year_start.as_str() && d<year_end.as_str())).collect::<Vec<_>>(),
        "records":records.into_iter().filter(|a| a["date"].as_str().is_some_and(|d| d>=year_start.as_str() && d<year_end.as_str())).rev().collect::<Vec<_>>(),
        "head_to_head":head_to_head,"winner_history":winner_history
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Database {
        store: Option<Store>,
        path: PathBuf,
    }

    impl Database {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ankiquest-competition-{}-{}-{}",
                std::process::id(),
                crate::now_ms(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let store = Store::open(&path).unwrap();
            initialize(&store.conn).unwrap();
            Self {
                store: Some(store),
                path,
            }
        }
        fn get(&mut self) -> &mut Store {
            self.store.as_mut().unwrap()
        }
    }

    impl Drop for Database {
        fn drop(&mut self) {
            self.store.take();
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn at(day: &str, hour: u32) -> i64 {
        date(day)
            .unwrap()
            .and_hms_opt(hour, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    fn reviews(day: &str, n: i64) -> Vec<game::Review> {
        (0..n)
            .map(|index| game::Review {
                id: at(day, 12) + index * 1000,
                cid: at(day, 12) + index,
                last_ivl: 10,
                ivl: None,
                time_ms: 10_000,
                kind: 1,
            })
            .collect()
    }

    fn player(user: &str, reviews: Vec<game::Review>) -> Participant {
        Participant {
            user: user.into(),
            display: user.to_uppercase(),
            reviews: reviews.into(),
            clock: Clock::default(),
            freeze_policy: FreezePolicy::default(),
        }
    }

    fn period(db: &Store, kind: &str, key: &str) -> Period {
        read_periods(&db.conn)
            .unwrap()
            .into_iter()
            .find(|p| p.kind == kind && p.key == key)
            .unwrap()
    }

    fn winner_total<'a>(view: &'a Value, scope: &str, user: &str) -> &'a Value {
        view[scope]["players"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["user"] == user)
            .unwrap()
    }

    #[test]
    fn shared_wins_remove_solo_headstarts_and_ignore_accounts_without_history() {
        let mut db = Database::new();
        let mut veteran = reviews("2019-01-01", 1);
        veteran.extend(reviews("2022-01-03", 1));
        let players = vec![
            player("veteran", veteran),
            // Clear the veteran's existing comeback/achievement bonuses too;
            // the shared window must preserve, rather than recompute, this win.
            player("newcomer", reviews("2022-01-03", 100)),
            player("waiting", vec![]),
        ];
        let now = at("2022-01-05", 12);
        refresh(db.get(), &players, &Week::default(), now, None).unwrap();
        assert_eq!(
            period(db.get(), "day", "2022-01-03").winners,
            vec!["newcomer"]
        );
        let view = winner_history(db.get(), &players).unwrap();
        assert_eq!(view["meta"]["start_date"], "2019-01-01");
        assert_eq!(view["shared"]["start_date"], "2022-01-03");
        assert_eq!(view["shared"]["player_count"], 2);
        assert_eq!(view["shared"]["periods"]["day"], 1);
        assert_eq!(winner_total(&view, "shared", "veteran")["day_wins"], 0);
        assert_eq!(winner_total(&view, "shared", "newcomer")["day_wins"], 1);
        assert_eq!(winner_total(&view, "lifetime", "veteran")["day_wins"], 1);
        assert_eq!(
            winner_total(&view, "lifetime", "veteran")["history_start"],
            "2019-01-01"
        );
        assert!(winner_total(&view, "lifetime", "waiting")["history_start"].is_null());
        assert_eq!(
            view["waiting_players"],
            json!([{"user":"waiting","display":"WAITING"}])
        );
        let dashboard = dashboard(db.get(), &players, &Week::default(), now, 2022, 1).unwrap();
        assert_eq!(dashboard["winner_history"], view);
    }

    #[test]
    fn shared_weeks_and_months_begin_with_the_first_full_common_period() {
        let mut db = Database::new();
        let mut a = reviews("2026-09-01", 1);
        a.extend(reviews("2026-09-14", 100));
        a.extend(reviews("2026-10-01", 10));
        let mut b = reviews("2026-09-16", 2);
        b.extend(reviews("2026-09-21", 3));
        b.extend(reviews("2026-10-01", 1));
        let players = vec![player("a", a), player("b", b)];
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-11-03", 12),
            None,
        )
        .unwrap();
        let view = winner_history(db.get(), &players).unwrap();
        assert_eq!(view["shared"]["start_date"], "2026-09-16");
        assert_eq!(
            view["shared"]["periods"],
            json!({"day":3,"week":2,"month":1})
        );
        assert_eq!(winner_total(&view, "shared", "a")["day_wins"], 1);
        assert_eq!(winner_total(&view, "shared", "b")["day_wins"], 2);
        assert_eq!(winner_total(&view, "shared", "a")["week_wins"], 1);
        assert_eq!(winner_total(&view, "shared", "b")["week_wins"], 1);
        assert_eq!(winner_total(&view, "shared", "a")["month_wins"], 1);
        assert_eq!(winner_total(&view, "lifetime", "a")["month_wins"], 2);
        assert!(period(db.get(), "week", "2026-08-31").partial);
    }

    #[test]
    fn head_to_head_skips_a_partial_join_week_but_keeps_the_pairs_next_full_week() {
        let mut db = Database::new();
        let mut a = reviews("2026-09-14", 100);
        a.extend(reviews("2026-09-21", 1));
        let mut b = reviews("2026-09-16", 1);
        b.extend(reviews("2026-09-21", 5));
        let players = vec![
            player("a", a),
            player("b", b),
            player("later", reviews("2026-09-30", 1)),
        ];
        let now = at("2026-10-03", 12);
        refresh(db.get(), &players, &Week::default(), now, None).unwrap();
        let view = dashboard(db.get(), &players, &Week::default(), now, 2026, 9).unwrap();
        let pair = view["head_to_head"]
            .as_array()
            .unwrap()
            .iter()
            .find(|pair| pair["a"] == "a" && pair["b"] == "b")
            .unwrap();
        // Player a wins the full week in which b only joined on Wednesday.
        // Their next week is comparable even though a third player joins later.
        assert_eq!(period(db.get(), "week", "2026-09-14").winners, vec!["a"]);
        assert_eq!(pair["weeks_a"], 0);
        assert_eq!(pair["weeks_b"], 1);
        assert_eq!(pair["weeks_tied"], 0);
        assert_eq!(pair["days_a"], 0);
        assert_eq!(pair["days_b"], 2);
        assert_eq!(view["winner_history"]["shared"]["start_date"], "2026-09-30");

        // A missing inactive daily snapshot also disqualifies an otherwise full
        // week, even though its aggregate row still includes both players.
        let mut missing = period(db.get(), "day", "2026-09-23");
        missing.standings.retain(|row| row.user != "b");
        db.get()
            .conn
            .execute(
                "update competition_periods set payload=?1 where kind='day' and key='2026-09-23'",
                [serde_json::to_string(&missing).unwrap()],
            )
            .unwrap();
        let view = dashboard(db.get(), &players, &Week::default(), now, 2026, 9).unwrap();
        let pair = view["head_to_head"]
            .as_array()
            .unwrap()
            .iter()
            .find(|pair| pair["a"] == "a" && pair["b"] == "b")
            .unwrap();
        assert_eq!(pair["weeks_b"], 0);
        assert_eq!(pair["days_b"], 2);
    }

    #[test]
    fn late_joined_history_does_not_rewrite_or_reuse_absent_frozen_snapshots() {
        let mut db = Database::new();
        let mut a = reviews("2026-09-01", 1);
        a.extend(reviews("2026-09-14", 10));
        let mut players = vec![player("a", a)];
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-09-22", 12),
            None,
        )
        .unwrap();
        let original = serde_json::to_value(period(db.get(), "day", "2026-09-14")).unwrap();
        let mut b = reviews("2026-09-01", 100);
        b.extend(reviews("2026-09-23", 2));
        players.push(player("b", b));
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-09-23", 12),
            None,
        )
        .unwrap();
        let pending = winner_history(db.get(), &players).unwrap();
        assert_eq!(pending["shared"]["start_date"], "2026-09-23");
        assert_eq!(pending["shared"]["periods"]["day"], 0);
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-10-03", 12),
            None,
        )
        .unwrap();
        let view = winner_history(db.get(), &players).unwrap();
        assert_eq!(
            serde_json::to_value(period(db.get(), "day", "2026-09-14")).unwrap(),
            original
        );
        assert_eq!(view["shared"]["start_date"], "2026-09-23");
        assert_eq!(
            view["shared"]["periods"],
            json!({"day":1,"week":0,"month":0})
        );
        assert_eq!(winner_total(&view, "shared", "b")["day_wins"], 1);
        assert_eq!(winner_total(&view, "lifetime", "a")["day_wins"], 2);
        assert_eq!(winner_total(&view, "lifetime", "b")["day_wins"], 1);
    }

    #[test]
    fn shared_aggregate_requires_every_daily_snapshot_even_after_history_started() {
        let mut db = Database::new();
        let mut a = reviews("2026-09-01", 1);
        a.extend(reviews("2026-09-14", 2));
        let mut b = reviews("2026-09-01", 1);
        b.extend(reviews("2026-09-14", 1));
        let players = vec![player("a", a), player("b", b)];
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-10-03", 12),
            None,
        )
        .unwrap();
        let mut periods = read_periods(&db.get().conn).unwrap();
        // Model an interrupted archived membership: the aggregate contains both
        // players, but an inactive day in its middle was saved without player b.
        let gap = periods
            .iter_mut()
            .find(|p| p.kind == "day" && p.start == "2026-09-16")
            .unwrap();
        gap.standings.retain(|row| row.user != "b");
        let view = winner_history_from_periods(None, &players, &periods).unwrap();
        assert_eq!(view["shared"]["start_date"], "2026-09-01");
        assert_eq!(
            view["shared"]["periods"],
            json!({"day":2,"week":0,"month":0})
        );
        assert_eq!(winner_total(&view, "lifetime", "a")["month_wins"], 1);
        assert_eq!(winner_total(&view, "shared", "a")["month_wins"], 0);
    }

    #[test]
    fn shared_wins_keep_ties_inactivity_and_retired_champions_honest() {
        let mut db = Database::new();
        let mut a = reviews("2026-09-01", 1);
        a.extend(reviews("2026-09-02", 1));
        a.extend(reviews("2026-09-03", 1));
        let mut b = reviews("2026-09-01", 1);
        b.extend(reviews("2026-09-02", 1));
        let mut retired = reviews("2026-09-01", 1);
        retired.extend(reviews("2026-09-02", 3));
        let mut players = vec![player("a", a), player("b", b), player("retired", retired)];
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-09-06", 12),
            None,
        )
        .unwrap();
        players.pop();
        let view = winner_history(db.get(), &players).unwrap();
        assert_eq!(view["shared"]["player_count"], 2);
        assert_eq!(view["shared"]["periods"]["day"], 3);
        assert_eq!(winner_total(&view, "shared", "a")["day_wins"], 2);
        assert_eq!(winner_total(&view, "shared", "b")["day_wins"], 1);
        assert_eq!(winner_total(&view, "lifetime", "retired")["day_wins"], 2);
        assert_eq!(view["shared"]["players"].as_array().unwrap().len(), 2);
        assert_eq!(view["lifetime"]["players"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn shared_history_waits_for_two_players_and_final_results() {
        let mut db = Database::new();
        let empty = winner_history(db.get(), &[]).unwrap();
        assert!(empty["meta"]["start_date"].is_null());
        assert!(empty["shared"]["start_date"].is_null());
        assert_eq!(empty["shared"]["player_count"], 0);
        assert_eq!(
            empty["shared"]["periods"],
            json!({"day":0,"week":0,"month":0})
        );
        let mut players = vec![
            player("a", reviews("2026-09-01", 1)),
            player("waiting", vec![]),
        ];
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-09-03", 12),
            None,
        )
        .unwrap();
        let solo = winner_history(db.get(), &players).unwrap();
        assert!(solo["shared"]["start_date"].is_null());
        assert_eq!(solo["shared"]["player_count"], 1);
        assert_eq!(winner_total(&solo, "shared", "a")["day_wins"], 0);
        assert_eq!(winner_total(&solo, "lifetime", "a")["day_wins"], 1);
        players.push(player("b", reviews("2026-09-03", 1)));
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-09-04", 12),
            None,
        )
        .unwrap();
        let pending = winner_history(db.get(), &players).unwrap();
        assert_eq!(pending["shared"]["start_date"], "2026-09-03");
        assert_eq!(pending["shared"]["player_count"], 2);
        assert_eq!(pending["shared"]["periods"]["day"], 0);
        assert_eq!(winner_total(&pending, "shared", "b")["day_wins"], 0);
        assert_eq!(winner_total(&pending, "lifetime", "b")["day_wins"], 0);
    }

    #[test]
    fn common_cutoffs_follow_daylight_saving_and_gap_hours() {
        let week = Week {
            tz: chrono_tz::Europe::Berlin,
            rollover_hour: 4,
        };
        let spring = week.boundary_ms(date("2026-03-29").unwrap())
            - week.boundary_ms(date("2026-03-28").unwrap());
        let autumn = week.boundary_ms(date("2026-10-25").unwrap())
            - week.boundary_ms(date("2026-10-24").unwrap());
        assert_eq!(spring, 23 * 3_600_000);
        assert_eq!(autumn, 25 * 3_600_000);
        let gap = Week {
            rollover_hour: 2,
            ..week
        };
        let cutoff = gap.boundary_ms(date("2026-03-29").unwrap());
        assert_eq!(cutoff, at("2026-03-29", 1));
        assert_eq!(gap.competition_date(cutoff).to_string(), "2026-03-29");
        assert_eq!(gap.competition_date(cutoff - 1).to_string(), "2026-03-28");
    }

    #[test]
    fn ties_share_wins_and_empty_days_have_no_winner() {
        let mut db = Database::new();
        let players = vec![
            player("a", reviews("2026-09-14", 1)),
            player("b", reviews("2026-09-14", 1)),
        ];
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-09-18", 12),
            None,
        )
        .unwrap();
        let tie = period(db.get(), "day", "2026-09-14");
        assert_eq!(tie.winners, vec!["a", "b"]);
        assert_eq!(tie.winning_margin, 0);
        assert!(tie.reconstructed);
        assert_eq!(tie.status, "final");
        assert!(period(db.get(), "day", "2026-09-15").winners.is_empty());
        let dashboard = dashboard(
            db.get(),
            &players,
            &Week::default(),
            at("2026-09-18", 12),
            2026,
            9,
        )
        .unwrap();
        assert_eq!(dashboard["players"][0]["day_wins"], 1);
        assert_eq!(dashboard["players"][0]["shared_wins"], 1);
        assert_eq!(dashboard["head_to_head"][0]["days_tied"], 1);
        assert_eq!(dashboard["meta"]["start_source"], "available_history");
    }

    #[test]
    fn late_sync_updates_provisional_but_never_finalized_scores() {
        let mut db = Database::new();
        let week = Week::default();
        let mut players = vec![player("a", reviews("2026-09-20", 1))];
        refresh(db.get(), &players, &week, at("2026-09-21", 5), None).unwrap();
        let provisional = period(db.get(), "day", "2026-09-20");
        assert_eq!(provisional.status, "provisional");
        assert!(
            weekly_recap(db.get(), "a", &week, at("2026-09-21", 5))
                .unwrap()
                .is_none()
        );
        players[0].reviews = reviews("2026-09-20", 10).into();
        refresh(db.get(), &players, &week, at("2026-09-21", 10), None).unwrap();
        assert_eq!(
            period(db.get(), "day", "2026-09-20").standings[0].reviews,
            10
        );
        refresh(db.get(), &players, &week, at("2026-09-22", 4), None).unwrap();
        let final_day = serde_json::to_value(period(db.get(), "day", "2026-09-20")).unwrap();
        let final_week = serde_json::to_value(period(db.get(), "week", "2026-09-14")).unwrap();
        assert!(
            weekly_recap(db.get(), "a", &week, at("2026-09-22", 4))
                .unwrap()
                .is_some()
        );
        players[0].reviews = reviews("2026-09-20", 100).into();
        refresh(db.get(), &players, &week, at("2026-09-23", 12), None).unwrap();
        assert_eq!(
            serde_json::to_value(period(db.get(), "day", "2026-09-20")).unwrap(),
            final_day
        );
        assert_eq!(
            serde_json::to_value(period(db.get(), "week", "2026-09-14")).unwrap(),
            final_week
        );
        assert_eq!(
            period(db.get(), "month", "2026-09").standings[0].reviews,
            10
        );
    }

    #[test]
    fn recap_validity_preserves_the_archived_clock_after_cutoff_changes() {
        let mut db = Database::new();
        let archive_week = Week::default();
        let current_week = Week {
            rollover_hour: 5,
            ..archive_week
        };
        let players = vec![player("a", reviews("2026-09-20", 1))];
        refresh(
            db.get(),
            &players,
            &archive_week,
            at("2026-09-20", 20),
            None,
        )
        .unwrap();
        let now = at("2026-09-22", 5);
        refresh(db.get(), &players, &current_week, now, None).unwrap();
        let recap = weekly_recap(db.get(), "a", &current_week, now)
            .unwrap()
            .unwrap();
        assert_eq!(recap.week_start, at("2026-09-14", 4));
        assert_eq!(recap.week_end, at("2026-09-21", 4));
        assert_eq!(recap.valid_until, at("2026-09-28", 4));
        assert_eq!(recap.days_studied, 1);
    }

    #[test]
    fn different_player_clocks_match_existing_weekly_allocation() {
        let mut db = Database::new();
        let week = Week::default();
        let mut eastern = player("east", reviews("2026-09-14", 1));
        eastern.clock = Clock {
            offset_west_min: -600,
            rollover_hour: 4,
        };
        let players = vec![eastern, player("utc", reviews("2026-09-14", 1))];
        let now = at("2026-09-14", 14);
        refresh(db.get(), &players, &week, now, None).unwrap();
        let sunday = period(db.get(), "day", "2026-09-13");
        assert_eq!(
            sunday
                .standings
                .iter()
                .find(|s| s.user == "east")
                .unwrap()
                .reviews,
            1
        );
        let current_week = period(db.get(), "week", "2026-09-14");
        for player in &players {
            let profile = game::compute_with_freezes(
                &player.user,
                &player.display,
                &player.reviews,
                &player.clock,
                &week,
                now,
                &player.freeze_policy,
            );
            assert_eq!(
                current_week
                    .standings
                    .iter()
                    .find(|s| s.user == player.user)
                    .map_or(0, |s| s.xp),
                profile.week_xp
            );
        }
    }

    #[test]
    fn repeated_hour_can_combine_anki_days_but_counts_one_competition_day() {
        let mut db = Database::new();
        let week = Week {
            tz: chrono_tz::Europe::Berlin,
            rollover_hour: 4,
        };
        let mut rows = reviews("2026-10-24", 1);
        rows.extend(reviews("2026-10-25", 1));
        let mut participant = player("a", rows);
        participant.clock.rollover_hour = 2;
        refresh(db.get(), &[participant], &week, at("2026-10-27", 12), None).unwrap();
        let repeated = period(db.get(), "day", "2026-10-24");
        assert_eq!(repeated.standings[0].reviews, 2);
        assert_eq!(repeated.standings[0].days_active, 1);
    }

    #[test]
    fn awards_trophies_year_review_and_recap_use_archived_history() {
        let mut db = Database::new();
        let first = date("2026-01-05").unwrap();
        let mut study = Vec::new();
        for i in 0..14 {
            study.extend(reviews(
                &(first + Duration::days(i)).to_string(),
                if i < 7 { 1 } else { 2 },
            ));
        }
        study.extend(reviews("2026-01-28", 3));
        let players = vec![player("a", study)];
        let now = at("2026-02-03", 12);
        refresh(db.get(), &players, &Week::default(), now, None).unwrap();
        let view = dashboard(db.get(), &players, &Week::default(), now, 2026, 1).unwrap();
        let awards = view["awards"].as_array().unwrap();
        assert_eq!(
            awards.iter().filter(|a| a["kind"] == "consistency").count(),
            2
        );
        assert!(
            awards
                .iter()
                .any(|a| a["kind"] == "most_improved" && a["value"] == 7 && a["previous"] == 7)
        );
        assert!(
            awards
                .iter()
                .any(|a| a["kind"] == "comeback" && a["value"] == 9)
        );
        assert_eq!(view["players"][0]["longest_winning_streak"], 14);
        assert_eq!(view["players"][0]["year_review"]["study_days"], 15);
        assert_eq!(view["players"][0]["year_review"]["reviews"], 24);
        assert_eq!(
            view["players"][0]["year_review"]["strongest_month"],
            "2026-01"
        );
        assert_eq!(view["players"][0]["season_wins"], 1);
        assert!(
            view["records"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["scope"] == "server"
                    && r["metric"] == "daily_reviews"
                    && r["value"] == 3)
        );
        let recap = weekly_recap(db.get(), "a", &Week::default(), at("2026-01-20", 12))
            .unwrap()
            .unwrap();
        assert_eq!(recap.days_studied, 7);
        assert_eq!(recap.daily_wins, 7);
        assert_eq!(recap.rank, Some(1));
        assert!(recap.improvement_xp > 0);
    }

    #[test]
    fn imported_older_history_expands_coverage_and_is_labeled_reconstructed() {
        let mut db = Database::new();
        let mut players = vec![player("a", reviews("2026-09-20", 1))];
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-09-22", 12),
            None,
        )
        .unwrap();
        let frozen = serde_json::to_value(period(db.get(), "day", "2026-09-20")).unwrap();
        std::sync::Arc::make_mut(&mut players[0].reviews).splice(0..0, reviews("2026-09-01", 2));
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-09-23", 12),
            None,
        )
        .unwrap();
        assert_eq!(
            read_metadata(&db.get().conn)
                .unwrap()
                .unwrap()
                .start_date
                .as_deref(),
            Some("2026-09-01")
        );
        assert!(period(db.get(), "day", "2026-09-01").reconstructed);
        assert_eq!(
            serde_json::to_value(period(db.get(), "day", "2026-09-20")).unwrap(),
            frozen
        );
        assert!(!period(db.get(), "day", "2026-09-23").reconstructed);
    }

    #[test]
    fn empty_history_is_honest_and_calendar_filters_are_validated() {
        let mut db = Database::new();
        let players = vec![player("a", vec![])];
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-09-22", 12),
            None,
        )
        .unwrap();
        let view = dashboard(
            db.get(),
            &players,
            &Week::default(),
            at("2026-09-22", 12),
            2026,
            9,
        )
        .unwrap();
        assert!(view["meta"]["start_date"].is_null());
        assert!(view["calendar"].as_array().unwrap().is_empty());
        assert_eq!(view["players"][0]["day_wins"], 0);
        assert!(
            dashboard(
                db.get(),
                &players,
                &Week::default(),
                at("2026-09-22", 12),
                2026,
                13
            )
            .is_err()
        );
    }

    #[test]
    fn full_history_preserves_scoring_and_stays_off_the_profile_response() {
        let mut rows = reviews("2026-09-14", 150);
        rows.extend(reviews("2026-09-16", 20));
        let profile = game::compute_with_freezes(
            "a",
            "A",
            &rows,
            &Clock::default(),
            &Week::default(),
            at("2026-09-17", 12),
            &FreezePolicy::default(),
        );
        assert_eq!(
            profile.history.iter().map(|d| d.xp).sum::<u64>(),
            profile.xp_total
        );
        assert_eq!(
            profile.history.iter().map(|d| d.reviews).sum::<u64>(),
            profile.lifetime.reviews
        );
        assert_eq!(
            profile
                .history
                .iter()
                .find(|d| d.date == "2026-09-15")
                .unwrap()
                .reviews,
            0
        );
        assert!(
            serde_json::to_value(&profile)
                .unwrap()
                .get("history")
                .is_none()
        );
    }

    #[test]
    fn archive_timezone_stays_fixed_and_newcomers_have_no_pre_join_losses() {
        let mut db = Database::new();
        let players = vec![
            player("a", reviews("2026-09-14", 1)),
            player("b", reviews("2026-09-16", 1)),
        ];
        refresh(
            db.get(),
            &players,
            &Week::default(),
            at("2026-09-20", 12),
            None,
        )
        .unwrap();
        let changed = Week {
            tz: chrono_tz::Asia::Tokyo,
            rollover_hour: 8,
        };
        refresh(db.get(), &players, &changed, at("2026-09-21", 12), None).unwrap();
        let view = dashboard(db.get(), &players, &changed, at("2026-09-21", 12), 2026, 9).unwrap();
        assert_eq!(view["meta"]["time_zone"], "UTC");
        assert_eq!(view["meta"]["rollover_hour"], 4);
        assert_eq!(view["head_to_head"][0]["days_a"], 0);
        assert_eq!(view["head_to_head"][0]["days_b"], 1);
    }

    #[test]
    fn improvement_counts_a_recorded_zero_week_but_not_a_missing_baseline() {
        let mut db = Database::new();
        let mut returning = reviews("2026-09-01", 1);
        returning.extend(reviews("2026-09-14", 100));
        let mut steady = reviews("2026-09-01", 1);
        steady.extend(reviews("2026-09-07", 1));
        steady.extend(reviews("2026-09-14", 2));
        let players = vec![
            player("returning", returning),
            player("steady", steady),
            player("newcomer", reviews("2026-09-14", 1000)),
        ];
        let now = at("2026-09-22", 12);
        refresh(db.get(), &players, &Week::default(), now, None).unwrap();
        let previous = period(db.get(), "week", "2026-09-07");
        assert_eq!(
            previous
                .standings
                .iter()
                .find(|row| row.user == "returning")
                .unwrap()
                .reviews,
            0
        );
        assert!(!previous.standings.iter().any(|row| row.user == "newcomer"));
        let view = dashboard(db.get(), &players, &Week::default(), now, 2026, 9).unwrap();
        let improved: Vec<_> = view["awards"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|award| award["kind"] == "most_improved" && award["date"] == "2026-09-14")
            .collect();
        assert_eq!(improved.len(), 1);
        assert_eq!(improved[0]["user"], "returning");
        assert_eq!(improved[0]["previous"], 0);
        assert_eq!(improved[0]["value"], 100);
    }
}
