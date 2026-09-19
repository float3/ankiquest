use crate::game::{Clock, Review};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

type Signature = Vec<Option<(SystemTime, u64)>>;

pub struct Store {
    pub(crate) conn: Connection,
    scratch: PathBuf,
    signatures: HashMap<String, Signature>,
}

fn signature(collection: &Path) -> Signature {
    [collection.to_path_buf(), wal_of(collection)]
        .iter()
        .map(|p| {
            let meta = fs::metadata(p).ok()?;
            Some((meta.modified().ok()?, meta.len()))
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
                 time_ms integer not null,
                 kind integer not null,
                 primary key (user, id)
             ) without rowid;
             create table if not exists clocks (
                 user text primary key,
                 offset_west_min integer not null,
                 rollover_hour integer not null
             );
             create table if not exists seen (
                 user text not null,
                 key text not null,
                 primary key (user, key)
             ) without rowid;",
        )?;
        crate::decks::initialize(&conn)?;
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
            "select id, cid, last_ivl, time_ms, kind from reviews where user = ?1 order by id",
        )?;
        let reviews = stmt
            .query_map([user], |r| {
                Ok(Review {
                    id: r.get(0)?,
                    cid: r.get(1)?,
                    last_ivl: r.get(2)?,
                    time_ms: r.get(3)?,
                    kind: r.get(4)?,
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

    pub fn is_known(&self, user: &str) -> Result<bool, Error> {
        Ok(self
            .conn
            .query_row("select 1 from clocks where user = ?1", [user], |_| Ok(()))
            .optional()?
            .is_some())
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
        let before = signature(&source);
        if before[0].is_none() || self.signatures.get(user) == Some(&before) {
            return Ok(false);
        }

        let copy = self.scratch.join(format!("{user}.anki2"));
        let _ = fs::remove_file(wal_of(&copy));
        let _ = fs::remove_file(shm_of(&copy));
        fs::copy(&source, &copy)?;
        if before[1].is_some() {
            fs::copy(wal_of(&source), wal_of(&copy))?;
        }
        if signature(&source) != before {
            return Ok(false);
        }

        let (reviews, clock) = read_collection(&copy)?;
        let _ = fs::remove_file(wal_of(&copy));
        let _ = fs::remove_file(shm_of(&copy));
        let _ = fs::remove_file(&copy);

        self.upsert(user, &reviews, &[], clock)?;
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
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare(
                "insert or ignore into reviews (user, id, cid, last_ivl, time_ms, kind)
                 values (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for r in reviews {
                insert.execute(params![user, r.id, r.cid, r.last_ivl, r.time_ms, r.kind])?;
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
        }
        tx.commit()?;
        Ok(())
    }
}

fn read_collection(path: &Path) -> Result<(Vec<Review>, Clock), Error> {
    let conn = Connection::open(path)?;
    let mut stmt = conn.prepare(
        "select id, cid, lastIvl, time, type from revlog where ease > 0 and type < 4 order by id",
    )?;
    let reviews = stmt
        .query_map([], |r| {
            Ok(Review {
                id: r.get(0)?,
                cid: r.get(1)?,
                last_ivl: r.get(2)?,
                time_ms: r.get(3)?,
                kind: r.get(4)?,
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
    fn ingests_reviews_and_clock_once() {
        let dir = temp_dir("ingest");
        let base = dir.join("sync");
        let source = fake_collection(&base, "hill");
        let mut store = Store::open(&dir.join("state")).unwrap();

        assert!(store.ingest(&base, "hill").unwrap());
        assert!(!store.ingest(&base, "hill").unwrap());
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
        assert!(!store.ingest(&base, "missing").unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn undone_reviews_are_deleted() {
        let dir = temp_dir("undo");
        let mut store = Store::open(&dir).unwrap();
        let review = |id| Review {
            id,
            cid: 1,
            last_ivl: 0,
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
