//! Self-service accounts: a username and password, and a token per device.
//! Configured users keep working; accounts only exist when registration is on.

use crate::{App, Error, now_ms};
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};

pub const MIN_PASSWORD: usize = 10;
pub const MAX_PASSWORD: usize = 256;
const MAX_DISPLAY: usize = 40;
const MAX_DEVICE: usize = 60;
const SIGNUPS_PER_HOUR: usize = 5;
const HOUR_MS: i64 = 3_600_000;
const RESERVED: &[&str] = &[
    "admin",
    "administrator",
    "aki",
    "ankilope",
    "ankiquest",
    "api",
    "auth",
    "help",
    "login",
    "me",
    "moderator",
    "null",
    "root",
    "signup",
    "static",
    "support",
    "system",
    "undefined",
];

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists accounts (
             user text primary key,
             display text not null,
             password_hash text not null,
             created_at integer not null
         ) without rowid;
         create table if not exists account_tokens (
             token_hash text primary key,
             user text not null references accounts (user) on delete cascade,
             device text not null,
             created_at integer not null
         ) without rowid;
         create index if not exists account_tokens_user on account_tokens (user);",
    )?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub display: String,
}

/// What every request needs without touching the store lock: who exists, and whose token is whose.
#[derive(Default)]
pub struct Directory {
    accounts: RwLock<HashMap<String, Account>>,
    tokens: RwLock<HashMap<String, String>>,
    signups: Mutex<HashMap<IpAddr, Vec<i64>>>,
}

impl Directory {
    pub fn load(conn: &Connection) -> Result<Self, Error> {
        let accounts = conn
            .prepare("select user, display from accounts")?
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, Account { display: r.get(1)? }))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?;
        let tokens = conn
            .prepare("select token_hash, user from account_tokens")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(Self {
            accounts: RwLock::new(accounts),
            tokens: RwLock::new(tokens),
            signups: Mutex::default(),
        })
    }

    pub fn account(&self, user: &str) -> Option<Account> {
        self.accounts.read().unwrap().get(user).cloned()
    }

    pub fn contains(&self, user: &str) -> bool {
        self.accounts.read().unwrap().contains_key(user)
    }

    pub fn users(&self) -> Vec<String> {
        self.accounts.read().unwrap().keys().cloned().collect()
    }

    pub fn token_owner(&self, token: &str) -> Option<String> {
        if token.is_empty() {
            return None;
        }
        self.tokens.read().unwrap().get(&token_hash(token)).cloned()
    }

    fn signup_allowed(&self, client: IpAddr, now: i64) -> bool {
        let mut signups = self.signups.lock().unwrap();
        signups.retain(|_, times| {
            times.retain(|at| now - at < HOUR_MS);
            !times.is_empty()
        });
        let times = signups.entry(client).or_default();
        if times.len() >= SIGNUPS_PER_HOUR {
            return false;
        }
        times.push(now);
        true
    }
}

pub fn valid_username(user: &str) -> bool {
    let mut chars = user.chars();
    (3..=32).contains(&user.len())
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && !RESERVED.contains(&user)
}

fn clean_display(display: Option<&str>, user: &str) -> Option<String> {
    let display = display
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .unwrap_or(user);
    (display.chars().count() <= MAX_DISPLAY && !display.chars().any(char::is_control))
        .then(|| display.to_string())
}

fn clean_device(device: Option<&str>) -> String {
    let device = device
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .unwrap_or("device");
    device
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DEVICE)
        .collect()
}

pub fn hash_password(password: &str) -> Result<String, Error> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|_| "password salt unavailable")?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| e.to_string())?;
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| e.to_string())?
        .to_string())
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

pub fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn new_token() -> Result<String, Error> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| "token randomness unavailable")?;
    Ok(random.iter().map(|b| format!("{b:02x}")).collect())
}

fn create(
    conn: &Connection,
    user: &str,
    display: &str,
    hash: &str,
    now: i64,
) -> Result<bool, Error> {
    Ok(conn.execute(
        "insert or ignore into accounts (user, display, password_hash, created_at) values (?1, ?2, ?3, ?4)",
        params![user, display, hash, now],
    )? == 1)
}

fn stored_hash(conn: &Connection, user: &str) -> Result<Option<String>, Error> {
    Ok(conn
        .query_row(
            "select password_hash from accounts where user = ?1",
            [user],
            |r| r.get(0),
        )
        .optional()?)
}

impl App {
    /// A new device token for an existing account, saved before it is handed out.
    fn issue_token(&self, user: &str, device: &str) -> Result<String, Error> {
        let token = new_token()?;
        let hash = token_hash(&token);
        self.store.lock().unwrap().conn.execute(
            "insert into account_tokens (token_hash, user, device, created_at) values (?1, ?2, ?3, ?4)",
            params![hash, user, device, now_ms()],
        )?;
        self.accounts
            .tokens
            .write()
            .unwrap()
            .insert(hash, user.to_string());
        Ok(token)
    }
}

fn reply(status: StatusCode, body: serde_json::Value) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

fn refuse(status: StatusCode, message: &str) -> Response {
    reply(status, serde_json::json!({"error": message}))
}

fn internal(error: Error) -> Response {
    eprintln!("accounts: {error}");
    refuse(
        StatusCode::INTERNAL_SERVER_ERROR,
        "The account could not be saved. Please try again.",
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Signup {
    user: String,
    password: String,
    #[serde(default)]
    display: Option<String>,
    #[serde(default)]
    device: Option<String>,
}

pub async fn signup(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !app.config.registration {
        return refuse(StatusCode::FORBIDDEN, "Sign-ups are closed on this server.");
    }
    let Ok(request) = serde_json::from_slice::<Signup>(&body) else {
        return refuse(StatusCode::BAD_REQUEST, "Send a username and a password.");
    };
    let user = request.user.trim().to_ascii_lowercase();
    if !valid_username(&user) {
        return refuse(
            StatusCode::BAD_REQUEST,
            "Usernames are 3 to 32 lowercase letters, digits, - or _, starting with a letter or digit.",
        );
    }
    if !(MIN_PASSWORD..=MAX_PASSWORD).contains(&request.password.chars().count()) {
        return refuse(
            StatusCode::BAD_REQUEST,
            "Passwords need at least 10 characters.",
        );
    }
    let Some(display) = clean_display(request.display.as_deref(), &user) else {
        return refuse(
            StatusCode::BAD_REQUEST,
            "Display names are at most 40 characters.",
        );
    };
    if app.config.users.contains_key(&user) || app.accounts.contains(&user) {
        return refuse(StatusCode::CONFLICT, "That username is taken.");
    }
    let client = crate::access::client_ip(&app.config, &headers, peer);
    if !app.accounts.signup_allowed(client, now_ms()) {
        return refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many new accounts from here. Try again later.",
        );
    }
    let password = request.password;
    let Ok(Ok(hash)) = tokio::task::spawn_blocking(move || hash_password(&password)).await else {
        return internal("password hashing failed".into());
    };
    match create(
        &app.store.lock().unwrap().conn,
        &user,
        &display,
        &hash,
        now_ms(),
    ) {
        Ok(true) => {}
        Ok(false) => return refuse(StatusCode::CONFLICT, "That username is taken."),
        Err(error) => return internal(error),
    }
    app.accounts.accounts.write().unwrap().insert(
        user.clone(),
        Account {
            display: display.clone(),
        },
    );
    match app.issue_token(&user, &clean_device(request.device.as_deref())) {
        Ok(token) => reply(
            StatusCode::CREATED,
            serde_json::json!({"user": user, "display": display, "token": token}),
        ),
        Err(error) => internal(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    user: String,
    password: String,
    #[serde(default)]
    device: Option<String>,
}

/// Checks an account password off the async runtime; unknown users cost the same as wrong passwords.
pub async fn password_matches(app: &App, user: &str, password: &str) -> bool {
    if password.chars().count() > MAX_PASSWORD {
        return false;
    }
    let stored = stored_hash(&app.store.lock().unwrap().conn, user)
        .ok()
        .flatten();
    let password = password.to_string();
    tokio::task::spawn_blocking(move || match stored {
        Some(hash) => verify_password(&hash, &password),
        None => {
            verify_password(dummy_hash(), &password);
            false
        }
    })
    .await
    .unwrap_or(false)
}

fn dummy_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY.get_or_init(|| hash_password("not an account").unwrap_or_default())
}

/// Signs a client in: trades the account password for a new device token.
pub async fn login(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(request) = serde_json::from_slice::<Login>(&body) else {
        return refuse(StatusCode::BAD_REQUEST, "Send your username and password.");
    };
    let now = now_ms();
    let client = crate::access::client_ip(&app.config, &headers, peer);
    if app.access.throttled(client, now) {
        return refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many attempts. Try again in one minute.",
        );
    }
    let user = request.user.trim().to_ascii_lowercase();
    if !password_matches(&app, &user, &request.password).await {
        app.access.failure(client, now);
        return refuse(
            StatusCode::UNAUTHORIZED,
            "Username or password not recognized.",
        );
    }
    let display = app.display(&user);
    match app.issue_token(&user, &clean_device(request.device.as_deref())) {
        Ok(token) => reply(
            StatusCode::OK,
            serde_json::json!({"user": user, "display": display, "token": token}),
        ),
        Err(error) => internal(error),
    }
}

/// Signs this device out: forgets the token it presents.
pub async fn logout(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return refuse(
            StatusCode::UNAUTHORIZED,
            "Send the device token to sign out.",
        );
    };
    let hash = token_hash(token);
    if app.accounts.tokens.write().unwrap().remove(&hash).is_none() {
        return refuse(StatusCode::UNAUTHORIZED, "Token not recognized.");
    }
    if let Err(error) = app
        .store
        .lock()
        .unwrap()
        .conn
        .execute("delete from account_tokens where token_hash = ?1", [hash])
    {
        return internal(error.into());
    }
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::Request;
    use std::path::PathBuf;
    use tower::ServiceExt;

    fn fixture(registration: bool) -> (Arc<App>, PathBuf) {
        let (store, path) = crate::decks::tests::temporary_store();
        let config = serde_json::from_value(serde_json::json!({
            "registration": registration,
            "users": {"alice": {"token": "alice-token"}}
        }))
        .unwrap();
        let accounts = Directory::load(&store.conn).unwrap();
        (
            Arc::new(App {
                config,
                access: crate::access::Access::default(),
                accounts,
                week: crate::game::Week::default(),
                store: std::sync::Mutex::new(store),
                players: RwLock::new(HashMap::new()),
            }),
            path,
        )
    }

    async fn call(
        app: &Arc<App>,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
        ip: &str,
    ) -> (StatusCode, serde_json::Value) {
        let mut request = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = crate::router(app.clone())
            .layer(MockConnectInfo(
                format!("{ip}:40000").parse::<SocketAddr>().unwrap(),
            ))
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    fn json_headers() -> [(&'static str, &'static str); 1] {
        [("content-type", "application/json")]
    }

    #[test]
    fn usernames_passwords_and_tokens() {
        assert!(valid_username("hill") && valid_username("ana-2") && valid_username("9lives"));
        for bad in ["ab", "Hill", "-hill", "hill!", "admin", &"a".repeat(33)] {
            assert!(!valid_username(bad), "{bad}");
        }
        let hash = hash_password("correct horse").unwrap();
        assert!(verify_password(&hash, "correct horse"));
        assert!(!verify_password(&hash, "wrong horse"));
        assert!(
            PasswordHash::new(dummy_hash()).is_ok(),
            "unknown users still pay for a real verification"
        );
        assert_eq!(token_hash("abc").len(), 64);
        assert_ne!(new_token().unwrap(), new_token().unwrap());
    }

    #[tokio::test]
    async fn sign_ups_are_closed_unless_registration_is_on() {
        let (app, path) = fixture(false);
        let (status, _) = call(
            &app,
            "POST",
            "/api/accounts",
            &json_headers(),
            r#"{"user":"ana","password":"long enough pw"}"#,
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn an_account_signs_up_signs_in_and_signs_out() {
        let (app, path) = fixture(true);
        let body =
            r#"{"user":"Ana","password":"long enough pw","display":"Ana R.","device":"Pixel"}"#;
        let (status, created) = call(
            &app,
            "POST",
            "/api/accounts",
            &json_headers(),
            body,
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["user"], "ana");
        assert_eq!(app.display("ana"), "Ana R.");
        let token = created["token"].as_str().unwrap().to_string();
        assert_eq!(app.accounts.token_owner(&token).as_deref(), Some("ana"));
        let bearer = format!("Bearer {token}");
        let (status, _) = call(
            &app,
            "GET",
            "/api/activity/ana",
            &[("authorization", &bearer)],
            "",
            "192.0.2.1",
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the device token works like a configured token"
        );

        let (status, _) = call(
            &app,
            "POST",
            "/api/accounts",
            &json_headers(),
            body,
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "usernames are unique");
        let (status, _) = call(
            &app,
            "POST",
            "/api/accounts",
            &json_headers(),
            r#"{"user":"alice","password":"long enough pw"}"#,
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "configured users are taken");
        let (status, _) = call(
            &app,
            "POST",
            "/api/accounts",
            &json_headers(),
            r#"{"user":"bo","password":"short"}"#,
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = call(
            &app,
            "POST",
            "/api/accounts/tokens",
            &json_headers(),
            r#"{"user":"ana","password":"wrong password"}"#,
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, second) = call(
            &app,
            "POST",
            "/api/accounts/tokens",
            &json_headers(),
            r#"{"user":"ana","password":"long enough pw","device":"Laptop"}"#,
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let second = second["token"].as_str().unwrap().to_string();
        assert_ne!(second, token);

        let (status, _) = call(
            &app,
            "DELETE",
            "/api/accounts/tokens",
            &[("authorization", &bearer)],
            "",
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(
            app.accounts.token_owner(&token).is_none(),
            "signing out forgets only that device"
        );
        assert_eq!(app.accounts.token_owner(&second).as_deref(), Some("ana"));

        let reloaded = Directory::load(&app.store.lock().unwrap().conn).unwrap();
        assert_eq!(
            reloaded.token_owner(&second).as_deref(),
            Some("ana"),
            "tokens survive a restart"
        );
        assert!(reloaded.token_owner(&token).is_none());
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn the_website_signs_an_account_in_with_its_password() {
        let (app, path) = fixture(true);
        let body = r#"{"user":"ana","password":"long enough pw"}"#;
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/accounts",
                &json_headers(),
                body,
                "192.0.2.1"
            )
            .await
            .0,
            StatusCode::CREATED
        );
        let sign_in = |password: &str| {
            Request::builder()
                .method("POST")
                .uri("/auth/session")
                .header("content-type", "application/json")
                .header("x-ankiquest-csrf", "1")
                .body(Body::from(
                    serde_json::json!({"user": "ana", "password": password}).to_string(),
                ))
                .unwrap()
        };
        let router = || {
            crate::router(app.clone()).layer(MockConnectInfo(
                "192.0.2.1:40000".parse::<SocketAddr>().unwrap(),
            ))
        };
        assert_eq!(
            router()
                .oneshot(sign_in("wrong password"))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let response = router().oneshot(sign_in("long enough pw")).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let (status, _) = call(
            &app,
            "GET",
            "/api/activity/ana",
            &[("cookie", &cookie)],
            "",
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "the session belongs to the account");
        let (status, _) = call(
            &app,
            "GET",
            "/api/activity/alice",
            &[("cookie", &cookie)],
            "",
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "and only to the account");
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn one_address_can_only_create_a_few_accounts_an_hour() {
        let (app, path) = fixture(true);
        for n in 0..SIGNUPS_PER_HOUR {
            let body = format!(r#"{{"user":"user{n}","password":"long enough pw"}}"#);
            assert_eq!(
                call(
                    &app,
                    "POST",
                    "/api/accounts",
                    &json_headers(),
                    &body,
                    "192.0.2.7"
                )
                .await
                .0,
                StatusCode::CREATED
            );
        }
        let (status, _) = call(
            &app,
            "POST",
            "/api/accounts",
            &json_headers(),
            r#"{"user":"oneMore","password":"long enough pw"}"#,
            "192.0.2.7",
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        let (status, _) = call(
            &app,
            "POST",
            "/api/accounts",
            &json_headers(),
            r#"{"user":"elsewhere","password":"long enough pw"}"#,
            "192.0.2.8",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
