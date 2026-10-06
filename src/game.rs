use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const SESSION_GAP_MS: i64 = 300_000;
const MATURE_IVL: i64 = 21;
pub const MAX_FREEZES: u32 = 3;
const QUEST_XP: u64 = 50;
const ALL_QUESTS_XP: u64 = 100;
const RECENT_DAYS: usize = 14;
/// Under diminishing weighting, each further answer of the same card on the same
/// day is worth this much of the last one, so repeating a card you keep failing
/// cannot out-earn learning it once.
const REPEAT_DECAY: f64 = 0.5;
/// How close something has to be before a nudge mentions it, in XP.
const WITHIN_REACH: u64 = 150;
const NUDGE_FROM_HOUR: i64 = 9;
const NUDGE_UNTIL_HOUR: i64 = 22;
const CHEERS: [&str; 5] = [
    "Keep going",
    "You're doing great",
    "Almost there",
    "Nearly there",
    "One more push",
];
const HEATMAP_DAYS: i64 = 182;

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Review {
    pub id: i64,
    pub cid: i64,
    pub last_ivl: i64,
    /// The interval the answer gave the card: days, or negative seconds for a
    /// learning step. Clients that predate it leave it out.
    #[serde(default)]
    pub ivl: Option<i64>,
    pub time_ms: i64,
    pub kind: u8,
}

/// Preference changes ordered by time, retaining save order for equal timestamps.
/// Replaying them with reviews keeps later toggles from changing past protection.
#[derive(Clone, Debug, PartialEq)]
pub struct FreezePreference {
    pub at: i64,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default)]
pub struct FreezePolicy {
    /// First migration time for an existing database; earlier Anki days retain
    /// their old automatic protection. New databases have no legacy period.
    pub legacy_until: Option<i64>,
    /// Saved rollout time: changing the default must not rewrite earlier rewards.
    /// An unrestricted default policy enables protection throughout its history.
    pub default_enabled_from: Option<i64>,
    pub preferences: Vec<FreezePreference>,
}

impl FreezePolicy {
    pub fn enabled_at(&self, at: i64) -> bool {
        let end = self.preferences.partition_point(|change| change.at <= at);
        if end > 0 {
            self.preferences[end - 1].enabled
        } else {
            self.default_enabled_from.is_none_or(|start| at >= start)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Clock {
    pub offset_west_min: i64,
    pub rollover_hour: i64,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            offset_west_min: 0,
            rollover_hour: 4,
        }
    }
}

impl Clock {
    fn local_secs(&self, ms: i64) -> i64 {
        ms.div_euclid(1000) - self.offset_west_min * 60
    }

    pub fn day(&self, ms: i64) -> i64 {
        (self.local_secs(ms) - self.rollover_hour * 3600).div_euclid(86400)
    }

    pub fn hour(&self, ms: i64) -> i64 {
        self.local_secs(ms).rem_euclid(86400) / 3600
    }

    fn day_start_ms(&self, day: i64) -> i64 {
        (day * 86400 + self.rollover_hour * 3600 + self.offset_west_min * 60) * 1000
    }
}

/// The leaderboard week shared by every player: Monday to Sunday in one time zone,
/// turning over at the same hour for everyone.
#[derive(Clone, Copy, Debug)]
pub struct Week {
    pub tz: Tz,
    pub rollover_hour: u32,
}

impl Default for Week {
    fn default() -> Self {
        Self {
            tz: Tz::UTC,
            rollover_hour: 4,
        }
    }
}

impl Week {
    /// The common competition date, independent of any player's Anki clock.
    pub fn competition_date(&self, ms: i64) -> NaiveDate {
        self.shifted(ms).date()
    }

    /// Resolve a calendar rollover in the configured time zone. On a spring
    /// daylight-saving gap, use the first valid local minute after the gap;
    /// on a repeated fall-back hour, use its first occurrence.
    pub fn boundary_ms(&self, date: NaiveDate) -> i64 {
        let local = date.and_hms_opt(self.rollover_hour.min(23), 0, 0).unwrap();
        for minute in 0..=180 {
            if let Some(at) = self
                .tz
                .from_local_datetime(&(local + Duration::minutes(minute)))
                .earliest()
            {
                return at.timestamp_millis();
            }
        }
        local.and_utc().timestamp_millis()
    }

    fn shifted(&self, ms: i64) -> NaiveDateTime {
        DateTime::from_timestamp_millis(ms)
            .unwrap_or_default()
            .with_timezone(&self.tz)
            .naive_local()
            - Duration::hours(i64::from(self.rollover_hour))
    }

    fn of(&self, ms: i64) -> (i32, u32) {
        let week = self.shifted(ms).date().iso_week();
        (week.year(), week.week())
    }

    fn month_of(&self, ms: i64) -> (i32, u32) {
        let date = self.shifted(ms).date();
        (date.year(), date.month())
    }

    fn year_of(&self, ms: i64) -> i32 {
        self.shifted(ms).date().year()
    }

    /// When the week containing `ms` ends, in milliseconds since the epoch.
    pub fn end_after(&self, ms: i64) -> i64 {
        let date = self.shifted(ms).date();
        let monday = date + Duration::days(7 - i64::from(date.weekday().num_days_from_monday()));
        let boundary = monday
            .and_hms_opt(self.rollover_hour, 0, 0)
            .unwrap_or_default();
        self.tz
            .from_local_datetime(&boundary)
            .earliest()
            .map_or(ms + 7 * 86_400_000, |t| t.timestamp_millis())
    }
}

#[derive(Clone, Default, Debug)]
struct DayStats {
    reviews: u64,
    new_cards: u64,
    mature: u64,
    time_ms: i64,
    max_combo: u64,
    sessions: u64,
    before_noon: u64,
    early: bool,
    late: bool,
    review_xp: f64,
    relearns: u64,
    max_ivl: i64,
    first_seen: u64,
    longest_session_ms: i64,
    quests_completed_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuestKind {
    Reviews,
    Minutes,
    Combo,
    Mature,
    NewCards,
    Sessions,
    BeforeNoon,
}

#[derive(Clone, Copy, Debug)]
struct Quest {
    kind: QuestKind,
    target: u64,
}

impl Quest {
    fn progress(&self, s: &DayStats) -> u64 {
        match self.kind {
            QuestKind::Reviews => s.reviews,
            QuestKind::Minutes => (s.time_ms / 60_000) as u64,
            QuestKind::Combo => s.max_combo,
            QuestKind::Mature => s.mature,
            QuestKind::NewCards => s.new_cards,
            QuestKind::Sessions => s.sessions,
            QuestKind::BeforeNoon => s.before_noon,
        }
    }

    fn title(&self) -> String {
        let n = self.target;
        match self.kind {
            QuestKind::Reviews => format!("Review {n} cards"),
            QuestKind::Minutes => format!("Study for {n} minutes"),
            QuestKind::Combo => format!("Reach a {n} card combo"),
            QuestKind::Mature => format!("Review {n} mature cards"),
            QuestKind::NewCards => format!("Learn {n} new cards"),
            QuestKind::Sessions => format!("Study in {n} separate sessions"),
            QuestKind::BeforeNoon => format!("Review {n} cards before noon"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Metric {
    Reviews,
    Streak,
    DayReviews,
    Combo,
    Hours,
    Mature,
    NewCards,
    Quests,
    PerfectDays,
    EarlyDays,
    LateDays,
    DaysActive,
    Relearns,
    MaxInterval,
    DistinctCards,
    LongestSession,
    MaxSessions,
    BestMonth,
    WeekendDays,
    Comebacks,
    FreezesUsed,
    FastDays,
    NewYearDays,
    Anniversaries,
    Years,
}

struct Def {
    metric: Metric,
    threshold: u64,
    title: &'static str,
}

const fn def(metric: Metric, threshold: u64, title: &'static str) -> Def {
    Def {
        metric,
        threshold,
        title,
    }
}

const ACHIEVEMENTS: &[Def] = &[
    def(Metric::Reviews, 100, "First Hundred"),
    def(Metric::Reviews, 1_000, "Thousand Flips"),
    def(Metric::Reviews, 5_000, "Card Shark"),
    def(Metric::Reviews, 10_000, "Ten Thousand Hours, Sort Of"),
    def(Metric::Reviews, 25_000, "Memory Palace"),
    def(Metric::Reviews, 50_000, "Living Library"),
    def(Metric::Reviews, 100_000, "Mnemosyne"),
    def(Metric::Reviews, 250_000, "Quarter Million"),
    def(Metric::Reviews, 500_000, "Half a Million"),
    def(Metric::Reviews, 1_000_000, "The Millionaire"),
    def(Metric::Streak, 3, "Warming Up"),
    def(Metric::Streak, 7, "Full Week"),
    def(Metric::Streak, 14, "Fortnight"),
    def(Metric::Streak, 30, "Month of Mondays"),
    def(Metric::Streak, 60, "Habit Formed"),
    def(Metric::Streak, 100, "Centurion"),
    def(Metric::Streak, 200, "Unbreakable"),
    def(Metric::Streak, 365, "Orbit Complete"),
    def(Metric::Streak, 500, "Five Hundred Sunrises"),
    def(Metric::Streak, 730, "Two Orbits"),
    def(Metric::Streak, 1_000, "Thousand Days"),
    def(Metric::Streak, 1_500, "Unstoppable"),
    def(Metric::Streak, 2_000, "Perennial"),
    def(Metric::DayReviews, 100, "Busy Day"),
    def(Metric::DayReviews, 250, "Marathon"),
    def(Metric::DayReviews, 500, "Ultramarathon"),
    def(Metric::DayReviews, 750, "Iron Lung"),
    def(Metric::DayReviews, 1_000, "Thousand in a Day"),
    def(Metric::DayReviews, 1_500, "Grinder"),
    def(Metric::DayReviews, 2_000, "Madness"),
    def(Metric::Combo, 50, "In the Zone"),
    def(Metric::Combo, 100, "Flow State"),
    def(Metric::Combo, 250, "Trance"),
    def(Metric::Combo, 500, "Zen"),
    def(Metric::Combo, 1_000, "Nirvana"),
    def(Metric::Hours, 10, "Ten Hours In"),
    def(Metric::Hours, 50, "Fifty Hours In"),
    def(Metric::Hours, 100, "Hundred Hours In"),
    def(Metric::Hours, 500, "Scholar"),
    def(Metric::Hours, 1_000, "Thousand Hours"),
    def(Metric::Hours, 2_000, "Two Thousand Hours"),
    def(Metric::Hours, 5_000, "Lifer"),
    def(Metric::Mature, 100, "Long Term"),
    def(Metric::Mature, 1_000, "Deep Roots"),
    def(Metric::Mature, 10_000, "Old Growth"),
    def(Metric::Mature, 25_000, "Ancient Forest"),
    def(Metric::Mature, 50_000, "Redwood"),
    def(Metric::Mature, 100_000, "Bedrock"),
    def(Metric::NewCards, 100, "Collector"),
    def(Metric::NewCards, 1_000, "Curator"),
    def(Metric::NewCards, 5_000, "Archivist"),
    def(Metric::NewCards, 10_000, "Encyclopedist"),
    def(Metric::NewCards, 25_000, "Lexicographer"),
    def(Metric::NewCards, 50_000, "Polymath"),
    def(Metric::Quests, 10, "Adventurer"),
    def(Metric::Quests, 50, "Quest Hound"),
    def(Metric::Quests, 200, "Completionist"),
    def(Metric::Quests, 500, "Quest Master"),
    def(Metric::Quests, 1_000, "Legend"),
    def(Metric::Quests, 2_500, "Mythic"),
    def(Metric::PerfectDays, 7, "Perfect Week"),
    def(Metric::PerfectDays, 30, "Perfect Month"),
    def(Metric::PerfectDays, 100, "Perfectionist"),
    def(Metric::PerfectDays, 365, "Flawless Year"),
    def(Metric::EarlyDays, 5, "Early Bird"),
    def(Metric::EarlyDays, 30, "Dawn Patrol"),
    def(Metric::EarlyDays, 100, "Sunrise Scholar"),
    def(Metric::LateDays, 5, "Night Owl"),
    def(Metric::LateDays, 30, "Nocturnal"),
    def(Metric::LateDays, 100, "Creature of the Night"),
    def(Metric::DaysActive, 30, "Regular"),
    def(Metric::DaysActive, 100, "Hundred Days"),
    def(Metric::DaysActive, 365, "A Year of Days"),
    def(Metric::DaysActive, 730, "Two Years of Days"),
    def(Metric::DaysActive, 1_000, "Thousand Days Studied"),
    def(Metric::DaysActive, 2_000, "Two Thousand Days Studied"),
    def(Metric::Relearns, 100, "Second Chance"),
    def(Metric::Relearns, 1_000, "Persistence"),
    def(Metric::Relearns, 10_000, "Never Give Up"),
    def(Metric::Relearns, 50_000, "Sisyphus"),
    def(Metric::MaxInterval, 180, "Half-Year Memory"),
    def(Metric::MaxInterval, 365, "Year-Old Memory"),
    def(Metric::MaxInterval, 1_825, "Five-Year Memory"),
    def(Metric::MaxInterval, 3_650, "Decade Memory"),
    def(Metric::DistinctCards, 1_000, "Wide Net"),
    def(Metric::DistinctCards, 5_000, "Big Deck"),
    def(Metric::DistinctCards, 20_000, "Vast Library"),
    def(Metric::DistinctCards, 50_000, "Everything Everywhere"),
    def(Metric::LongestSession, 60, "Deep Work"),
    def(Metric::LongestSession, 120, "Two-Hour Sitting"),
    def(Metric::LongestSession, 240, "Iron Chair"),
    def(Metric::MaxSessions, 5, "Snacker"),
    def(Metric::MaxSessions, 10, "Grazer"),
    def(Metric::MaxSessions, 20, "Can't Stop"),
    def(Metric::BestMonth, 1_000, "Busy Month"),
    def(Metric::BestMonth, 3_000, "Big Month"),
    def(Metric::BestMonth, 10_000, "Monster Month"),
    def(Metric::BestMonth, 20_000, "Month of Madness"),
    def(Metric::WeekendDays, 10, "Weekend Warrior"),
    def(Metric::WeekendDays, 52, "Weekend Regular"),
    def(Metric::WeekendDays, 200, "No Days Off"),
    def(Metric::Comebacks, 1, "Comeback"),
    def(Metric::Comebacks, 3, "Phoenix"),
    def(Metric::FreezesUsed, 1, "Saved by the Ice"),
    def(Metric::FreezesUsed, 10, "Glacier"),
    def(Metric::FreezesUsed, 50, "Ice Age"),
    def(Metric::FastDays, 1, "Speed Demon"),
    def(Metric::FastDays, 10, "Lightning"),
    def(Metric::FastDays, 50, "Quicksilver"),
    def(Metric::NewYearDays, 1, "New Year, Same Me"),
    def(Metric::NewYearDays, 3, "Tradition"),
    def(Metric::Anniversaries, 1, "Anniversary"),
    def(Metric::Anniversaries, 5, "Old Friends"),
    def(Metric::Anniversaries, 10, "Lifelong"),
    def(Metric::Years, 1, "One Year In"),
    def(Metric::Years, 3, "Three Years In"),
    def(Metric::Years, 5, "Veteran"),
    def(Metric::Years, 10, "Decade of Anki"),
];

#[derive(Default)]
struct Totals {
    reviews: u64,
    best_streak: u64,
    best_day: u64,
    best_combo: u64,
    time_ms: i64,
    mature: u64,
    new_cards: u64,
    quests: u64,
    perfect_days: u64,
    early_days: u64,
    late_days: u64,
    days_active: u64,
    relearns: u64,
    max_ivl: u64,
    distinct_cards: u64,
    longest_session_ms: i64,
    max_sessions: u64,
    best_month: u64,
    weekend_days: u64,
    comebacks: u64,
    freezes_used: u64,
    fast_days: u64,
    new_year_days: u64,
    anniversaries: u64,
    years: u64,
}

impl Totals {
    fn value(&self, metric: Metric) -> u64 {
        match metric {
            Metric::Reviews => self.reviews,
            Metric::Streak => self.best_streak,
            Metric::DayReviews => self.best_day,
            Metric::Combo => self.best_combo,
            Metric::Hours => (self.time_ms / 3_600_000) as u64,
            Metric::Mature => self.mature,
            Metric::NewCards => self.new_cards,
            Metric::Quests => self.quests,
            Metric::PerfectDays => self.perfect_days,
            Metric::EarlyDays => self.early_days,
            Metric::LateDays => self.late_days,
            Metric::DaysActive => self.days_active,
            Metric::Relearns => self.relearns,
            Metric::MaxInterval => self.max_ivl,
            Metric::DistinctCards => self.distinct_cards,
            Metric::LongestSession => (self.longest_session_ms / 60_000) as u64,
            Metric::MaxSessions => self.max_sessions,
            Metric::BestMonth => self.best_month,
            Metric::WeekendDays => self.weekend_days,
            Metric::Comebacks => self.comebacks,
            Metric::FreezesUsed => self.freezes_used,
            Metric::FastDays => self.fast_days,
            Metric::NewYearDays => self.new_year_days,
            Metric::Anniversaries => self.anniversaries,
            Metric::Years => self.years,
        }
    }
}

fn describe(metric: Metric, n: u64) -> String {
    match metric {
        Metric::Reviews => format!("Review {n} cards in total"),
        Metric::Streak => format!("Reach a {n} day streak"),
        Metric::DayReviews => format!("Review {n} cards in one day"),
        Metric::Combo => format!("Reach a {n} card combo"),
        Metric::Hours => format!("Study for {n} hours in total"),
        Metric::Mature => format!("Review {n} mature cards"),
        Metric::NewCards => format!("Learn {n} new cards"),
        Metric::Quests => format!("Complete {n} quests"),
        Metric::PerfectDays => format!("Complete every quest on {n} days"),
        Metric::EarlyDays => format!("Study before 7am on {n} days"),
        Metric::LateDays => format!("Study after 11pm on {n} days"),
        Metric::DaysActive => format!("Study on {n} different days"),
        Metric::Relearns => format!("Relearn {n} forgotten cards"),
        Metric::MaxInterval => format!("Recall a card last seen {n} days before"),
        Metric::DistinctCards => format!("Review {n} different cards"),
        Metric::LongestSession => format!("Study {n} minutes in one sitting"),
        Metric::MaxSessions => format!("Study in {n} separate sessions in one day"),
        Metric::BestMonth => format!("Review {n} cards in one calendar month"),
        Metric::WeekendDays => format!("Study on {n} Saturdays or Sundays"),
        Metric::Comebacks => match n {
            1 => "Come back after a month away".into(),
            _ => format!("Come back after a month away {n} times"),
        },
        Metric::FreezesUsed => format!("Have a streak freeze save you {n} times"),
        Metric::FastDays => {
            format!("Review 100+ cards in a day averaging under 5 seconds, {n} times")
        }
        Metric::NewYearDays => format!("Study on New Year's Day {n} times"),
        Metric::Anniversaries => format!("Study on the anniversary of your first review {n} times"),
        Metric::Years => format!("Keep going for {n} years since your first review"),
    }
}

fn achievement_xp(metric: Metric, threshold: u64) -> u64 {
    let tier = ACHIEVEMENTS
        .iter()
        .filter(|d| d.metric as u8 == metric as u8 && d.threshold <= threshold)
        .count() as u64;
    100 * tier
}

#[derive(Serialize, Clone, Debug)]
pub struct QuestView {
    pub title: String,
    pub progress: u64,
    pub target: u64,
    pub done: bool,
    pub reward: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct AchievementView {
    pub id: String,
    pub title: String,
    pub description: String,
    pub reward: u64,
    pub progress: u64,
    pub target: u64,
    pub unlocked: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct HeatCell {
    pub date: String,
    pub reviews: u64,
    pub xp: u64,
    pub frozen: bool,
}

/// One encouraging message, with the key that keeps it to once a day.
#[derive(Debug, Clone, PartialEq)]
pub struct Nudge {
    pub key: String,
    pub title: String,
    pub body: String,
}

/// What is within reach right now: a place on the board, a personal best, a level.
/// Silent outside waking hours and on days without a single review, so nothing
/// here ever tells someone to start studying.
pub fn nudges(profile: &Profile, ahead: Option<(&str, u64)>) -> Vec<Nudge> {
    if profile.today.reviews == 0
        || profile.local_hour < NUDGE_FROM_HOUR
        || profile.local_hour >= NUDGE_UNTIL_HOUR
    {
        return Vec::new();
    }
    let cheer = |offset: usize| {
        CHEERS[(seed_of(&profile.user) as usize + profile.day as usize + offset) % CHEERS.len()]
    };
    let per_review = (profile.today.xp as f64 / profile.today.reviews as f64).max(1.0);
    let reviews_for = |xp: u64| (xp as f64 / per_review).ceil() as u64;
    let mut nudges = Vec::new();

    if let Some((who, their_xp)) = ahead {
        let gap = their_xp.saturating_sub(profile.week_xp);
        if (1..=WITHIN_REACH).contains(&gap) {
            nudges.push(Nudge {
                key: format!("nudge:place:{}", profile.day),
                title: cheer(0).into(),
                body: format!(
                    "{gap} XP behind {who} this week, about {} more {}.",
                    reviews_for(gap),
                    plural(reviews_for(gap), "review", "reviews")
                ),
            });
        }
    }

    let best = profile.records.day.xp;
    let short_of_best = best.saturating_sub(profile.today.xp);
    if (1..=WITHIN_REACH).contains(&short_of_best) {
        nudges.push(Nudge {
            key: format!("nudge:record:{}", profile.day),
            title: cheer(1).into(),
            body: format!(
                "{short_of_best} XP from your best 24 hours ({best} XP), about {} more {}.",
                reviews_for(short_of_best),
                plural(reviews_for(short_of_best), "review", "reviews")
            ),
        });
    }

    let to_level = profile.xp_for_next.saturating_sub(profile.xp_into_level);
    if (1..=WITHIN_REACH).contains(&to_level) {
        nudges.push(Nudge {
            key: format!("nudge:level:{}", profile.day),
            title: cheer(2).into(),
            body: format!("Level {} is {to_level} XP away.", profile.level + 1),
        });
    }
    nudges
}

fn plural(count: u64, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

/// The best a player has ever managed within one window: from its first to its last review.
#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Record {
    pub xp: u64,
    pub reviews: u64,
    pub at: i64,
    pub until: i64,
}

/// Personal bests over rolling windows: 1 hour, 24 hours, 7, 30 and 365 days.
#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Records {
    pub hour: Record,
    pub day: Record,
    pub week: Record,
    pub month: Record,
    pub year: Record,
}

impl Records {
    pub const NAMES: [&'static str; 5] = ["hour", "day", "week", "month", "year"];
    pub const WINDOWS_MS: [i64; 5] = [
        3_600_000,
        86_400_000,
        7 * 86_400_000,
        30 * 86_400_000,
        365 * 86_400_000,
    ];
    pub const TITLES: [&'static str; 5] = [
        "New best hour!",
        "New best 24 hours!",
        "New best 7 days!",
        "New best 30 days!",
        "New best 365 days!",
    ];
    pub const SPANS: [&'static str; 5] = ["one hour", "24 hours", "7 days", "30 days", "365 days"];

    pub fn get(&self, window: &str) -> Record {
        match window {
            "hour" => self.hour,
            "week" => self.week,
            "month" => self.month,
            "year" => self.year,
            _ => self.day,
        }
    }
}

/// The best rolling window of `window_ms` over time-ordered `(at, xp, reviews)` entries.
fn best_window(entries: &[(i64, f64, u64)], window_ms: i64) -> Record {
    let mut best = Record::default();
    let mut xp = 0.0;
    let mut reviews = 0;
    let mut start = 0;
    for (at, gained, count) in entries {
        xp += gained;
        reviews += count;
        while entries[start].0 <= at - window_ms {
            xp -= entries[start].1;
            reviews -= entries[start].2;
            start += 1;
        }
        let rounded = xp.round() as u64;
        if rounded > best.xp {
            best = Record {
                xp: rounded,
                reviews,
                at: entries[start].0,
                until: *at,
            };
        }
    }
    best
}

/// Reviews as they were earned, plus each day's bonus XP (quests, achievements,
/// streak) at that day's last review, so rolling windows add up to the days they cover.
fn timeline(
    earned: &[(i64, f64)],
    day_xp: &BTreeMap<i64, u64>,
    clock: &Clock,
) -> Vec<(i64, f64, u64)> {
    let mut entries = Vec::with_capacity(earned.len() + day_xp.len());
    let mut index = 0;
    while index < earned.len() {
        let day = clock.day(earned[index].0);
        let mut reviewed = 0.0;
        while index < earned.len() && clock.day(earned[index].0) == day {
            entries.push((earned[index].0, earned[index].1, 1));
            reviewed += earned[index].1;
            index += 1;
        }
        let bonus = day_xp.get(&day).map_or(0.0, |xp| *xp as f64) - reviewed.round();
        if bonus > 0.0 {
            entries.push((earned[index - 1].0, bonus, 0));
        }
    }
    entries
}

/// Streak lengths worth a message of their own, beyond any streak achievement.
fn streak_milestone(streak: u64) -> bool {
    matches!(streak, 7 | 30 | 50 | 100 | 200 | 365 | 500)
        || (streak > 365 && streak.is_multiple_of(365))
}

/// XP earned in each leaderboard period. The hour counts review XP only, without
/// the quest, achievement and streak bonuses that land on a whole Anki day.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Periods {
    pub hour: u64,
    pub day: u64,
    pub week: u64,
    pub month: u64,
    pub year: u64,
    pub all: u64,
}

impl Periods {
    pub const NAMES: [&'static str; 6] = ["hour", "day", "week", "month", "year", "all"];

    pub fn get(&self, period: &str) -> u64 {
        match period {
            "hour" => self.hour,
            "day" => self.day,
            "month" => self.month,
            "year" => self.year,
            "all" => self.all,
            _ => self.week,
        }
    }
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Today {
    pub reviews: u64,
    pub minutes: u64,
    pub xp: u64,
    pub max_combo: u64,
    pub current_combo: u64,
    pub new_cards: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Lifetime {
    pub reviews: u64,
    pub hours: u64,
    pub days_active: u64,
    pub best_streak: u64,
    /// When the longest streak ran out, and the first day ever studied.
    pub best_streak_at: i64,
    pub first_day_at: i64,
    pub best_day: u64,
    pub best_combo: u64,
    pub quests: u64,
    /// Hard cards learned at last; see [`conquests`].
    pub conquered: u64,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub key: String,
    pub title: String,
    pub body: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct HistoryDay {
    /// UTC start of the player's Anki day. Competition periods use this instant,
    /// exactly as the existing weekly/monthly leaderboard does.
    pub at: i64,
    pub date: String,
    pub xp: u64,
    pub reviews: u64,
    pub time_ms: i64,
    pub new_cards: u64,
    pub streak: u64,
    pub frozen: bool,
}

#[derive(Serialize, Clone, Copy, Debug)]
#[serde(rename_all = "snake_case")]
pub enum StreakState {
    Studied,
    Pending,
    Protected,
}

#[derive(Serialize, Clone, Debug)]
pub struct Profile {
    pub user: String,
    pub display: String,
    pub level: u64,
    pub xp_total: u64,
    pub xp_into_level: u64,
    pub xp_for_next: u64,
    pub week_xp: u64,
    pub periods: Periods,
    pub records: Records,
    pub streak: u64,
    pub streak_state: StreakState,
    pub freezes: u32,
    pub stored_freezes: u32,
    pub freezes_enabled: bool,
    pub freeze_earned_today: bool,
    pub at_risk: bool,
    pub day_ends_at: i64,
    pub streak_warning: Option<crate::feedback::Notice>,
    pub local_hour: i64,
    pub today: Today,
    pub lifetime: Lifetime,
    pub quests: Vec<QuestView>,
    pub achievements: Vec<AchievementView>,
    pub heatmap: Vec<HeatCell>,
    /// The latest hard cards learned at last, newest first.
    pub conquests: Vec<Conquest>,
    pub last_review_id: i64,
    #[serde(skip)]
    pub day: i64,
    #[serde(skip)]
    pub events: Vec<Event>,
    #[serde(skip)]
    pub history: Vec<HistoryDay>,
}

pub fn level_need(level: u64) -> u64 {
    ((level as f64).powf(1.5) * 10.0).round() as u64 * 10
}

pub fn level_for(xp: u64) -> (u64, u64, u64) {
    let mut level = 1;
    let mut rest = xp;
    loop {
        let need = level_need(level);
        if rest < need {
            return (level, rest, need);
        }
        rest -= need;
        level += 1;
    }
}

pub fn date_string(day: i64) -> String {
    let (y, m, d) = civil(day);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil(day: i64) -> (i64, i64, i64) {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn is_weekend(day: i64) -> bool {
    (day + 3).rem_euclid(7) >= 5
}

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn seed_of(user: &str) -> u64 {
    user.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn round_to(x: f64, step: u64) -> u64 {
    ((x / step as f64).round() as u64) * step
}

fn gen_quests(seed: u64, day: i64, recent: &VecDeque<DayStats>) -> Vec<Quest> {
    let n = recent.len().max(1) as f64;
    let avg = |f: fn(&DayStats) -> f64| recent.iter().map(f).sum::<f64>() / n;
    let reviews = avg(|s| s.reviews as f64).max(20.0);
    let minutes = avg(|s| s.time_ms as f64 / 60_000.0).max(5.0);
    let mature = avg(|s| s.mature as f64);
    let new_cards = avg(|s| s.new_cards as f64);

    let mut rng = mix(seed ^ day as u64);
    let mut next = move || {
        rng = mix(rng);
        rng
    };
    let factor = [0.8, 1.0, 1.2][(next() % 3) as usize];
    let review_target = round_to(reviews * factor, 5).max(10);

    let mut pool = vec![
        Quest {
            kind: QuestKind::Minutes,
            target: (minutes * factor).round().max(5.0) as u64,
        },
        Quest {
            kind: QuestKind::Combo,
            target: [20, 30, 50][(next() % 3) as usize].min(review_target),
        },
        Quest {
            kind: QuestKind::Sessions,
            target: 2,
        },
        Quest {
            kind: QuestKind::BeforeNoon,
            target: round_to(reviews * 0.3, 5).max(5),
        },
    ];
    if mature >= 5.0 {
        pool.push(Quest {
            kind: QuestKind::Mature,
            target: round_to(mature * 0.8, 5).max(5),
        });
    }
    if new_cards >= 1.0 {
        pool.push(Quest {
            kind: QuestKind::NewCards,
            target: (new_cards.round() as u64).max(3),
        });
    }

    let mut quests = vec![Quest {
        kind: QuestKind::Reviews,
        target: review_target,
    }];
    for _ in 0..2 {
        let i = (next() % pool.len() as u64) as usize;
        quests.push(pool.swap_remove(i));
    }
    quests
}

/// How review XP is weighted, chosen once per server with `review_weighting`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewWeighting {
    /// Every answer earns its full type rate; learning and relearning steps are 6 XP.
    #[default]
    Flat,
    /// Learning steps are 9 XP, and answering the same card again on the same day
    /// is worth half as much each time.
    Diminishing,
}

static DIMINISHING: AtomicBool = AtomicBool::new(false);

/// Sets the server's review weighting. XP is computed from history on demand,
/// so every profile follows a change from the next request on.
pub fn set_review_weighting(weighting: ReviewWeighting) {
    DIMINISHING.store(weighting == ReviewWeighting::Diminishing, Ordering::Relaxed);
}

pub fn review_weighting() -> ReviewWeighting {
    if DIMINISHING.load(Ordering::Relaxed) {
        ReviewWeighting::Diminishing
    } else {
        ReviewWeighting::Flat
    }
}

#[cfg(test)]
fn review_base_xp(r: &Review) -> f64 {
    review_base_xp_with(r, ReviewWeighting::Flat)
}

fn review_base_xp_with(r: &Review, weighting: ReviewWeighting) -> f64 {
    let base = match (r.kind, weighting) {
        (1, _) => 10.0,
        (0, ReviewWeighting::Diminishing) => 9.0,
        (0 | 2, _) => 6.0,
        _ => 2.0,
    };
    if r.last_ivl >= MATURE_IVL {
        base + 5.0
    } else {
        base
    }
}

#[cfg(test)]
fn collect_days(
    reviews: &[Review],
    clock: &Clock,
) -> (BTreeMap<i64, DayStats>, u64, Vec<(i64, f64)>) {
    collect_days_weighted(reviews, clock, 0, ReviewWeighting::Flat)
}

fn collect_days_with_quests(
    reviews: &[Review],
    clock: &Clock,
    seed: u64,
) -> (BTreeMap<i64, DayStats>, u64, Vec<(i64, f64)>) {
    collect_days_weighted(reviews, clock, seed, review_weighting())
}

fn collect_days_weighted(
    reviews: &[Review],
    clock: &Clock,
    seed: u64,
    weighting: ReviewWeighting,
) -> (BTreeMap<i64, DayStats>, u64, Vec<(i64, f64)>) {
    let mut days: BTreeMap<i64, DayStats> = BTreeMap::new();
    let mut earned: Vec<(i64, f64)> = Vec::new();
    let mut seen = HashSet::new();
    let mut combo = 0u64;
    let mut session_ms = 0i64;
    let mut prev: Option<(i64, i64)> = None;
    let mut counted_day = None;
    let mut recent = VecDeque::new();
    let mut quests = Vec::new();
    let mut answers: HashMap<i64, i32> = HashMap::new();
    for r in reviews {
        let day = clock.day(r.id);
        let hour = clock.hour(r.id);
        if counted_day != Some(day) {
            answers.clear();
            if let Some(previous) = counted_day.and_then(|d| days.get(&d)) {
                recent.push_back(previous.clone());
                if recent.len() > RECENT_DAYS {
                    recent.pop_front();
                }
            }
            quests = gen_quests(seed, day, &recent);
            counted_day = Some(day);
        }
        let s = days.entry(day).or_default();
        let continues = prev.is_some_and(|(d, id)| d == day && r.id - id < SESSION_GAP_MS);
        if continues {
            combo += 1;
            session_ms += r.time_ms;
        } else {
            combo = 1;
            session_ms = r.time_ms;
            s.sessions += 1;
        }
        prev = Some((day, r.id));
        s.reviews += 1;
        s.time_ms += r.time_ms;
        s.max_combo = s.max_combo.max(combo);
        s.longest_session_ms = s.longest_session_ms.max(session_ms);
        s.max_ivl = s.max_ivl.max(r.last_ivl);
        if r.kind == 2 {
            s.relearns += 1;
        }
        if seen.insert(r.cid) {
            s.first_seen += 1;
            if r.kind == 0 {
                s.new_cards += 1;
            }
        }
        if r.last_ivl >= MATURE_IVL {
            s.mature += 1;
        }
        let morning = hour >= clock.rollover_hour;
        if morning && hour < 12 {
            s.before_noon += 1;
        }
        if morning && hour < 7 {
            s.early = true;
        }
        if hour >= 23 || !morning {
            s.late = true;
        }
        let decay = match weighting {
            ReviewWeighting::Flat => 1.0,
            ReviewWeighting::Diminishing => {
                let again = answers.entry(r.cid).or_insert(0);
                *again += 1;
                REPEAT_DECAY.powi(*again - 1)
            }
        };
        let xp = review_base_xp_with(r, weighting) * decay * (1.0 + combo.min(100) as f64 / 200.0);
        s.review_xp += xp;
        earned.push((r.id, xp));
        if s.quests_completed_at.is_none() && quests.iter().all(|q| q.progress(s) >= q.target) {
            s.quests_completed_at = Some(r.id);
        }
    }
    (days, combo, earned)
}

#[cfg(test)]
pub fn compute(
    user: &str,
    display: &str,
    reviews: &[Review],
    clock: &Clock,
    week: &Week,
    now_ms: i64,
) -> Profile {
    compute_with_freezes(
        user,
        display,
        reviews,
        clock,
        week,
        now_ms,
        &FreezePolicy::default(),
    )
}

pub fn compute_with_freezes(
    user: &str,
    display: &str,
    reviews: &[Review],
    clock: &Clock,
    week: &Week,
    now_ms: i64,
    freeze_policy: &FreezePolicy,
) -> Profile {
    let (days, last_combo, earned) = collect_days_with_quests(reviews, clock, seed_of(user));
    let today = clock.day(now_ms);
    let legacy_until = freeze_policy.legacy_until.map(|at| clock.day(at));
    let seed = seed_of(user);
    let first = days.keys().next().copied().unwrap_or(today);

    let mut totals = Totals::default();
    let mut streak = 0u64;
    let mut best_streak_day = first;
    let mut freezes = 0u32;
    let mut freeze_earned_today = false;
    let mut frozen = BTreeSet::new();
    let mut recent: VecDeque<DayStats> = VecDeque::new();
    let mut unlocked: Vec<Option<i64>> = vec![None; ACHIEVEMENTS.len()];
    let mut day_xp: BTreeMap<i64, u64> = BTreeMap::new();
    let mut xp_total = 0u64;
    let mut today_quests = Vec::new();
    let mut events = Vec::new();
    let mut history = Vec::new();
    let empty = DayStats::default();
    let (first_year, first_month, first_date) = civil(first);
    let mut month = (first_year, first_month);
    let mut month_reviews = 0u64;
    let mut last_active: Option<i64> = None;

    for day in first..=today {
        let legacy = legacy_until.is_some_and(|cutoff| day < cutoff);
        if legacy_until == Some(day) {
            // Preserve previously used protection and its streak/XP history,
            // but begin the quest-earned system with an empty bank.
            freezes = 0;
        }
        let stats = days.get(&day);
        let (year, month_of_year, date) = civil(day);
        if (year, month_of_year) != month {
            month = (year, month_of_year);
            month_reviews = 0;
        }
        totals.years = ((day - first) / 365) as u64;
        if let Some(s) = stats {
            month_reviews += s.reviews;
            totals.best_month = totals.best_month.max(month_reviews);
            totals.relearns += s.relearns;
            totals.max_ivl = totals.max_ivl.max(s.max_ivl.max(0) as u64);
            totals.distinct_cards += s.first_seen;
            totals.longest_session_ms = totals.longest_session_ms.max(s.longest_session_ms);
            totals.max_sessions = totals.max_sessions.max(s.sessions);
            totals.weekend_days += u64::from(is_weekend(day));
            totals.fast_days += u64::from(s.reviews >= 100 && s.time_ms < s.reviews as i64 * 5_000);
            totals.new_year_days += u64::from(month_of_year == 1 && date == 1);
            totals.anniversaries +=
                u64::from(year > first_year && month_of_year == first_month && date == first_date);
            if last_active.is_some_and(|last| day - last >= 30) {
                totals.comebacks += 1;
            }
            last_active = Some(day);
        }
        if stats.is_some() {
            streak += 1;
            if streak > totals.best_streak {
                totals.best_streak = streak;
                best_streak_day = day;
            }
            if legacy && streak.is_multiple_of(7) && freezes < 2 {
                freezes += 1;
            }
        } else if day != today {
            if streak > 0
                && freezes > 0
                && (legacy || freeze_policy.enabled_at(clock.day_start_ms(day + 1) - 1))
            {
                freezes -= 1;
                frozen.insert(day);
                totals.freezes_used += 1;
            } else {
                streak = 0;
            }
        }

        let quests = gen_quests(seed, day, &recent);
        let mut xp = 0u64;
        if let Some(s) = stats {
            xp += (s.review_xp * (1.0 + streak.min(30) as f64 / 100.0)).round() as u64;
            let done = quests.iter().filter(|q| q.progress(s) >= q.target).count() as u64;
            xp += done * QUEST_XP;
            totals.quests += done;
            if done == quests.len() as u64 {
                xp += ALL_QUESTS_XP;
                totals.perfect_days += 1;
                // The first completion is the only earning opportunity that day,
                // including when protection is off or the bank is already full.
                if !legacy
                    && freezes < MAX_FREEZES
                    && s.quests_completed_at
                        .is_some_and(|at| at <= now_ms && freeze_policy.enabled_at(at))
                {
                    freezes += 1;
                    freeze_earned_today = day == today;
                }
            }
            totals.reviews += s.reviews;
            totals.time_ms += s.time_ms;
            totals.mature += s.mature;
            totals.new_cards += s.new_cards;
            totals.best_day = totals.best_day.max(s.reviews);
            totals.best_combo = totals.best_combo.max(s.max_combo);
            totals.early_days += u64::from(s.early);
            totals.late_days += u64::from(s.late);
            totals.days_active += 1;
            recent.push_back(s.clone());
            if recent.len() > RECENT_DAYS {
                recent.pop_front();
            }
        }
        for (i, d) in ACHIEVEMENTS.iter().enumerate() {
            if unlocked[i].is_none() && totals.value(d.metric) >= d.threshold {
                unlocked[i] = Some(day);
                xp += achievement_xp(d.metric, d.threshold);
            }
        }
        if day == today {
            let s = stats.unwrap_or(&empty);
            for (i, q) in quests.iter().enumerate() {
                let progress = q.progress(s);
                let view = QuestView {
                    title: q.title(),
                    progress: progress.min(q.target),
                    target: q.target,
                    done: progress >= q.target,
                    reward: QUEST_XP,
                };
                if view.done {
                    events.push(Event {
                        key: format!("quest:{day}:{i}"),
                        title: "Quest complete".into(),
                        body: format!("{} (+{QUEST_XP} XP)", view.title),
                    });
                }
                today_quests.push(view);
            }
        }
        xp_total += xp;
        let s = stats.unwrap_or(&empty);
        history.push(HistoryDay {
            at: clock.day_start_ms(day),
            date: date_string(day),
            xp,
            reviews: s.reviews,
            time_ms: s.time_ms,
            new_cards: s.new_cards,
            streak,
            frozen: frozen.contains(&day),
        });
        if xp > 0 {
            day_xp.insert(day, xp);
        }
    }

    let (level, xp_into_level, xp_for_next) = level_for(xp_total);
    if level > 1 {
        events.push(Event {
            key: format!("level:{level}"),
            title: format!("Level {level}"),
            body: format!("You reached level {level} with {xp_total} XP."),
        });
    }

    let achievements = ACHIEVEMENTS
        .iter()
        .zip(&unlocked)
        .map(|(d, at)| {
            let id = format!("{:?}-{}", d.metric, d.threshold).to_lowercase();
            let reward = achievement_xp(d.metric, d.threshold);
            if at.is_some() {
                events.push(Event {
                    key: format!("achievement:{id}"),
                    title: format!("Achievement: {}", d.title),
                    body: format!("{} (+{reward} XP)", describe(d.metric, d.threshold)),
                });
            }
            AchievementView {
                id,
                title: d.title.into(),
                description: describe(d.metric, d.threshold),
                reward,
                progress: totals.value(d.metric).min(d.threshold),
                target: d.threshold,
                unlocked: at.map(date_string),
            }
        })
        .collect();

    let heatmap = (today - HEATMAP_DAYS + 1..=today)
        .map(|day| HeatCell {
            date: date_string(day),
            reviews: days.get(&day).map_or(0, |s| s.reviews),
            xp: day_xp.get(&day).copied().unwrap_or(0),
            frozen: frozen.contains(&day),
        })
        .collect();

    let this_week = week.of(now_ms);
    let this_month = week.month_of(now_ms);
    let this_year = week.year_of(now_ms);
    let in_period = |days_back: i64, matches: &dyn Fn(i64) -> bool| -> u64 {
        day_xp
            .range(today - days_back..=today)
            .filter(|(d, _)| matches(clock.day_start_ms(**d)))
            .map(|(_, xp)| xp)
            .sum()
    };
    let week_xp = in_period(9, &|start| week.of(start) == this_week);
    let periods = Periods {
        hour: earned
            .iter()
            .filter(|(at, _)| *at > now_ms - 3_600_000 && *at <= now_ms)
            .map(|(_, xp)| xp)
            .sum::<f64>()
            .round() as u64,
        day: day_xp.get(&today).copied().unwrap_or(0),
        week: week_xp,
        month: in_period(40, &|start| week.month_of(start) == this_month),
        year: in_period(400, &|start| week.year_of(start) == this_year),
        all: xp_total,
    };

    // The hour counts reviews only: a day's bonus would land in it as one lump.
    let reviewed: Vec<(i64, f64, u64)> = earned.iter().map(|(at, xp)| (*at, *xp, 1)).collect();
    let everything = timeline(&earned, &day_xp, clock);
    let best = |window: usize, entries: &[(i64, f64, u64)]| {
        best_window(entries, Records::WINDOWS_MS[window])
    };
    let records = Records {
        hour: best(0, &reviewed),
        day: best(1, &everything),
        week: best(2, &everything),
        month: best(3, &everything),
        year: best(4, &everything),
    };
    let today_start = clock.day_start_ms(today);
    let studied_today = days.contains_key(&today);
    for window in 0..Records::NAMES.len() {
        let entries = if window == 0 { &reviewed } else { &everything };
        let history_long_enough = entries
            .first()
            .is_some_and(|(at, _, _)| *at <= now_ms - Records::WINDOWS_MS[window]);
        if !studied_today || !history_long_enough {
            continue;
        }
        let before = best(
            window,
            &entries[..entries.partition_point(|(at, _, _)| *at < today_start)],
        );
        let now = records.get(Records::NAMES[window]);
        if before.xp > 0 && now.xp > before.xp && now.until >= today_start {
            events.push(Event {
                key: format!("record:{}:{today}", Records::NAMES[window]),
                title: Records::TITLES[window].into(),
                body: format!(
                    "{} XP in {}, beating your old best of {} XP.",
                    now.xp,
                    Records::SPANS[window],
                    before.xp
                ),
            });
        }
    }
    if studied_today
        && streak_milestone(streak)
        && !ACHIEVEMENTS
            .iter()
            .any(|d| matches!(d.metric, Metric::Streak) && d.threshold == streak)
    {
        events.push(Event {
            key: format!("streak:{}:{streak}", today - streak as i64 + 1),
            title: format!("{streak}-day streak!"),
            body: format!("You have studied {streak} days in a row."),
        });
    }

    let all_conquests = conquests(reviews, now_ms);
    events.extend(
        all_conquests
            .iter()
            .filter(|c| now_ms - c.at <= ANNOUNCE_CONQUESTS_MS)
            .map(conquest_event),
    );
    let conquered = all_conquests.len() as u64;
    let recent_conquests: Vec<Conquest> = all_conquests
        .into_iter()
        .rev()
        .take(RECENT_CONQUESTS)
        .collect();

    let now = days.get(&today).unwrap_or(&empty);
    let at_risk = streak > 0 && !days.contains_key(&today);
    let day_ends_at = clock.day_start_ms(today + 1);
    let effective_freezes = if freeze_policy.enabled_at(now_ms) {
        freezes
    } else {
        0
    };
    let streak_state = if now.reviews > 0 {
        StreakState::Studied
    } else if frozen.contains(&(today - 1)) {
        // Freezes cover ended days. Today's unstudied day is still open.
        StreakState::Protected
    } else {
        StreakState::Pending
    };
    Profile {
        user: user.into(),
        display: display.into(),
        level,
        xp_total,
        xp_into_level,
        xp_for_next,
        week_xp,
        periods,
        records,
        streak,
        streak_state,
        freezes: effective_freezes,
        stored_freezes: freezes,
        freezes_enabled: freeze_policy.enabled_at(now_ms),
        freeze_earned_today,
        at_risk,
        day_ends_at,
        streak_warning: at_risk.then(|| {
            crate::feedback::streak_warning(streak, effective_freezes, day_ends_at, now_ms)
        }),
        local_hour: clock.hour(now_ms),
        today: Today {
            reviews: now.reviews,
            minutes: (now.time_ms / 60_000) as u64,
            xp: day_xp.get(&today).copied().unwrap_or(0),
            max_combo: now.max_combo,
            current_combo: if reviews.last().is_some_and(|r| clock.day(r.id) == today) {
                last_combo
            } else {
                0
            },
            new_cards: now.new_cards,
        },
        lifetime: Lifetime {
            reviews: totals.reviews,
            hours: (totals.time_ms / 3_600_000) as u64,
            days_active: totals.days_active,
            best_streak_at: clock.day_start_ms(best_streak_day),
            first_day_at: clock.day_start_ms(first),
            best_streak: totals.best_streak,
            best_day: totals.best_day,
            best_combo: totals.best_combo,
            quests: totals.quests,
            conquered,
        },
        quests: today_quests,
        achievements,
        heatmap,
        conquests: recent_conquests,
        last_review_id: reviews.last().map_or(0, |r| r.id),
        day: today,
        events,
        history,
    }
}

/// Lapses before a card counts as hard to learn, and before Anki calls it a leech by default.
pub const HARD_LAPSES: u32 = 4;
pub const LEECH_LAPSES: u32 = 8;
const RECENT_CONQUESTS: usize = 12;
/// Older conquests are listed but not announced, so resending a history that now
/// carries intervals does not celebrate cards learned long ago.
const ANNOUNCE_CONQUESTS_MS: i64 = 2 * 86_400_000;
/// Without the new interval, a pass is trusted once no relearning step followed it for this long.
const UNCONFIRMED_PASS_MS: i64 = 86_400_000;

/// A card that lapsed at least [`HARD_LAPSES`] times and has since earned a mature interval.
/// It carries no card id or content: only what can be shared.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Conquest {
    /// The answer that earned the mature interval. It also names the conquest in links.
    pub at: i64,
    pub lapses: u32,
    /// Answers of the card up to and including this one.
    pub answers: u32,
    /// The card's first answer in the retained history.
    pub since: i64,
    pub leech: bool,
}

/// Hard cards learned at last, oldest first.
///
/// A lapse is a review answered into relearning: a negative new interval, or, from
/// clients that do not send it, a relearning step right after the review. A card is
/// conquered when an answer gives it a mature interval after its latest lapse; without
/// the new interval, when a review finds it after a mature interval and it is not failed.
/// Lapses without relearning steps cannot be told from a pass, so they go uncounted.
pub fn conquests(reviews: &[Review], now_ms: i64) -> Vec<Conquest> {
    let mut cards: HashMap<i64, Vec<&Review>> = HashMap::new();
    for r in reviews {
        cards.entry(r.cid).or_default().push(r);
    }
    let mut found = Vec::new();
    for rows in cards.values() {
        let mut lapses = 0u32;
        let mut conquered_at = None;
        for (i, r) in rows.iter().enumerate() {
            let next = rows.get(i + 1);
            let failed = r.kind == 1
                && match r.ivl {
                    Some(ivl) => ivl < 0,
                    None => next.is_some_and(|n| n.kind == 2),
                };
            if failed {
                lapses += 1;
                continue;
            }
            if lapses < HARD_LAPSES || conquered_at == Some(lapses) {
                continue;
            }
            let mature = match r.ivl {
                Some(ivl) => r.kind <= 2 && ivl >= MATURE_IVL,
                None => {
                    r.kind == 1
                        && r.last_ivl >= MATURE_IVL
                        && (next.is_some() || now_ms - r.id >= UNCONFIRMED_PASS_MS)
                }
            };
            if mature {
                conquered_at = Some(lapses);
                found.push(Conquest {
                    at: r.id,
                    lapses,
                    answers: i as u32 + 1,
                    since: rows[0].id,
                    leech: lapses >= LEECH_LAPSES,
                });
            }
        }
    }
    found.sort_by_key(|c| c.at);
    found
}

fn conquest_event(c: &Conquest) -> Event {
    Event {
        key: format!("conquered:{}", c.at),
        title: if c.leech {
            "Leech tamed"
        } else {
            "Card conquered"
        }
        .into(),
        body: format!(
            "A card you forgot {} times now stays with you for weeks.",
            c.lapses
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: i64 = 86_400_000;
    const NOON: i64 = 12 * 3_600_000;

    /// One card's answers a day apart: (kind, last_ivl, ivl).
    fn card(cid: i64, answers: &[(u8, i64, Option<i64>)]) -> Vec<Review> {
        answers
            .iter()
            .enumerate()
            .map(|(i, &(kind, last_ivl, ivl))| Review {
                id: (cid * 1000 + i as i64) * DAY_MS,
                cid,
                last_ivl,
                ivl,
                time_ms: 5000,
                kind,
            })
            .collect()
    }

    /// A review failed into relearning, then the relearning step.
    fn lapse(last_ivl: i64, ivl: Option<i64>) -> [(u8, i64, Option<i64>); 2] {
        [(1, last_ivl, ivl.map(|_| -600)), (2, -600, ivl.map(|_| 1))]
    }

    #[test]
    fn a_card_forgotten_four_times_is_conquered_when_it_earns_a_mature_interval() {
        let mut answers = vec![(0, 0, Some(1))];
        for _ in 0..4 {
            answers.extend(lapse(3, Some(1)));
        }
        answers.extend([(1, 1, Some(8)), (1, 8, Some(25)), (1, 25, Some(60))]);
        let reviews = card(7, &answers);
        let found = conquests(&reviews, i64::MAX / 2);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].lapses, 4);
        assert!(!found[0].leech);
        assert_eq!(found[0].at, reviews[10].id, "the answer that gave 25 days");
        assert_eq!(found[0].answers, 11);
        assert_eq!(found[0].since, reviews[0].id);

        let three: Vec<_> = card(
            8,
            &[lapse(3, Some(1)), lapse(3, Some(1)), lapse(3, Some(1))].concat(),
        )
        .into_iter()
        .chain(card(8, &[(1, 8, Some(25))]).into_iter().map(|r| Review {
            id: r.id + 99 * DAY_MS,
            ..r
        }))
        .collect();
        assert!(
            conquests(&three, i64::MAX / 2).is_empty(),
            "three lapses are not hard enough"
        );
    }

    #[test]
    fn a_leech_is_tamed_and_lapsing_again_lets_it_be_conquered_again() {
        let mut answers = Vec::new();
        for _ in 0..8 {
            answers.extend(lapse(5, Some(1)));
        }
        answers.push((1, 10, Some(30)));
        answers.push((1, 30, Some(70)));
        answers.extend(lapse(70, Some(1)));
        answers.push((1, 10, Some(28)));
        let found = conquests(&card(3, &answers), i64::MAX / 2);
        assert_eq!(
            found
                .iter()
                .map(|c| (c.lapses, c.leech))
                .collect::<Vec<_>>(),
            [(8, true), (9, true)]
        );
    }

    #[test]
    fn older_clients_are_judged_by_relearning_steps_and_the_next_review() {
        let mut answers = Vec::new();
        for _ in 0..4 {
            answers.extend(lapse(3, None));
        }
        // The review that finds a mature interval: unconfirmed until something follows it.
        answers.push((1, 21, None));
        let reviews = card(5, &answers);
        let last = reviews.last().unwrap().id;
        assert!(conquests(&reviews, last + 3_600_000).is_empty());
        assert_eq!(conquests(&reviews, last + DAY_MS).len(), 1);

        // Failed again: the relearning step that follows shows it was not learned.
        let mut failed = answers.clone();
        failed.push((2, -600, None));
        assert!(conquests(&card(5, &failed), i64::MAX / 2).is_empty());
    }

    #[test]
    fn recent_conquests_are_announced_and_listed_newest_first() {
        let mut answers = vec![(0, 0, Some(1))];
        for _ in 0..4 {
            answers.extend(lapse(3, Some(1)));
        }
        answers.push((1, 1, Some(30)));
        let reviews = card(9, &answers);
        let at = reviews.last().unwrap().id;
        let clock = Clock::default();
        let week = Week::default();
        let profile = compute("hill", "Hill", &reviews, &clock, &week, at + 60_000);
        assert_eq!(profile.conquests.len(), 1);
        assert_eq!(profile.lifetime.conquered, 1);
        assert!(
            profile
                .events
                .iter()
                .any(|e| e.key == format!("conquered:{at}"))
        );
        let later = compute("hill", "Hill", &reviews, &clock, &week, at + 3 * DAY_MS);
        assert!(
            !later.events.iter().any(|e| e.key.starts_with("conquered:")),
            "old conquests are listed, not announced"
        );
        assert_eq!(later.conquests.len(), 1);
    }

    fn utc() -> Clock {
        Clock::default()
    }

    fn reviews_on(day: i64, count: i64, cid_base: i64) -> Vec<Review> {
        (0..count)
            .map(|i| Review {
                id: day * DAY_MS + NOON + i * 10_000,
                cid: cid_base + i,
                last_ivl: 0,
                ivl: None,
                time_ms: 5_000,
                kind: 1,
            })
            .collect()
    }

    fn history(days: &[i64]) -> Vec<Review> {
        days.iter()
            .flat_map(|d| reviews_on(*d, 5, d * 1000))
            .collect()
    }

    fn at(day: i64) -> i64 {
        day * DAY_MS + NOON + 3_600_000
    }

    #[test]
    fn streak_status_separates_today_reviews_from_previous_day_protection() {
        let reviews = perfect_day(&[], 1, &utc());
        let changes = [protection(0, true)];
        let studied = protected(&reviews, at(1), &changes);
        let pending = protected(&reviews, at(2), &changes);
        let frozen = protected(&reviews, at(3), &changes);
        for (profile, expected) in [
            (&studied, "studied"),
            (&pending, "pending"),
            (&frozen, "protected"),
        ] {
            assert_eq!(
                serde_json::to_value(profile).unwrap()["streak_state"],
                expected
            );
        }
        assert_eq!(frozen.today.reviews, 0);
        assert_eq!(frozen.streak, pending.streak);
        let mut resumed = reviews;
        resumed.extend(reviews_on(3, 1, 30_000));
        let resumed = protected(&resumed, at(3), &changes);
        assert_eq!(
            serde_json::to_value(resumed).unwrap()["streak_state"],
            "studied"
        );
    }

    #[test]
    fn streak_status_changes_at_the_players_anki_cutoff() {
        let reviews = reviews_on(1, 1, 1000);
        let before = protected(&reviews, at(1), &[]);
        let after = protected(&reviews, before.day_ends_at, &[]);
        assert_eq!(
            serde_json::to_value(&before).unwrap()["streak_state"],
            "studied"
        );
        assert_eq!(
            serde_json::to_value(&after).unwrap()["streak_state"],
            "pending"
        );
        assert_eq!(after.streak, before.streak);
    }

    fn protection(at: i64, enabled: bool) -> FreezePreference {
        FreezePreference { at, enabled }
    }

    fn protected(reviews: &[Review], now: i64, changes: &[FreezePreference]) -> Profile {
        compute_with_freezes(
            "a",
            "a",
            reviews,
            &utc(),
            &Week::default(),
            now,
            &FreezePolicy {
                preferences: changes.to_vec(),
                ..FreezePolicy::default()
            },
        )
    }

    /// Finish every possible quest metric, with a second session before noon.
    fn perfect_day(prior: &[Review], day: i64, clock: &Clock) -> Vec<Review> {
        let (days, _, _) = collect_days_with_quests(prior, clock, seed_of("a"));
        let recent = days
            .values()
            .rev()
            .take(RECENT_DAYS)
            .cloned()
            .rev()
            .collect();
        let quests = gen_quests(seed_of("a"), day, &recent);
        let count = quests.iter().map(|q| q.target).max().unwrap().max(2) as i64 + 1;
        let start = clock.day_start_ms(day) + 2 * 3_600_000;
        (0..count)
            .map(|i| {
                let id = start + i * 10_000 + if i == count - 1 { SESSION_GAP_MS } else { 0 };
                Review {
                    id,
                    cid: id,
                    last_ivl: MATURE_IVL,
                    ivl: None,
                    time_ms: 60_000,
                    kind: 0,
                }
            })
            .collect()
    }

    fn perfect_history(days: &[i64]) -> Vec<Review> {
        let mut reviews = Vec::new();
        for day in days {
            reviews.extend(perfect_day(&reviews, *day, &utc()));
        }
        reviews
    }

    fn first_completion(reviews: &[Review], day: i64) -> i64 {
        collect_days_with_quests(reviews, &utc(), seed_of("a"))
            .0
            .get(&day)
            .unwrap()
            .quests_completed_at
            .expect("fixture completes every quest")
    }

    #[test]
    fn day_respects_rollover_and_offset() {
        let clock = Clock {
            offset_west_min: -120,
            rollover_hour: 4,
        };
        let one_am_utc = 100 * DAY_MS + 3_600_000;
        assert_eq!(clock.hour(one_am_utc), 3);
        assert_eq!(clock.day(one_am_utc), 99);
        assert_eq!(clock.day(one_am_utc + 2 * 3_600_000), 100);
    }

    #[test]
    fn dates() {
        assert_eq!(date_string(0), "1970-01-01");
        assert_eq!(date_string(20_714), "2026-09-18");
        assert_eq!(date_string(19_782), "2024-02-29");
    }

    #[test]
    fn periods_narrow_from_lifetime_down_to_the_hour() {
        let day = 20_022i64;
        let now = day * DAY_MS + 12 * 3_600_000;
        let review = |ms: i64| Review {
            id: ms,
            cid: ms,
            last_ivl: 0,
            ivl: None,
            time_ms: 5_000,
            kind: 1,
        };
        let reviews = vec![
            review((day - 200) * DAY_MS + 12 * 3_600_000),
            review((day - 10) * DAY_MS + 12 * 3_600_000),
            review(now - 2 * 3_600_000),
            review(now - 30 * 60_000),
        ];
        let p = compute("a", "a", &reviews, &utc(), &Week::default(), now);
        assert!(
            p.periods.hour > 0,
            "the last review counts towards this hour"
        );
        assert!(p.periods.day > p.periods.hour);
        assert!(p.periods.week >= p.periods.day);
        assert!(p.periods.month > p.periods.week);
        assert!(p.periods.year > p.periods.month);
        assert_eq!(p.periods.all, p.xp_total);
        assert_eq!(p.periods.week, p.week_xp);
        for name in Periods::NAMES {
            assert!(p.periods.get(name) > 0, "{name} should have XP");
        }
        let quiet = compute(
            "a",
            "a",
            &reviews,
            &utc(),
            &Week::default(),
            now + 5 * 3_600_000,
        );
        assert_eq!(
            quiet.periods.hour, 0,
            "an hour without reviews earns nothing"
        );
        assert_eq!(quiet.periods.day, p.periods.day);
    }

    #[test]
    fn week_is_shared_across_time_zones() {
        let berlin = chrono_tz::Europe::Berlin;
        let week = Week {
            tz: berlin,
            rollover_hour: 4,
        };
        let at_berlin = |d, h| {
            berlin
                .with_ymd_and_hms(2026, 9, d, h, 0, 0)
                .unwrap()
                .timestamp_millis()
        };
        let buenos_aires = Clock {
            offset_west_min: 180,
            rollover_hour: 4,
        };
        let review = |id| Review {
            id,
            cid: id,
            last_ivl: 0,
            ivl: None,
            time_ms: 5_000,
            kind: 1,
        };

        assert_eq!(week.of(at_berlin(21, 3)), week.of(at_berlin(20, 12)));
        assert_ne!(week.of(at_berlin(21, 5)), week.of(at_berlin(20, 12)));
        assert_eq!(week.end_after(at_berlin(21, 10)), at_berlin(28, 4));
        assert_eq!(week.end_after(at_berlin(21, 3)), at_berlin(21, 4));

        let sunday = at_berlin(20, 20);
        let late_sunday = at_berlin(21, 8);
        let p = compute(
            "a",
            "a",
            &[review(sunday), review(late_sunday)],
            &buenos_aires,
            &week,
            late_sunday,
        );
        assert_eq!(
            p.week_xp, 0,
            "their Sunday started before the shared week did"
        );
        assert_eq!(p.today.reviews, 2);

        let monday = at_berlin(21, 10);
        let p = compute(
            "a",
            "a",
            &[review(sunday), review(monday)],
            &buenos_aires,
            &week,
            monday,
        );
        assert!(
            p.week_xp > 0,
            "their Monday started after the shared week did"
        );
    }

    #[test]
    fn levels_are_monotonic() {
        assert_eq!(level_for(0), (1, 0, 100));
        assert_eq!(level_for(99).0, 1);
        assert_eq!(level_for(100), (2, 0, 280));
        let mut last = 0;
        for level in 1..200 {
            assert!(level_need(level) > last);
            last = level_need(level);
        }
    }

    #[test]
    fn streak_counts_consecutive_days() {
        let p = compute(
            "a",
            "a",
            &history(&[10, 11, 12]),
            &utc(),
            &Week::default(),
            at(12),
        );
        assert_eq!(p.streak, 3);
        assert!(!p.at_risk);
    }

    #[test]
    fn today_without_reviews_is_at_risk_not_broken() {
        let p = compute(
            "a",
            "a",
            &history(&[10, 11, 12]),
            &utc(),
            &Week::default(),
            at(13),
        );
        assert_eq!(p.streak, 3);
        assert!(p.at_risk);
    }

    #[test]
    fn missed_day_without_freeze_resets() {
        let p = compute(
            "a",
            "a",
            &history(&[10, 11, 13]),
            &utc(),
            &Week::default(),
            at(13),
        );
        assert_eq!(p.streak, 1);
        assert_eq!(p.lifetime.best_streak, 2);
    }

    #[test]
    fn freezes_default_on_and_earn_from_quests_without_a_settings_change() {
        let empty = protected(&[], at(1), &[]);
        assert!(empty.freezes_enabled);
        assert_eq!(empty.stored_freezes, 0);
        let reviews = perfect_history(&[1]);
        let earned = protected(&reviews, at(1), &[]);
        assert!(earned.freezes_enabled);
        assert_eq!(earned.freezes, 1);
        assert!(earned.freeze_earned_today);
        let paused = protected(&reviews, at(1), &[protection(at(1), false)]);
        assert!(!paused.freezes_enabled);
        assert_eq!(paused.stored_freezes, 1);
        assert_eq!(paused.freezes, 0);
        let covered = protected(&reviews, at(3), &[]);
        assert_eq!(covered.streak, 1);
        assert_eq!(covered.heatmap.iter().filter(|cell| cell.frozen).count(), 1);
    }

    #[test]
    fn freeze_default_rollout_does_not_backfill_and_keeps_explicit_opt_outs() {
        let mut reviews = perfect_history(&[1, 2]);
        let policy = FreezePolicy {
            default_enabled_from: Some(utc().day_start_ms(3)),
            ..FreezePolicy::default()
        };
        let run = |reviews: &[Review], now, policy: &FreezePolicy| {
            compute_with_freezes("a", "a", reviews, &utc(), &Week::default(), now, policy)
        };
        let boundary = policy.default_enabled_from.unwrap();
        let old = run(&reviews, boundary - 1, &policy);
        let enabled = run(&reviews, boundary, &policy);
        assert!(!old.freezes_enabled);
        assert!(enabled.freezes_enabled);
        assert_eq!(enabled.stored_freezes, 0);
        assert_eq!(enabled.xp_total, old.xp_total);
        assert_eq!(enabled.streak, old.streak);
        let earlier = run(&reviews, at(2), &policy);
        let old_rule = run(
            &reviews,
            at(2),
            &FreezePolicy {
                preferences: vec![protection(0, false)],
                ..FreezePolicy::default()
            },
        );
        assert_eq!(
            serde_json::to_value(earlier).unwrap(),
            serde_json::to_value(old_rule).unwrap()
        );
        reviews.extend(perfect_day(&reviews, 3, &utc()));
        let earned = run(&reviews, at(4), &policy);
        assert_eq!(earned.stored_freezes, 1);
        let off = FreezePolicy {
            preferences: vec![protection(at(1), false)],
            ..policy.clone()
        };
        assert!(!run(&reviews, at(4), &off).freezes_enabled);
        assert_eq!(run(&reviews, at(4), &off).stored_freezes, 0);
        let opted_in = FreezePolicy {
            preferences: vec![protection(0, true)],
            ..policy
        };
        assert_eq!(run(&reviews, at(4), &opted_in).stored_freezes, 3);
    }

    #[test]
    fn explicitly_disabled_freezes_start_empty_without_historical_rewards() {
        let reviews = perfect_history(&[1, 2, 3, 4, 5, 6, 7]);
        let p = protected(&reviews, at(9), &[protection(0, false)]);
        assert!(!p.freezes_enabled);
        assert_eq!(p.stored_freezes, 0);
        assert_eq!(p.freezes, 0);
        assert_eq!(p.streak, 0);
        assert!(p.heatmap.iter().all(|c| !c.frozen));
        let opted_in = protected(
            &reviews,
            at(9),
            &[protection(0, false), protection(at(9), true)],
        );
        assert!(opted_in.freezes_enabled);
        assert_eq!(opted_in.freezes, 0);
        assert!(!opted_in.freeze_earned_today);
        assert_eq!(protected(&[], at(9), &[protection(0, true)]).freezes, 0);
        let ordinary = protected(
            &history(&[1, 2, 3, 4, 5, 6, 7]),
            at(7),
            &[protection(0, true)],
        );
        assert_eq!(
            ordinary.freezes, 0,
            "a seven-day streak alone earns nothing"
        );
    }

    #[test]
    fn migration_preserves_previously_protected_days_and_streak() {
        let reviews = history(&[1, 2, 3, 4, 5, 6, 7, 9]);
        let policy = FreezePolicy {
            legacy_until: Some(at(10)),
            preferences: vec![],
            default_enabled_from: Some(at(10)),
        };
        let p = compute_with_freezes(
            "a",
            "a",
            &reviews,
            &utc(),
            &Week::default(),
            at(10),
            &policy,
        );
        assert_eq!(
            p.streak, 8,
            "upgrading must not erase the freeze that covered day8"
        );
        assert_eq!(p.lifetime.best_streak, 8);
        assert!(
            p.heatmap
                .iter()
                .any(|c| c.date == date_string(8) && c.frozen)
        );
        assert_eq!(p.freezes, 0);
        assert_eq!(p.stored_freezes, 0);
        assert!(p.freezes_enabled);
        let before_cutover = compute_with_freezes(
            "a",
            "a",
            &reviews,
            &utc(),
            &Week::default(),
            at(9),
            &FreezePolicy {
                legacy_until: Some(at(10)),
                preferences: vec![],
                default_enabled_from: Some(at(10)),
            },
        );
        assert_eq!(p.xp_total, before_cutover.xp_total);
        assert_eq!(p.lifetime.quests, before_cutover.lifetime.quests);
        for cell in before_cutover
            .heatmap
            .iter()
            .filter(|c| c.reviews > 0 || c.frozen)
        {
            let after = p.heatmap.iter().find(|c| c.date == cell.date).unwrap();
            assert_eq!(
                (after.reviews, after.xp, after.frozen),
                (cell.reviews, cell.xp, cell.frozen)
            );
        }
    }

    #[test]
    fn migration_clears_unspent_legacy_stock_and_new_quests_use_the_new_rules() {
        let mut reviews = history(&(1..=14).collect::<Vec<_>>());
        let policy = FreezePolicy {
            legacy_until: Some(utc().day_start_ms(15)),
            preferences: vec![protection(utc().day_start_ms(15), true)],
            ..FreezePolicy::default()
        };
        let run = |reviews: &[Review], day| {
            compute_with_freezes(
                "a",
                "a",
                reviews,
                &utc(),
                &Week::default(),
                at(day),
                &policy,
            )
        };
        let old = run(&reviews, 14);
        assert_eq!(old.stored_freezes, 2);
        let upgraded = run(&reviews, 15);
        assert_eq!(upgraded.stored_freezes, 0);
        assert_eq!(upgraded.streak, 14);
        assert_eq!(upgraded.xp_total, old.xp_total);
        assert_eq!(
            run(&reviews, 16).streak,
            0,
            "old stock cannot cover a new missed day"
        );
        reviews.extend(perfect_day(&reviews, 15, &utc()));
        let completed = run(&reviews, 15);
        assert_eq!(completed.freezes, 1);
        assert!(completed.freeze_earned_today);
        let covered = run(&reviews, 17);
        assert_eq!(covered.freezes, 0);
        assert_eq!(covered.streak, 15);
    }

    #[test]
    fn freeze_requires_all_quests_and_is_earned_only_once_per_day() {
        let mut reviews = perfect_history(&[1]);
        let completion = first_completion(&reviews, 1);
        let changes = [protection(0, true)];
        let before: Vec<_> = reviews
            .iter()
            .copied()
            .filter(|r| r.id < completion)
            .collect();
        let p = protected(&before, completion - 1, &changes);
        assert!(p.quests.iter().any(|q| !q.done));
        assert_eq!(p.freezes, 0);

        let p = protected(&reviews, at(1), &changes);
        assert!(p.quests.iter().all(|q| q.done));
        assert!(p.freeze_earned_today);
        assert_eq!(p.freezes, 1);
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::to_value(protected(&reviews, at(1), &changes)).unwrap(),
            "repeated reads replay the same result"
        );
        reviews.extend(reviews_on(1, 5, 99));
        assert_eq!(protected(&reviews, at(1), &changes).freezes, 1);
    }

    #[test]
    fn freeze_reward_matches_visible_quest_completion_after_day_rollover() {
        let mut reviews = perfect_history(&[1]);
        let first_today = reviews.len();
        reviews.extend(perfect_day(&reviews, 2, &utc()));
        // Read the visible quest state independently of the collector's saved
        // completion timestamp, including the previous day's adaptive targets.
        let completed = (first_today..reviews.len())
            .find(|&i| {
                protected(&reviews[..=i], reviews[i].id, &[])
                    .quests
                    .iter()
                    .all(|quest| quest.done)
            })
            .expect("the second day completes its displayed quests");
        let completion = reviews[completed].id;
        assert!(
            protected(&reviews[..completed], completion - 1, &[])
                .quests
                .iter()
                .any(|quest| !quest.done)
        );
        assert_eq!(
            protected(
                &reviews,
                at(2),
                &[protection(0, false), protection(completion, true)]
            )
            .stored_freezes,
            1,
            "opting in at the visible completion earns today's freeze"
        );
        assert_eq!(
            protected(
                &reviews,
                at(2),
                &[protection(0, false), protection(completion + 1, true)]
            )
            .stored_freezes,
            0,
            "opting in after completion cannot claim a past reward"
        );
    }

    #[test]
    fn preference_at_first_completion_controls_earning_without_backfill() {
        let mut reviews = perfect_history(&[1]);
        let completion = first_completion(&reviews, 1);
        assert_eq!(
            protected(
                &reviews,
                at(1),
                &[protection(0, false), protection(completion, true)]
            )
            .freezes,
            1
        );
        assert_eq!(
            protected(
                &reviews,
                at(1),
                &[protection(0, false), protection(completion + 1, true)]
            )
            .freezes,
            0
        );
        reviews.extend(reviews_on(1, 3, 999));
        let p = protected(
            &reviews,
            at(1),
            &[
                protection(0, true),
                protection(completion, false),
                protection(completion + 1, true),
            ],
        );
        assert_eq!(
            p.freezes, 0,
            "more reviews cannot retry a missed earning opportunity"
        );
        let p = protected(
            &reviews,
            at(1),
            &[protection(0, true), protection(completion + 1, false)],
        );
        assert_eq!(
            p.stored_freezes, 1,
            "turning off keeps what was already earned"
        );
        assert_eq!(
            p.freezes, 0,
            "older clients must not promise disabled coverage"
        );
    }

    #[test]
    fn freezes_cap_at_three_and_can_be_earned_again_after_spending() {
        let reviews = perfect_history(&[1, 2, 3, 4]);
        let changes = [protection(0, true)];
        let full = protected(&reviews, at(4), &changes);
        assert_eq!(MAX_FREEZES, 3);
        assert_eq!(full.freezes, MAX_FREEZES);
        assert!(
            !full.freeze_earned_today,
            "completing quests at capacity grants no extra charge"
        );
        let mut after_gap = reviews;
        after_gap.extend(perfect_day(&after_gap, 6, &utc()));
        let p = protected(&after_gap, at(6), &changes);
        assert_eq!(p.freezes, 3);
        assert!(p.freeze_earned_today);
        assert_eq!(
            p.streak, 5,
            "covered days preserve rather than increase the streak"
        );
        let frozen = p.heatmap.iter().find(|c| c.date == date_string(5)).unwrap();
        assert!(frozen.frozen);
        assert_eq!(frozen.reviews, 0);
        assert_eq!(frozen.xp, achievement_xp(Metric::FreezesUsed, 1));
    }

    #[test]
    fn ended_missed_days_spend_one_each_until_empty() {
        let reviews = perfect_history(&[1, 2, 3]);
        let changes = [protection(0, true)];
        let today = protected(&reviews, at(4), &changes);
        assert_eq!(today.freezes, 3, "today has not ended yet");
        assert_eq!(today.streak, 3);
        let p = protected(&reviews, at(7), &changes);
        assert_eq!(p.freezes, 0);
        assert_eq!(p.streak, 3);
        assert_eq!(p.heatmap.iter().filter(|c| c.frozen).count(), 3);
        let broken = protected(&reviews, at(8), &changes);
        assert_eq!(broken.streak, 0);
        assert_eq!(broken.heatmap.iter().filter(|c| c.frozen).count(), 3);
    }

    #[test]
    fn disabling_preserves_bank_and_past_coverage_but_does_not_cover_new_gaps() {
        let reviews = perfect_history(&[1, 2, 3]);
        let off = [protection(0, true), protection(at(5), false)];
        let paused = protected(&reviews, at(6), &off);
        assert_eq!(paused.stored_freezes, 2);
        assert_eq!(paused.freezes, 0);
        assert_eq!(paused.streak, 0);
        assert_eq!(paused.heatmap.iter().filter(|c| c.frozen).count(), 1);
        assert!(
            paused
                .heatmap
                .iter()
                .any(|c| c.date == date_string(4) && c.frozen)
        );
        let mut on_again = off.to_vec();
        on_again.push(protection(at(6), true));
        let p = protected(&reviews, at(6), &on_again);
        assert_eq!(p.freezes, 2);
        assert_eq!(
            p.streak, 0,
            "reenabling cannot retroactively cover yesterday"
        );
        assert_eq!(p.heatmap.iter().filter(|c| c.frozen).count(), 1);
        assert_eq!(
            protected(&reviews, at(8), &on_again).freezes,
            2,
            "an already broken streak spends nothing"
        );
    }

    #[test]
    fn missed_day_checks_setting_just_before_the_anki_rollover() {
        let clock = Clock {
            offset_west_min: -120,
            rollover_hour: 4,
        };
        let reviews = perfect_day(&[], 1, &clock);
        let boundary = clock.day_start_ms(3);
        let run = |now, changes: &[FreezePreference]| {
            compute_with_freezes(
                "a",
                "a",
                &reviews,
                &clock,
                &Week::default(),
                now,
                &FreezePolicy {
                    preferences: changes.to_vec(),
                    ..FreezePolicy::default()
                },
            )
        };
        assert_eq!(run(boundary - 1, &[protection(0, true)]).stored_freezes, 1);
        let p = run(
            boundary,
            &[protection(0, true), protection(boundary, false)],
        );
        assert_eq!(
            p.stored_freezes, 0,
            "a change on the new day preserves yesterday's enabled state"
        );
        assert_eq!(p.streak, 1);
        let p = run(
            boundary,
            &[protection(0, true), protection(boundary - 1, false)],
        );
        assert_eq!(p.stored_freezes, 1);
        assert_eq!(p.streak, 0);
        let p = run(
            boundary,
            &[
                protection(0, true),
                protection(at(1), false),
                protection(boundary, true),
            ],
        );
        assert_eq!(p.streak, 0);
        assert_eq!(p.freezes, 1);
    }

    #[test]
    fn future_reviews_and_preferences_cannot_award_a_freeze_early() {
        let reviews = perfect_history(&[1]);
        let completion = first_completion(&reviews, 1);
        let changes = [protection(0, true)];
        let before = protected(&reviews, completion - 1, &changes);
        assert_eq!(before.freezes, 0);
        assert!(!before.freeze_earned_today);
        assert_eq!(protected(&reviews, completion, &changes).freezes, 1);
        let future = protected(
            &reviews,
            at(1),
            &[protection(0, false), protection(at(2), true)],
        );
        assert!(!future.freezes_enabled);
        assert_eq!(future.stored_freezes, 0);
        assert_eq!(
            changes,
            [protection(0, true)],
            "preview and replay never mutate preferences"
        );
    }

    #[test]
    fn preference_changes_at_the_same_time_use_the_latest_save() {
        let reviews = perfect_history(&[1]);
        let completion = first_completion(&reviews, 1);
        let changes = [protection(completion, true), protection(completion, false)];
        assert_eq!(protected(&reviews, at(1), &changes).stored_freezes, 0);
        let changes = [protection(completion, false), protection(completion, true)];
        assert_eq!(protected(&reviews, at(1), &changes).freezes, 1);
    }

    #[test]
    fn combo_breaks_on_gap() {
        let mut reviews = reviews_on(5, 10, 0);
        let mut later = reviews_on(5, 4, 100);
        for r in &mut later {
            r.id += 3_600_000;
        }
        reviews.extend(later);
        let p = compute("a", "a", &reviews, &utc(), &Week::default(), at(5));
        assert_eq!(p.today.max_combo, 10);
        assert_eq!(p.today.current_combo, 4);
        assert_eq!(p.today.reviews, 14);
    }

    #[test]
    fn answer_button_never_matters() {
        let r = Review {
            id: 0,
            cid: 1,
            last_ivl: 30,
            ivl: None,
            time_ms: 4_000,
            kind: 1,
        };
        assert_eq!(review_base_xp(&r), 15.0);
    }

    #[test]
    fn quick_answers_earn_full_xp() {
        let r = Review {
            id: 0,
            cid: 1,
            last_ivl: 30,
            ivl: None,
            time_ms: 200,
            kind: 1,
        };
        assert_eq!(review_base_xp(&r), 15.0);
    }

    #[test]
    fn repeating_a_card_keeps_the_same_xp_as_answering_different_cards() {
        let answer = |day: i64, index: i64, cid: i64| Review {
            id: day * DAY_MS + NOON + index * 10_000,
            cid,
            last_ivl: 0,
            ivl: None,
            time_ms: 5_000,
            kind: 1,
        };
        let same_card: Vec<Review> = (0..6).map(|i| answer(1, i, 7)).collect();
        let (_, _, repeated) = collect_days(&same_card, &utc());
        let varied: Vec<Review> = (0..6).map(|i| answer(1, i, 7 + i)).collect();
        let (_, _, spread) = collect_days(&varied, &utc());
        assert_eq!(repeated, spread, "card identity does not reduce review XP");
        assert!(repeated[5].1 > repeated[0].1, "the combo bonus still grows");

        let (_, _, fresh) = collect_days(&[answer(1, 0, 7), answer(2, 0, 7)], &utc());
        assert_eq!(fresh[0].1, fresh[1].1, "a new day resets the combo");
    }

    #[test]
    fn learning_and_relearning_keep_the_original_rate_for_every_step() {
        for kind in [0, 2] {
            let steps: Vec<Review> = (0..3)
                .map(|index| Review {
                    id: DAY_MS + NOON + index * 600_000,
                    cid: 7,
                    last_ivl: 0,
                    ivl: None,
                    time_ms: 5_000,
                    kind,
                })
                .collect();
            let (_, _, earned) = collect_days(&steps, &utc());
            for (_, xp) in earned {
                assert!((xp - 6.03).abs() < 1e-9, "kind {kind} earned {xp}");
            }
        }
    }

    #[test]
    fn diminishing_weighting_pays_less_for_each_repeat_of_a_card() {
        let answer = |day: i64, index: i64, cid: i64| Review {
            id: day * DAY_MS + NOON + index * 10_000,
            cid,
            last_ivl: 0,
            ivl: None,
            time_ms: 5_000,
            kind: 1,
        };
        let run = |reviews: &[Review]| {
            collect_days_weighted(reviews, &utc(), 0, ReviewWeighting::Diminishing).2
        };
        let total = |earned: &[(i64, f64)]| earned.iter().map(|(_, xp)| xp).sum::<f64>();

        let stuck: Vec<Review> = (0..6).map(|i| answer(1, i, 7)).collect();
        let repeated = run(&stuck);
        for index in 1..repeated.len() {
            assert!(
                repeated[index].1 < repeated[index - 1].1,
                "answer {index} should pay less than the one before it"
            );
        }
        assert!(
            total(&repeated) < 3.0 * repeated[0].1,
            "one card cannot be farmed"
        );

        let varied: Vec<Review> = (0..6).map(|i| answer(1, i, 7 + i)).collect();
        assert!(
            total(&run(&varied)) > 2.0 * total(&repeated),
            "studying six cards must beat answering one card six times"
        );

        let fresh = run(&[answer(1, 0, 7), answer(2, 0, 7)]);
        assert_eq!(fresh[0].1, fresh[1].1, "a new day starts the card over");
    }

    #[test]
    fn diminishing_weighting_pays_learning_about_as_much_as_a_review() {
        let answer = |index: i64, kind: u8, last_ivl: i64| Review {
            id: DAY_MS + NOON + index * 600_000,
            cid: 7,
            last_ivl,
            ivl: None,
            time_ms: 5_000,
            kind,
        };
        let run = |reviews: &[Review]| {
            collect_days_weighted(reviews, &utc(), 0, ReviewWeighting::Diminishing).2
        };
        let learning = run(&[answer(0, 0, 0), answer(1, 0, 0), answer(2, 0, 0)]);
        let learned: f64 = learning.iter().map(|(_, xp)| xp).sum();
        let known = run(&[answer(0, 1, 30)])[0].1;
        assert!(
            (learned - known).abs() < 2.0,
            "learning {learned}, knowing {known}"
        );
        assert!(
            learning[0].1 < known,
            "a single learning step is not a whole review"
        );

        let lapse = run(&[answer(0, 1, 30), answer(1, 2, 0), answer(2, 2, 0)]);
        let relearning: f64 = lapse[1..].iter().map(|(_, xp)| xp).sum();
        assert!(
            relearning < lapse[0].1 / 2.0,
            "relearning {relearning} vs {}",
            lapse[0].1
        );
    }

    #[test]
    fn review_weighting_defaults_to_flat_in_config() {
        #[derive(Deserialize)]
        struct C {
            #[serde(default)]
            w: ReviewWeighting,
        }
        let parse = |s: &str| serde_json::from_str::<C>(s).unwrap().w;
        assert_eq!(parse("{}"), ReviewWeighting::Flat);
        assert_eq!(parse(r#"{"w":"flat"}"#), ReviewWeighting::Flat);
        assert_eq!(
            parse(r#"{"w":"diminishing"}"#),
            ReviewWeighting::Diminishing
        );
    }

    #[test]
    fn records_keep_the_best_hour_day_week_month_and_year() {
        let mut reviews = reviews_on(1, 5, 100);
        reviews.extend(reviews_on(40, 30, 200));
        reviews.extend(reviews_on(41, 5, 300));
        reviews.extend(reviews_on(400, 5, 400));
        let p = compute("a", "a", &reviews, &utc(), &Week::default(), at(400));

        assert_eq!(
            p.records.day.reviews, 30,
            "the busiest 24 hours is the record"
        );
        assert!((40 * DAY_MS + NOON..41 * DAY_MS).contains(&p.records.day.at));
        assert!(p.records.day.until > p.records.day.at);
        assert!(p.records.day.xp >= p.periods.day);
        assert_eq!(
            p.records.week.reviews, 35,
            "day 40 and 41 are within 7 days"
        );
        assert!(p.records.week.xp > p.records.day.xp);
        assert_eq!(p.records.month.reviews, 35);
        assert_eq!(
            p.records.year.reviews, 40,
            "any 365 days hold three of the four days"
        );
        assert_eq!(p.records.hour.reviews, 30, "one sitting, one hour");
        assert!(p.records.hour.xp > 0 && p.records.hour.xp <= p.records.day.xp);
    }

    #[test]
    fn a_rolling_day_spans_the_anki_rollover() {
        let at_hour = |day: i64, hour: i64, count: i64, cid: i64| -> Vec<Review> {
            (0..count)
                .map(|i| Review {
                    id: day * DAY_MS + hour * 3_600_000 + i * 10_000,
                    cid: cid + i,
                    last_ivl: 0,
                    ivl: None,
                    time_ms: 5_000,
                    kind: 1,
                })
                .collect()
        };
        let mut reviews = at_hour(5, 2, 10, 100);
        reviews.extend(at_hour(5, 6, 10, 200));
        let p = compute("a", "a", &reviews, &utc(), &Week::default(), at(6));
        assert_ne!(utc().day(reviews[0].id), utc().day(reviews[19].id));
        assert_eq!(p.records.day.reviews, 20, "two Anki days, one 24 hours");
    }

    #[test]
    fn beating_a_personal_best_is_celebrated_with_the_old_best() {
        let mut reviews = reviews_on(1, 5, 100);
        reviews.extend(reviews_on(3, 20, 200));
        let p = compute("a", "a", &reviews, &utc(), &Week::default(), at(3));
        let records: Vec<&Event> = p
            .events
            .iter()
            .filter(|e| e.key.starts_with("record:"))
            .collect();
        assert_eq!(
            records.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(),
            ["record:hour:3", "record:day:3"],
            "7 days of history are needed before a best week counts"
        );
        assert_eq!(records[1].title, "New best 24 hours!");
        assert!(
            records[1].body.contains("beating your old best of"),
            "{}",
            records[1].body
        );

        let mut slower = reviews_on(1, 20, 100);
        slower.extend(reviews_on(3, 5, 200));
        let p = compute("a", "a", &slower, &utc(), &Week::default(), at(3));
        assert!(!p.events.iter().any(|e| e.key.starts_with("record:")));

        let first_day = compute(
            "a",
            "a",
            &reviews_on(3, 20, 200),
            &utc(),
            &Week::default(),
            at(3),
        );
        assert!(
            !first_day
                .events
                .iter()
                .any(|e| e.key.starts_with("record:")),
            "a first day beats nothing"
        );
    }

    #[test]
    fn streak_milestones_without_an_achievement_get_their_own_message() {
        let fifty: Vec<i64> = (1..=50).collect();
        let p = compute("a", "a", &history(&fifty), &utc(), &Week::default(), at(50));
        assert_eq!(p.streak, 50);
        assert!(
            p.events
                .iter()
                .any(|e| e.key == "streak:1:50" && e.title == "50-day streak!")
        );

        let week: Vec<i64> = (1..=7).collect();
        let p = compute("a", "a", &history(&week), &utc(), &Week::default(), at(7));
        assert!(
            !p.events.iter().any(|e| e.key.starts_with("streak:")),
            "the Full Week achievement already says it"
        );
        assert!(p.events.iter().any(|e| e.key == "achievement:streak-7"));
    }

    #[test]
    fn the_best_hour_is_any_sixty_minutes_not_a_clock_hour() {
        let spread = |minutes: &[i64]| -> Vec<Review> {
            minutes
                .iter()
                .enumerate()
                .map(|(index, minute)| Review {
                    id: DAY_MS + NOON + minute * 60_000,
                    cid: 500 + index as i64,
                    last_ivl: 0,
                    ivl: None,
                    time_ms: 5_000,
                    kind: 1,
                })
                .collect()
        };
        let reviews = spread(&[0, 50, 55, 58, 59, 200]);
        let p = compute("a", "a", &reviews, &utc(), &Week::default(), at(2));
        assert_eq!(
            p.records.hour.reviews, 5,
            "a window that straddles the clock hour still counts"
        );
        assert_eq!(p.records.hour.at, DAY_MS + NOON);

        let alone = compute("a", "a", &spread(&[0]), &utc(), &Week::default(), at(2));
        assert_eq!(alone.records.hour.reviews, 1);
        let none = compute("a", "a", &[], &utc(), &Week::default(), at(2));
        assert_eq!(none.records, Records::default());
    }

    #[test]
    fn a_nudge_only_speaks_when_something_is_within_reach() {
        let reviews = reviews_on(1, 12, 900);
        let mut profile = compute("a", "Ana", &reviews, &utc(), &Week::default(), at(1));
        profile.local_hour = 12;
        let bodies = |profile: &Profile, ahead: Option<(&str, u64)>| {
            nudges(profile, ahead)
                .into_iter()
                .map(|n| n.body)
                .collect::<Vec<_>>()
        };

        let just_ahead = profile.week_xp + 60;
        let said = bodies(&profile, Some(("Cerro", just_ahead)));
        assert!(
            said.iter().any(|body| body.contains("60 XP behind Cerro")),
            "{said:?}"
        );
        assert!(
            said.iter().any(|body| body.contains("more reviews")),
            "the gap is also told in reviews: {said:?}"
        );
        assert!(bodies(&profile, Some(("Cerro", profile.week_xp + 5_000))).is_empty());
        assert!(
            bodies(&profile, Some(("Cerro", 0))).is_empty(),
            "nobody chases from the front"
        );

        profile.xp_into_level = profile.xp_for_next - 40;
        let climbing = bodies(&profile, None);
        assert_eq!(climbing.len(), 1);
        assert!(climbing[0].contains("40 XP away"), "{climbing:?}");

        profile.records.day.xp = profile.today.xp + 30;
        let all_three = nudges(&profile, Some(("Cerro", just_ahead)));
        assert_eq!(all_three.len(), 3, "each rule may speak once a day");
        let keys: Vec<&str> = all_three.iter().map(|n| n.key.as_str()).collect();
        assert!(
            keys.iter()
                .all(|key| key.ends_with(&profile.day.to_string()))
        );
        assert_eq!(
            all_three
                .iter()
                .map(|n| n.key.clone())
                .collect::<HashSet<_>>()
                .len(),
            3
        );
    }

    #[test]
    fn a_nudge_stays_quiet_at_night_and_on_a_day_without_studying() {
        let reviews = reviews_on(1, 12, 900);
        let mut profile = compute("a", "Ana", &reviews, &utc(), &Week::default(), at(1));
        profile.xp_into_level = profile.xp_for_next - 40;
        profile.local_hour = 12;
        assert_eq!(nudges(&profile, None).len(), 1);

        for hour in [0, 3, 8, 22, 23] {
            profile.local_hour = hour;
            assert!(nudges(&profile, None).is_empty(), "nothing at {hour}:00");
        }
        profile.local_hour = 12;
        profile.today.reviews = 0;
        assert!(
            nudges(&profile, None).is_empty(),
            "a nudge cheers studying on, it does not ask for it"
        );
    }

    #[test]
    fn quests_are_deterministic_and_distinct() {
        let reviews = history(&[1, 2, 3]);
        let a = compute("a", "a", &reviews, &utc(), &Week::default(), at(3));
        let b = compute("a", "a", &reviews, &utc(), &Week::default(), at(3));
        assert_eq!(a.quests.len(), 3);
        let titles = |p: &Profile| p.quests.iter().map(|q| q.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(&a), titles(&b));
        let unique: HashSet<_> = titles(&a).into_iter().collect();
        assert_eq!(unique.len(), 3);
    }

    #[test]
    fn xp_sums_into_heatmap_and_total() {
        let p = compute(
            "a",
            "a",
            &history(&[1, 2, 3]),
            &utc(),
            &Week::default(),
            at(3),
        );
        let heat: u64 = p.heatmap.iter().map(|c| c.xp).sum();
        assert_eq!(heat, p.xp_total);
        assert!(p.xp_total > 0);
        assert_eq!(p.week_xp, p.xp_total);
    }

    #[test]
    fn achievements_unlock_with_events() {
        let reviews: Vec<Review> = reviews_on(1, 120, 0);
        let p = compute("a", "a", &reviews, &utc(), &Week::default(), at(1));
        let first = p
            .achievements
            .iter()
            .find(|a| a.id == "reviews-100")
            .unwrap();
        assert_eq!(first.unlocked.as_deref(), Some("1970-01-02"));
        assert!(p.events.iter().any(|e| e.key == "achievement:reviews-100"));
        assert!(
            p.events
                .iter()
                .any(|e| e.key == "achievement:dayreviews-100")
        );
    }

    #[test]
    fn calendar_and_memory_achievements() {
        let mut reviews = reviews_on(0, 5, 0);
        reviews.extend(reviews_on(2, 5, 100));
        reviews.extend(reviews_on(40, 5, 200));
        let mut fast = reviews_on(365, 120, 1000);
        for r in &mut fast {
            r.time_ms = 3_000;
        }
        fast[0].last_ivl = 400;
        reviews.extend(fast);
        let p = compute("a", "a", &reviews, &utc(), &Week::default(), at(365));
        let unlocked = |id: &str| {
            p.achievements
                .iter()
                .find(|a| a.id == id)
                .unwrap_or_else(|| panic!("no achievement {id}"))
                .unlocked
                .is_some()
        };
        for id in [
            "comebacks-1",
            "anniversaries-1",
            "years-1",
            "newyeardays-1",
            "maxinterval-365",
            "fastdays-1",
            "dayreviews-100",
        ] {
            assert!(unlocked(id), "{id} should be unlocked");
        }
        assert!(!unlocked("comebacks-3"));
        assert!(!unlocked("maxinterval-1825"));
        let weekend = p
            .achievements
            .iter()
            .find(|a| a.id == "weekenddays-10")
            .unwrap();
        assert_eq!(weekend.progress, 1);
    }

    #[test]
    fn achievement_ids_are_unique() {
        let p = compute("a", "a", &[], &utc(), &Week::default(), at(1));
        let ids: HashSet<_> = p.achievements.iter().map(|a| a.id.clone()).collect();
        assert_eq!(ids.len(), p.achievements.len());
    }

    #[test]
    fn empty_history() {
        let p = compute("a", "a", &[], &utc(), &Week::default(), at(50));
        assert_eq!(p.level, 1);
        assert_eq!(p.streak, 0);
        assert_eq!(p.quests.len(), 3);
        assert_eq!(p.heatmap.len(), HEATMAP_DAYS as usize);
    }
}
