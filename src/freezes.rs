use crate::game::{FreezePolicy, FreezePreference};
use crate::store::{Error, Store};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeSet;

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

#[cfg(test)]
mod tests {
    use crate::decks::tests::temporary_store;

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
}
