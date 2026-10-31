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
             reached_day integer
         ) without rowid;",
    )?;
    Ok(())
}

/// The default value and allowed range of each mode that has one.
fn limits(mode: &str) -> Option<Option<(u32, u32, u32)>> {
    match mode {
        "off" => Some(None),
        "cards" => Some(Some((100, 1, 5000))),
        "minutes" => Some(Some((20, 1, 600))),
        "backlog" => Some(Some((10, 1, 100))),
        _ => None,
    }
}

/// A goal as clients send and receive it. `value` may be left out, and is kept but unused
/// while the mode is `off`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    pub mode: String,
    #[serde(default)]
    pub value: Option<u32>,
}

impl Setting {
    /// The setting as stored, with the mode's default filled in, or `None` when it is invalid.
    fn checked(self) -> Option<Self> {
        match limits(&self.mode)? {
            None => Some(Self {
                mode: self.mode,
                value: None,
            }),
            Some((default, low, high)) => {
                let value = self.value.unwrap_or(default);
                (low..=high).contains(&value).then_some(Self {
                    mode: self.mode,
                    value: Some(value),
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
    /// Reviews or minutes so far; `null` for `off` and `backlog`.
    pub progress: Option<u64>,
    pub reached: bool,
}

fn stored(conn: &Connection, user: &str) -> rusqlite::Result<(Setting, Option<i64>)> {
    Ok(conn
        .query_row(
            "select mode, value, reached_day from goals where user = ?1",
            [user],
            |r| {
                Ok((
                    Setting {
                        mode: r.get(0)?,
                        value: r.get::<_, Option<u32>>(1)?.or(Some(100)),
                    },
                    r.get(2)?,
                ))
            },
        )
        .optional()?
        .unwrap_or((
            Setting {
                mode: "backlog".into(),
                value: Some(10),
            },
            None,
        )))
}

/// The player's goal and how far today's study is toward it.
pub fn view(conn: &Connection, user: &str, profile: &Profile) -> rusqlite::Result<GoalView> {
    let (setting, reached_day) = stored(conn, user)?;
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
    let (setting, reached_day) = stored(conn, user)?;
    if setting.mode != "backlog" || day != today || reached_day == Some(day) {
        return Ok(false);
    }
    conn.execute(
        "update goals set reached_day = ?2 where user = ?1",
        params![user, day],
    )?;
    Ok(true)
}

fn save(conn: &Connection, user: &str, setting: &Setting) -> rusqlite::Result<()> {
    conn.execute(
        "insert into goals (user, mode, value) values (?1, ?2, ?3)
         on conflict (user) do update set mode = excluded.mode, value = coalesce(excluded.value, goals.value)",
        params![user, setting.mode, setting.value],
    )?;
    Ok(())
}

async fn get_goal(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Setting>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let store = app.store.lock().unwrap();
    Ok(Json(stored(&store.conn, &user).map_err(store_error)?.0))
}

async fn set_goal(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(setting): Json<Setting>,
) -> Result<Json<Setting>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let setting = setting.checked().ok_or(StatusCode::BAD_REQUEST)?;
    let store = app.store.lock().unwrap();
    save(&store.conn, &user, &setting).map_err(store_error)?;
    Ok(Json(stored(&store.conn, &user).map_err(store_error)?.0))
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
    let today = clock_in(&store.conn, &user)
        .map_err(store_error)?
        .day(now_ms());
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
        }
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
            Some(setting("backlog", Some(10)))
        );
        assert_eq!(
            setting("off", Some(7)).checked(),
            Some(setting("off", None))
        );
        for bad in [
            setting("cards", Some(0)),
            setting("cards", Some(5001)),
            setting("minutes", Some(601)),
            setting("backlog", Some(101)),
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
    async fn the_goal_is_private_validated_and_defaults_to_a_tenth_of_the_backlog() {
        let (app, path) = fixture();
        let get = |user: &str, token: &str| {
            get_goal(State(app.clone()), UrlPath(user.into()), headers(token))
        };
        assert_eq!(
            get("hill", "hill").await.unwrap().0,
            setting("backlog", Some(10))
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
            put(setting("minutes", Some(30))).await.unwrap().0,
            setting("minutes", Some(30))
        );
        assert_eq!(
            get("hill", "hill").await.unwrap().0,
            setting("minutes", Some(30))
        );
        let off: Setting = serde_json::from_value(serde_json::json!({"mode": "off"})).unwrap();
        assert_eq!(put(off).await.unwrap().0, setting("off", Some(30)));
        assert_eq!(
            get("hill", "hill").await.unwrap().0,
            setting("off", Some(30))
        );
        assert_eq!(
            get("cerro", "cerro").await.unwrap().0,
            setting("backlog", Some(10))
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
        save(&store.conn, "hill", &setting("cards", Some(100))).unwrap();
        let today = view(&store.conn, "hill", &profile).unwrap();
        assert_eq!(
            (today.progress, today.reached),
            (Some(3), false),
            "three of a hundred cards"
        );
        assert!(event(&store.conn, "hill", &profile).unwrap().is_none());
        save(&store.conn, "hill", &setting("cards", Some(3))).unwrap();
        let moment = event(&store.conn, "hill", &profile).unwrap().unwrap();
        assert_eq!(moment.key, format!("goal:{}", profile.day));
        assert_eq!(moment.title, "Daily goal reached!");
        assert_eq!(moment.body, "3 cards today in 0 minutes.");
        save(&store.conn, "hill", &setting("backlog", Some(100))).unwrap();
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
}
