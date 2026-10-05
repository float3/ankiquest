//! What a player can do about their data: download all of it, or delete their account and everything with it.
//! Every table is listed in `OWNED` or `SHARED`, and a test fails when a new one is not, so nothing new escapes deletion or export.

use crate::accounts::{password_matches, refuse, reply};
use crate::{App, access, now_ms};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::{Json, Router, routing::get};
use rusqlite::Connection;
use rusqlite::types::ValueRef;
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;

/// Who runs this server, for the privacy notice.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    pub operator: Option<String>,
    pub contact: Option<String>,
    pub hosting: Option<String>,
    pub imprint: Option<String>,
}

/// What a download contains: (section, query taking the player as ?1).
const EXPORT: &[(&str, &str)] = &[
    ("account", "select user, display, created_at from accounts where user = ?1"),
    ("devices", "select device, created_at from account_tokens where user = ?1"),
    ("visibility", "select public from visibility where user = ?1"),
    ("reviews", "select id, cid, last_ivl, time_ms, kind from reviews where user = ?1 order by id"),
    ("anki_day", "select offset_west_min, rollover_hour from clocks where user = ?1"),
    ("syncs", "select received_at from review_syncs where user = ?1"),
    ("language", "select language from languages where user = ?1"),
    ("companion", "select companion from companion_preferences where user = ?1"),
    ("reminders", "select settings from reminder_settings where user = ?1"),
    ("streak_freezes", "select at, enabled from freeze_preferences where user = ?1 order by at"),
    ("encouragement", "select * from player_settings where user = ?1"),
    ("deck_alerts", "select enabled from incoming_settings where user = ?1"),
    ("muted_deck_senders", "select sender from incoming_muted_senders where recipient = ?1"),
    ("muted_nudge_senders", "select sender from friend_nudge_muted_senders where recipient = ?1"),
    ("unsubscribed_senders", "select sender from sender_unsubscriptions where recipient = ?1"),
    ("unsubscribed_decks", "select sender, deck_id from unsubscribed_decks where recipient = ?1"),
    ("decks", "select id, name, enabled from decks where owner = ?1"),
    ("deck_recipients", "select deck_id, recipient from deck_recipients where owner = ?1"),
    ("deck_completions", "select deck_id, day from deck_completions where owner = ?1 order by day"),
    ("notifications_received", "select * from notifications where recipient = ?1 order by id"),
    ("messages_sent", "select * from notifications where sender = ?1 order by id"),
    ("deck_copies_sent", "select o.id, o.deck, o.created_at, length(o.package) as bytes, group_concat(r.recipient) as recipients
         from deck_copy_offers o left join deck_copy_recipients r on r.offer = o.id where o.sender = ?1 group by o.id"),
    ("deck_copies_received", "select o.id, o.sender, o.deck, o.created_at from deck_copy_offers o
         join deck_copy_recipients r on r.offer = o.id where r.recipient = ?1"),
    ("challenges_created", "select * from community_challenges where creator = ?1"),
    ("challenge_memberships", "select * from community_members where user = ?1"),
    ("weekly_suggestions", "select * from community_weekly_suggestions where owner = ?1"),
    ("groups", "select g.id, g.name, m.owner, m.joined_at from group_members m join groups g on g.id = m.group_id where m.user = ?1"),
    ("friendships", "select case when a = ?1 then b else a end as friend, requested_by, accepted, created_at
         from friendships where a = ?1 or b = ?1"),
    ("avatar", "select revision, image from avatars where user = ?1"),
    ("server_log", "select at, level, message from server_log where user = ?1 order by at"),
];

/// Rows that belong to the player alone: (table, condition on ?1).
const OWNED: &[(&str, &str)] = &[
    ("account_tokens", "user = ?1"),
    ("visibility", "user = ?1"),
    ("reviews", "user = ?1"),
    ("clocks", "user = ?1"),
    ("review_syncs", "user = ?1"),
    ("seen", "user = ?1 or key like 'friend-nudge:%:' || ?1"),
    ("languages", "user = ?1"),
    ("companion_preferences", "user = ?1"),
    ("reminder_settings", "user = ?1"),
    ("reminder_baselines", "user = ?1"),
    ("freeze_preferences", "user = ?1"),
    ("freeze_legacy_players", "user = ?1"),
    ("player_settings", "user = ?1"),
    ("incoming_settings", "user = ?1"),
    ("incoming_muted_senders", "recipient = ?1 or sender = ?1"),
    (
        "friend_nudge_muted_senders",
        "recipient = ?1 or sender = ?1",
    ),
    ("sender_unsubscriptions", "recipient = ?1 or sender = ?1"),
    ("unsubscribed_decks", "recipient = ?1 or sender = ?1"),
    ("subscription_baselines", "recipient = ?1 or sender = ?1"),
    ("decks", "owner = ?1"),
    ("deck_recipients", "owner = ?1 or recipient = ?1"),
    ("deck_completions", "owner = ?1"),
    (
        "deck_copy_recipients",
        "recipient = ?1 or offer in (select id from deck_copy_offers where sender = ?1)",
    ),
    ("deck_copy_offers", "sender = ?1"),
    ("notifications", "recipient = ?1 or sender = ?1"),
    ("community_members", "user = ?1"),
    ("community_requests", "creator = ?1"),
    ("community_weekly_suggestions", "owner = ?1 or friend = ?1"),
    ("friendships", "a = ?1 or b = ?1"),
    ("avatars", "user = ?1"),
    ("server_log", "user = ?1"),
    ("accounts", "user = ?1"),
];

/// Tables without per-player rows, or handled separately in `forget`.
#[cfg(test)]
const SHARED: &[&str] = &[
    "server_settings",
    "competition_meta",
    "competition_periods",
    "community_refresh",
    "freeze_policy",
    "freeze_defaults",
    "community_challenges",
    "community_deferred_goals",
    "community_weekly_goals",
    "groups",
    "group_members",
];

fn value(value: ValueRef) -> serde_json::Value {
    match value {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(n) => n.into(),
        ValueRef::Real(n) => n.into(),
        ValueRef::Text(text) => String::from_utf8_lossy(text).into(),
        ValueRef::Blob(bytes) => base64(bytes).into(),
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

/// Everything stored about `user`, as one JSON document.
pub fn export(conn: &Connection, user: &str) -> rusqlite::Result<serde_json::Value> {
    let mut document = serde_json::Map::new();
    document.insert("user".into(), user.into());
    document.insert("exported_at".into(), now_ms().into());
    for (section, query) in EXPORT {
        let mut statement = conn.prepare(query)?;
        let names: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(String::from)
            .collect();
        let rows = statement
            .query_map([user], |row| {
                let mut object = serde_json::Map::new();
                for (index, name) in names.iter().enumerate() {
                    object.insert(name.clone(), value(row.get_ref(index)?));
                }
                Ok(serde_json::Value::Object(object))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        document.insert((*section).into(), rows.into());
    }
    Ok(document.into())
}

/// Deletes the player's rows everywhere. Groups pass to their next member, and goals the player created end for everyone.
pub fn forget(conn: &Connection, user: &str) -> rusqlite::Result<()> {
    let transaction = conn.unchecked_transaction()?;
    let groups: Vec<i64> = transaction
        .prepare("select group_id from group_members where user = ?1")?
        .query_map([user], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for group in groups {
        crate::social::leave(&transaction, group, user)?;
    }
    let created = "select id from community_challenges where creator = ?1";
    for table in [
        "community_members",
        "community_deferred_goals",
        "community_weekly_goals",
    ] {
        transaction.execute(
            &format!("delete from {table} where challenge in ({created})"),
            [user],
        )?;
    }
    transaction.execute(
        &format!("delete from notifications where challenge_id in ({created})"),
        [user],
    )?;
    transaction.execute(&format!("update community_weekly_suggestions set challenge = null where challenge in ({created})"), [user])?;
    transaction.execute(
        "delete from community_challenges where creator = ?1",
        [user],
    )?;
    for (table, condition) in OWNED {
        transaction.execute(&format!("delete from {table} where {condition}"), [user])?;
    }
    transaction.commit()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deletion {
    password: String,
}

async fn download(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !access::authorized(&app, &user, &headers) {
        return refuse(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    let document = export(&app.store.lock().unwrap().conn, &user);
    match document {
        Ok(document) => (
            [
                (header::CACHE_CONTROL, "no-store".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"ankiquest-{user}.json\""),
                ),
            ],
            Json(document),
        )
            .into_response(),
        Err(error) => {
            log_error!("data export failed: {error}");
            refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                "The download could not be prepared. Please try again.",
            )
        }
    }
}

async fn account(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !access::authorized(&app, &user, &headers) {
        return refuse(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    let created: rusqlite::Result<i64> = app.store.lock().unwrap().conn.query_row(
        "select created_at from accounts where user = ?1",
        [&user],
        |r| r.get(0),
    );
    match created {
        Ok(created_at) => reply(
            StatusCode::OK,
            serde_json::json!({"user": user, "display": app.display(&user), "created_at": created_at}),
        ),
        Err(_) => refuse(
            StatusCode::NOT_FOUND,
            "This player is not a self-service account.",
        ),
    }
}

/// Deletes the account. The password is asked for again, so a borrowed device cannot do it.
async fn delete(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(user): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !access::authorized(&app, &user, &headers) {
        return refuse(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    if !app.accounts.contains(&user) {
        return refuse(
            StatusCode::CONFLICT,
            "This player comes from the server configuration. Ask whoever runs the server to remove it.",
        );
    }
    let Ok(request) = serde_json::from_slice::<Deletion>(&body) else {
        return refuse(
            StatusCode::BAD_REQUEST,
            "Enter your password to delete your account.",
        );
    };
    let now = now_ms();
    let client = access::client_ip(&app.config, &headers, peer);
    if app.access.throttled(client, now) {
        return refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many attempts. Try again in one minute.",
        );
    }
    if !password_matches(&app, &user, &request.password).await {
        app.access.failure(client, now);
        return refuse(StatusCode::FORBIDDEN, "That password is not right.");
    }
    if let Err(error) = forget(&app.store.lock().unwrap().conn, &user) {
        log_error!("account deletion failed: {error}");
        return refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Your account could not be deleted. Please try again.",
        );
    }
    app.accounts.remove(&user);
    app.players.write().unwrap().remove(&user);
    app.forget_standing(&user);
    log_info!("an account was deleted");
    StatusCode::NO_CONTENT.into_response()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The privacy notice, filled in with who runs this server.
pub fn notice_page(notice: &Notice, push: bool, log_days: u32) -> String {
    let operator = notice.operator.as_deref().map_or_else(
        || "the person who runs this AnkiQuest server".to_string(),
        escape,
    );
    let contact = notice
        .contact
        .as_deref()
        .map_or_else(String::new, |contact| {
            let contact = escape(contact);
            format!(" Contact: <a href=\"mailto:{contact}\">{contact}</a>.")
        });
    let imprint = notice.imprint.as_deref().map_or_else(String::new, |url| {
        format!(" <a href=\"{}\">Legal notice</a>.", escape(url))
    });
    let hosting = notice.hosting.as_deref().map_or_else(
        || "The server is run by the operator above.".to_string(),
        |hosting| format!("The server runs at {}, which processes the data only on the operator's behalf (Art. 28 GDPR).", escape(hosting)),
    );
    let push = if push {
        "If the operator has set up phone push for you, the title and text of each notification pass through the ntfy push service on their way to your phone."
    } else {
        "The add-on and the app fetch notifications from this server; no push service is involved."
    };
    let logs = if log_days == 0 {
        "until the operator deletes them".to_string()
    } else {
        format!("for {log_days} days")
    };
    web!("privacy.html")
        .replace("{{operator}}", &operator)
        .replace("{{contact}}", &contact)
        .replace("{{imprint}}", &imprint)
        .replace("{{hosting}}", &hosting)
        .replace("{{push}}", push)
        .replace("{{logs}}", &logs)
}

async fn privacy_page(State(app): State<Arc<App>>) -> Html<String> {
    Html(notice_page(
        &app.config.privacy,
        app.config
            .ntfy
            .as_deref()
            .is_some_and(|ntfy| !ntfy.trim().is_empty()),
        app.config.log_retention_days,
    ))
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/privacy", get(privacy_page))
        .route("/api/export/{user}", get(download))
        .route(
            "/api/accounts/{user}",
            get(account)
                .delete(delete)
                .layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::Request;
    use std::collections::{BTreeSet, HashMap};
    use std::path::PathBuf;
    use std::sync::{Mutex, RwLock};
    use tower::ServiceExt;

    fn fixture() -> (Arc<App>, PathBuf) {
        let (store, path) = crate::decks::tests::temporary_store();
        let config = serde_json::from_value(serde_json::json!({
            "registration": true,
            "users": {"alice": {"token": "alice-token"}},
            "privacy": {"operator": "Ada <Operator>", "contact": "privacy@example.org"}
        }))
        .unwrap();
        let accounts = crate::accounts::Directory::load(&store.conn).unwrap();
        (
            Arc::new(App {
                config,
                access: crate::access::Access::default(),
                accounts,
                week: crate::game::Week::default(),
                store: Mutex::new(store),
                players: RwLock::new(HashMap::new()),
                standings: Default::default(),
            }),
            path,
        )
    }

    async fn call(
        app: &Arc<App>,
        method: &str,
        path: &str,
        token: &str,
        body: serde_json::Value,
    ) -> (StatusCode, Vec<u8>) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(if body.is_null() {
                String::new()
            } else {
                body.to_string()
            }))
            .unwrap();
        let response = crate::router(app.clone())
            .layer(MockConnectInfo(
                "192.0.2.9:40000".parse::<SocketAddr>().unwrap(),
            ))
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        (
            status,
            to_bytes(response.into_body(), 1 << 22)
                .await
                .unwrap()
                .to_vec(),
        )
    }

    async fn sign_up(app: &Arc<App>, user: &str) -> String {
        let (_, body) = call(
            app,
            "POST",
            "/api/accounts",
            "",
            serde_json::json!({"user": user, "password": "long enough pw"}),
        )
        .await;
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn count(app: &App, sql: &str) -> i64 {
        app.store
            .lock()
            .unwrap()
            .conn
            .query_row(sql, [], |r| r.get(0))
            .unwrap()
    }

    fn seed(app: &App, user: &str, other: &str) {
        let conn = &app.store.lock().unwrap().conn;
        conn.execute_batch(&format!(
            "insert into reviews (user, id, cid, last_ivl, time_ms, kind) values ('{user}', 1, 10, 1, 5000, 1), ('{other}', 2, 20, 1, 5000, 1);
             insert into clocks (user, offset_west_min, rollover_hour) values ('{user}', 0, 4);
             insert into languages (user, language) values ('{user}', 'es');
             insert into decks (owner, id, name, enabled) values ('{user}', 'd1', 'Spanish', 1), ('{other}', 'd2', 'French', 1);
             insert into deck_recipients (owner, deck_id, recipient) values ('{user}', 'd1', '{other}'), ('{other}', 'd2', '{user}');
             insert into notifications (recipient, sender, title, body, day, created_at, kind) values ('{other}', '{user}', 'hi', 'from {user}', 1, 1, 'reply'), ('{user}', '{other}', 'hey', 'to {user}', 1, 1, 'reply'), ('{other}', '', 'kept', 'kept', 1, 1, 'nudge');
             insert into avatars (user, revision, image) values ('{user}', 1, x'89504e47');
             insert into community_challenges (id, creator, title, kind, cooperative, target, start_at, end_at) values (5, '{user}', 'Mine', 'reviews', 1, 10, 0, 1), (6, '{other}', 'Theirs', 'reviews', 1, 10, 0, 1);
             insert into community_members (challenge, user, status) values (5, '{other}', 'active'), (6, '{user}', 'active'), (6, '{other}', 'active');"
        ))
        .unwrap();
    }

    #[test]
    fn every_table_is_either_owned_or_shared() {
        let (app, path) = fixture();
        let tables: BTreeSet<String> = app
            .store
            .lock()
            .unwrap()
            .conn
            .prepare(
                "select name from sqlite_master where type = 'table' and name not like 'sqlite_%'",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let known: BTreeSet<String> = OWNED
            .iter()
            .map(|(table, _)| table.to_string())
            .chain(SHARED.iter().map(|t| t.to_string()))
            .collect();
        assert_eq!(
            tables, known,
            "decide how deletion and export treat every new table"
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn the_download_holds_the_players_data_and_nobody_elses() {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        seed(&app, "ana", "alice");
        let (status, body) = call(
            &app,
            "GET",
            "/api/export/ana",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let document: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(document["account"][0]["user"], "ana");
        assert_eq!(document["reviews"].as_array().unwrap().len(), 1);
        assert_eq!(document["messages_sent"][0]["body"], "from ana");
        assert_eq!(document["notifications_received"][0]["body"], "to ana");
        assert_eq!(document["avatar"][0]["image"], "iVBORw==");
        let text = String::from_utf8(body).unwrap();
        assert!(
            !text.contains("argon2") && !text.contains("token_hash"),
            "no secrets in the download"
        );
        assert!(
            !text.contains("French"),
            "nothing of other players' own data"
        );
        let (status, _) = call(
            &app,
            "GET",
            "/api/export/ana",
            "alice-token",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn deleting_an_account_removes_it_everywhere_and_keeps_everyone_else() {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        let bo = sign_up(&app, "bobby").await;
        seed(&app, "ana", "alice");
        call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "create_group", "name": "Club"}),
        )
        .await;
        let invite: String = app
            .store
            .lock()
            .unwrap()
            .conn
            .query_row("select invite from groups", [], |r| r.get(0))
            .unwrap();
        call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "join", "invite": invite}),
        )
        .await;
        call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "befriend", "user": "bobby"}),
        )
        .await;

        let (status, _) = call(
            &app,
            "DELETE",
            "/api/accounts/ana",
            &ana,
            serde_json::json!({"password": "wrong password"}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = call(
            &app,
            "DELETE",
            "/api/accounts/ana",
            &bo,
            serde_json::json!({"password": "long enough pw"}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = call(
            &app,
            "DELETE",
            "/api/accounts/alice",
            "alice-token",
            serde_json::json!({"password": "x"}),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "configured players are removed by the operator"
        );

        let (status, _) = call(
            &app,
            "DELETE",
            "/api/accounts/ana",
            &ana,
            serde_json::json!({"password": "long enough pw"}),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let document = export(&app.store.lock().unwrap().conn, "ana").unwrap();
        for (section, rows) in document.as_object().unwrap() {
            if let Some(rows) = rows.as_array() {
                assert!(rows.is_empty(), "{section} still holds {rows:?}");
            }
        }
        assert_eq!(
            count(&app, "select count(*) from reviews where user = 'alice'"),
            1
        );
        assert_eq!(
            count(&app, "select count(*) from decks where owner = 'alice'"),
            1
        );
        assert_eq!(count(&app, "select count(*) from deck_recipients"), 0);
        assert_eq!(
            count(&app, "select count(*) from notifications"),
            1,
            "only the message that did not involve ana is left"
        );
        assert_eq!(
            count(&app, "select count(*) from community_challenges"),
            1,
            "goals ana created end for everyone"
        );
        assert_eq!(count(&app, "select count(*) from community_members"), 1);
        assert_eq!(
            count(
                &app,
                "select count(*) from group_members where user = 'bobby' and owner = 1"
            ),
            1,
            "the group passes on"
        );
        assert_eq!(count(&app, "select count(*) from friendships"), 0);
        assert!(!app.accounts.contains("ana"));
        let (status, _) = call(
            &app,
            "GET",
            "/api/social/ana",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "the old token stops working"
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn the_privacy_notice_names_the_operator_and_is_public() {
        let (app, path) = fixture();
        let (status, body) = call(&app, "GET", "/privacy", "", serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        let page = String::from_utf8(body).unwrap();
        assert!(page.contains("Ada &lt;Operator&gt;"));
        assert!(page.contains("mailto:privacy@example.org"));
        assert!(!page.contains("{{"), "every placeholder is filled");
        let anonymous = notice_page(&Notice::default(), false, 0);
        assert!(
            anonymous.contains("the person who runs this AnkiQuest server")
                && !anonymous.contains("{{")
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn base64_matches_the_standard_encoding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
