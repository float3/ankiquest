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
use std::collections::BTreeSet;
use std::sync::Arc;

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
}

#[derive(Serialize)]
struct RankResponse {
    order: Vec<String>,
    change: Option<feedback::Notice>,
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
    let users = if authorized(&app, &user, &headers) {
        app.circle(&app.store.lock().unwrap().conn, &user)
            .map_err(store_error)?
    } else {
        board_users(&app, &headers, None)?
    };
    let board = standings(&app, &users, None);
    let places: Vec<feedback::Place> = board
        .iter()
        .map(|standing| feedback::Place {
            user: &standing.user,
            display: &standing.display,
            week_xp: standing.week_xp,
        })
        .collect();
    let mut change = feedback::rank_change(&query.previous, &places, &user);
    if let Some(change) = &mut change {
        i18n::notice(change, &language);
    }
    Ok(Json(RankResponse {
        order: board.iter().map(|standing| standing.user.clone()).collect(),
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
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
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
            Json(RankQuery { previous: vec![] }),
        )
        .await
        .unwrap()
        .0;
        assert!(first_look.change.is_none());
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
