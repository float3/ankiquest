use crate::access::authorized;
use crate::app::App;
use crate::app::now_ms;
use crate::game;
use crate::game::{FreezePolicy, FreezePreference};
use crate::store::{Error, Store};
use crate::web_util::failed_for;
use axum::Json;
use axum::Router;
use axum::extract::Path as UrlPath;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::routing::get;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Protects `dates` (`YYYY-MM-DD`) for `user`, or lifts that protection again.
/// The running service picks the change up on its next tick.
pub fn grant(
    conn: &Connection,
    user: &str,
    dates: &[String],
    granted: bool,
    at: i64,
) -> Result<usize, Error> {
    let tx = conn.unchecked_transaction()?;
    let mut changed = 0;
    for date in dates {
        changed += if granted {
            tx.execute(
                "insert or ignore into freeze_grants (user, date, at) values (?1, ?2, ?3)",
                params![user, date, at],
            )?
        } else {
            tx.execute(
                "delete from freeze_grants where user = ?1 and date = ?2",
                params![user, date],
            )?
        };
    }
    tx.execute(
        "insert or ignore into freeze_grant_reloads (user) values (?1)",
        [user],
    )?;
    tx.commit()?;
    Ok(changed)
}

/// Players whose granted days changed and are not yet reloaded.
pub fn grant_reloads(conn: &Connection) -> Result<Vec<String>, Error> {
    Ok(conn
        .prepare("select user from freeze_grant_reloads")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}

/// Marks a player's granted days as loaded, once their reload succeeded.
pub fn grant_reloaded(conn: &Connection, user: &str) -> Result<(), Error> {
    conn.execute("delete from freeze_grant_reloads where user = ?1", [user])?;
    Ok(())
}

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "create table if not exists freeze_preferences (
             id integer primary key,
             user text not null,
             at integer not null,
             enabled integer not null check (enabled in (0, 1))
         );
         create index if not exists freeze_preferences_user
             on freeze_preferences (user, at, id);
         create table if not exists freeze_policy (
             id integer primary key check (id = 1),
             legacy_until integer
         );
         create table if not exists freeze_legacy_players (
             user text primary key
         );
         create table if not exists freeze_defaults (
             id integer primary key check (id = 1),
             enabled_from integer not null
         );
         create table if not exists freeze_grants (
             user text not null,
             date text not null,
             at integer not null,
             primary key (user, date)
         );
         create table if not exists freeze_grant_reloads (
             user text primary key
         );",
    )?;
    // No preference now means enabled. Keep a fixed boundary so upgrading an
    // opt-in server does not retroactively earn/spend freezes or rewrite XP.
    tx.execute(
        "insert or ignore into freeze_defaults (id, enabled_from) values (1, ?1)",
        [crate::app::now_ms()],
    )?;
    // Existing review histories keep protection already applied by the old rule.
    // New databases never use that rule. This boundary is fixed across restarts.
    let initialized = tx.execute(
        "insert or ignore into freeze_policy (id, legacy_until)
         values (1, case when exists (select 1 from reviews) then ?1 else null end)",
        [crate::app::now_ms()],
    )?;
    if initialized > 0 {
        tx.execute(
            "insert into freeze_legacy_players (user) select distinct user from reviews",
            [],
        )?;
    }
    tx.commit()?;
    Ok(())
}

impl Store {
    pub fn freeze_policy(&self, user: &str) -> Result<FreezePolicy, Error> {
        Ok(FreezePolicy {
            legacy_until: self.conn.query_row(
                "select case when exists (select 1 from freeze_legacy_players where user = ?1)
                 then legacy_until else null end from freeze_policy where id = 1",
                [user],
                |r| r.get(0),
            )?,
            default_enabled_from: Some(self.conn.query_row(
                "select enabled_from from freeze_defaults where id = 1",
                [],
                |r| r.get(0),
            )?),
            preferences: self.freeze_preferences(user)?,
            granted: self.granted_freezes(user)?,
        })
    }

    pub fn granted_freezes(&self, user: &str) -> Result<BTreeSet<String>, Error> {
        Ok(self
            .conn
            .prepare("select date from freeze_grants where user = ?1")?
            .query_map([user], |r| r.get(0))?
            .collect::<Result<_, _>>()?)
    }

    pub fn freeze_preferences(&self, user: &str) -> Result<Vec<FreezePreference>, Error> {
        let mut stmt = self.conn.prepare(
            "select at, enabled from freeze_preferences where user = ?1 order by at, id",
        )?;
        Ok(stmt
            .query_map([user], |r| {
                Ok(FreezePreference {
                    at: r.get(0)?,
                    enabled: r.get(1)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    /// Keep the time of each change so later settings cannot rewrite past rewards
    /// or missed days. Repeated saves of the same preference are a no-op.
    pub fn set_freezes_enabled(&mut self, user: &str, enabled: bool, at: i64) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        let previous: Option<(i64, bool)> = tx
            .query_row(
                "select at, enabled from freeze_preferences where user = ?1 order by at desc, id desc limit 1",
                [user],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let default_enabled_from: i64 = tx.query_row(
            "select enabled_from from freeze_defaults where id = 1",
            [],
            |r| r.get(0),
        )?;
        if previous.map_or(at >= default_enabled_from, |(_, value)| value) != enabled {
            tx.execute(
                "insert into freeze_preferences (user, at, enabled) values (?1, ?2, ?3)",
                params![user, at.max(previous.map_or(at, |(at, _)| at)), enabled],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct FreezeSettings {
    pub(crate) enabled: bool,
    pub(crate) freezes: u32,
    capacity: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FreezeUpdate {
    pub(crate) enabled: bool,
}

fn freeze_settings(app: &App, store: &Store, user: &str) -> Result<FreezeSettings, Error> {
    let policy = store.freeze_policy(user)?;
    let profile = app.profile(user);
    Ok(FreezeSettings {
        enabled: policy.enabled_at(now_ms()),
        freezes: profile.map_or(0, |profile| profile.stored_freezes),
        capacity: game::MAX_FREEZES,
    })
}

async fn get_freezes(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Json<FreezeSettings>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    freeze_settings(&app, &app.store.lock().unwrap(), &user)
        .map(Json)
        .map_err(failed_for("streak freeze settings for", &user))
}

pub(crate) async fn set_freezes(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(update): Json<FreezeUpdate>,
) -> Result<Json<FreezeSettings>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let store_error = failed_for("streak freeze settings for", &user);
    let mut store = app.store.lock().unwrap();
    store
        .set_freezes_enabled(&user, update.enabled, now_ms())
        .map_err(store_error)?;
    // Cache the timeline with the reviews: profile/preview are read-only and are
    // also called while the database lock is held by uploads and imports.
    let known = app.players.read().unwrap().contains_key(&user);
    if known {
        app.reload_player(&store, &user).map_err(store_error)?;
    }
    freeze_settings(&app, &store, &user)
        .map(Json)
        .map_err(store_error)
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new().route(
        "/api/streak-freezes/{user}",
        get(get_freezes).post(set_freezes),
    )
}

#[cfg(test)]
mod tests {
    use crate::app::load_player;
    use crate::decks::tests::temporary_store;
    use crate::freezes::FreezeUpdate;
    use crate::freezes::get_freezes;
    use crate::freezes::set_freezes;
    use crate::study::Pending;
    use crate::study::Upload;
    use crate::study::preview;
    use crate::study::upload;
    use crate::test_support::*;

    #[test]
    fn a_new_database_never_enables_legacy_rules_after_importing_reviews() {
        let (store, path) = temporary_store();
        store
            .conn
            .execute(
                "insert into reviews (user, id, cid, last_ivl, time_ms, kind)
             values ('cerro', 1000, 1, 0, 5000, 1)",
                [],
            )
            .unwrap();
        drop(store);
        let store = crate::store::Store::open(&path).unwrap();
        assert!(store.freeze_policy("cerro").unwrap().legacy_until.is_none());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn migration_preserves_an_existing_history_boundary_once() {
        let (store, path) = temporary_store();
        assert!(store.freeze_policy("cerro").unwrap().legacy_until.is_none());
        // Recreate the database shape before this feature, with a known player.
        store
            .conn
            .execute_batch(
                "drop table freeze_policy;
             drop table freeze_legacy_players;
             drop table freeze_preferences;
             insert into reviews (user, id, cid, last_ivl, time_ms, kind)
             values ('cerro', 1000, 1, 0, 5000, 1);",
            )
            .unwrap();
        drop(store);
        let store = crate::store::Store::open(&path).unwrap();
        let boundary = store.freeze_policy("cerro").unwrap().legacy_until;
        assert!(boundary.is_some());
        assert!(
            store
                .freeze_policy("new-player")
                .unwrap()
                .legacy_until
                .is_none(),
            "players who join after the upgrade must never use the automatic legacy rule"
        );
        assert!(store.freeze_preferences("cerro").unwrap().is_empty());
        store
            .conn
            .execute(
                "insert into reviews (user, id, cid, last_ivl, time_ms, kind)
             values ('new-player', 2000, 2, 0, 5000, 1)",
                [],
            )
            .unwrap();
        drop(store);
        let store = crate::store::Store::open(&path).unwrap();
        assert_eq!(store.freeze_policy("cerro").unwrap().legacy_until, boundary);
        assert!(
            store
                .freeze_policy("new-player")
                .unwrap()
                .legacy_until
                .is_none()
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn opt_out_rollout_keeps_its_boundary_and_saved_choices_across_restarts() {
        let (store, path) = temporary_store();
        store.conn.execute_batch(
            "drop table freeze_defaults;
             insert into freeze_preferences (user, at, enabled) values ('cerro', 10, 0), ('hill', 10, 1);"
        ).unwrap();
        drop(store);
        let store = crate::store::Store::open(&path).unwrap();
        let default = store.freeze_policy("new-player").unwrap();
        let boundary = default.default_enabled_from.unwrap();
        assert!(!default.enabled_at(boundary - 1));
        assert!(default.enabled_at(boundary));
        assert!(!store.freeze_policy("cerro").unwrap().enabled_at(boundary));
        assert!(store.freeze_policy("hill").unwrap().enabled_at(10));
        assert_eq!(store.freeze_preferences("cerro").unwrap().len(), 1);
        drop(store);
        let store = crate::store::Store::open(&path).unwrap();
        assert_eq!(
            store
                .freeze_policy("new-player")
                .unwrap()
                .default_enabled_from,
            Some(boundary)
        );
        assert!(
            !store
                .freeze_policy("cerro")
                .unwrap()
                .enabled_at(crate::app::now_ms())
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn first_opt_out_is_saved_even_without_uploaded_reviews() {
        let (mut store, path) = temporary_store();
        let at = crate::app::now_ms();
        store.set_freezes_enabled("cerro", false, at).unwrap();
        store.set_freezes_enabled("cerro", false, at + 1).unwrap();
        let before = store.freeze_preferences("cerro").unwrap();
        assert_eq!(before.len(), 1);
        assert!(!before[0].enabled);
        drop(store);
        let store = crate::store::Store::open(&path).unwrap();
        assert_eq!(store.freeze_preferences("cerro").unwrap(), before);
        assert!(store.freeze_preferences("hill").unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn preference_changes_before_rollout_are_private_and_survive_restart() {
        let (mut store, path) = temporary_store();
        assert!(store.freeze_preferences("cerro").unwrap().is_empty());
        store.set_freezes_enabled("cerro", false, 10).unwrap();
        assert!(store.freeze_preferences("cerro").unwrap().is_empty());
        store.set_freezes_enabled("cerro", true, 20).unwrap();
        store.set_freezes_enabled("cerro", true, 30).unwrap();
        store.set_freezes_enabled("cerro", false, 40).unwrap();
        store.set_freezes_enabled("cerro", true, 50).unwrap();
        assert!(store.freeze_preferences("hill").unwrap().is_empty());
        let before = store.freeze_preferences("cerro").unwrap();
        assert_eq!(before.len(), 3);
        assert_eq!(before[0].at, 20);
        assert!(!before[1].enabled);
        drop(store);
        let store = crate::store::Store::open(&path).unwrap();
        assert_eq!(store.freeze_preferences("cerro").unwrap(), before);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn changes_in_the_same_millisecond_keep_their_order() {
        let (mut store, path) = temporary_store();
        store.set_freezes_enabled("cerro", true, 10).unwrap();
        store.set_freezes_enabled("cerro", false, 10).unwrap();
        store.set_freezes_enabled("cerro", true, 10).unwrap();
        let changes = store.freeze_preferences("cerro").unwrap();
        assert_eq!(changes.len(), 3);
        assert!(changes[2].enabled);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn streak_freeze_settings_require_the_players_own_token() {
        let (app, path) = fixture();
        for headers in [HeaderMap::new(), headers("hill"), headers("unknown")] {
            assert_eq!(
                get_freezes(State(app.clone()), UrlPath("cerro".into()), headers.clone())
                    .await
                    .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                set_freezes(
                    State(app.clone()),
                    UrlPath("cerro".into()),
                    headers,
                    Json(FreezeUpdate { enabled: true }),
                )
                .await
                .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert!(
            app.store
                .lock()
                .unwrap()
                .freeze_preferences("cerro")
                .unwrap()
                .is_empty()
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn streak_freezes_default_on_and_first_opt_out_survives_restart() {
        let (app, path) = fixture();
        let settings = get_freezes(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
        )
        .await
        .unwrap()
        .0;
        assert!(settings.enabled);
        assert_eq!((settings.freezes, settings.capacity), (0, 3));
        let disabled = set_freezes(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(FreezeUpdate { enabled: false }),
        )
        .await
        .unwrap()
        .0;
        assert!(!disabled.enabled);
        assert!(app.players.read().unwrap().is_empty());
        drop(app);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.freeze_preferences("cerro").unwrap().len(), 1);
        assert!(!store.freeze_preferences("cerro").unwrap()[0].enabled);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn streak_freezes_start_empty_can_be_toggled_before_upload_and_reload() {
        let (app, path) = fixture();
        let settings = get_freezes(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
        )
        .await
        .unwrap()
        .0;
        assert!(settings.enabled);
        assert_eq!((settings.freezes, settings.capacity), (0, 3));
        for enabled in [true, true, false, true] {
            let settings = set_freezes(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(FreezeUpdate { enabled }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(settings.enabled, enabled);
            assert_eq!(settings.freezes, 0);
        }
        assert!(
            app.players.read().unwrap().is_empty(),
            "settings do not add an empty leaderboard entry"
        );
        let other = get_freezes(State(app.clone()), UrlPath("hill".into()), headers("hill"))
            .await
            .unwrap()
            .0;
        assert!(other.enabled);
        drop(app);
        let store = Store::open(&path).unwrap();
        let history = store.freeze_preferences("cerro").unwrap();
        assert_eq!(
            history.len(),
            2,
            "repeated saves cannot change the activation time"
        );
        assert!(history.last().unwrap().enabled);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn streak_freeze_updates_cannot_set_a_balance_or_omit_enabled() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({"enabled": null}),
            serde_json::json!({"enabled": "true"}),
            serde_json::json!({"enabled": true, "freezes": 3}),
        ] {
            assert!(serde_json::from_value::<FreezeUpdate>(value).is_err());
        }
    }

    #[tokio::test]
    async fn streak_freeze_previews_do_not_bank_rewards_and_uploads_only_earn_once() {
        let (app, path) = fixture();
        let clock = Clock::default();
        let day = clock.day(now_ms()) - 1;
        let start = day * 86_400_000;
        // Enough morning reviews, time, combo and two sessions for any initial quests.
        let reviews: Vec<Review> = (0..60)
            .map(|i| Review {
                id: start + 6 * 3_600_000 + i * 10_000 + if i >= 30 { 600_000 } else { 0 },
                cid: i,
                last_ivl: 30,
                ivl: None,
                time_ms: 10_000,
                kind: 0,
            })
            .collect();
        {
            let mut store = app.store.lock().unwrap();
            store.upsert("cerro", &[], &[], clock).unwrap();
            store.set_freezes_enabled("cerro", true, start).unwrap();
            app.players
                .write()
                .unwrap()
                .insert("cerro".into(), load_player(&store, "cerro").unwrap());
        }
        assert_eq!(app.profile("cerro").unwrap().freezes, 0);
        let projected = preview(
            State(app.clone()),
            UrlPath("cerro".into()),
            HeaderMap::new(),
            Json(Pending {
                reviews: reviews.clone(),
                seen_through: None,
            }),
        )
        .await
        .unwrap()
        .0
        .profile;
        assert_eq!(projected.freezes, 1);
        assert_eq!(
            app.profile("cerro").unwrap().freezes,
            0,
            "preview never saves reviews or a reward"
        );
        for _ in 0..2 {
            let response = upload(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(Upload {
                    reviews: reviews.clone(),
                    deleted: vec![],
                    clock,
                    silent: true,
                    decks: None,
                    catalog: false,
                }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(response.profile.freezes, 1);
            assert!(response.profile.freezes_enabled);
        }
        let paused = set_freezes(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(FreezeUpdate { enabled: false }),
        )
        .await
        .unwrap()
        .0;
        assert!(!paused.enabled);
        assert_eq!(paused.freezes, 1);
        assert!(
            !app.profile("cerro").unwrap().freezes_enabled,
            "cached profiles see the new setting immediately"
        );
        assert_eq!(
            app.profile("cerro").unwrap().freezes,
            0,
            "older clients only see available protection"
        );
        assert_eq!(app.profile("cerro").unwrap().stored_freezes, 1);
        let config = app.config.clone();
        drop(app);
        let store = Store::open(&path).unwrap();
        let player = load_player(&store, "cerro").unwrap();
        let reloaded = App {
            config,
            access: access::Access::default(),
            accounts: Default::default(),
            week: Week::default(),
            store: Mutex::new(store),
            players: RwLock::new(HashMap::from([("cerro".into(), player)])),
            standings: Default::default(),
        };
        assert_eq!(reloaded.profile("cerro").unwrap().freezes, 0);
        assert_eq!(reloaded.profile("cerro").unwrap().stored_freezes, 1);
        assert!(!reloaded.profile("cerro").unwrap().freezes_enabled);
        drop(reloaded);
        std::fs::remove_dir_all(path).unwrap();
    }
}
