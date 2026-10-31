//! The daily goal, kept on the server so every client agrees on it. Reviews and minutes are
//! judged from uploads; only clients know due counts, so they report a reached backlog goal.

use crate::access::authorized;
use crate::app::{App, now_ms};
use crate::game::{Event, Profile, goal_event};
use crate::store::{Error, clock_in};
use crate::web_util::store_error;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists goals (
             user text primary key,
             mode text not null,
             value integer,
             reached_day integer,
             since integer
         ) without rowid;",
    )?;
    if !crate::store::has_column(conn, "goals", "since")? {
        conn.execute("alter table goals add column since integer", [])?;
    }
    Ok(())
}

/// The default value and allowed range of each mode that has one.
fn limits(mode: &str) -> Option<Option<(u32, u32, u32)>> {
    match mode {
        "off" => Some(None),
        "cards" => Some(Some((100, 1, 5000))),
        "minutes" => Some(Some((20, 1, 600))),
        "backlog" => Some(Some((30, 1, 365))),
        _ => None,
    }
}

/// A goal as clients send it. `value` may be left out, and is kept but unused while the mode
/// is `off`; `restart` starts the backlog plan again from today.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    pub mode: String,
    #[serde(default)]
    pub value: Option<u32>,
    #[serde(default)]
    pub restart: bool,
}

/// A goal as the server keeps it: `since` is the Anki day the plan started, and `chosen` says
/// whether the player ever picked one.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Goal {
    pub mode: String,
    pub value: Option<u32>,
    pub since: i64,
    pub chosen: bool,
}

impl Setting {
    /// The setting as stored, with the mode's default filled in, or `None` when it is invalid.
    fn checked(self) -> Option<Self> {
        match limits(&self.mode)? {
            None => Some(Self {
                value: None,
                ..self
            }),
            Some((default, low, high)) => {
                let value = self.value.unwrap_or(default);
                (low..=high).contains(&value).then_some(Self {
                    value: Some(value),
                    ..self
                })
            }
        }
    }
}

/// The goal in the owner's profile, with today's progress.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct GoalView {
    pub mode: String,
    pub value: Option<u32>,
    pub since: i64,
    /// Reviews or minutes so far; `null` for `off` and `backlog`.
    pub progress: Option<u64>,
    pub reached: bool,
}

fn stored(conn: &Connection, user: &str, today: i64) -> rusqlite::Result<(Goal, Option<i64>)> {
    Ok(conn
        .query_row(
            "select mode, value, reached_day, since from goals where user = ?1",
            [user],
            |r| {
                Ok((
                    Goal {
                        mode: r.get(0)?,
                        value: r.get::<_, Option<u32>>(1)?.or(Some(100)),
                        since: r.get::<_, Option<i64>>(3)?.unwrap_or(today),
                        chosen: true,
                    },
                    r.get(2)?,
                ))
            },
        )
        .optional()?
        .unwrap_or((
            Goal {
                mode: "backlog".into(),
                value: Some(30),
                since: today,
                chosen: false,
            },
            None,
        )))
}

/// The player's goal and how far today's study is toward it.
pub fn view(conn: &Connection, user: &str, profile: &Profile) -> rusqlite::Result<GoalView> {
    let (setting, reached_day) = stored(conn, user, profile.day)?;
    let value = u64::from(setting.value.unwrap_or(0));
    let progress = match setting.mode.as_str() {
        "cards" => Some(profile.today.reviews),
        "minutes" => Some(profile.today.minutes),
        _ => None,
    };
    let reached = match setting.mode.as_str() {
        "backlog" => reached_day == Some(profile.day),
        "off" => false,
        _ => progress.is_some_and(|progress| progress >= value),
    };
    Ok(GoalView {
        mode: setting.mode,
        value: setting.value,
        since: setting.since,
        progress,
        reached,
    })
}

/// The `goal:{day}` moment, once today's goal is reached.
pub fn event(conn: &Connection, user: &str, profile: &Profile) -> rusqlite::Result<Option<Event>> {
    Ok(view(conn, user, profile)?
        .reached
        .then(|| goal_event(profile.day, profile.today.reviews, profile.today.minutes)))
}

/// Records a reached backlog goal for `day`, once, if that is the player's mode and today.
fn report(conn: &Connection, user: &str, day: i64, today: i64) -> rusqlite::Result<bool> {
    let (setting, reached_day) = stored(conn, user, today)?;
    if setting.mode != "backlog" || day != today || reached_day == Some(day) {
        return Ok(false);
    }
    conn.execute(
        "update goals set reached_day = ?2 where user = ?1",
        params![user, day],
    )?;
    Ok(true)
}

/// Saves a goal. A new mode or value, or a restart, starts the plan from `today`.
fn save(conn: &Connection, user: &str, setting: &Setting, today: i64) -> rusqlite::Result<()> {
    let (current, _) = stored(conn, user, today)?;
    let value = setting.value.or(current.value);
    let unchanged = current.chosen && current.mode == setting.mode && current.value == value;
    let since = if unchanged && !setting.restart {
        current.since
    } else {
        today
    };
    conn.execute(
        "insert into goals (user, mode, value, since) values (?1, ?2, ?3, ?4)
         on conflict (user) do update set mode = excluded.mode, value = excluded.value, since = excluded.since",
        params![user, setting.mode, value, since],
    )?;
    Ok(())
}

fn today_of(conn: &Connection, user: &str) -> rusqlite::Result<i64> {
    Ok(clock_in(conn, user)?.day(now_ms()))
}

async fn get_goal(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Goal>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let store = app.store.lock().unwrap();
    let today = today_of(&store.conn, &user).map_err(store_error)?;
    Ok(Json(stored(&store.conn, &user, today).map_err(store_error)?.0))
}

async fn set_goal(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(setting): Json<Setting>,
) -> Result<Json<Goal>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let setting = setting.checked().ok_or(StatusCode::BAD_REQUEST)?;
    let store = app.store.lock().unwrap();
    let today = today_of(&store.conn, &user).map_err(store_error)?;
    save(&store.conn, &user, &setting, today).map_err(store_error)?;
    Ok(Json(stored(&store.conn, &user, today).map_err(store_error)?.0))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reached {
    day: i64,
}

async fn reached(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(reached): Json<Reached>,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let store = app.store.lock().unwrap();
    let today = today_of(&store.conn, &user).map_err(store_error)?;
    if report(&store.conn, &user, reached.day, today).map_err(store_error)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::CONFLICT)
    }
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/goal/{user}", get(get_goal).put(set_goal))
        .route("/api/goal/{user}/reached", post(reached))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    fn setting(mode: &str, value: Option<u32>) -> Setting {
        Setting {
            mode: mode.into(),
            value,
            restart: false,
        }
    }

    fn shown(goal: Goal) -> (String, Option<u32>, bool) {
        (goal.mode, goal.value, goal.chosen)
    }

    fn plain(mode: &str, value: u32, chosen: bool) -> (String, Option<u32>, bool) {
        (mode.into(), Some(value), chosen)
    }

    #[test]
    fn modes_have_defaults_and_ranges() {
        assert_eq!(
            setting("cards", None).checked(),
            Some(setting("cards", Some(100)))
        );
        assert_eq!(
            setting("minutes", None).checked(),
            Some(setting("minutes", Some(20)))
        );
        assert_eq!(
            setting("backlog", None).checked(),
            Some(setting("backlog", Some(30)))
        );
        assert_eq!(
            setting("off", Some(7)).checked(),
            Some(setting("off", None))
        );
        for bad in [
            setting("cards", Some(0)),
            setting("cards", Some(5001)),
            setting("minutes", Some(601)),
            setting("backlog", Some(366)),
            setting("weekly", Some(3)),
        ] {
            assert_eq!(bad.checked(), None);
        }
        assert_eq!(
            setting("cards", Some(5000)).checked(),
            Some(setting("cards", Some(5000)))
        );
    }

    #[tokio::test]
    async fn the_goal_is_private_validated_and_defaults_to_a_month_of_backlog() {
        let (app, path) = fixture();
        let get = |user: &str, token: &str| {
            get_goal(State(app.clone()), UrlPath(user.into()), headers(token))
        };
        assert_eq!(
            shown(get("hill", "hill").await.unwrap().0),
            plain("backlog", 30, false)
        );
        assert_eq!(
            get("hill", "cerro").await.err(),
            Some(StatusCode::UNAUTHORIZED)
        );
        let put = |body: Setting| {
            set_goal(
                State(app.clone()),
                UrlPath("hill".into()),
                headers("hill"),
                Json(body),
            )
        };
        assert_eq!(
            put(setting("minutes", Some(0))).await.err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            shown(put(setting("minutes", Some(30))).await.unwrap().0),
            plain("minutes", 30, true)
        );
        assert_eq!(
            shown(get("hill", "hill").await.unwrap().0),
            plain("minutes", 30, true)
        );
        let off: Setting = serde_json::from_value(serde_json::json!({"mode": "off"})).unwrap();
        assert_eq!(shown(put(off).await.unwrap().0), plain("off", 30, true));
        assert_eq!(
            shown(get("hill", "hill").await.unwrap().0),
            plain("off", 30, true)
        );
        assert_eq!(
            shown(get("cerro", "cerro").await.unwrap().0),
            plain("backlog", 30, false)
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn a_backlog_reach_is_accepted_once_for_today_only() {
        let (app, path) = fixture();
        let today = Clock::default().day(now_ms());
        let report = |day: i64| {
            reached(
                State(app.clone()),
                UrlPath("hill".into()),
                headers("hill"),
                Json(Reached { day }),
            )
        };
        let _ = set_goal(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(setting("cards", Some(100))),
        )
        .await
        .unwrap();
        assert_eq!(report(today).await.err(), Some(StatusCode::CONFLICT));
        let _ = set_goal(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(setting("backlog", Some(80))),
        )
        .await
        .unwrap();
        assert_eq!(report(today - 1).await.err(), Some(StatusCode::CONFLICT));
        assert_eq!(report(today).await.unwrap(), StatusCode::NO_CONTENT);
        assert_eq!(report(today).await.err(), Some(StatusCode::CONFLICT));
        assert_eq!(
            reached(
                State(app.clone()),
                UrlPath("hill".into()),
                headers("cerro"),
                Json(Reached { day: today })
            )
            .await
            .err(),
            Some(StatusCode::UNAUTHORIZED)
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn progress_and_the_moment_follow_the_mode() {
        let (store, path) = crate::decks::tests::temporary_store();
        let mut profile = game::compute_with_history(
            "hill",
            "Hill",
            &reviews_at(now_ms() - 600_000, 3),
            &ClockHistory::default(),
            &Week::default(),
            now_ms(),
            &game::FreezePolicy::default(),
        );
        save(&store.conn, "hill", &setting("cards", Some(100)), profile.day).unwrap();
        let today = view(&store.conn, "hill", &profile).unwrap();
        assert_eq!(
            (today.progress, today.reached),
            (Some(3), false),
            "three of a hundred cards"
        );
        assert!(event(&store.conn, "hill", &profile).unwrap().is_none());
        save(&store.conn, "hill", &setting("cards", Some(3)), profile.day).unwrap();
        let moment = event(&store.conn, "hill", &profile).unwrap().unwrap();
        assert_eq!(moment.key, format!("goal:{}", profile.day));
        assert_eq!(moment.title, "Daily goal reached!");
        assert_eq!(moment.body, "3 cards today in 0 minutes.");
        save(&store.conn, "hill", &setting("backlog", Some(100)), profile.day).unwrap();
        assert_eq!(view(&store.conn, "hill", &profile).unwrap().progress, None);
        assert!(event(&store.conn, "hill", &profile).unwrap().is_none());
        assert!(report(&store.conn, "hill", profile.day, profile.day).unwrap());
        assert!(event(&store.conn, "hill", &profile).unwrap().is_some());
        profile.day += 1;
        assert!(
            event(&store.conn, "hill", &profile).unwrap().is_none(),
            "a reported reach counts for its own day"
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn a_plan_starts_when_the_goal_changes_or_restarts() {
        let (store, path) = crate::decks::tests::temporary_store();
        let since = |today| stored(&store.conn, "hill", today).unwrap().0.since;
        assert_eq!(since(5), 5, "an unchosen goal starts today");
        save(&store.conn, "hill", &setting("backlog", Some(14)), 10).unwrap();
        assert_eq!(since(12), 10);
        save(&store.conn, "hill", &setting("backlog", Some(14)), 12).unwrap();
        assert_eq!(since(12), 10, "saving the same goal keeps its plan");
        save(&store.conn, "hill", &setting("backlog", Some(30)), 13).unwrap();
        assert_eq!(since(13), 13, "a new length starts a new plan");
        let restart = Setting {
            restart: true,
            ..setting("backlog", Some(30))
        };
        save(&store.conn, "hill", &restart, 20).unwrap();
        assert_eq!(since(20), 20);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
}
