use crate::game::{Clock, ClockChange, ClockHistory, Review};
use rusqlite::{Connection, OptionalExtension, params};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// The modification time (in nanoseconds) and size of a collection and its WAL file.
pub(crate) type Signature = Vec<Option<(i64, u64)>>;

pub struct Store {
    pub(crate) conn: Connection,
    scratch: PathBuf,
}

fn signature(collection: &Path) -> Result<Signature, Error> {
    [collection.to_path_buf(), wal_of(collection)]
        .iter()
        .enumerate()
        .map(|(index, path)| match fs::metadata(path) {
            Ok(metadata) => Ok(Some((
                metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos() as i64,
                metadata.len(),
            ))),
            Err(error) if index == 1 && error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        })
        .collect()
}

/// A sync server collection read into memory, ready for [`Store::record_sync`].
pub(crate) struct Synced {
    signature: Signature,
    reviews: Vec<Review>,
    clock: Clock,
    updated_at: i64,
}

/// Reads `user`'s collection from the sync server unless its files still match `last`.
/// It needs nothing from the store, so the worker copies and reads collections without
/// holding the store lock. Imports into the same `scratch` must not run at the same time.
pub(crate) fn read_sync(
    scratch: &Path,
    sync_base: &Path,
    user: &str,
    last: Option<&Signature>,
) -> Result<Option<Synced>, Error> {
    let source = sync_base.join(user).join("collection.anki2");
    let before = signature(&source)?;
    if last == Some(&before) {
        return Ok(None);
    }

    let copy = scratch.join(format!("{user}.anki2"));
    let _ = fs::remove_file(wal_of(&copy));
    let _ = fs::remove_file(shm_of(&copy));
    fs::copy(&source, &copy)?;
    if before[1].is_some() {
        fs::copy(wal_of(&source), wal_of(&copy))?;
    }
    if signature(&source)? != before {
        return Err(format!("collection changed while importing {user}; retry required").into());
    }

    let (reviews, clock) = read_collection(&copy)?;
    let _ = fs::remove_file(wal_of(&copy));
    let _ = fs::remove_file(shm_of(&copy));
    let _ = fs::remove_file(&copy);

    let updated_at = before
        .iter()
        .flatten()
        .map(|(modified, _)| modified / 1_000_000)
        .max()
        .unwrap_or_default();
    Ok(Some(Synced {
        signature: before,
        reviews,
        clock,
        updated_at,
    }))
}

fn wal_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-wal");
    name.into()
}

fn shm_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-shm");
    name.into()
}

fn config_number(conn: &Connection, key: &str) -> Option<i64> {
    let raw: Vec<u8> = conn
        .query_row("select val from config where key = ?1", [key], |r| r.get(0))
        .ok()?;
    serde_json::from_slice::<f64>(&raw).ok().map(|n| n as i64)
}

/// The player's current clock, or the default for someone who never uploaded.
pub fn clock_in(conn: &Connection, user: &str) -> rusqlite::Result<Clock> {
    let clock = conn
        .query_row(
            "select offset_west_min, rollover_hour from clocks where user = ?1",
            [user],
            |r| {
                Ok(Clock {
                    offset_west_min: r.get(0)?,
                    rollover_hour: r.get(1)?,
                })
            },
        )
        .optional()?;
    Ok(clock.unwrap_or_default())
}

impl Store {
    pub fn open(state_dir: &Path) -> Result<Self, Error> {
        fs::create_dir_all(state_dir)?;
        let scratch = state_dir.join("scratch");
        fs::create_dir_all(&scratch)?;
        let conn = Connection::open(state_dir.join("ankiquest.db"))?;
        #[cfg(test)]
        conn.execute_batch("pragma synchronous = off;")?;
        conn.execute_batch(
            "pragma journal_mode = wal;
             create table if not exists reviews (
                 user text not null,
                 id integer not null,
                 cid integer not null,
                 last_ivl integer not null,
                 ivl integer,
                 time_ms integer not null,
                 kind integer not null,
                 primary key (user, id)
             ) without rowid;
             create table if not exists clocks (
                 user text primary key,
                 offset_west_min integer not null,
                 rollover_hour integer not null
             );
             create table if not exists clock_history (
                 user text not null,
                 effective_from_ms integer not null,
                 offset_west_min integer not null,
                 rollover_hour integer not null,
                 primary key (user, effective_from_ms)
             ) without rowid;
             create table if not exists seen (
                 user text not null,
                 key text not null,
                 primary key (user, key)
             ) without rowid;
             create table if not exists review_syncs (
                 user text primary key,
                 received_at integer not null
             ) without rowid;",
        )?;
        crate::accounts::initialize(&conn)?;
        crate::oauth::initialize(&conn)?;
        crate::social::initialize(&conn)?;
        crate::avatars::initialize(&conn)?;
        crate::conquests::initialize(&conn)?;
        crate::i18n::initialize(&conn)?;
        crate::companions::initialize(&conn)?;
        crate::decks::initialize(&conn)?;
        crate::deck_copies::initialize(&conn)?;
        crate::subscriptions::initialize(&conn)?;
        crate::freezes::initialize(&conn)?;
        crate::competition::initialize(&conn)?;
        crate::reminders::initialize(&conn)?;
        crate::challenges::initialize(&conn)?;
        crate::weekly_challenges::initialize(&conn)?;
        crate::admin::initialize(&conn)?;
        crate::dev::initialize(&conn)?;
        crate::goals::initialize(&conn)?;
        crate::wins::initialize(&conn)?;
        initialize_sync_signatures(&conn)?;
        crate::migrations::run(&conn)?;
        Ok(Self { conn, scratch })
    }

    pub fn users(&self) -> Result<Vec<String>, Error> {
        let mut stmt = self.conn.prepare("select user from clocks order by user")?;
        let users = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(users)
    }

    pub fn reviews(&self, user: &str) -> Result<Vec<Review>, Error> {
        let mut stmt = self.conn.prepare(
            "select id, cid, last_ivl, ivl, time_ms, kind from reviews where user = ?1 order by id",
        )?;
        let reviews = stmt
            .query_map([user], |r| {
                Ok(Review {
                    id: r.get(0)?,
                    cid: r.get(1)?,
                    last_ivl: r.get(2)?,
                    ivl: r.get(3)?,
                    time_ms: r.get(4)?,
                    kind: r.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(reviews)
    }

    pub fn clock(&self, user: &str) -> Result<Clock, Error> {
        Ok(clock_in(&self.conn, user)?)
    }

    /// Every clock the player has kept, so each review reads against the one it
    /// was recorded under rather than the one they keep today.
    pub fn clock_history(&self, user: &str) -> Result<ClockHistory, Error> {
        let mut statement = self.conn.prepare(
            "select effective_from_ms, offset_west_min, rollover_hour from clock_history
             where user = ?1 order by effective_from_ms",
        )?;
        let changes = statement
            .query_map([user], |r| {
                Ok(ClockChange {
                    effective_from_ms: r.get(0)?,
                    clock: Clock {
                        offset_west_min: r.get(1)?,
                        rollover_hour: r.get(2)?,
                    },
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if changes.is_empty() {
            return Ok(ClockHistory::single(self.clock(user)?));
        }
        Ok(ClockHistory::new(changes))
    }

    pub fn is_known(&self, user: &str) -> Result<bool, Error> {
        Ok(self
            .conn
            .query_row("select 1 from clocks where user = ?1", [user], |_| Ok(()))
            .optional()?
            .is_some())
    }

    pub fn last_review_sync(&self, user: &str) -> Result<Option<i64>, Error> {
        self.conn
            .query_row(
                "select received_at from review_syncs where user = ?1",
                [user],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn mark_seen(&self, user: &str, key: &str) -> Result<bool, Error> {
        let changed = self.conn.execute(
            "insert or ignore into seen (user, key) values (?1, ?2)",
            [user, key],
        )?;
        Ok(changed > 0)
    }

    pub(crate) fn is_seen(&self, user: &str, key: &str) -> Result<bool, Error> {
        Ok(self.conn.query_row(
            "select exists (select 1 from seen where user = ?1 and key = ?2)",
            [user, key],
            |r| r.get(0),
        )?)
    }

    pub(crate) fn scratch(&self) -> &Path {
        &self.scratch
    }

    /// The collection files the last import of `user` saw. They are kept in the database,
    /// so a restart does not import every collection again.
    pub(crate) fn sync_signature(&self, user: &str) -> Result<Option<Signature>, Error> {
        let files: Option<String> = self
            .conn
            .query_row(
                "select files from sync_signatures where user = ?1",
                [user],
                |r| r.get(0),
            )
            .optional()?;
        // An unreadable signature only costs one more import.
        Ok(files.and_then(|files| serde_json::from_str(&files).ok()))
    }

    /// Stores what [`read_sync`] read, dated from when the collection last changed.
    pub(crate) fn record_sync(&mut self, user: &str, synced: &Synced) -> Result<(), Error> {
        self.upsert_at(user, &synced.reviews, &[], synced.clock, synced.updated_at)?;
        self.conn.execute(
            "insert into sync_signatures (user, files) values (?1, ?2)
             on conflict (user) do update set files = excluded.files",
            params![user, serde_json::to_string(&synced.signature)?],
        )?;
        Ok(())
    }

    /// [`read_sync`] and [`Store::record_sync`] in one step, holding the store throughout.
    #[cfg(test)]
    pub fn ingest(&mut self, sync_base: &Path, user: &str) -> Result<bool, Error> {
        let last = self.sync_signature(user)?;
        match read_sync(&self.scratch, sync_base, user, last.as_ref())? {
            Some(synced) => {
                self.record_sync(user, &synced)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub fn upsert(
        &mut self,
        user: &str,
        reviews: &[Review],
        deleted: &[i64],
        clock: Clock,
    ) -> Result<(), Error> {
        self.upsert_at(user, reviews, deleted, clock, crate::app::now_ms())
    }

    fn upsert_at(
        &mut self,
        user: &str,
        reviews: &[Review],
        deleted: &[i64],
        clock: Clock,
        received_at: i64,
    ) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare(
                // A resend from a newer client fills in the interval older uploads left out.
                "insert into reviews (user, id, cid, last_ivl, ivl, time_ms, kind)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 on conflict (user, id) do update set ivl = coalesce(reviews.ivl, excluded.ivl)",
            )?;
            for r in reviews {
                insert.execute(params![
                    user, r.id, r.cid, r.last_ivl, r.ivl, r.time_ms, r.kind
                ])?;
            }
            let mut delete = tx.prepare("delete from reviews where user = ?1 and id = ?2")?;
            for id in deleted {
                delete.execute(params![user, id])?;
            }
            // A clock that differs from the last one we saw starts a new entry,
            // dated from this upload: the reviews already here were kept under
            // the old one. The first entry a player gets reaches back forever,
            // since there is nothing before it to date the change against.
            // A clock read before the newest change is stale and leaves both alone.
            let latest = tx
                .query_row(
                    "select effective_from_ms, offset_west_min, rollover_hour from clock_history
                     where user = ?1 order by effective_from_ms desc limit 1",
                    params![user],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()?;
            let stale = latest.is_some_and(|(from, _, _)| received_at < from);
            if !stale {
                tx.execute(
                    "insert into clocks (user, offset_west_min, rollover_hour) values (?1, ?2, ?3)
                     on conflict (user) do update set
                         offset_west_min = excluded.offset_west_min,
                         rollover_hour = excluded.rollover_hour",
                    params![user, clock.offset_west_min, clock.rollover_hour],
                )?;
            }
            let moved = latest.is_none_or(|(_, offset_west_min, rollover_hour)| {
                offset_west_min != clock.offset_west_min || rollover_hour != clock.rollover_hour
            });
            if moved && !stale {
                let effective_from_ms = if latest.is_none() { 0 } else { received_at };
                tx.execute(
                    "insert into clock_history
                         (user, effective_from_ms, offset_west_min, rollover_hour)
                     values (?1, ?2, ?3, ?4)
                     on conflict (user, effective_from_ms) do update set
                         offset_west_min = excluded.offset_west_min,
                         rollover_hour = excluded.rollover_hour",
                    params![
                        user,
                        effective_from_ms,
                        clock.offset_west_min,
                        clock.rollover_hour
                    ],
                )?;
            }
            tx.execute(
                "insert into review_syncs (user, received_at) values (?1, ?2)
                 on conflict(user) do update set received_at = max(review_syncs.received_at, excluded.received_at)",
                params![user, received_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

fn initialize_sync_signatures(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "create table if not exists sync_signatures (
             user text primary key,
             files text not null
         ) without rowid;",
    )
}

fn read_collection(path: &Path) -> Result<(Vec<Review>, Clock), Error> {
    let conn = Connection::open(path)?;
    let mut stmt = conn.prepare(
        "select id, cid, lastIvl, ivl, time, type from revlog where ease > 0 and type < 4 order by id",
    )?;
    let reviews = stmt
        .query_map([], |r| {
            Ok(Review {
                id: r.get(0)?,
                cid: r.get(1)?,
                last_ivl: r.get(2)?,
                ivl: Some(r.get(3)?),
                time_ms: r.get(4)?,
                kind: r.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let defaults = Clock::default();
    let clock = Clock {
        offset_west_min: config_number(&conn, "localOffset").unwrap_or(defaults.offset_west_min),
        rollover_hour: config_number(&conn, "rollover").unwrap_or(defaults.rollover_hour),
    };
    Ok((reviews, clock))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ankiquest-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_collection(base: &Path, user: &str) -> Connection {
        fs::create_dir_all(base.join(user)).unwrap();
        let conn = Connection::open(base.join(user).join("collection.anki2")).unwrap();
        conn.execute_batch(
            "create table revlog (id integer primary key, cid integer, usn integer, ease integer,
                 ivl integer, lastIvl integer, factor integer, time integer, type integer);
             create table config (KEY text primary key, usn integer, mtime_secs integer, val blob) without rowid;
             insert into config values ('rollover', 0, 0, cast('5' as blob)), ('localOffset', 0, 0, cast('-120' as blob));
             insert into revlog values (1000, 1, 0, 3, 1, 0, 2500, 4000, 0);
             insert into revlog values (2000, 2, 0, 1, 1, 30, 2500, 6000, 1);
             insert into revlog values (3000, 2, 0, 0, 1, 30, 2500, 0, 4);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn a_changed_clock_is_recorded_and_older_reviews_keep_the_old_one() {
        let dir = temp_dir("clock-history");
        let mut store = Store::open(&dir).unwrap();
        let berlin = Clock {
            offset_west_min: -120,
            rollover_hour: 4,
        };
        let california = Clock {
            offset_west_min: 420,
            rollover_hour: 4,
        };
        let study = |id| Review {
            id,
            cid: id,
            last_ivl: 0,
            ivl: None,
            time_ms: 5_000,
            kind: 1,
        };
        let morning = study(1_791_010_800_000); // 3 Oct 09:00 Berlin
        let afternoon = study(1_791_151_200_000); // 4 Oct 15:00 California

        store
            .upsert_at("hill", &[morning], &[], berlin, 1_791_011_000_000)
            .unwrap();
        store
            .upsert_at("hill", &[], &[], berlin, 1_791_050_000_000)
            .unwrap();
        store
            .upsert_at("hill", &[afternoon], &[], california, 1_791_151_500_000)
            .unwrap();

        let history = store.clock_history("hill").unwrap();
        assert_eq!(history.current(), california);
        assert_eq!(
            history.at(morning.id),
            berlin,
            "the Berlin morning is still read on the clock it was studied under"
        );
        assert_eq!(
            history.day(morning.id) + 1,
            history.day(afternoon.id),
            "the two days run consecutively, with no empty day between them"
        );

        store
            .upsert_at("hill", &[], &[], california, 1_791_200_000_000)
            .unwrap();
        assert_eq!(
            store.clock_history("hill").unwrap(),
            history,
            "an unchanged clock does not record another move"
        );

        store
            .upsert_at("hill", &[], &[], berlin, 1_791_100_000_000)
            .unwrap();
        assert_eq!(
            store.clock_history("hill").unwrap(),
            history,
            "a clock read before the newest change is stale"
        );
        assert_eq!(store.clock("hill").unwrap(), california);
    }

    #[test]
    fn a_player_recorded_before_clocks_had_a_history_gets_one_reaching_back() {
        let dir = temp_dir("clock-history-migration");
        let berlin = Clock {
            offset_west_min: -120,
            rollover_hour: 4,
        };
        {
            let store = Store::open(&dir).unwrap();
            store
                .conn
                .execute(
                    "insert into clocks (user, offset_west_min, rollover_hour) values (?1, ?2, ?3)",
                    params!["hill", berlin.offset_west_min, berlin.rollover_hour],
                )
                .unwrap();
            store
                .conn
                .execute_batch("delete from clock_history; pragma user_version = 0;")
                .unwrap();
        }
        let store = Store::open(&dir).unwrap();
        let history = store.clock_history("hill").unwrap();
        assert_eq!(history.current(), berlin);
        assert_eq!(
            history.at(0),
            berlin,
            "the only clock we know about covers everything already uploaded"
        );
    }

    #[test]
    fn ingests_reviews_and_clock_once() {
        let dir = temp_dir("ingest");
        let base = dir.join("sync");
        let source = fake_collection(&base, "hill");
        let mut store = Store::open(&dir.join("state")).unwrap();

        assert!(store.ingest(&base, "hill").unwrap());
        assert!(!store.ingest(&base, "hill").unwrap());
        let first_import_at = store.last_review_sync("hill").unwrap();
        drop(store);
        let mut store = Store::open(&dir.join("state")).unwrap();
        assert!(
            !store.ingest(&base, "hill").unwrap(),
            "an unchanged collection is not imported again after a restart"
        );
        assert_eq!(store.last_review_sync("hill").unwrap(), first_import_at);
        let reviews = store.reviews("hill").unwrap();
        assert_eq!(reviews.len(), 2);
        assert_eq!(reviews[1].last_ivl, 30);
        assert_eq!(
            store.clock("hill").unwrap(),
            Clock {
                offset_west_min: -120,
                rollover_hour: 5
            }
        );
        assert_eq!(store.users().unwrap(), vec!["hill".to_string()]);

        source
            .execute(
                "insert into revlog values (4000, 3, 0, 3, 1, 0, 2500, 4000, 0)",
                [],
            )
            .unwrap();
        drop(source);
        assert!(store.ingest(&base, "hill").unwrap());
        assert_eq!(store.reviews("hill").unwrap().len(), 3);

        assert!(store.mark_seen("hill", "level:2").unwrap());
        assert!(!store.mark_seen("hill", "level:2").unwrap());
        assert!(store.ingest(&base, "missing").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_resend_fills_in_intervals_older_uploads_left_out() {
        let dir = temp_dir("intervals");
        let mut store = Store::open(&dir).unwrap();
        let review = |ivl| Review {
            id: 1,
            cid: 1,
            last_ivl: 0,
            ivl,
            time_ms: 5000,
            kind: 1,
        };
        let clock = Clock::default();
        store.upsert("hill", &[review(None)], &[], clock).unwrap();
        assert_eq!(store.reviews("hill").unwrap()[0].ivl, None);
        store
            .upsert("hill", &[review(Some(4))], &[], clock)
            .unwrap();
        store
            .upsert("hill", &[review(Some(9))], &[], clock)
            .unwrap();
        assert_eq!(
            store.reviews("hill").unwrap()[0].ivl,
            Some(4),
            "a known interval stays"
        );
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn undone_reviews_are_deleted() {
        let dir = temp_dir("undo");
        let mut store = Store::open(&dir).unwrap();
        let review = |id| Review {
            id,
            cid: 1,
            last_ivl: 0,
            ivl: None,
            time_ms: 4000,
            kind: 1,
        };
        let clock = Clock::default();
        store
            .upsert("hill", &[review(1), review(2), review(3)], &[], clock)
            .unwrap();
        store
            .upsert("hill", &[review(4)], &[2, 3, 99], clock)
            .unwrap();
        let ids: Vec<i64> = store
            .reviews("hill")
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec![1, 4]);
        let _ = fs::remove_dir_all(&dir);
    }
}
