//! A player's visual study companion, shared by the website and installed clients.
use crate::access::require_owner;
use crate::{
    App,
    store::{Error, Store},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const DEFAULT: &str = "aki";

pub fn valid(value: &str) -> bool {
    matches!(value, "aki" | "ankilope" | "none")
}

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists companion_preferences (
            user text primary key,
            companion text not null check (companion in ('aki', 'ankilope', 'none'))
        ) without rowid;",
    )?;
    Ok(())
}

impl Store {
    pub fn companion(&self, user: &str) -> Result<String, Error> {
        Ok(self
            .conn
            .query_row(
                "select companion from companion_preferences where user = ?1",
                [user],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or_else(|| DEFAULT.into()))
    }

    pub fn set_companion(&self, user: &str, companion: &str) -> Result<(), Error> {
        self.conn.execute(
            "insert into companion_preferences (user, companion) values (?1, ?2)
             on conflict (user) do update set companion = excluded.companion",
            params![user, companion],
        )?;
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Preference {
    pub companion: String,
}

pub async fn get(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Preference>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let companion = app
        .store
        .lock()
        .unwrap()
        .companion(&user)
        .map_err(crate::store_error)?;
    Ok(Json(Preference { companion }))
}

pub async fn set(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(preference): Json<Preference>,
) -> Result<Json<Preference>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    if !valid(&preference.companion) {
        return Err(StatusCode::BAD_REQUEST);
    }
    app.store
        .lock()
        .unwrap()
        .set_companion(&user, &preference.companion)
        .map_err(crate::store_error)?;
    Ok(Json(preference))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_is_per_player_and_survives_reopening_the_store() {
        let (store, path) = crate::test_support::temporary_store();
        assert_eq!(store.companion("alice").unwrap(), DEFAULT);
        store.set_companion("alice", "ankilope").unwrap();
        store.set_companion("bob", "none").unwrap();
        drop(store);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.companion("alice").unwrap(), "ankilope");
        assert_eq!(reopened.companion("bob").unwrap(), "none");
        assert_eq!(reopened.companion("carol").unwrap(), DEFAULT);
    }
}
