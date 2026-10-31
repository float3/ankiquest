//! The server log and runtime setting overrides, both kept in the SQLite state,
//! and the `ankiquest admin` command that reads and changes them while the
//! service runs. The service picks up overrides on its next poll.

use crate::game::{DAY_MS, ReviewWeighting, StreakChange, StreakRule};
use crate::store::{Error, Store};
use chrono::DateTime;
use rusqlite::{Connection, OpenFlags, params, types::ValueRef};
use std::path::Path;
use std::sync::Mutex;

pub const DEFAULT_LOG_RETENTION_DAYS: u32 = 90;
/// Entries waiting for the next flush. Past this, only stderr gets them.
const MAX_PENDING: usize = 10_000;

/// Records an error in the server log, and on stderr for the journal.
/// `log_error!(user: name, "...")` attributes it to a player.
macro_rules! log_error {
    (user: $user:expr, $($arg:tt)*) => {
        $crate::admin::record("error", Some(&*$user), format!($($arg)*))
    };
    ($($arg:tt)*) => {
        $crate::admin::record("error", None, format!($($arg)*))
    };
}

/// Like `log_error!`, for things worth knowing that went right.
macro_rules! log_info {
    (user: $user:expr, $($arg:tt)*) => {
        $crate::admin::record("info", Some(&*$user), format!($($arg)*))
    };
    ($($arg:tt)*) => {
        $crate::admin::record("info", None, format!($($arg)*))
    };
}

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists server_settings (
             key text primary key,
             value text not null,
             updated_at integer not null
         ) without rowid;
         create table if not exists server_log (
             id integer primary key,
             at integer not null,
             level text not null,
             user text,
             message text not null
         );
         create index if not exists server_log_at on server_log (at);
         create table if not exists streak_minutes (
             at integer primary key,
             minutes integer not null
         );",
    )?;
    Ok(())
}

struct Entry {
    at: i64,
    level: &'static str,
    user: Option<String>,
    message: String,
}

// Handlers log while holding the store, so entries wait here for the poll.
static PENDING: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

pub fn record(level: &'static str, user: Option<&str>, message: String) {
    eprintln!("{message}");
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if pending.len() < MAX_PENDING {
        pending.push(Entry {
            at: crate::app::now_ms(),
            level,
            user: user.map(str::to_string),
            message,
        });
    }
}

/// Writes waiting entries and drops those older than the retention.
pub fn flush(conn: &Connection, now_ms: i64, retention_days: u32) -> Result<(), Error> {
    let entries = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
    let tx = conn.unchecked_transaction()?;
    for entry in &entries {
        insert(&tx, entry)?;
    }
    if retention_days > 0 {
        tx.execute(
            "delete from server_log where at < ?1",
            [now_ms - i64::from(retention_days) * DAY_MS],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn insert(conn: &Connection, entry: &Entry) -> Result<(), Error> {
    conn.execute(
        "insert into server_log (at, level, user, message) values (?1, ?2, ?3, ?4)",
        params![entry.at, entry.level, entry.user, entry.message],
    )?;
    Ok(())
}

/// Settings that can change while the service runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub review_weighting: ReviewWeighting,
    pub log_retention_days: u32,
    pub streak_minutes: u32,
}

const KEYS: [(&str, &str); 3] = [
    ("review_weighting", "flat or diminishing"),
    (
        "log_retention_days",
        "days to keep the server log; 0 keeps it forever",
    ),
    (
        "streak_minutes",
        "review minutes a day needs for the streak; 0 lets any review count. A change applies from each player's next Anki day",
    ),
];

/// Most review minutes a day can be asked for: Anki caps each answer's time, so
/// more than a day's worth could never be reached.
const MAX_STREAK_MINUTES: u32 = 1_440;

impl Settings {
    fn set(&mut self, key: &str, value: &str) -> Result<(), Error> {
        match key {
            "review_weighting" => {
                self.review_weighting = serde_json::from_value(value.into())
                    .map_err(|_| "review_weighting must be flat or diminishing")?;
            }
            "log_retention_days" => {
                self.log_retention_days = value
                    .parse()
                    .map_err(|_| "log_retention_days must be a whole number of days")?;
            }
            "streak_minutes" => {
                self.streak_minutes = value
                    .parse()
                    .ok()
                    .filter(|minutes| *minutes <= MAX_STREAK_MINUTES)
                    .ok_or("streak_minutes must be a whole number of minutes, at most 1440")?;
            }
            _ => return Err(unknown(key)),
        }
        Ok(())
    }

    fn get(&self, key: &str) -> String {
        match key {
            "review_weighting" => serde_json::to_value(self.review_weighting)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string(),
            "log_retention_days" => self.log_retention_days.to_string(),
            "streak_minutes" => self.streak_minutes.to_string(),
            _ => unreachable!(),
        }
    }

    /// The configured settings with the stored overrides on top.
    pub fn effective(self, conn: &Connection) -> Result<Self, Error> {
        let mut settings = self;
        for (key, value, _) in overrides(conn)? {
            // Only `admin set` writes these, and it validates first.
            if let Err(error) = settings.set(&key, &value) {
                log_error!("ignoring stored setting {key}: {error}");
            }
        }
        Ok(settings)
    }
}

/// Records `minutes` as the streak minimum from `now_ms` on if it differs from the
/// last recorded value, and returns every change so far. Nothing is recorded while
/// the minimum stays 0, so a server that never sets one keeps the old rule.
pub fn streak_rule(conn: &Connection, minutes: u32, now_ms: i64) -> Result<StreakRule, Error> {
    let tx = conn.unchecked_transaction()?;
    let changes: Vec<StreakChange> = tx
        .prepare("select at, minutes from streak_minutes order by at")?
        .query_map([], |r| {
            Ok(StreakChange {
                at: r.get(0)?,
                minutes: r.get(1)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut rule = StreakRule { changes };
    let last = rule.changes.last().copied();
    if last.map_or(0, |change| change.minutes) != minutes {
        let at = now_ms.max(last.map_or(now_ms, |change| change.at + 1));
        tx.execute(
            "insert into streak_minutes (at, minutes) values (?1, ?2)",
            params![at, minutes],
        )?;
        rule.changes.push(StreakChange { at, minutes });
    }
    tx.commit()?;
    Ok(rule)
}

fn unknown(key: &str) -> Error {
    let known: Vec<&str> = KEYS.iter().map(|(key, _)| *key).collect();
    format!("unknown setting {key}; known: {}", known.join(", ")).into()
}

fn overrides(conn: &Connection) -> Result<Vec<(String, String, i64)>, Error> {
    let mut stmt =
        conn.prepare("select key, value, updated_at from server_settings order by key")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

pub const USAGE: &str = "usage: ankiquest [config.json] admin <command>
  settings                        show the settings the service is using
  set <setting> <value>           override a setting while the service runs
  reset <setting>                 go back to the configured value
  logs [--since <n>m|h|d|all] [--level info|error] [--user <player>] [--limit <n>]
                                  print the server log (default: last 7 days, 200 lines)
  sql <query>                     run a read-only SQL query against the state
  remove-account <user>           delete a self-service account and everything about it
  grant-freeze <user> <date>...   protect a player's streak on these days (YYYY-MM-DD)
  revoke-freeze <user> <date>...  take that protection away again";

#[derive(Debug, PartialEq)]
pub enum Command {
    Settings,
    Set(String, String),
    Reset(String),
    Logs(LogFilter),
    Sql(String),
    RemoveAccount(String),
    GrantFreeze(String, Vec<String>, bool),
}

#[derive(Debug, PartialEq)]
pub struct LogFilter {
    since_ms: Option<i64>,
    level: Option<String>,
    user: Option<String>,
    limit: u32,
}

fn key_of(name: &str) -> String {
    name.replace('-', "_")
}

pub fn parse(args: &[String]) -> Result<Command, Error> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    Ok(match words.as_slice() {
        ["settings"] => Command::Settings,
        ["set", key, value] => Command::Set(key_of(key), value.to_string()),
        ["reset", key] => Command::Reset(key_of(key)),
        ["sql", query] => Command::Sql(query.to_string()),
        ["remove-account", user] => Command::RemoveAccount(user.to_string()),
        [verb @ ("grant-freeze" | "revoke-freeze"), user, dates @ ..] if !dates.is_empty() => {
            let dates = dates
                .iter()
                .map(|date| {
                    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                        .map(|d| d.format("%Y-%m-%d").to_string())
                        .map_err(|_| format!("{date} is not a YYYY-MM-DD date"))
                })
                .collect::<Result<_, _>>()?;
            Command::GrantFreeze(user.to_string(), dates, *verb == "grant-freeze")
        }
        ["logs", options @ ..] => {
            let mut filter = LogFilter {
                since_ms: Some(7 * DAY_MS),
                level: None,
                user: None,
                limit: 200,
            };
            let mut options = options.iter();
            while let Some(option) = options.next() {
                let value = options
                    .next()
                    .ok_or_else(|| format!("{option} needs a value"))?;
                match *option {
                    "--since" => filter.since_ms = parse_age(value)?,
                    "--level" if matches!(*value, "info" | "error") => {
                        filter.level = Some(value.to_string());
                    }
                    "--level" => return Err("--level must be info or error".into()),
                    "--user" => filter.user = Some(value.to_string()),
                    "--limit" => {
                        filter.limit = value.parse().map_err(|_| "--limit must be a number")?;
                    }
                    other => return Err(format!("unknown option {other}").into()),
                }
            }
            Command::Logs(filter)
        }
        _ => return Err(USAGE.into()),
    })
}

fn parse_age(value: &str) -> Result<Option<i64>, Error> {
    if value == "all" {
        return Ok(None);
    }
    let invalid = || format!("--since takes a number with m, h or d, or all; not {value}");
    let split = value.char_indices().last().map_or(0, |(at, _)| at);
    let (number, unit) = value.split_at(split);
    let number: i64 = number.parse().map_err(|_| invalid())?;
    let unit = match unit {
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => DAY_MS,
        _ => return Err(invalid().into()),
    };
    Ok(Some(number.checked_mul(unit).ok_or_else(invalid)?))
}

pub fn run(
    state_dir: &Path,
    configured: Settings,
    command: Command,
    now_ms: i64,
) -> Result<String, Error> {
    if let Command::Sql(query) = &command {
        return sql(state_dir, query);
    }
    let store = Store::open(state_dir)?;
    let conn = &store.conn;
    Ok(match command {
        Command::Settings => {
            let effective = configured.effective(conn)?;
            let overridden = overrides(conn)?;
            let mut out = String::new();
            for (key, about) in KEYS {
                let source = match overridden.iter().find(|(k, _, _)| k == key) {
                    Some((_, _, at)) => format!(
                        "set by admin {}, configured {}",
                        timestamp(*at),
                        configured.get(key)
                    ),
                    None => "configured".to_string(),
                };
                out += &format!("{key} = {}  ({source})\n    {about}\n", effective.get(key));
            }
            out
        }
        Command::Set(key, value) => {
            let mut check = configured;
            check.set(&key, &value)?;
            let value = check.get(&key);
            conn.execute(
                "insert into server_settings (key, value, updated_at) values (?1, ?2, ?3)
                 on conflict (key) do update set value = excluded.value,
                                                  updated_at = excluded.updated_at",
                params![key, value, now_ms],
            )?;
            admin_log(conn, now_ms, format!("admin set {key} to {value}"))?;
            format!("{key} = {value}; the service applies it within a minute.\n")
        }
        Command::Reset(key) => {
            if !KEYS.iter().any(|(k, _)| *k == key) {
                return Err(unknown(&key));
            }
            let removed = conn.execute("delete from server_settings where key = ?1", [&key])?;
            if removed == 0 {
                format!("{key} was not overridden.\n")
            } else {
                admin_log(conn, now_ms, format!("admin reset {key}"))?;
                format!(
                    "{key} = {} again, as configured; the service applies it within a minute.\n",
                    configured.get(&key)
                )
            }
        }
        Command::Logs(filter) => logs(conn, &filter, now_ms)?,
        Command::RemoveAccount(user) => {
            if !conn
                .prepare("select 1 from accounts where user = ?1")?
                .exists([&user])?
            {
                return Err(format!(
                    "{user} is not a self-service account; remove configured players from the configuration"
                )
                .into());
            }
            conn.execute(
                "insert or ignore into account_removals (user, requested_at) values (?1, ?2)",
                params![user, now_ms],
            )?;
            admin_log(conn, now_ms, "admin asked to remove an account".into())?;
            format!("{user} and everything about them will be removed within a minute.\n")
        }
        Command::GrantFreeze(user, dates, granted) => {
            let changed = crate::freezes::grant(conn, &user, &dates, granted, now_ms)?;
            let verb = if granted { "granted" } else { "revoked" };
            admin_log(
                conn,
                now_ms,
                format!(
                    "admin {verb} streak protection for {user} on {}",
                    dates.join(", ")
                ),
            )?;
            format!(
                "{verb} {changed} day(s) for {user}; the service applies it within a minute.
"
            )
        }
        Command::Sql(_) => unreachable!(),
    })
}

fn admin_log(conn: &Connection, now_ms: i64, message: String) -> Result<(), Error> {
    insert(
        conn,
        &Entry {
            at: now_ms,
            level: "info",
            user: None,
            message,
        },
    )
}

fn timestamp(at_ms: i64) -> String {
    DateTime::from_timestamp_millis(at_ms).map_or_else(
        || at_ms.to_string(),
        |at| at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    )
}

fn logs(conn: &Connection, filter: &LogFilter, now_ms: i64) -> Result<String, Error> {
    let mut stmt = conn.prepare(
        "select at, level, user, message from (
             select id, at, level, user, message from server_log
             where at >= ?1 and (?2 is null or level = ?2) and (?3 is null or user = ?3)
             order by id desc limit ?4
         ) order by id",
    )?;
    let since = filter.since_ms.map_or(i64::MIN, |age| now_ms.saturating_sub(age));
    let rows = stmt.query_map(
        params![since, filter.level, filter.user, filter.limit],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
            ))
        },
    )?;
    let mut out = String::new();
    for row in rows {
        let (at, level, user, message) = row?;
        let user = user.map(|u| format!(" [{u}]")).unwrap_or_default();
        out += &format!("{} {level:<5}{user} {message}\n", timestamp(at));
    }
    if out.is_empty() {
        out = "no log entries match.\n".into();
    }
    Ok(out)
}

/// Tab-separated rows with a header, from a connection that cannot write.
fn sql(state_dir: &Path, query: &str) -> Result<String, Error> {
    let conn = Connection::open_with_flags(
        state_dir.join("ankiquest.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut stmt = conn.prepare(query)?;
    let columns = stmt.column_count();
    let mut out = stmt.column_names().join("\t") + "\n";
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let cells: Vec<String> = (0..columns)
            .map(|i| match row.get_ref(i) {
                Ok(ValueRef::Null) => "NULL".into(),
                Ok(ValueRef::Integer(n)) => n.to_string(),
                Ok(ValueRef::Real(x)) => x.to_string(),
                Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).replace(['\t', '\n'], " "),
                Ok(ValueRef::Blob(b)) => format!("<{} bytes>", b.len()),
                Err(e) => format!("<{e}>"),
            })
            .collect();
        out += &cells.join("\t");
        out.push('\n');
    }
    Ok(out)
}

/// The last time a setting was changed, so the service can say when it applies one.
#[cfg(test)]
fn updated_at(conn: &Connection, key: &str) -> Result<Option<i64>, Error> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "select updated_at from server_settings where key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    fn configured() -> Settings {
        Settings {
            review_weighting: ReviewWeighting::Flat,
            log_retention_days: DEFAULT_LOG_RETENTION_DAYS,
            streak_minutes: 0,
        }
    }

    fn state(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ankiquest-admin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn commands_parse_with_dashes_or_underscores() {
        assert_eq!(parse(&words(&["settings"])).unwrap(), Command::Settings);
        assert_eq!(
            parse(&words(&["set", "review-weighting", "diminishing"])).unwrap(),
            Command::Set("review_weighting".into(), "diminishing".into())
        );
        assert_eq!(
            parse(&words(&[
                "logs", "--since", "3h", "--user", "hill", "--level", "error"
            ]))
            .unwrap(),
            Command::Logs(LogFilter {
                since_ms: Some(3 * 3_600_000),
                level: Some("error".into()),
                user: Some("hill".into()),
                limit: 200,
            })
        );
        for bad in [
            vec![],
            vec!["set", "review_weighting"],
            vec!["logs", "--since"],
            vec!["logs", "--since", "3w"],
            vec!["logs", "--level", "debug"],
            vec!["frobnicate"],
            vec!["grant-freeze", "hill"],
            vec!["grant-freeze", "hill", "2026-09-31"],
            vec!["revoke-freeze", "hill", "12.09.2026"],
        ] {
            assert!(parse(&words(&bad)).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn granted_freezes_are_stored_and_queue_a_reload() {
        let dir = state("grant-freeze");
        let run = |args: &[&str]| run(&dir, configured(), parse(&words(args)).unwrap(), NOW);
        assert!(
            run(&[
                "grant-freeze",
                "hill",
                "2026-09-12",
                "2026-09-13",
                "2026-09-14"
            ])
            .unwrap()
            .starts_with("granted 3 day(s)")
        );
        assert!(
            run(&["grant-freeze", "hill", "2026-09-12"])
                .unwrap()
                .starts_with("granted 0 day(s)")
        );
        run(&["revoke-freeze", "hill", "2026-09-13"]).unwrap();
        let store = Store::open(&dir).unwrap();
        assert_eq!(
            store
                .granted_freezes("hill")
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["2026-09-12", "2026-09-14"]
        );
        assert!(store.granted_freezes("cerro").unwrap().is_empty());
        assert_eq!(
            crate::freezes::grant_reloads(&store.conn).unwrap(),
            ["hill"]
        );
        crate::freezes::grant_reloaded(&store.conn, "hill").unwrap();
        assert!(
            crate::freezes::grant_reloads(&store.conn)
                .unwrap()
                .is_empty()
        );
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn overrides_win_over_the_config_until_reset() {
        let dir = state("overrides");
        let run = |args: &[&str]| run(&dir, configured(), parse(&words(args)).unwrap(), NOW);

        assert!(run(&["set", "review_weighting", "sideways"]).is_err());
        assert!(run(&["set", "log_retention_days", "-1"]).is_err());
        assert!(run(&["set", "colour", "blue"]).is_err());
        assert!(run(&["set", "streak_minutes", "ten"]).is_err());
        assert!(run(&["set", "streak_minutes", "1441"]).is_err());

        run(&["set", "review-weighting", "diminishing"]).unwrap();
        run(&["set", "log_retention_days", "365"]).unwrap();
        let store = Store::open(&dir).unwrap();
        assert_eq!(
            configured().effective(&store.conn).unwrap(),
            Settings {
                review_weighting: ReviewWeighting::Diminishing,
                log_retention_days: 365,
                streak_minutes: 0,
            }
        );
        assert_eq!(
            updated_at(&store.conn, "review_weighting").unwrap(),
            Some(NOW)
        );
        assert!(
            run(&["settings"])
                .unwrap()
                .contains("review_weighting = diminishing  (set by admin")
        );

        run(&["reset", "review_weighting"]).unwrap();
        assert_eq!(
            configured()
                .effective(&store.conn)
                .unwrap()
                .review_weighting,
            ReviewWeighting::Flat
        );
        assert!(
            run(&["reset", "review_weighting"])
                .unwrap()
                .contains("was not overridden")
        );

        let log = run(&["logs"]).unwrap();
        assert!(
            log.contains("admin set review_weighting to diminishing"),
            "{log}"
        );
        assert!(log.contains("admin reset review_weighting"), "{log}");
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_log_keeps_entries_for_the_retention_and_filters_them() {
        let dir = state("log");
        let store = Store::open(&dir).unwrap();
        let conn = &store.conn;
        let entry = |at, level, user: Option<&str>, message: &str| Entry {
            at,
            level,
            user: user.map(str::to_string),
            message: message.into(),
        };
        insert(conn, &entry(NOW - 100 * DAY_MS, "info", None, "ancient")).unwrap();
        insert(conn, &entry(NOW - 30 * DAY_MS, "info", None, "last month")).unwrap();
        insert(
            conn,
            &entry(NOW - DAY_MS, "error", Some("hill"), "upload failed"),
        )
        .unwrap();
        insert(conn, &entry(NOW - 60_000, "info", Some("cerro"), "synced")).unwrap();

        flush(conn, NOW, 90).unwrap();
        let everything = run(
            &dir,
            configured(),
            parse(&words(&["logs", "--since", "all"])).unwrap(),
            NOW,
        )
        .unwrap();
        assert!(
            !everything.contains("ancient"),
            "older than 90 days is dropped"
        );
        assert!(
            everything.contains("last month"),
            "30 days old is kept: {everything}"
        );

        let recent = logs(
            conn,
            &LogFilter {
                since_ms: Some(7 * DAY_MS),
                level: None,
                user: None,
                limit: 200,
            },
            NOW,
        )
        .unwrap();
        assert!(!recent.contains("last month") && recent.contains("synced"));
        let errors = logs(
            conn,
            &LogFilter {
                since_ms: None,
                level: Some("error".into()),
                user: None,
                limit: 200,
            },
            NOW,
        )
        .unwrap();
        assert_eq!(errors.lines().count(), 1);
        assert!(errors.contains("error [hill] upload failed"), "{errors}");
        let newest = logs(
            conn,
            &LogFilter {
                since_ms: None,
                level: None,
                user: None,
                limit: 1,
            },
            NOW,
        )
        .unwrap();
        assert!(newest.contains("synced") && newest.lines().count() == 1);

        flush(conn, NOW, 0).unwrap();
        assert!(
            logs(
                conn,
                &LogFilter {
                    since_ms: None,
                    level: None,
                    user: None,
                    limit: 200
                },
                NOW
            )
            .unwrap()
            .contains("last month"),
            "0 keeps everything"
        );
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sql_reads_but_cannot_write() {
        let dir = state("sql");
        Store::open(&dir).unwrap();
        let out = sql(&dir, "select 1 as one, 'a\tb' as text, null as empty").unwrap();
        assert_eq!(out, "one\ttext\tempty\n1\ta b\tNULL\n");
        assert!(sql(&dir, "delete from server_log").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn streak_minimum_changes_are_recorded_once_with_their_time() {
        let dir = state("streak");
        let store = Store::open(&dir).unwrap();
        let conn = &store.conn;
        assert_eq!(streak_rule(conn, 0, NOW).unwrap(), StreakRule::default());
        let change = |at, minutes| StreakChange { at, minutes };
        assert_eq!(
            streak_rule(conn, 10, NOW + 1).unwrap().changes,
            [change(NOW + 1, 10)]
        );
        assert_eq!(
            streak_rule(conn, 10, NOW + 2).unwrap().changes,
            [change(NOW + 1, 10)],
            "an unchanged value keeps its start"
        );
        assert_eq!(
            streak_rule(conn, 5, NOW + 3).unwrap().changes,
            [change(NOW + 1, 10), change(NOW + 3, 5)]
        );
        assert_eq!(
            streak_rule(conn, 0, NOW + 3).unwrap().changes,
            [change(NOW + 1, 10), change(NOW + 3, 5), change(NOW + 4, 0)],
            "changes keep their order even within one millisecond"
        );
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn streak_minutes_can_be_overridden() {
        let dir = state("streak-override");
        run(
            &dir,
            configured(),
            parse(&words(&["set", "streak-minutes", "10"])).unwrap(),
            NOW,
        )
        .unwrap();
        let store = Store::open(&dir).unwrap();
        assert_eq!(
            configured().effective(&store.conn).unwrap().streak_minutes,
            10
        );
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
