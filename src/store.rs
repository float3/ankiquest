use crate::game::{Clock, ClockChange, ClockHistory, Review};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

type Signature = Vec<Option<(SystemTime, u64)>>;

pub struct Store {
    pub(crate) conn: Connection,
    scratch: PathBuf,
    signatures: HashMap<String, Signature>,
}

fn signature(collection: &Path) -> Result<Signature, Error> {
    [collection.to_path_buf(), wal_of(collection)]
        .iter()
        .enumerate()
        .map(|(index, path)| match fs::metadata(path) {
            Ok(metadata) => Ok(Some((metadata.modified()?, metadata.len()))),
            Err(error) if index == 1 && error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        })
        .collect()
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

impl Store {
    pub fn open(state_dir: &Path) -> Result<Self, Error> {
        fs::create_dir_all(state_dir)?;
        let scratch = state_dir.join("scratch");
        fs::create_dir_all(&scratch)?;
        let conn = Connection::open(state_dir.join("ankiquest.db"))?;
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
        let has_ivl: bool = conn.query_row(
            "select exists (select 1 from pragma_table_info('reviews') where name = 'ivl')",
            [],
            |r| r.get(0),
        )?;
        if !has_ivl {
            conn.execute_batch("alter table reviews add column ivl integer")?;
        }
        // Players recorded before clocks had a history keep the only clock we
        // know about, reaching back over all of their reviews. A move they made
        // before this point has to be declared; it is not in the review data.
        conn.execute(
            "insert into clock_history (user, effective_from_ms, offset_west_min, rollover_hour)
             select user, 0, offset_west_min, rollover_hour from clocks
             where not exists (select 1 from clock_history where clock_history.user = clocks.user)",
            [],
        )?;
        crate::accounts::initialize(&conn)?;
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
        conn.execute_batch("create table if not exists community_refresh (id integer primary key, refreshed_at integer not null);")?;
        Ok(Self {
            conn,
            scratch,
            signatures: HashMap::new(),
        })
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
        let clock = self
            .conn
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

    pub fn ingest(&mut self, sync_base: &Path, user: &str) -> Result<bool, Error> {
        let source = sync_base.join(user).join("collection.anki2");
        let before = signature(&source)?;
        if self.signatures.get(user) == Some(&before) {
            return Ok(false);
        }

        let copy = self.scratch.join(format!("{user}.anki2"));
        let _ = fs::remove_file(wal_of(&copy));
        let _ = fs::remove_file(shm_of(&copy));
        fs::copy(&source, &copy)?;
        if before[1].is_some() {
            fs::copy(wal_of(&source), wal_of(&copy))?;
        }
        if signature(&source)? != before {
            return Err(
                format!("collection changed while importing {user}; retry required").into(),
            );
        }

        let (reviews, clock) = read_collection(&copy)?;
        let _ = fs::remove_file(wal_of(&copy));
        let _ = fs::remove_file(shm_of(&copy));
        let _ = fs::remove_file(&copy);

        let source_updated_at = before
            .iter()
            .flatten()
            .map(|(modified, _)| {
                modified
                    .duration_since(UNIX_EPOCH)
                    .map(|age| age.as_millis() as i64)
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .max()
            .unwrap_or_default();
        self.upsert_at(user, &reviews, &[], clock, source_updated_at)?;
        self.signatures.insert(user.into(), before);
        Ok(true)
    }

    pub fn upsert(
        &mut self,
        user: &str,
        reviews: &[Review],
        deleted: &[i64],
        clock: Clock,
    ) -> Result<(), Error> {
        let received_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
        self.upsert_at(user, reviews, deleted, clock, received_at)
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
            tx.execute(
                "insert into clocks (user, offset_west_min, rollover_hour) values (?1, ?2, ?3)
                 on conflict (user) do update set
                     offset_west_min = excluded.offset_west_min,
                     rollover_hour = excluded.rollover_hour",
                params![user, clock.offset_west_min, clock.rollover_hour],
            )?;
            // A clock that differs from the last one we saw starts a new entry,
            // dated from this upload: the reviews already here were kept under
            // the old one. The first entry a player gets reaches back forever,
            // since there is nothing before it to date the change against.
            let latest = tx
                .query_row(
                    "select offset_west_min, rollover_hour from clock_history
                     where user = ?1 order by effective_from_ms desc limit 1",
                    params![user],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()?;
            let moved = latest.is_none_or(|(offset_west_min, rollover_hour)| {
                offset_west_min != clock.offset_west_min || rollover_hour != clock.rollover_hour
            });
            if moved {
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
            store.conn.execute("delete from clock_history", []).unwrap();
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
        assert!(store.ingest(&base, "hill").unwrap());
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
