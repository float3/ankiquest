//! Private, time-limited Anki deck packages shared with selected players.

use crate::access::authorized;
use crate::app::{App, now_ms};
use crate::game::DAY_MS;
use crate::store::Error;
use crate::web_util::store_error;
use crate::{deck_copies, decks};
use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, Response, StatusCode, header};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

pub const MAX_BYTES: usize = 25 * 1024 * 1024;
pub const MAX_RECIPIENTS: usize = 20;
const MAX_ACTIVE: i64 = 10;
const RETENTION_MS: i64 = 30 * DAY_MS;

#[derive(Debug, Serialize)]
pub struct Offer {
    pub id: i64,
    pub sender: String,
    pub deck: String,
    pub bytes: i64,
    pub created_at: i64,
}

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists deck_copy_offers (
            id integer primary key autoincrement,
            sender text not null,
            deck text not null,
            package blob not null,
            created_at integer not null
        );
        create table if not exists deck_copy_recipients (
            offer integer not null,
            recipient text not null,
            primary key (offer, recipient)
        ) without rowid;
        create index if not exists deck_copy_recipient_lookup
            on deck_copy_recipients (recipient, offer);",
    )?;
    Ok(())
}

pub fn valid_request(sender: &str, deck: &str, recipients: &[String], package: &[u8]) -> bool {
    let unique: HashSet<_> = recipients.iter().collect();
    !deck.trim().is_empty()
        && deck.len() <= 160
        && !deck.chars().any(char::is_control)
        && !recipients.is_empty()
        && recipients.len() <= MAX_RECIPIENTS
        && unique.len() == recipients.len()
        && recipients.iter().all(|recipient| recipient != sender)
        && package.len() <= MAX_BYTES
        && package.starts_with(b"PK\x03\x04")
}

fn prune(conn: &Connection, now_ms: i64) -> Result<(), Error> {
    conn.execute(
        "delete from deck_copy_recipients where offer in
            (select id from deck_copy_offers where created_at < ?1)",
        [now_ms - RETENTION_MS],
    )?;
    conn.execute(
        "delete from deck_copy_offers where created_at < ?1 or
            not exists (select 1 from deck_copy_recipients r where r.offer = deck_copy_offers.id)",
        [now_ms - RETENTION_MS],
    )?;
    Ok(())
}

pub fn create(
    conn: &mut Connection,
    sender: &str,
    deck: &str,
    recipients: &[String],
    package: &[u8],
    now_ms: i64,
) -> Result<Option<i64>, Error> {
    if !valid_request(sender, deck, recipients, package) {
        return Ok(None);
    }
    let tx = conn.transaction()?;
    prune(&tx, now_ms)?;
    let active: i64 = tx.query_row(
        "select count(*) from deck_copy_offers where sender = ?1",
        [sender],
        |row| row.get(0),
    )?;
    if active >= MAX_ACTIVE {
        return Ok(None);
    }
    tx.execute(
        "insert into deck_copy_offers (sender, deck, package, created_at) values (?1, ?2, ?3, ?4)",
        params![sender, deck, package, now_ms],
    )?;
    let id = tx.last_insert_rowid();
    for recipient in recipients {
        tx.execute(
            "insert into deck_copy_recipients (offer, recipient) values (?1, ?2)",
            params![id, recipient],
        )?;
    }
    tx.commit()?;
    Ok(Some(id))
}

pub fn inbox(conn: &Connection, recipient: &str, now_ms: i64) -> Result<Vec<Offer>, Error> {
    prune(conn, now_ms)?;
    let mut query = conn.prepare(
        "select o.id, o.sender, o.deck, length(o.package), o.created_at
         from deck_copy_offers o join deck_copy_recipients r on r.offer = o.id
         where r.recipient = ?1 and o.created_at >= ?2 order by o.id desc",
    )?;
    Ok(query
        .query_map(params![recipient, now_ms - RETENTION_MS], |row| {
            Ok(Offer {
                id: row.get(0)?,
                sender: row.get(1)?,
                deck: row.get(2)?,
                bytes: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

pub fn package(
    conn: &Connection,
    recipient: &str,
    id: i64,
    now_ms: i64,
) -> Result<Option<Vec<u8>>, Error> {
    Ok(conn
        .query_row(
            "select o.package from deck_copy_offers o join deck_copy_recipients r on r.offer = o.id
             where o.id = ?1 and r.recipient = ?2 and o.created_at >= ?3",
            params![id, recipient, now_ms - RETENTION_MS],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn dismiss(conn: &mut Connection, recipient: &str, id: i64) -> Result<bool, Error> {
    let tx = conn.transaction()?;
    let removed = tx.execute(
        "delete from deck_copy_recipients where offer = ?1 and recipient = ?2",
        params![id, recipient],
    )? > 0;
    tx.execute(
        "delete from deck_copy_offers where id = ?1 and not exists
            (select 1 from deck_copy_recipients where offer = ?1)",
        [id],
    )?;
    tx.commit()?;
    Ok(removed)
}

#[derive(Deserialize)]
struct CopyRequest {
    deck: String,
    recipients: String,
}

#[derive(Serialize)]
struct CopyInbox {
    offers: Vec<deck_copies::Offer>,
    friends: Vec<decks::Recipient>,
    max_bytes: usize,
}

async fn deck_copy_inbox(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Json<CopyInbox>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let (offers, reachable) = {
        let store = app.store.lock().unwrap();
        (
            deck_copies::inbox(&store.conn, &user, now_ms()).map_err(store_error)?,
            app.reachable(&store, &user).map_err(store_error)?,
        )
    };
    let friends = app.sorted_recipients(reachable);
    Ok(Json(CopyInbox {
        offers,
        friends,
        max_bytes: deck_copies::MAX_BYTES,
    }))
}

async fn offer_deck_copy(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Query(request): Query<CopyRequest>,
    package: Bytes,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let recipients: Vec<String> =
        serde_json::from_str(&request.recipients).map_err(|_| StatusCode::BAD_REQUEST)?;
    if package.len() > deck_copies::MAX_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let reachable = app
        .reachable(&app.store.lock().unwrap(), &user)
        .map_err(store_error)?;
    if !deck_copies::valid_request(&user, &request.deck, &recipients, &package)
        || recipients.iter().any(|id| !reachable.contains(id))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let id = deck_copies::create(
        &mut app.store.lock().unwrap().conn,
        &user,
        &request.deck,
        &recipients,
        &package,
        now_ms(),
    )
    .map_err(store_error)?
    .ok_or(StatusCode::TOO_MANY_REQUESTS)?;
    Ok(Json(serde_json::json!({"id":id})))
}

async fn download_deck_copy(
    State(app): State<Arc<App>>,
    UrlPath((user, id)): UrlPath<(String, i64)>,
    headers: HeaderMap,
) -> Result<Response<Body>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let package = deck_copies::package(&app.store.lock().unwrap().conn, &user, id, now_ms())
        .map_err(store_error)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=ankiquest-shared-deck.apkg",
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(package))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn dismiss_deck_copy(
    State(app): State<Arc<App>>,
    UrlPath((user, id)): UrlPath<(String, i64)>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if deck_copies::dismiss(&mut app.store.lock().unwrap().conn, &user, id).map_err(store_error)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route(
            "/api/deck-copies/{user}",
            get(deck_copy_inbox)
                .post(offer_deck_copy)
                .layer(axum::extract::DefaultBodyLimit::max(deck_copies::MAX_BYTES)),
        )
        .route(
            "/api/deck-copies/{user}/{id}",
            get(download_deck_copy).delete(dismiss_deck_copy),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck_copies;
    use crate::test_support::*;

    #[test]
    fn copies_reach_only_selected_friends_and_expire() {
        let mut db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        let package = b"PK\x03\x04fake-for-store-test";
        let id = create(
            &mut db,
            "cerro",
            "Spanish",
            &["hill".into(), "friend".into()],
            package,
            1_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(inbox(&db, "hill", 1_000).unwrap()[0].deck, "Spanish");
        assert!(inbox(&db, "stranger", 1_000).unwrap().is_empty());
        assert_eq!(
            self::package(&db, "hill", id, 1_000).unwrap().unwrap(),
            package
        );
        assert!(self::package(&db, "stranger", id, 1_000).unwrap().is_none());
        assert!(!dismiss(&mut db, "stranger", id).unwrap());
        assert!(dismiss(&mut db, "hill", id).unwrap());
        assert!(self::package(&db, "hill", id, 1_000).unwrap().is_none());
        assert!(
            self::package(&db, "friend", id, 1_000 + RETENTION_MS + 1)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_non_packages_and_duplicate_or_self_recipients() {
        assert!(!valid_request(
            "cerro",
            "Spanish",
            &["hill".into()],
            b"not an apkg"
        ));
        assert!(!valid_request(
            "cerro",
            "Spanish",
            &["hill".into(), "hill".into()],
            b"PK\x03\x04"
        ));
        assert!(!valid_request(
            "cerro",
            "Spanish",
            &["cerro".into()],
            b"PK\x03\x04"
        ));
        assert!(!valid_request(
            "cerro",
            "Bad\nname",
            &["hill".into()],
            b"PK\x03\x04"
        ));
    }

    #[tokio::test]
    async fn deck_copies_are_private_to_selected_players() {
        let (app, path) = fixture();
        let request = || CopyRequest {
            deck: "Spanish".into(),
            recipients: r#"["hill"]"#.into(),
        };
        let package = Bytes::from_static(b"PK\x03\x04example-package");
        assert_eq!(
            offer_deck_copy(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("hill"),
                Query(request()),
                package.clone()
            )
            .await
            .unwrap_err(),
            StatusCode::UNAUTHORIZED,
        );
        let id = offer_deck_copy(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Query(request()),
            package.clone(),
        )
        .await
        .unwrap()
        .0["id"]
            .as_i64()
            .unwrap();
        let hill = deck_copy_inbox(State(app.clone()), UrlPath("hill".into()), headers("hill"))
            .await
            .unwrap()
            .0;
        assert_eq!(hill.offers.len(), 1);
        assert_eq!(hill.offers[0].deck, "Spanish");
        assert!(
            deck_copy_inbox(
                State(app.clone()),
                UrlPath("friend".into()),
                headers("friend")
            )
            .await
            .unwrap()
            .0
            .offers
            .is_empty()
        );
        assert_eq!(
            download_deck_copy(
                State(app.clone()),
                UrlPath(("friend".into(), id)),
                headers("friend")
            )
            .await
            .unwrap_err(),
            StatusCode::NOT_FOUND,
        );
        let download = download_deck_copy(
            State(app.clone()),
            UrlPath(("hill".into(), id)),
            headers("hill"),
        )
        .await
        .unwrap();
        assert_eq!(
            axum::body::to_bytes(download.into_body(), deck_copies::MAX_BYTES)
                .await
                .unwrap(),
            package
        );
        assert_eq!(
            dismiss_deck_copy(
                State(app.clone()),
                UrlPath(("hill".into(), id)),
                headers("friend")
            )
            .await
            .unwrap_err(),
            StatusCode::UNAUTHORIZED,
        );
        assert_eq!(
            dismiss_deck_copy(
                State(app.clone()),
                UrlPath(("hill".into(), id)),
                headers("hill")
            )
            .await
            .unwrap(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            download_deck_copy(
                State(app.clone()),
                UrlPath(("hill".into(), id)),
                headers("hill")
            )
            .await
            .unwrap_err(),
            StatusCode::NOT_FOUND
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
