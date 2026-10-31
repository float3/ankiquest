//! Celebratory moments kept per player, as `docs/sharing.md` describes them.
use crate::store::Error;
use rusqlite::{Connection, OptionalExtension, Row};

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists wins (
             id integer primary key,
             user text not null,
             kind text not null,
             key text not null,
             day integer not null,
             created_at integer not null,
             title text not null,
             body text not null,
             stats text not null,
             art text not null,
             shared_friends integer not null default 0,
             link_token text unique,
             unique(user, key)
         );",
    )?;
    Ok(())
}

#[allow(dead_code)]
pub(crate) struct Win {
    pub(crate) id: i64,
    pub(crate) user: String,
    pub(crate) kind: String,
    pub(crate) key: String,
    pub(crate) day: i64,
    /// Milliseconds since the epoch.
    pub(crate) created_at: i64,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) stats: serde_json::Value,
    pub(crate) art: String,
    pub(crate) shared_friends: bool,
    pub(crate) link_token: Option<String>,
}

const COLUMNS: &str =
    "id, user, kind, key, day, created_at, title, body, stats, art, shared_friends, link_token";

fn row(row: &Row) -> rusqlite::Result<Win> {
    let stats: String = row.get(8)?;
    Ok(Win {
        id: row.get(0)?,
        user: row.get(1)?,
        kind: row.get(2)?,
        key: row.get(3)?,
        day: row.get(4)?,
        created_at: row.get(5)?,
        title: row.get(6)?,
        body: row.get(7)?,
        stats: serde_json::from_str(&stats).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, error.into())
        })?,
        art: row.get(9)?,
        shared_friends: row.get(10)?,
        link_token: row.get(11)?,
    })
}

/// `user`'s win `id`.
pub(crate) fn win(conn: &Connection, user: &str, id: i64) -> rusqlite::Result<Option<Win>> {
    conn.query_row(
        &format!("select {COLUMNS} from wins where user = ?1 and id = ?2"),
        rusqlite::params![user, id],
        row,
    )
    .optional()
}

/// The win whose public link is `token`.
pub(crate) fn by_token(conn: &Connection, token: &str) -> rusqlite::Result<Option<Win>> {
    conn.query_row(
        &format!("select {COLUMNS} from wins where link_token = ?1"),
        [token],
        row,
    )
    .optional()
}
