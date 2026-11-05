//! Schema changes that run once, in order. `pragma user_version` counts the steps a database has had.
//!
//! Each module's `initialize()` still creates its tables with `create table if not exists` on every
//! start. Anything else (a new column, a backfill, a rename, a cleanup) is a new step at the end of
//! `STEPS`. Never edit or reorder a step that has shipped.

use crate::notifications::NUDGE_ALLOWED_SQL;
use crate::store::Error;
use rusqlite::Connection;

type Step = fn(&Connection) -> Result<(), Error>;

const STEPS: &[Step] = &[baseline, notification_fetches];

/// Brings the database up to the latest step, then turns foreign keys on for the connection.
pub fn run(conn: &Connection) -> Result<(), Error> {
    run_steps(conn, STEPS)
}

fn run_steps(conn: &Connection, steps: &[Step]) -> Result<(), Error> {
    let version: i64 = conn.query_row("pragma user_version", [], |r| r.get(0))?;
    if usize::try_from(version).map_or(true, |version| version > steps.len()) {
        return Err(format!(
            "the database is at schema version {version}, but this server only knows {}; run a newer server",
            steps.len()
        )
        .into());
    }
    // Steps run with foreign keys off, as SQLite asks for when a step rebuilds a table, and
    // each one checks them itself before it commits.
    conn.execute_batch("pragma foreign_keys = off;")?;
    let ran = steps
        .iter()
        .enumerate()
        .skip(version as usize)
        .try_for_each(|(done, step)| {
            let transaction = conn.unchecked_transaction()?;
            step(&transaction)?;
            let broken: i64 = transaction.query_row(
                "select count(*) from pragma_foreign_key_check",
                [],
                |r| r.get(0),
            )?;
            if broken > 0 {
                return Err(format!(
                    "schema step {} leaves {broken} rows pointing at a missing parent",
                    done + 1
                )
                .into());
            }
            transaction.pragma_update(None, "user_version", (done + 1) as i64)?;
            transaction.commit()?;
            Ok::<_, Error>(())
        });
    // The bundled SQLite already defaults to on; say so here rather than rely on how it was built.
    conn.execute_batch("pragma foreign_keys = on;")?;
    ran
}

/// Whether `table` already has `column`.
fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "select exists (select 1 from pragma_table_info(?1) where name = ?2)",
        [table, column],
        |r| r.get(0),
    )
}

fn add_column(conn: &Connection, table: &str, column: &str, definition: &str) -> Result<(), Error> {
    if !has_column(conn, table, column)? {
        conn.execute(
            &format!("alter table {table} add column {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

/// Version 1: what servers before versioning did on every start. Databases from then have no
/// version and any mix of these changes, so every part checks before it acts.
fn baseline(conn: &Connection) -> Result<(), Error> {
    add_column(conn, "reviews", "ivl", "integer")?;
    // Players recorded before clocks had a history keep the only clock we
    // know about, reaching back over all of their reviews. A move they made
    // before this point has to be declared; it is not in the review data.
    conn.execute(
        "insert into clock_history (user, effective_from_ms, offset_west_min, rollover_hour)
         select user, 0, offset_west_min, rollover_hour from clocks
         where not exists (select 1 from clock_history where clock_history.user = clocks.user)",
        [],
    )?;
    for (column, definition) in [
        ("sender", "text not null default ''"),
        ("replied", "integer not null default 0"),
        ("kind", "text not null default 'message'"),
        ("push_only", "integer not null default 0"),
        ("retry_at", "integer not null default 0"),
        ("push_attempts", "integer not null default 0"),
        ("push_cancelled", "integer not null default 0"),
        ("completion_cancelled", "integer not null default 0"),
        ("read_at", "integer"),
        ("challenge_id", "integer"),
        ("reply_to_title", "text"),
        ("reply_to_body", "text"),
        ("topic", "text"),
        ("win_id", "integer"),
        ("quiet", "integer not null default 0"),
    ] {
        add_column(conn, "notifications", column, definition)?;
    }
    add_column(
        conn,
        "player_settings",
        "celebrations",
        "integer not null default 1",
    )?;
    if !has_column(conn, "player_settings", "friend_nudges")? {
        // Friend nudges start with the choice the player made for nudges.
        conn.execute_batch(
            "alter table player_settings add column friend_nudges integer not null default 0;
             update player_settings set friend_nudges = nudges;",
        )?;
    }
    conn.execute_batch(
        "create index if not exists notifications_pending on notifications (pushed, retry_at, id);
         create index if not exists notifications_sender on notifications (sender, id);",
    )?;
    // Older servers left nudges queued after opt-out. Retain their inbox history
    // without resuming those pushes when the player next enables nudges.
    conn.execute(
        &format!(
            "update notifications set push_cancelled = 1
             where pushed = 0 and push_cancelled = 0 and kind = 'nudge'
             and not {NUDGE_ALLOWED_SQL}"
        ),
        [],
    )?;
    add_column(conn, "goals", "since", "integer")?;
    // An old global warning may be waiting on a failed push when the upgraded
    // server starts. It must not bypass the new opt-ins or quiet hours.
    conn.execute(
        "update notifications set pushed = 1, push_only = 1 where kind = 'risk' and pushed = 0",
        [],
    )?;
    Ok(())
}

/// When the player's own inbox request first returned a notification; null until then.
fn notification_fetches(conn: &Connection) -> Result<(), Error> {
    add_column(conn, "notifications", "fetched_at", "integer")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::test_support::cleanup;

    fn version(conn: &Connection) -> usize {
        conn.query_row("pragma user_version", [], |r| r.get::<_, i64>(0))
            .unwrap() as usize
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ankiquest-migrations-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("select count(*) from {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn steps_run_once_in_order_and_record_the_version() {
        fn first(conn: &Connection) -> Result<(), Error> {
            conn.execute_batch("create table log (step integer); insert into log values (1);")?;
            Ok(())
        }
        fn second(conn: &Connection) -> Result<(), Error> {
            conn.execute("insert into log values (2)", [])?;
            Ok(())
        }
        let conn = Connection::open_in_memory().unwrap();
        run_steps(&conn, &[first]).unwrap();
        assert_eq!(version(&conn), 1);
        run_steps(&conn, &[first, second]).unwrap();
        run_steps(&conn, &[first, second]).unwrap();
        assert_eq!(version(&conn), 2);
        let steps: Vec<i64> = conn
            .prepare("select step from log")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(steps, [1, 2]);
    }

    #[test]
    fn a_failed_step_leaves_no_trace_and_a_newer_database_is_refused() {
        fn broken(conn: &Connection) -> Result<(), Error> {
            conn.execute_batch("create table half (x integer); select * from missing;")?;
            Ok(())
        }
        let conn = Connection::open_in_memory().unwrap();
        assert!(run_steps(&conn, &[broken]).is_err());
        assert_eq!(version(&conn), 0);
        assert!(
            !conn
                .prepare("select 1 from sqlite_master where name = 'half'")
                .unwrap()
                .exists([])
                .unwrap()
        );
        conn.pragma_update(None, "user_version", 5).unwrap();
        assert!(run_steps(&conn, &[broken]).is_err());
        assert_eq!(version(&conn), 5);
    }

    #[test]
    fn an_unversioned_database_takes_the_baseline_and_keeps_its_rows() {
        let dir = temp_dir("baseline");
        drop(Store::open(&dir).unwrap());
        {
            // A database from before versioning, which already had every column.
            let conn = Connection::open(dir.join("ankiquest.db")).unwrap();
            conn.execute_batch(
                "pragma user_version = 0;
                 insert into goals (user, mode, value, since) values ('hill', 'reviews', 50, 7);
                 insert into player_settings (user, nudges, friend_nudges) values ('hill', 1, 0);",
            )
            .unwrap();
        }
        let store = Store::open(&dir).unwrap();
        assert_eq!(version(&store.conn), STEPS.len());
        let (since, friend_nudges): (i64, i64) = store
            .conn
            .query_row(
                "select since, friend_nudges from goals, player_settings",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((since, friend_nudges), (7, 0));
        cleanup(store, dir);
    }

    #[test]
    fn a_step_that_leaves_a_dangling_row_is_refused() {
        fn dangling(conn: &Connection) -> Result<(), Error> {
            conn.execute_batch(
                "create table parent (id integer primary key);
                 create table child (parent integer references parent (id));
                 insert into child values (1);",
            )?;
            Ok(())
        }
        let conn = Connection::open_in_memory().unwrap();
        assert!(run_steps(&conn, &[dangling]).is_err());
        assert_eq!(version(&conn), 0);
    }

    #[test]
    fn foreign_keys_are_on_so_deleting_a_parent_cascades() {
        let dir = temp_dir("cascades");
        let store = Store::open(&dir).unwrap();
        let conn = &store.conn;
        let on: bool = conn
            .query_row("pragma foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert!(on);
        conn.execute_batch(
            "insert into accounts values ('hill', 'Hill', '', 0);
             insert into account_tokens values ('token', 'hill', 'Phone', 0);
             insert into groups values (1, 'Study', 'invite', 'hill', 0);
             insert into group_members values (1, 'hill', 1, 0);",
        )
        .unwrap();
        assert!(
            conn.execute(
                "insert into account_tokens values ('other', 'gone', 'Phone', 0)",
                []
            )
            .is_err()
        );
        assert!(
            conn.execute("insert into group_members values (2, 'hill', 1, 0)", [])
                .is_err()
        );
        assert!(
            conn.execute(
                "insert into community_members (challenge, user, status) values (9, 'hill', 'invited')",
                []
            )
            .is_err()
        );
        conn.execute("delete from groups where id = 1", []).unwrap();
        assert_eq!(count(conn, "group_members"), 0);
        conn.execute("delete from accounts where user = 'hill'", [])
            .unwrap();
        assert_eq!(count(conn, "account_tokens"), 0);
        cleanup(store, dir);
    }
}
