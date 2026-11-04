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
}

#[derive(Deserialize, Default)]
struct BoardQuery {
    period: Option<String>,
    scope: Option<String>,
}

/// The players a board shows this viewer, or why it cannot.
fn board_users(
    app: &App,
    headers: &HeaderMap,
    scope: Option<&str>,
) -> Result<BTreeSet<String>, StatusCode> {
    let scope = social::Scope::parse(scope).ok_or(StatusCode::BAD_REQUEST)?;
    let viewer = access::viewer(app, headers);
    app.board(&app.store.lock().unwrap().conn, &viewer, scope)
        .map_err(store_error)?
        .ok_or(StatusCode::FORBIDDEN)
}

async fn leaderboard(
    State(app): State<Arc<App>>,
    Query(query): Query<BoardQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<Standing>>, StatusCode> {
    let users = board_users(&app, &headers, query.scope.as_deref())?;
    Ok(Json(standings(&app, &users, query.period)))
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
        board_users(&app, &headers, None)?
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
    let profiles = app.standings_among(&board_users(&app, &headers, query.scope.as_deref())?);
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
    ends_at: i64,
    timezone: String,
}

async fn week_info(State(app): State<Arc<App>>) -> Json<WeekInfo> {
    Json(WeekInfo {
        ends_at: app.week.end_after(now_ms()),
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
        let Json(board) = leaderboard(State(app), Query(BoardQuery::default()), HeaderMap::new())
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
}
