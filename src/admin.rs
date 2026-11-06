//! The server log and runtime setting overrides, both kept in the SQLite state,
//! and the `ankiquest admin` command that reads and changes them while the
//! service runs. The service picks up overrides on its next poll.

use crate::game::{DAY_MS, PlayerStreakChange, ReviewWeighting, StreakChange, StreakRule};
use crate::store::{Error, Store};
use chrono::DateTime;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, types::ValueRef};
use std::collections::BTreeSet;
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

/// Like `log_error!`, for things someone should look at that are not failures.
macro_rules! log_warning {
    (user: $user:expr, $($arg:tt)*) => {
        $crate::admin::record("warning", Some(&*$user), format!($($arg)*))
    };
    ($($arg:tt)*) => {
        $crate::admin::record("warning", None, format!($($arg)*))
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
         );
         create table if not exists player_streak_minutes (
             user text not null,
             at integer not null,
             minutes integer,
             primary key (user, at)
         ) without rowid;",
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
    let mut rule = StreakRule {
        changes,
        players: player_streak_changes(&tx)?,
    };
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

fn player_streak_changes(
    conn: &Connection,
) -> Result<std::collections::BTreeMap<String, Vec<PlayerStreakChange>>, Error> {
    let mut players = std::collections::BTreeMap::<String, Vec<PlayerStreakChange>>::new();
    let mut stmt =
        conn.prepare("select user, at, minutes from player_streak_minutes order by user, at")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            PlayerStreakChange {
                at: r.get(1)?,
                minutes: r.get(2)?,
            },
        ))
    })?;
    for row in rows {
        let (user, change) = row?;
        players.entry(user).or_default().push(change);
    }
    Ok(players)
}

/// Records a per-player streak minimum (`None` hands the player back to the
/// server's) from `now_ms` on, unless it is already in force. Returns whether
/// anything changed.
fn set_player_streak_minutes(
    conn: &Connection,
    user: &str,
    minutes: Option<u32>,
    now_ms: i64,
) -> Result<bool, Error> {
    let last: Option<(i64, Option<u32>)> = conn
        .prepare(
            "select at, minutes from player_streak_minutes where user = ?1 order by at desc limit 1",
        )?
        .query_map([user], |r| Ok((r.get(0)?, r.get(1)?)))?
        .next()
        .transpose()?;
    if last.and_then(|(_, minutes)| minutes) == minutes {
        return Ok(false);
    }
    let at = now_ms.max(last.map_or(now_ms, |(at, _)| at + 1));
    conn.execute(
        "insert into player_streak_minutes (user, at, minutes) values (?1, ?2, ?3)",
        params![user, at, minutes],
    )?;
    Ok(true)
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
  logs [--since <n>m|h|d|all] [--level info|warning|error] [--user <player>] [--limit <n>]
                                  print the server log (default: last 7 days, 200 lines)
  sql <query>                     run a read-only SQL query against the state
  remove-account <user>           delete a self-service account and everything about it
  remove-player <user>            delete everything stored about any player, also one no
                                  longer configured; backs the database up first
  group-add <group> <user>...|--all
                                  put players in a group (by id), with the same limits as
                                  joining by invite
  group-remove <group> <user>...  take players out of a group; the longest member inherits
                                  it, and decks stop going to people no longer connected
  befriend <user> <user>...|--all make every pair of these players friends
  unfriend <user> [<user>...]     end the friendships between the first player and the
                                  others, or all of the first player's friendships
  set-password <user>             give a self-service account a new random password, for
                                  someone who forgot theirs; prints it once
  adopt-players                   turn every configured player into a regular account that
                                  keeps its name, display name and token, so the player
                                  can then be removed from the configuration
  grant-freeze <user> <date>...   protect a player's streak on these days (YYYY-MM-DD)
  revoke-freeze <user> <date>...  take that protection away again
  streak-minutes <user> <minutes|default>
                                  give one player their own streak minimum from their
                                  next Anki day on; default goes back to the server's";

#[derive(Debug, PartialEq)]
pub enum Command {
    Settings,
    Set(String, String),
    Reset(String),
    Logs(LogFilter),
    Sql(String),
    RemoveAccount(String),
    SetPassword(String),
    AdoptPlayers,
    GrantFreeze(String, Vec<String>, bool),
    PlayerStreakMinutes(String, Option<u32>),
    RemovePlayer(String),
    GroupAdd(i64, Players),
    GroupRemove(i64, Vec<String>),
    Befriend(Players),
    Unfriend(String, Vec<String>),
}

/// The players a command names, or everyone.
#[derive(Debug, PartialEq)]
pub enum Players {
    All,
    Named(Vec<String>),
}

impl Command {
    /// Commands about who plays and who is connected, which need the configuration; see [`people`].
    pub fn is_about_people(&self) -> bool {
        matches!(
            self,
            Command::RemovePlayer(_)
                | Command::GroupAdd(..)
                | Command::GroupRemove(..)
                | Command::Befriend(_)
                | Command::Unfriend(..)
        )
    }
}

fn group_id(value: &str) -> Result<i64, Error> {
    value.parse().map_err(|_| {
        format!("{value} is not a group id; find it with: sql \"select id, name from groups\"")
            .into()
    })
}

fn names(words: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for word in words {
        if !names.iter().any(|name| name == word) {
            names.push(word.to_string());
        }
    }
    names
}

fn players(words: &[&str]) -> Players {
    if words == ["--all"] {
        Players::All
    } else {
        Players::Named(names(words))
    }
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
        ["remove-player", user] => Command::RemovePlayer(user.to_string()),
        ["group-add", group, users @ ..] if !users.is_empty() => {
            Command::GroupAdd(group_id(group)?, players(users))
        }
        ["group-remove", group, users @ ..] if !users.is_empty() => {
            Command::GroupRemove(group_id(group)?, names(users))
        }
        ["befriend", users @ ..] if users == ["--all"] || users.len() >= 2 => {
            Command::Befriend(players(users))
        }
        ["unfriend", user, others @ ..] => Command::Unfriend(user.to_string(), names(others)),
        ["set-password", user] => Command::SetPassword(user.to_string()),
        ["adopt-players"] => Command::AdoptPlayers,
        ["streak-minutes", user, "default"] => Command::PlayerStreakMinutes(user.to_string(), None),
        ["streak-minutes", user, minutes] => Command::PlayerStreakMinutes(
            user.to_string(),
            Some(
                minutes
                    .parse()
                    .ok()
                    .filter(|minutes| *minutes <= MAX_STREAK_MINUTES)
                    .ok_or(
                        "streak-minutes takes a whole number of minutes, at most 1440, or default",
                    )?,
            ),
        ),
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
                    "--level" if matches!(*value, "info" | "warning" | "error") => {
                        filter.level = Some(value.to_string());
                    }
                    "--level" => return Err("--level must be info, warning or error".into()),
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
    if command == Command::AdoptPlayers || command.is_about_people() {
        return Err("this command needs the configuration; use adopt() or people()".into());
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
            for (user, changes) in player_streak_changes(conn)? {
                if let Some(PlayerStreakChange {
                    at,
                    minutes: Some(minutes),
                }) = changes.last()
                {
                    out += &format!(
                        "streak_minutes for {user} = {minutes}  (set by admin {})\n",
                        timestamp(*at)
                    );
                }
            }
            out
        }
        Command::PlayerStreakMinutes(user, minutes) => {
            if !store.users()?.contains(&user) {
                return Err(format!("{user} is not a player on this server").into());
            }
            let shown = minutes.map_or("the server's value".to_string(), |m| m.to_string());
            if set_player_streak_minutes(conn, &user, minutes, now_ms)? {
                admin_log(
                    conn,
                    now_ms,
                    format!("admin set streak_minutes for {user} to {shown}"),
                )?;
                format!(
                    "streak_minutes for {user} = {shown} from their next Anki day; the service applies it within a minute.\n"
                )
            } else {
                format!("streak_minutes for {user} is already {shown}.\n")
            }
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
        Command::SetPassword(user) => {
            let password = temporary_password()?;
            let hash = crate::accounts::hash_password(&password)?;
            if conn.execute(
                "update accounts set password_hash = ?2 where user = ?1",
                params![user, hash],
            )? == 0
            {
                return Err(format!(
                    "{user} is not a self-service account; configured players sign in with the token in the configuration"
                )
                .into());
            }
            admin_log(
                conn,
                now_ms,
                format!("admin set a temporary password for {user}"),
            )?;
            format!(
                "{user} can now sign in with this password:

    {password}

it works straight away and is shown only now. tell them to change it under Settings.
other devices stay signed in.
"
            )
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
        _ => unreachable!(),
    })
}

/// Sixteen letters and digits in groups of four, without look-alikes such as 0/o and 1/l,
/// so it can be read out or typed from a message.
fn temporary_password() -> Result<String, Error> {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut out = String::new();
    while out.len() < 19 {
        for byte in crate::web_util::random_bytes("password randomness unavailable")? {
            // 248 is the largest multiple of 31 that fits, so every character is as likely.
            if byte < 248 && out.len() < 19 {
                if out.len() % 5 == 4 {
                    out.push('-');
                }
                out.push(ALPHABET[usize::from(byte) % ALPHABET.len()] as char);
            }
        }
    }
    Ok(out)
}

/// A configured player to turn into an account: name, display name and token.
pub struct Configured {
    pub user: String,
    pub display: Option<String>,
    pub token: Option<String>,
}

/// Makes every configured player with a token a regular account with the same name, display
/// name and token. Existing phones stay signed in, and the player can then leave the
/// configuration. Players who already have an account are left alone.
pub fn adopt(state_dir: &Path, players: &[Configured], now_ms: i64) -> Result<String, Error> {
    let mut store = Store::open(state_dir)?;
    let tx = store.conn.transaction()?;
    let mut out = String::new();
    let mut sorted: Vec<&Configured> = players.iter().collect();
    sorted.sort_by(|a, b| a.user.cmp(&b.user));
    for player in sorted {
        let Some(token) = player.token.as_deref().filter(|t| !t.is_empty()) else {
            out += &format!(
                "{}: skipped, no token in the configuration
",
                player.user
            );
            continue;
        };
        let display = player.display.as_deref().unwrap_or(&player.user);
        if !crate::accounts::create(&tx, &player.user, display, "", now_ms)? {
            out += &format!(
                "{}: already an account
",
                player.user
            );
            continue;
        }
        tx.execute(
            "insert or ignore into account_tokens (token_hash, user, device, created_at) values (?1, ?2, ?3, ?4)",
            params![crate::accounts::token_hash(token), player.user, crate::accounts::ADOPTED_DEVICE, now_ms],
        )?;
        admin_log(
            &tx,
            now_ms,
            format!(
                "admin turned the configured player {} into an account",
                player.user
            ),
        )?;
        out += &format!(
            "{}: now an account, signed in on the same devices
",
            player.user
        );
    }
    tx.commit()?;
    out += "remove adopted players from the configuration and restart; they keep everything.
            they have no password yet: give them one with set-password, or let them link a sign-in provider.
";
    Ok(out)
}

/// What [`people`] needs from the configuration besides the state.
pub struct Setup<'a> {
    pub state_dir: &'a Path,
    /// Where each change backs the database up before it writes.
    pub backups: &'a Path,
    /// The players in the configuration.
    pub configured: BTreeSet<String>,
    pub sync_base: Option<&'a Path>,
}

/// Who plays here, as the running service sees it.
struct Roster {
    configured: BTreeSet<String>,
    accounts: BTreeSet<String>,
    /// Everyone with a history, configured or with an account.
    known: BTreeSet<String>,
}

impl Roster {
    fn load(store: &Store, configured: BTreeSet<String>) -> Result<Self, Error> {
        let accounts: BTreeSet<String> = store
            .conn
            .prepare("select user from accounts")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let mut known: BTreeSet<String> = store.users()?.into_iter().collect();
        known.extend(configured.iter().cloned());
        known.extend(accounts.iter().cloned());
        Ok(Roster {
            configured,
            accounts,
            known,
        })
    }

    /// Players others can befriend: configured players and accounts, as `App::is_user`.
    fn players(&self) -> BTreeSet<String> {
        self.configured.union(&self.accounts).cloned().collect()
    }

    fn named(&self, who: Players) -> Result<Vec<String>, Error> {
        let players = self.players();
        match who {
            Players::All => Ok(players.into_iter().collect()),
            Players::Named(users) => {
                let strangers: Vec<&str> = users
                    .iter()
                    .filter(|user| !players.contains(*user))
                    .map(String::as_str)
                    .collect();
                if strangers.is_empty() {
                    Ok(users)
                } else {
                    Err(format!(
                        "not a player on this server: {}; nothing was changed",
                        strangers.join(", ")
                    )
                    .into())
                }
            }
        }
    }

    /// As `App::circle`.
    fn circle(&self, conn: &Connection, user: &str) -> rusqlite::Result<BTreeSet<String>> {
        crate::social::circle_of(conn, user, !self.accounts.contains(user), || {
            self.known
                .iter()
                .filter(|other| !self.accounts.contains(*other))
                .cloned()
                .collect()
        })
    }

    /// Stops deck notifications `user` no longer has a connection for, and says how many.
    fn prune(&self, conn: &Connection, user: &str) -> Result<String, Error> {
        let removed = crate::social::prune_outside(conn, user, &self.circle(conn, user)?)?;
        Ok(if removed == 0 {
            String::new()
        } else {
            format!(
                "; {removed} deck notification(s) to or from people no longer connected stopped"
            )
        })
    }
}

/// Copies the whole database to `dir` before a change, so it can be undone by hand.
fn backup(conn: &Connection, dir: &Path, verb: &str, now_ms: i64) -> Result<String, Error> {
    std::fs::create_dir_all(dir).map_err(|error| {
        format!(
            "could not create the backup folder {}: {error}; nothing was changed",
            dir.display()
        )
    })?;
    let stamp = DateTime::from_timestamp_millis(now_ms).map_or_else(
        || now_ms.to_string(),
        |at| at.format("%Y%m%d-%H%M%S").to_string(),
    );
    let mut path = dir.join(format!("ankiquest-{stamp}-{verb}.db"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("ankiquest-{stamp}-{verb}-{n}.db"));
    }
    let shown = path.display().to_string();
    conn.execute("vacuum into ?1", [&shown]).map_err(|error| {
        format!("could not back the database up to {shown}: {error}; nothing was changed")
    })?;
    Ok(format!("backed up the database to {shown}\n"))
}

/// The commands about who plays and who is connected with whom. Each checks first, then backs
/// the database up, then writes and says what changed. The running service reads groups and
/// friendships from the database on every request, so changes apply straight away; a removed
/// player leaves its memory on its next tick.
pub fn people(setup: &Setup, command: Command, now_ms: i64) -> Result<String, Error> {
    let store = Store::open(setup.state_dir)?;
    let roster = Roster::load(&store, setup.configured.clone())?;
    let conn = &store.conn;
    let group_name = |group: i64| -> Result<String, Error> {
        conn.query_row("select name from groups where id = ?1", [group], |r| {
            r.get(0)
        })
        .optional()?
        .ok_or_else(|| {
            format!(
                "there is no group {group}; list them with: sql \"select id, name from groups\""
            )
            .into()
        })
    };
    let live = "the service sees this straight away, no restart needed.\n";
    Ok(match command {
        Command::GroupAdd(group, who) => {
            let name = group_name(group)?;
            let users = roster.named(who)?;
            let mut out = backup(conn, setup.backups, "group-add", now_ms)?;
            let tx = conn.unchecked_transaction()?;
            let mut added = Vec::new();
            for user in &users {
                if crate::social::is_member(&tx, group, user)? {
                    out += &format!("{user}: already in {name}\n");
                    continue;
                }
                let groups: i64 = tx.query_row(
                    "select count(*) from group_members where user = ?1",
                    [user],
                    |r| r.get(0),
                )?;
                let members: i64 = tx.query_row(
                    "select count(*) from group_members where group_id = ?1",
                    [group],
                    |r| r.get(0),
                )?;
                if groups >= crate::social::MAX_GROUPS {
                    out += &format!(
                        "{user}: skipped, already in {} groups\n",
                        crate::social::MAX_GROUPS
                    );
                    continue;
                }
                if members >= crate::social::MAX_MEMBERS {
                    out += &format!(
                        "{user}: skipped, {name} is full ({} members)\n",
                        crate::social::MAX_MEMBERS
                    );
                    continue;
                }
                tx.execute(
                    "insert into group_members (group_id, user, owner, joined_at) values (?1, ?2, 0, ?3)",
                    params![group, user, now_ms],
                )?;
                out += &format!("{user}: added to {name}\n");
                added.push(user.as_str());
            }
            if !added.is_empty() {
                admin_log(
                    &tx,
                    now_ms,
                    format!("admin added {} to group {group}", added.join(", ")),
                )?;
            }
            tx.commit()?;
            out + live
        }
        Command::GroupRemove(group, users) => {
            let name = group_name(group)?;
            let mut out = backup(conn, setup.backups, "group-remove", now_ms)?;
            let tx = conn.unchecked_transaction()?;
            let mut removed = Vec::new();
            for user in &users {
                if !crate::social::is_member(&tx, group, user)? {
                    out += &format!("{user}: not in {name}\n");
                    continue;
                }
                let owned = crate::social::owns(&tx, group, user)?;
                crate::social::leave(&tx, group, user)?;
                let pruned = roster.prune(&tx, user)?;
                out += &format!("{user}: removed from {name}{pruned}\n");
                if owned {
                    let next: Option<String> = tx
                        .query_row(
                            "select user from group_members where group_id = ?1 and owner = 1",
                            [group],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if let Some(next) = next {
                        out += &format!("{next} now owns {name}\n");
                    }
                }
                removed.push(user.as_str());
            }
            if !tx
                .prepare("select 1 from groups where id = ?1")?
                .exists([group])?
            {
                out += &format!("{name} had nobody left and was deleted\n");
            }
            if !removed.is_empty() {
                admin_log(
                    &tx,
                    now_ms,
                    format!("admin removed {} from group {group}", removed.join(", ")),
                )?;
            }
            tx.commit()?;
            out + live
        }
        Command::Befriend(who) => {
            let users = roster.named(who)?;
            if users.len() < 2 {
                return Err("befriend needs at least two players".into());
            }
            let mut out = backup(conn, setup.backups, "befriend", now_ms)?;
            let tx = conn.unchecked_transaction()?;
            let (mut made, mut already) = (0, 0);
            for (i, x) in users.iter().enumerate() {
                for y in &users[i + 1..] {
                    let (a, b) = crate::social::pair(x, y);
                    let accepted: Option<bool> = tx
                        .query_row(
                            "select accepted from friendships where a = ?1 and b = ?2",
                            params![a, b],
                            |r| r.get(0),
                        )
                        .optional()?;
                    match accepted {
                        Some(true) => {
                            already += 1;
                            continue;
                        }
                        Some(false) => {
                            tx.execute(
                                "update friendships set accepted = 1 where a = ?1 and b = ?2",
                                params![a, b],
                            )?;
                            out += &format!(
                                "{a} and {b}: friends now (the open request was accepted)\n"
                            );
                        }
                        None => {
                            tx.execute(
                                "insert into friendships (a, b, requested_by, accepted, created_at) values (?1, ?2, ?1, 1, ?3)",
                                params![a, b, now_ms],
                            )?;
                            out += &format!("{a} and {b}: friends now\n");
                        }
                    }
                    made += 1;
                }
            }
            if already > 0 {
                out += &format!("{already} pair(s) were friends already\n");
            }
            if made > 0 {
                admin_log(
                    &tx,
                    now_ms,
                    format!(
                        "admin made {made} pair(s) of friends among {}",
                        users.join(", ")
                    ),
                )?;
            }
            tx.commit()?;
            out + live
        }
        Command::Unfriend(user, others) => {
            let rows: Vec<(String, String, bool)> = conn
                .prepare(
                    "select a, b, accepted from friendships where a = ?1 or b = ?1 order by a, b",
                )?
                .query_map([&user], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?;
            let rows: Vec<(String, String, bool)> = rows
                .into_iter()
                .filter(|(a, b, _)| {
                    let other = if *a == user { b } else { a };
                    others.is_empty() || others.contains(other)
                })
                .collect();
            if rows.is_empty() {
                return Ok(format!(
                    "{user} has no such friendship or request; nothing was changed.\n"
                ));
            }
            let mut out = backup(conn, setup.backups, "unfriend", now_ms)?;
            let tx = conn.unchecked_transaction()?;
            let mut ended = Vec::new();
            for (a, b, accepted) in &rows {
                tx.execute(
                    "delete from friendships where a = ?1 and b = ?2",
                    params![a, b],
                )?;
                let other = if *a == user { b } else { a };
                let what = if *accepted {
                    "no longer friends"
                } else {
                    "friend request withdrawn"
                };
                out += &format!("{user} and {other}: {what}\n");
                ended.push(other.as_str());
            }
            let pruned = roster.prune(&tx, &user)?;
            if !pruned.is_empty() {
                out += &format!("{user}{pruned}\n");
            }
            admin_log(
                &tx,
                now_ms,
                format!(
                    "admin ended the friendships of {user} with {}",
                    ended.join(", ")
                ),
            )?;
            tx.commit()?;
            out + live
        }
        Command::RemovePlayer(user) => {
            if !crate::privacy::mentioned(conn, &user)? {
                return Err(format!("nothing is stored about {user}; nothing was changed").into());
            }
            let mut out = backup(conn, setup.backups, "remove-player", now_ms)?;
            let removed = crate::privacy::forget(conn, &user)?;
            // The running service then drops them from memory and signs their devices out.
            conn.execute(
                "insert or ignore into account_removals (user, requested_at) values (?1, ?2)",
                params![user, now_ms],
            )?;
            admin_log(conn, now_ms, format!("admin removed the player {user}"))?;
            out += &format!("removed everything about {user}:\n");
            for (table, rows) in removed {
                out += &format!("    {table}: {rows} row(s)\n");
            }
            out += "the service forgets them within a minute and signs their devices out.\n";
            if roster.configured.contains(&user) {
                out += &format!(
                    "{user} is still in the configuration: take them out of users and restart, or they come back with nothing.\n"
                );
            }
            if let Some(base) = setup.sync_base {
                let folder = base.join(&user);
                if folder.join("collection.anki2").is_file() {
                    out += &format!(
                        "their sync folder {} is still there, so the next import brings their reviews back; remove them from the sync server too.\n",
                        folder.display()
                    );
                }
            }
            out
        }
        _ => return Err("not a command about players; use run()".into()),
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
    let since = filter
        .since_ms
        .map_or(i64::MIN, |age| now_ms.saturating_sub(age));
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
    use crate::test_support::cleanup;

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
    fn configured_players_become_accounts_that_keep_their_token() {
        let dir = state("adopt");
        {
            let store = Store::open(&dir).unwrap();
            crate::accounts::create(&store.conn, "bo", "Bo", "", 0).unwrap();
        }
        let players = [
            Configured {
                user: "ana".into(),
                display: Some("Ana María".into()),
                token: Some("ana-token".into()),
            },
            Configured {
                user: "bo".into(),
                display: None,
                token: Some("bo-token".into()),
            },
            Configured {
                user: "cy".into(),
                display: None,
                token: None,
            },
        ];
        let out = adopt(&dir, &players, 5).unwrap();
        assert!(out.contains("ana: now an account"), "{out}");
        assert!(out.contains("bo: already an account"), "{out}");
        assert!(out.contains("cy: skipped"), "{out}");
        let store = Store::open(&dir).unwrap();
        let directory = crate::accounts::Directory::load(&store.conn).unwrap();
        assert_eq!(
            directory.token_owner("ana-token").as_deref(),
            Some("ana"),
            "the phone stays signed in"
        );
        assert_eq!(directory.account("ana").unwrap().display, "Ana María");
        let device: String = store
            .conn
            .query_row(
                "select device from account_tokens where user = 'ana'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(device, crate::accounts::ADOPTED_DEVICE);
        assert_eq!(
            directory.token_owner("bo-token"),
            None,
            "an existing account is left alone"
        );
        assert!(!directory.contains("cy"));
        assert!(
            adopt(&dir, &players, 6)
                .unwrap()
                .contains("ana: already an account"),
            "running it twice is safe"
        );
        assert_eq!(
            parse(&words(&["adopt-players"])).unwrap(),
            Command::AdoptPlayers
        );
    }

    #[test]
    fn a_forgotten_password_is_replaced_by_a_random_one() {
        let dir = state("set-password");
        {
            let store = Store::open(&dir).unwrap();
            crate::accounts::create(&store.conn, "ana", "Ana", "", 0).unwrap();
        }
        assert_eq!(
            parse(&words(&["set-password", "ana"])).unwrap(),
            Command::SetPassword("ana".into())
        );
        let out = run(&dir, configured(), Command::SetPassword("ana".into()), 5).unwrap();
        let password = out
            .lines()
            .map(str::trim)
            .find(|line| line.len() == 19 && line.matches('-').count() == 3)
            .unwrap()
            .to_string();
        assert!(
            password
                .chars()
                .all(|c| c == '-' || "abcdefghjkmnpqrstuvwxyz23456789".contains(c))
        );
        let store = Store::open(&dir).unwrap();
        let hash: String = store
            .conn
            .query_row(
                "select password_hash from accounts where user = 'ana'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(crate::accounts::verify_password(&hash, &password));
        let logged: String = store
            .conn
            .query_row("select message from server_log", [], |r| r.get(0))
            .unwrap();
        assert!(!logged.contains(&password), "the log never holds it");
        drop(store);
        let again = run(&dir, configured(), Command::SetPassword("ana".into()), 6).unwrap();
        assert!(!again.contains(&password), "a new one each time");
        assert!(run(&dir, configured(), Command::SetPassword("bo".into()), 7).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
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
        cleanup(store, dir);
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
        cleanup(store, dir);
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
        cleanup(store, dir);
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
        cleanup(store, dir);
    }

    #[test]
    fn a_player_can_get_their_own_streak_minimum() {
        let dir = state("player-streak");
        let cmd = |args: &[&str], now| run(&dir, configured(), parse(&words(args))?, now);
        assert!(parse(&words(&["streak-minutes", "aldanita", "ten"])).is_err());
        assert!(parse(&words(&["streak-minutes", "aldanita", "1441"])).is_err());
        let store = Store::open(&dir).unwrap();
        store
            .conn
            .execute(
                "insert into clocks (user, offset_west_min, rollover_hour) values ('aldanita', 0, 4)",
                [],
            )
            .unwrap();
        drop(store);
        assert!(cmd(&["streak-minutes", "nobody", "3"], NOW).is_err());
        assert!(
            cmd(&["streak-minutes", "aldanita", "3"], NOW)
                .unwrap()
                .contains("= 3 from their next Anki day")
        );
        assert!(
            cmd(&["streak-minutes", "aldanita", "3"], NOW + 5)
                .unwrap()
                .contains("already 3")
        );
        assert!(
            cmd(&["settings"], NOW + 5)
                .unwrap()
                .contains("streak_minutes for aldanita = 3")
        );

        let store = Store::open(&dir).unwrap();
        let rule = streak_rule(&store.conn, 10, NOW + 6).unwrap();
        assert_eq!(rule.for_player("aldanita").minutes_at(NOW + 7), 3);
        assert_eq!(rule.for_player("ivancito").minutes_at(NOW + 7), 10);
        drop(store);

        cmd(&["streak-minutes", "aldanita", "default"], NOW + 8).unwrap();
        let store = Store::open(&dir).unwrap();
        let rule = streak_rule(&store.conn, 10, NOW + 9).unwrap();
        assert_eq!(rule.for_player("aldanita").minutes_at(NOW + 7), 3);
        assert_eq!(rule.for_player("aldanita").minutes_at(NOW + 9), 10);
        cleanup(store, dir);
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
        cleanup(store, dir);
    }

    fn setup<'a>(dir: &'a std::path::Path, backups: &'a std::path::Path) -> Setup<'a> {
        Setup {
            state_dir: dir,
            backups,
            configured: ["cfg".to_string()].into(),
            sync_base: None,
        }
    }

    fn rows(store: &Store, sql: &str) -> Vec<String> {
        store
            .conn
            .prepare(sql)
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn backups_in(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |entries| entries.count())
    }

    #[test]
    fn groups_and_friendships_change_with_the_apis_checks_and_a_backup_first() {
        let dir = state("people");
        let backups = dir.join("backups");
        {
            let store = Store::open(&dir).unwrap();
            for user in ["ana", "bo", "cy"] {
                crate::accounts::create(&store.conn, user, user, "", 0).unwrap();
            }
            store
                .conn
                .execute_batch(
                    "insert into groups (id, name, invite, created_by, created_at) values (1, 'Club', 'x', 'ana', 0);
                     insert into group_members (group_id, user, owner, joined_at) values (1, 'ana', 1, 0), (1, 'bo', 0, 1);
                     insert into decks (owner, id, name, enabled) values ('ana', 'd', 'Spanish', 1);
                     insert into deck_recipients (owner, deck_id, recipient) values ('ana', 'd', 'bo');
                     insert into friendships (a, b, requested_by, accepted, created_at) values ('bo', 'cy', 'cy', 0, 0);",
                )
                .unwrap();
        }
        let go = |args: &[&str]| people(&setup(&dir, &backups), parse(&words(args)).unwrap(), NOW);

        let error = go(&["group-add", "1", "cy", "nobody"])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("nobody") && error.contains("nothing was changed"),
            "{error}"
        );
        assert_eq!(backups_in(&backups), 0, "nothing to undo, so no backup");
        assert!(
            go(&["group-add", "9", "cy"])
                .unwrap_err()
                .to_string()
                .contains("no group 9")
        );

        let out = go(&["group-add", "1", "--all"]).unwrap();
        assert!(out.contains("backed up the database to"), "{out}");
        assert!(out.contains("ana: already in Club"), "{out}");
        assert!(
            out.contains("cfg: added to Club") && out.contains("cy: added to Club"),
            "{out}"
        );
        assert!(out.contains("no restart"), "{out}");
        assert_eq!(backups_in(&backups), 1);
        let store = Store::open(&dir).unwrap();
        assert_eq!(
            rows(
                &store,
                "select user from group_members where group_id = 1 order by user"
            ),
            ["ana", "bo", "cfg", "cy"]
        );

        let out = go(&["befriend", "ana", "bo", "cy"]).unwrap();
        assert!(out.contains("ana and bo: friends now"), "{out}");
        assert!(
            out.contains("bo and cy: friends now (the open request was accepted)"),
            "{out}"
        );
        assert!(
            go(&["befriend", "ana", "bo"])
                .unwrap()
                .contains("1 pair(s) were friends already")
        );
        assert_eq!(
            rows(
                &store,
                "select a || '-' || b from friendships where accepted = 1 order by a, b"
            ),
            ["ana-bo", "ana-cy", "bo-cy"]
        );

        let out = go(&["group-remove", "1", "ana", "zed"]).unwrap();
        assert!(out.contains("ana: removed from Club"), "{out}");
        assert!(out.contains("zed: not in Club"), "{out}");
        assert!(
            out.contains("bo now owns Club"),
            "the longest member inherits it: {out}"
        );
        assert_eq!(
            rows(&store, "select owner from deck_recipients"),
            ["ana"],
            "still friends, so the deck still goes to bo"
        );

        let out = go(&["unfriend", "ana"]).unwrap();
        assert!(
            out.contains("ana and bo: no longer friends") && out.contains("ana and cy"),
            "{out}"
        );
        assert!(out.contains("1 deck notification(s)"), "{out}");
        assert!(rows(&store, "select owner from deck_recipients").is_empty());
        assert!(
            go(&["unfriend", "ana", "bo"])
                .unwrap()
                .contains("nothing was changed")
        );

        let out = go(&["group-remove", "1", "bo", "cfg", "cy"]).unwrap();
        assert!(
            out.contains("Club had nobody left and was deleted"),
            "{out}"
        );
        assert!(rows(&store, "select name from groups").is_empty());
        assert!(
            rows(&store, "select message from server_log")
                .iter()
                .any(|m| m == "admin added cfg, cy to group 1"),
        );
        cleanup(store, dir);
    }

    #[test]
    fn a_player_who_left_the_configuration_can_be_removed_entirely() {
        let dir = state("remove-player");
        let backups = dir.join("backups");
        {
            let store = Store::open(&dir).unwrap();
            crate::accounts::create(&store.conn, "ana", "Ana", "", 0).unwrap();
            store
                .conn
                .execute_batch(
                    "insert into reviews (user, id, cid, last_ivl, time_ms, kind) values ('gone', 1, 10, 1, 5000, 1), ('ana', 2, 20, 1, 5000, 1);
                     insert into deck_recipients (owner, deck_id, recipient) values ('ana', 'd', 'gone'), ('gone', 'e', 'ana');
                     insert into notifications (recipient, sender, title, body, day, created_at, kind) values ('ana', 'gone', 't', 'b', 1, 1, 'reply'), ('gone', 'ana', 't', 'b', 1, 1, 'reply');
                     insert into community_weekly_suggestions (owner, week_start, expires_at, friend) values ('ana', 1, 2, 'gone');
                     insert into friendships (a, b, requested_by, accepted, created_at) values ('ana', 'gone', 'gone', 1, 0);
                     insert into groups (id, name, invite, created_by, created_at) values (1, 'Club', 'x', 'gone', 0);
                     insert into group_members (group_id, user, owner, joined_at) values (1, 'gone', 1, 0), (1, 'ana', 0, 1);",
                )
                .unwrap();
        }
        let go = |user: &str| {
            people(
                &setup(&dir, &backups),
                parse(&words(&["remove-player", user])).unwrap(),
                NOW,
            )
        };
        assert!(
            go("nobody")
                .unwrap_err()
                .to_string()
                .contains("nothing is stored about nobody")
        );
        assert_eq!(backups_in(&backups), 0);
        let out = go("gone").unwrap();
        assert!(out.contains("backed up the database to"), "{out}");
        for removed in [
            "reviews: 1 row(s)",
            "deck_recipients: 2 row(s)",
            "notifications: 2 row(s)",
            "community_weekly_suggestions: 1 row(s)",
            "friendships: 1 row(s)",
            "group_members: 1 row(s)",
        ] {
            assert!(out.contains(removed), "{removed} in {out}");
        }
        assert_eq!(backups_in(&backups), 1);
        let store = Store::open(&dir).unwrap();
        assert_eq!(
            rows(&store, "select user from account_removals"),
            ["gone"],
            "the running service is told to drop them from memory"
        );
        store
            .conn
            .execute("delete from account_removals", [])
            .unwrap();
        assert!(!crate::privacy::mentioned(&store.conn, "gone").unwrap());
        assert_eq!(
            rows(&store, "select user from group_members where owner = 1"),
            ["ana"]
        );
        assert_eq!(rows(&store, "select user from reviews"), ["ana"]);
        assert_eq!(
            rows(&store, "select friend from community_weekly_suggestions"),
            Vec::<String>::new()
        );
        cleanup(store, dir);
    }

    #[test]
    fn people_commands_parse() {
        assert_eq!(
            parse(&words(&["group-add", "3", "ana", "bo", "ana"])).unwrap(),
            Command::GroupAdd(3, Players::Named(vec!["ana".into(), "bo".into()]))
        );
        assert_eq!(
            parse(&words(&["befriend", "--all"])).unwrap(),
            Command::Befriend(Players::All)
        );
        assert!(parse(&words(&["befriend", "ana"])).is_err());
        assert!(parse(&words(&["group-add", "club", "ana"])).is_err());
        assert!(parse(&words(&["group-remove", "1"])).is_err());
        assert_eq!(
            parse(&words(&["unfriend", "ana"])).unwrap(),
            Command::Unfriend("ana".into(), vec![])
        );
        assert_eq!(
            parse(&words(&["logs", "--level", "warning"])).unwrap(),
            Command::Logs(LogFilter {
                since_ms: Some(7 * DAY_MS),
                level: Some("warning".into()),
                user: None,
                limit: 200,
            })
        );
        assert!(
            run(
                &state("people-run"),
                configured(),
                Command::RemovePlayer("x".into()),
                NOW
            )
            .is_err(),
            "run() has no configuration to check against"
        );
    }
}
