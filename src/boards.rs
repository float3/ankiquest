//! The shared boards: the leaderboard, records, rank changes and the competition week.

use crate::access::authorized;
use crate::app::{App, now_ms};
use crate::game::{Periods, Profile, Records};
use crate::web_util::store_error;
use crate::{access, feedback, game, i18n, social};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

/// How many people a record names: the holder and whoever came closest.
const PODIUM: usize = 3;

#[derive(Serialize)]
pub(crate) struct Standing {
    pub(crate) user: String,
    display: String,
    level: u64,
    xp_total: u64,
    pub(crate) week_xp: u64,
    xp: u64,
    period: String,
    periods: Periods,
    streak: u64,
    streak_state: game::StreakState,
    day_ends_at: i64,
    today_reviews: u64,
    today_minutes: u64,
    /// Review minutes today needs for the streak; 0 when any review counts.
    streak_minutes: u64,
    /// XP still counting toward the week that just reset, while this player's current day
    /// began before the reset; see [`game::ClosingWeek`]. Left out otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    closing_xp: Option<u64>,
}

#[derive(Deserialize, Default)]
struct BoardQuery {
    period: Option<String>,
    scope: Option<String>,
    /// The group of `scope=group`.
    group: Option<String>,
}

impl BoardQuery {
    fn scope(&self) -> Result<Option<social::Scope>, StatusCode> {
        social::Scope::parse(self.scope.as_deref(), self.group.as_deref())
            .ok_or(StatusCode::BAD_REQUEST)
    }
}

/// How many players the global board lists, besides the viewer's own row.
const GLOBAL_ROWS: usize = 100;

/// The players a board shows this viewer, or why it cannot.
fn board_users(
    app: &App,
    viewer: &social::Viewer,
    scope: Option<social::Scope>,
) -> Result<BTreeSet<String>, StatusCode> {
    app.board(&app.store.lock().unwrap().conn, viewer, scope)
        .map_err(store_error)?
        .ok_or(StatusCode::FORBIDDEN)
}

/// A circle or group board lists [`Standing`]s; the global board lists [`GlobalRow`]s.
#[derive(Serialize)]
#[serde(untagged)]
enum Board {
    Standings(Vec<Standing>),
    Global(Vec<GlobalRow>),
}

/// A row of the global board.
#[derive(Serialize)]
struct GlobalRow {
    /// The player's place among everyone on the global board, from 1.
    rank: usize,
    #[serde(flatten)]
    row: GlobalStanding,
    /// Only on the viewer's own row.
    #[serde(skip_serializing_if = "Option::is_none")]
    me: Option<bool>,
    /// Only on the viewer's own row: whether other players see it on this board.
    #[serde(skip_serializing_if = "Option::is_none")]
    shown: Option<bool>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum GlobalStanding {
    /// The viewer or someone in their circle: as on the circle board.
    Full(Standing),
    /// Someone outside the viewer's circle, regardless of profile visibility.
    Stranger(Stranger),
}

/// What the global board shows of someone outside the viewer's circle.
#[derive(Serialize)]
struct Stranger {
    user: String,
    display: String,
    level: u64,
    xp: u64,
    period: String,
    streak: u64,
}

impl From<Standing> for Stranger {
    fn from(standing: Standing) -> Self {
        Stranger {
            user: standing.user,
            display: standing.display,
            level: standing.level,
            xp: standing.xp,
            period: standing.period,
            streak: standing.streak,
        }
    }
}

/// Everyone on the global board for this viewer, and those of them the viewer may see in
/// full. Public profiles are listed unless their player turned the global leaderboard off.
/// Signed-in players also see private profiles that turned it on, and themselves, but not
/// the people they muted.
fn global_users(
    app: &App,
    viewer: &social::Viewer,
) -> Result<(BTreeSet<String>, BTreeSet<String>), crate::Error> {
    let store = app.store.lock().unwrap();
    let conn = &store.conn;
    let mut users = app
        .board(conn, viewer, Some(social::Scope::Global))?
        .unwrap_or_default();
    let full = if let Some(me) = &viewer.member {
        users.extend(
            store
                .global_leaderboard_players()?
                .into_iter()
                .filter(|user| app.is_user(user)),
        );
        let muted: BTreeSet<String> = conn
            .prepare("select sender from muted_people where recipient = ?1")?
            .query_map([me], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        users.retain(|user| !muted.contains(user));
        users.insert(me.clone());
        app.circle(conn, me)?
    } else {
        BTreeSet::new()
    };
    Ok((users, full))
}

/// The top of the global board, plus the viewer's own row when it falls below.
fn global_board(
    app: &App,
    viewer: &social::Viewer,
    period: Option<String>,
) -> Result<Vec<GlobalRow>, crate::Error> {
    let (users, full) = global_users(app, viewer)?;
    let me = viewer.member.as_deref();
    let shown = match me {
        Some(me) => {
            let store = app.store.lock().unwrap();
            store.global_leaderboard(me)?
        }
        None => false,
    };
    Ok(standings(app, &users, period)
        .into_iter()
        .enumerate()
        .filter(|(place, standing)| *place < GLOBAL_ROWS || Some(standing.user.as_str()) == me)
        .map(|(place, standing)| {
            let mine = Some(standing.user.as_str()) == me;
            GlobalRow {
                rank: place + 1,
                me: mine.then_some(true),
                shown: mine.then_some(shown),
                row: if full.contains(&standing.user) {
                    GlobalStanding::Full(standing)
                } else {
                    GlobalStanding::Stranger(standing.into())
                },
            }
        })
        .collect())
}

async fn leaderboard(
    State(app): State<Arc<App>>,
    Query(query): Query<BoardQuery>,
    headers: HeaderMap,
) -> Result<Json<Board>, StatusCode> {
    let scope = query.scope()?;
    let viewer = access::viewer(&app, &headers);
    // The public default board has the same privacy rules as explicit global scope.
    if scope == Some(social::Scope::Global) || (scope.is_none() && viewer.member.is_none()) {
        return global_board(&app, &viewer, query.period)
            .map(|rows| Json(Board::Global(rows)))
            .map_err(store_error);
    }
    let users = board_users(&app, &viewer, scope)?;
    Ok(Json(Board::Standings(standings(
        &app,
        &users,
        query.period,
    ))))
}

pub(crate) fn standings(
    app: &App,
    users: &BTreeSet<String>,
    period: Option<String>,
) -> Vec<Standing> {
    let period = period
        .filter(|name| Periods::NAMES.contains(&name.as_str()))
        .unwrap_or_else(|| "week".into());
    let mut standings: Vec<Standing> = app
        .standings_among(users)
        .into_iter()
        .map(|p| Standing {
            user: p.user.clone(),
            display: p.display.clone(),
            level: p.level,
            xp_total: p.xp_total,
            week_xp: p.week_xp,
            xp: p.periods.get(&period),
            period: period.clone(),
            periods: p.periods.clone(),
            streak: p.streak,
            streak_state: p.streak_state,
            day_ends_at: p.day_ends_at,
            today_reviews: p.today.reviews,
            today_minutes: p.today.minutes,
            streak_minutes: p.today.streak_minutes,
            closing_xp: p.closing_week.map(|closing| closing.xp),
        })
        .collect();
    standings.sort_by(|a, b| {
        (b.xp, b.xp_total)
            .cmp(&(a.xp, a.xp_total))
            .then_with(|| a.user.cmp(&b.user))
    });
    standings
}

#[derive(Deserialize)]
struct RankQuery {
    #[serde(default)]
    previous: Vec<String>,
    /// The `week` of the answer `previous` came from. Older clients leave it out.
    #[serde(default)]
    week: Option<i64>,
}

#[derive(Serialize)]
struct RankResponse {
    order: Vec<String>,
    /// When this competition week began (ms), for the client to send back with `order`.
    week: i64,
    change: Option<feedback::Notice>,
}

/// The competition week of the last order each device was answered with, for clients that
/// do not send `week` back. A device's first answer in a week compares nothing, so an order
/// seen before the weekly reset is never read as this week's. Devices are told apart by the
/// owner's token or browser session; everyone else shares one entry per player. Kept in
/// memory only: after a restart every device's first answer has no change either.
#[derive(Default)]
pub(crate) struct RankWeeks(Mutex<HashMap<(String, String), i64>>);

impl RankWeeks {
    /// Notes that `device` was answered with `user`'s order in `week`, and whether the answer
    /// it had before belonged to that week too.
    fn answered(&self, device: String, user: &str, week: i64) -> bool {
        let mut weeks = self.0.lock().unwrap();
        weeks.retain(|_, seen| *seen >= week);
        weeks.insert((device, user.to_string()), week) == Some(week)
    }
}

/// How `me` moved among the players who have earned XP this week. Everyone else sits at
/// zero, ordered by lifetime XP, and moving past them is no news: right after the weekly
/// reset whoever studies first (often someone in an earlier time zone) would otherwise
/// "pass" everyone whose score just reset.
fn rank_change_this_week(
    previous: &[String],
    places: &[feedback::Place],
    me: &str,
) -> Option<feedback::Notice> {
    let active: Vec<feedback::Place> = places
        .iter()
        .filter(|place| place.week_xp > 0 || place.user == me)
        .map(|place| feedback::Place {
            user: place.user,
            display: place.display,
            week_xp: place.week_xp,
        })
        .collect();
    let previous: Vec<String> = previous
        .iter()
        .filter(|user| active.iter().any(|place| place.user == user.as_str()))
        .cloned()
        .collect();
    feedback::rank_change(&previous, &active, me)
}

/// The weekly order, and how `user` moved since the order a device saw last.
async fn rank(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(query): Json<RankQuery>,
) -> Result<Json<RankResponse>, StatusCode> {
    let language =
        i18n::selected(&headers, &app.store.lock().unwrap(), &user).map_err(store_error)?;
    let owner = authorized(&app, &user, &headers);
    let users = if owner {
        app.circle(&app.store.lock().unwrap().conn, &user)
            .map_err(store_error)?
    } else {
        board_users(&app, &access::viewer(&app, &headers), None)?
    };
    let week = app.week.start_of(now_ms());
    let board = standings(&app, &users, None);
    let places: Vec<feedback::Place> = board
        .iter()
        .map(|standing| feedback::Place {
            user: &standing.user,
            display: &standing.display,
            week_xp: standing.week_xp,
        })
        .collect();
    let device = owner
        .then(|| {
            let credential = access::credential(&app, &headers);
            credential.token.or(credential.session)
        })
        .flatten()
        .unwrap_or_default();
    let answered_this_week = app.rank_weeks.answered(device, &user, week);
    let same_week = query.week.map_or(answered_this_week, |seen| seen == week);
    let mut change = same_week
        .then(|| rank_change_this_week(&query.previous, &places, &user))
        .flatten();
    if let Some(change) = &mut change {
        i18n::notice(change, &language);
    }
    Ok(Json(RankResponse {
        order: board.iter().map(|standing| standing.user.clone()).collect(),
        week,
        change,
    }))
}

#[derive(Debug, Serialize)]
struct RecordHolder {
    user: String,
    display: String,
    value: u64,
    detail: u64,
    at: i64,
    /// For rolling windows, the last review inside it; otherwise the same as `at`.
    until: i64,
}

#[derive(Debug, Serialize)]
struct RecordBoard {
    window: String,
    unit: &'static str,
    holders: Vec<RecordHolder>,
}

fn podium(
    window: &str,
    unit: &'static str,
    profiles: &[Arc<Profile>],
    of: impl Fn(&Profile) -> (u64, u64, i64, i64),
) -> RecordBoard {
    let mut holders: Vec<RecordHolder> = profiles
        .iter()
        .map(|player| (player, of(player)))
        .filter(|(_, (value, _, _, _))| *value > 0)
        .map(|(player, (value, detail, at, until))| RecordHolder {
            user: player.user.clone(),
            display: player.display.clone(),
            value,
            detail,
            at,
            until,
        })
        .collect();
    holders.sort_by_key(|holder| std::cmp::Reverse((holder.value, holder.detail)));
    holders.truncate(PODIUM);
    RecordBoard {
        window: window.to_string(),
        unit,
        holders,
    }
}

/// The best rolling hour, 24 hours, 7, 30 and 365 days anyone here has ever had, plus the
/// longest streak and the most days studied. Each names whoever came closest,
/// so a near miss is visible rather than hidden.
async fn records(
    State(app): State<Arc<App>>,
    Query(query): Query<BoardQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<RecordBoard>>, StatusCode> {
    let scope = query.scope()?;
    let viewer = access::viewer(&app, &headers);
    let profiles = app.standings_among(&board_users(&app, &viewer, scope)?);
    let mut board: Vec<RecordBoard> = Records::NAMES
        .iter()
        .map(|window| {
            podium(window, "xp", &profiles, |player| {
                let record = player.records.get(window);
                (record.xp, record.reviews, record.at, record.until)
            })
        })
        .collect();
    board.push(podium("streak", "days", &profiles, |player| {
        (
            player.lifetime.best_streak,
            0,
            player.lifetime.best_streak_at,
            player.lifetime.best_streak_at,
        )
    }));
    board.push(podium("days", "days", &profiles, |player| {
        (
            player.lifetime.days_active,
            0,
            player.lifetime.first_day_at,
            player.lifetime.first_day_at,
        )
    }));
    Ok(Json(board))
}

#[derive(Serialize)]
struct WeekInfo {
    /// When the current competition week began (ms), so a client can say a fresh week has
    /// just started for a player whose local Monday has not come yet.
    starts_at: i64,
    ends_at: i64,
    timezone: String,
}

async fn week_info(State(app): State<Arc<App>>) -> Json<WeekInfo> {
    let now = now_ms();
    Json(WeekInfo {
        starts_at: app.week.start_of(now),
        ends_at: app.week.end_after(now),
        timezone: app.config.week_timezone.clone(),
    })
}
pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/leaderboard", get(leaderboard))
        .route("/api/records", get(records))
        .route("/api/week", get(week_info))
        .route("/api/rank/{user}", post(rank))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::study::{Upload, upload};
    use crate::test_support::*;

    #[tokio::test]
    async fn leaderboard_exposes_current_study_state_and_per_player_cutoff() {
        let (app, _path) = fixture();
        app.players.write().unwrap().insert(
            "cerro".into(),
            Player {
                reviews: Default::default(),
                clock: Clock::default(),
                clock_history: ClockHistory::default(),
                freeze_policy: game::FreezePolicy::default(),
                revision: next_revision(),
            },
        );
        let expected = app.profile("cerro").unwrap();
        let Json(board) = leaderboard(State(app), Query(BoardQuery::default()), headers("hill"))
            .await
            .unwrap();
        let value = serde_json::to_value(&board).unwrap();
        assert_eq!(value[0]["streak_state"], "pending");
        assert_eq!(value[0]["today_reviews"], 0);
        assert_eq!(value[0]["day_ends_at"], expected.day_ends_at);
    }

    fn studied(user: &str, count: i64, day: i64) -> Upload {
        Upload {
            reviews: (0..count)
                .map(|index| Review {
                    id: day * 86_400_000 + 12 * 3_600_000 + index * 60_000 + user.len() as i64,
                    cid: index + 1,
                    last_ivl: 30,
                    ivl: None,
                    time_ms: 5_000,
                    kind: 1,
                })
                .collect(),
            deleted: vec![],
            clock: Clock::default(),
            silent: true,
            catalog: false,
            offsets: Vec::new(),
            decks: None,
        }
    }

    #[tokio::test]
    async fn the_records_board_names_whoever_had_the_best_window() {
        let (app, path) = fixture();
        let day = Clock::default().day(now_ms());
        for (user, count) in [("cerro", 30), ("hill", 8)] {
            let _ = upload(
                State(app.clone()),
                UrlPath(user.into()),
                headers(user),
                Json(studied(user, count, day)),
            )
            .await
            .unwrap();
        }
        let board = records(
            State(app.clone()),
            Query(BoardQuery::default()),
            HeaderMap::new(),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            board.iter().map(|r| r.window.as_str()).collect::<Vec<_>>(),
            [Records::NAMES.as_slice(), &["streak", "days"]].concat()
        );
        for window in board.iter().filter(|window| window.unit == "xp") {
            assert_eq!(
                window
                    .holders
                    .iter()
                    .map(|h| h.user.as_str())
                    .collect::<Vec<_>>(),
                vec!["cerro", "hill"],
                "{} lists the holder and whoever came closest",
                window.window
            );
            assert_eq!(window.holders[0].display, "Cerro");
            assert!(window.holders[0].value > window.holders[1].value);
            assert!(window.holders[1].detail > 0);
        }
        assert_eq!(
            board[0].holders[0].detail, 30,
            "all thirty land inside one hour"
        );
        let lifetime: Vec<&RecordBoard> = board
            .iter()
            .filter(|window| window.unit == "days")
            .collect();
        assert_eq!(
            lifetime
                .iter()
                .map(|w| w.window.as_str())
                .collect::<Vec<_>>(),
            vec!["streak", "days"]
        );
        for window in lifetime {
            assert_eq!(
                window.holders.len(),
                2,
                "{} names everyone who has studied",
                window.window
            );
            assert!(
                window
                    .holders
                    .iter()
                    .all(|holder| holder.value == 1 && holder.detail == 0)
            );
            assert!(window.holders.iter().all(|holder| holder.at > 0));
        }
        cleanup(app, path);
    }

    #[tokio::test]
    async fn rank_reports_the_order_and_how_a_player_moved() {
        let (app, path) = fixture();
        let start = now_ms() - 600_000;
        upload_as(&app, "hill", plain_upload(reviews_at(start, 3))).await;
        upload_as(&app, "cerro", plain_upload(reviews_at(start, 1))).await;
        upload_as(&app, "cerro", plain_upload(reviews_at(start, 20))).await;
        let answer = rank(
            State(app.clone()),
            UrlPath("cerro".into()),
            HeaderMap::new(),
            Json(RankQuery {
                previous: vec!["hill".into(), "cerro".into()],
                week: Some(app.week.start_of(now_ms())),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(answer.order, vec!["cerro", "hill"]);
        let change = answer.change.unwrap();
        assert_eq!(change.title, "👑 You took the crown");
        assert_eq!(change.body, "You passed Hill.");
        let first_look = rank(
            State(app.clone()),
            UrlPath("cerro".into()),
            HeaderMap::new(),
            Json(RankQuery {
                previous: vec![],
                week: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(first_look.change.is_none());
        assert_eq!(first_look.week, app.week.start_of(now_ms()));
        cleanup(app, path);
    }

    async fn rank_of(
        app: &Arc<App>,
        user: &str,
        previous: &[&str],
        week: Option<i64>,
    ) -> RankResponse {
        rank(
            State(app.clone()),
            UrlPath(user.into()),
            HeaderMap::new(),
            Json(RankQuery {
                previous: previous.iter().map(|user| user.to_string()).collect(),
                week,
            }),
        )
        .await
        .unwrap()
        .0
    }

    async fn phone_rank(app: &Arc<App>, user: &str, previous: &[&str]) -> RankResponse {
        rank(
            State(app.clone()),
            UrlPath(user.into()),
            headers(user),
            Json(RankQuery {
                previous: previous.iter().map(|user| user.to_string()).collect(),
                week: None,
            }),
        )
        .await
        .unwrap()
        .0
    }

    /// The scenario from issue #125: Hill in Berlin and Cerro in Buenos Aires on a Berlin
    /// week. Phones that keep only the order compare it across the weekly reset.
    #[tokio::test]
    async fn an_order_seen_before_the_reset_is_not_compared_across_time_zones() {
        let (store, path) = temporary_store();
        let config = serde_json::from_value(serde_json::json!({
            "users": {
                "cerro": {"display": "Cerro", "token": "cerro-secret"},
                "hill": {"display": "Hill", "token": "hill-secret"}
            }
        }))
        .unwrap();
        let mut app = app_from(store, config);
        let berlin = chrono_tz::Europe::Berlin;
        app.week = Week {
            tz: berlin,
            rollover_hour: 4,
        };
        let app = Arc::new(app);
        let at_berlin = |day, hour, minute| {
            use chrono::TimeZone;
            berlin
                .with_ymd_and_hms(2026, 9, day, hour, minute, 0)
                .unwrap()
                .timestamp_millis()
        };
        let in_berlin = Clock {
            offset_west_min: -120,
            rollover_hour: 4,
        };
        let in_buenos_aires = Clock {
            offset_west_min: 180,
            rollover_hour: 4,
        };
        let study = |clock, first, count| Upload {
            clock,
            ..plain_upload(reviews_at(first, count))
        };

        crate::app::fix_now_ms(Some(at_berlin(20, 18, 0)));
        upload_as(&app, "hill", study(in_berlin, at_berlin(20, 17, 0), 40)).await;
        upload_as(
            &app,
            "cerro",
            study(in_buenos_aires, at_berlin(20, 16, 0), 10),
        )
        .await;
        let sunday = phone_rank(&app, "cerro", &[]).await;
        assert_eq!(sunday.order, vec!["hill", "cerro"]);
        assert!(phone_rank(&app, "hill", &[]).await.change.is_none());

        crate::app::fix_now_ms(Some(at_berlin(21, 9, 30)));
        assert!(app.week.start_of(now_ms()) > sunday.week);
        upload_as(
            &app,
            "cerro",
            study(in_buenos_aires, at_berlin(21, 9, 10), 5),
        )
        .await;
        crate::app::fix_now_ms(Some(at_berlin(21, 10, 30)));
        upload_as(&app, "hill", study(in_berlin, at_berlin(21, 10, 0), 2)).await;

        crate::app::fix_now_ms(Some(at_berlin(21, 11, 0)));
        let monday = phone_rank(&app, "cerro", &["hill", "cerro"]).await;
        assert_eq!(monday.order, vec!["cerro", "hill"]);
        assert!(
            monday.change.is_none(),
            "Cerro led all week; Hill only led last week: {:?}",
            monday.change.map(|change| change.title)
        );
        assert!(
            phone_rank(&app, "hill", &["hill", "cerro"])
                .await
                .change
                .is_none(),
            "nobody passed Hill this week either"
        );

        crate::app::fix_now_ms(Some(at_berlin(21, 12, 0)));
        upload_as(&app, "hill", study(in_berlin, at_berlin(21, 11, 30), 30)).await;
        let climbed = phone_rank(&app, "hill", &["cerro", "hill"]).await;
        assert_eq!(climbed.order, vec!["hill", "cerro"]);
        assert_eq!(climbed.change.unwrap().title, "👑 You took the crown");
        assert_eq!(
            phone_rank(&app, "cerro", &["cerro", "hill"])
                .await
                .change
                .unwrap()
                .title,
            "👑 Hill took the crown"
        );
        crate::app::fix_now_ms(None);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn passing_players_whose_week_just_reset_is_no_news() {
        let (app, path) = fixture();
        let now = now_ms();
        let week = app.week.start_of(now);
        assert!(week <= now && now < app.week.end_after(now));
        // Hill studied a lot last week and nothing yet this week; Cerro is the first to study.
        upload_as(
            &app,
            "hill",
            plain_upload(reviews_at(week - 3 * 86_400_000, 40)),
        )
        .await;
        upload_as(&app, "cerro", plain_upload(reviews_at(now - 600_000, 2))).await;
        let answer = rank_of(&app, "cerro", &["hill", "cerro"], Some(week)).await;
        assert_eq!(answer.order, vec!["cerro", "hill"]);
        assert!(
            answer.change.is_none(),
            "{:?}",
            answer.change.map(|c| c.title)
        );
        assert!(
            rank_of(&app, "cerro", &["hill", "cerro"], None)
                .await
                .change
                .is_none(),
            "older clients without a week are spared too"
        );
        // Once Hill studies more than Cerro this week, passing is real again.
        upload_as(&app, "hill", plain_upload(reviews_at(now - 300_000, 20))).await;
        let answer = rank_of(&app, "hill", &["cerro", "hill"], Some(week)).await;
        assert_eq!(answer.change.unwrap().title, "👑 You took the crown");
        // An order seen in another week is not compared with this one.
        assert!(
            rank_of(
                &app,
                "hill",
                &["cerro", "hill"],
                Some(week - 7 * 86_400_000)
            )
            .await
            .change
            .is_none()
        );
        cleanup(app, path);
    }

    /// A new account that studied `count` cards ten minutes ago, and its device token.
    fn account(app: &App, user: &str, count: i64) -> String {
        let display = format!("{}{}", user[..1].to_uppercase(), &user[1..]);
        crate::accounts::create(&app.store.lock().unwrap().conn, user, &display, "", 0).unwrap();
        app.accounts.insert(user, &display);
        with_reviews(app, user, count);
        app.issue_token(user, "Phone").unwrap()
    }

    fn opt_in(app: &App, user: &str) {
        let mut store = app.store.lock().unwrap();
        store.set_global_leaderboard(user, true).unwrap();
    }

    fn make_private(app: &App, user: &str) {
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "insert into visibility (user, public) values (?1, 0)",
                [user],
            )
            .unwrap();
    }

    async fn get(
        app: &Arc<App>,
        uri: &str,
        token: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let mut request = axum::http::Request::builder().uri(uri);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        json_response(route(app, request.body(axum::body::Body::empty()).unwrap()).await).await
    }

    fn users(board: &serde_json::Value) -> Vec<&str> {
        board
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["user"].as_str().unwrap())
            .collect()
    }

    fn row<'a>(board: &'a serde_json::Value, user: &str) -> &'a serde_json::Value {
        board
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["user"] == user)
            .unwrap()
    }

    #[tokio::test]
    async fn without_a_scope_and_with_friends_the_board_is_the_circle_as_before() {
        let (app, path) = fixture();
        with_reviews(&app, "hill", 5);
        with_reviews(&app, "cerro", 3);
        let ana = account(&app, "ana", 40);
        opt_in(&app, "ana");
        let (status, default) = get(&app, "/api/leaderboard", Some("hill-secret")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(users(&default), ["hill", "cerro"]);
        assert!(default[0]["today_reviews"].is_u64());
        assert!(default[0].get("rank").is_none(), "{default}");
        for scope in ["friends", "circle"] {
            let uri = format!("/api/leaderboard?scope={scope}");
            assert_eq!(get(&app, &uri, Some("hill-secret")).await.1, default);
        }
        let (_, own) = get(&app, "/api/leaderboard", Some(&ana)).await;
        assert_eq!(users(&own), ["ana"], "an account starts alone");
        assert_eq!(
            get(&app, "/api/leaderboard?scope=nobody", Some(&ana))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_group_board_lists_its_members_only_for_members() {
        let (app, path) = fixture();
        let ana = account(&app, "ana", 4);
        let bobby = account(&app, "bobby", 6);
        let carla = account(&app, "carla", 8);
        app.store
            .lock()
            .unwrap()
            .conn
            .execute_batch(
                "insert into groups values (1, 'Spanish club', 'invite', 'ana', 0);
                 insert into group_members values (1, 'ana', 1, 0), (1, 'bobby', 0, 0);
                 insert into groups values (2, 'Night owls', 'other', 'carla', 0);
                 insert into group_members values (2, 'carla', 1, 0), (2, 'ana', 0, 0);",
            )
            .unwrap();
        for uri in [
            "/api/leaderboard?scope=group&group=1",
            "/api/leaderboard?scope=group:1",
        ] {
            let (status, board) = get(&app, uri, Some(&bobby)).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(users(&board), ["bobby", "ana"], "{uri}");
        }
        let (_, circle) = get(&app, "/api/leaderboard?scope=friends", Some(&ana)).await;
        assert_eq!(users(&circle), ["carla", "bobby", "ana"]);
        for uri in [
            "/api/leaderboard?scope=group&group=1",
            "/api/leaderboard?scope=group&group=99",
            "/api/records?scope=group&group=1",
        ] {
            assert_eq!(
                get(&app, uri, Some(&carla)).await.0,
                StatusCode::FORBIDDEN,
                "{uri}"
            );
        }
        assert_eq!(
            get(&app, "/api/leaderboard?scope=group", Some(&ana))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn the_global_board_lists_everyone_but_who_opted_out_or_went_private() {
        let (app, path) = app_with(serde_json::json!({"registration": true}));
        let ana = account(&app, "ana", 4);
        account(&app, "bobby", 6);
        let carla = account(&app, "carla", 8);
        let assert_summary = |stranger: &serde_json::Value| {
            let fields: BTreeSet<&str> = stranger
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                fields,
                BTreeSet::from(["display", "level", "period", "rank", "streak", "user", "xp"]),
                "strangers see a standing without study details"
            );
        };
        let (status, board) =
            get(&app, "/api/leaderboard?scope=global&period=day", Some(&ana)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            users(&board),
            ["carla", "bobby", "ana"],
            "profiles are public and listed until their player says otherwise"
        );
        assert_summary(row(&board, "bobby"));
        assert_eq!(row(&board, "ana")["shown"], true);
        assert_eq!(
            get(&app, "/api/decks/ana", Some(&ana)).await.1["global_leaderboard"],
            true
        );

        // Bobby makes his profile private, which takes him off the board until he turns it on.
        make_private(&app, "bobby");
        let (_, board) = get(&app, "/api/leaderboard?scope=global&period=day", Some(&ana)).await;
        assert_eq!(users(&board), ["carla", "ana"]);
        opt_in(&app, "bobby");
        let (_, board) = get(&app, "/api/leaderboard?scope=global&period=day", Some(&ana)).await;
        assert_eq!(users(&board), ["carla", "bobby", "ana"]);
        let mine = row(&board, "ana");
        assert_eq!(
            (mine["me"].as_bool(), mine["shown"].as_bool()),
            (Some(true), Some(true))
        );
        assert_eq!(mine["rank"], 3);
        assert!(
            mine["today_reviews"].is_u64(),
            "the viewer sees their own row in full"
        );
        let stranger = row(&board, "bobby");
        assert_summary(stranger);
        assert_eq!(stranger["display"], "Bobby");
        assert_eq!(stranger["period"], "day");
        assert_eq!(
            get(&app, "/api/profile/bobby", Some(&ana)).await.0,
            StatusCode::NOT_FOUND,
            "the profile stays private"
        );
        assert_eq!(
            get(&app, "/api/profile/carla", Some(&ana)).await.0,
            StatusCode::OK,
            "a profile nobody made private is public"
        );

        // Carla turns it off through the settings the app reads.
        let set = axum::http::Request::builder()
            .method("POST")
            .uri("/api/decks/carla")
            .header("authorization", format!("Bearer {carla}"))
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"decks": [], "global_leaderboard": false}"#,
            ))
            .unwrap();
        let (status, settings) = json_response(route(&app, set).await).await;
        assert_eq!(
            (status, settings["global_leaderboard"].as_bool()),
            (StatusCode::OK, Some(false))
        );
        assert_eq!(
            get(&app, "/api/decks/carla", Some(&carla)).await.1["global_leaderboard"],
            false
        );
        let (_, board) = get(&app, "/api/leaderboard?scope=global", Some(&carla)).await;
        assert_eq!(users(&board), ["carla", "bobby", "ana"]);
        assert_eq!(row(&board, "carla")["shown"], false);
        let (_, board) = get(&app, "/api/leaderboard?scope=global", Some(&ana)).await;
        assert!(!users(&board).contains(&"carla"), "an opt-out stays out");

        // Friends see each other in full there too.
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "insert into friendships (a, b, requested_by, accepted, created_at)
                 values ('ana', 'bobby', 'ana', 1, 0)",
                [],
            )
            .unwrap();
        let (_, board) = get(&app, "/api/leaderboard?scope=global", Some(&ana)).await;
        assert!(row(&board, "bobby")["today_reviews"].is_u64());
        assert!(row(&board, "bobby").get("me").is_none());

        // Without signing in, only the listed public profiles are there.
        for route in ["/api/leaderboard?scope=global", "/api/leaderboard"] {
            let (status, board) = get(&app, route, None).await;
            assert_eq!((status, users(&board)), (StatusCode::OK, vec!["ana"]));
            assert_summary(row(&board, "ana"));
        }
        let (_, records) = get(&app, "/api/records?scope=global", None).await;
        assert!(
            !records.to_string().contains("carla") && !records.to_string().contains("bobby"),
            "{records}"
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn the_global_board_leaves_out_people_the_viewer_muted() {
        let (app, path) = app_with(serde_json::json!({"registration": true}));
        let ana = account(&app, "ana", 4);
        let bobby = account(&app, "bobby", 6);
        opt_in(&app, "ana");
        opt_in(&app, "bobby");
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "insert into muted_people (recipient, sender, created_at) values ('ana', 'bobby', 0)",
                [],
            )
            .unwrap();
        let (_, board) = get(&app, "/api/leaderboard?scope=global", Some(&ana)).await;
        assert_eq!(users(&board), ["ana"]);
        let (_, board) = get(&app, "/api/leaderboard?scope=global", Some(&bobby)).await;
        assert_eq!(
            users(&board),
            ["bobby", "ana"],
            "muting is one way, as everywhere else"
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn the_global_board_lists_the_top_hundred_and_the_viewer() {
        let (app, path) = app_with(serde_json::json!({"registration": true}));
        for index in 0..105 {
            let user = format!("player{index:03}");
            account(&app, &user, index + 2);
            opt_in(&app, &user);
        }
        let last = account(&app, "zoe", 1);
        let (_, board) = get(&app, "/api/leaderboard?scope=global", Some(&last)).await;
        let rows = board.as_array().unwrap();
        assert_eq!(rows.len(), GLOBAL_ROWS + 1);
        assert_eq!(rows[0]["rank"], 1);
        assert_eq!(rows[GLOBAL_ROWS - 1]["rank"], GLOBAL_ROWS);
        let mine = &rows[GLOBAL_ROWS];
        assert_eq!(
            (&mine["user"], &mine["rank"], &mine["me"]),
            (
                &serde_json::json!("zoe"),
                &serde_json::json!(106),
                &serde_json::json!(true)
            )
        );
        let top = app
            .issue_token(rows[0]["user"].as_str().unwrap(), "Tablet")
            .unwrap();
        let (_, board) = get(&app, "/api/leaderboard?scope=global", Some(&top)).await;
        assert_eq!(
            board.as_array().unwrap().len(),
            GLOBAL_ROWS,
            "the viewer is in the top already"
        );
        cleanup(app, path);
    }
}
