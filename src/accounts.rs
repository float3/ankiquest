//! Self-service accounts: a username and password, and a token per device.
//! Configured users keep working; accounts only exist when registration is on.

use crate::access::Credential;
use crate::web_util::{bearer, hex, normalize_user, random_bytes};
use crate::{Error, app::App, app::now_ms};
use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
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
    "tokens",
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
         create index if not exists account_tokens_user on account_tokens (user);
         create table if not exists account_removals (
             user text primary key,
             requested_at integer not null
         ) without rowid;",
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

    /// Forgets an account and every device token it signed in with.
    pub fn remove(&self, user: &str) {
        self.accounts.write().unwrap().remove(user);
        self.tokens
            .write()
            .unwrap()
            .retain(|_, owner| owner != user);
    }

    pub(crate) fn insert(&self, user: &str, display: &str) {
        self.accounts.write().unwrap().insert(
            user.to_string(),
            Account {
                display: display.to_string(),
            },
        );
    }

    /// Forgets the tokens of `user` for which `ends(token hash)` holds.
    fn forget_tokens(&self, user: &str, mut ends: impl FnMut(&str) -> bool) {
        self.tokens
            .write()
            .unwrap()
            .retain(|hash, owner| owner != user || !ends(hash));
    }

    pub fn token_owner(&self, token: &str) -> Option<String> {
        if token.is_empty() {
            return None;
        }
        self.tokens.read().unwrap().get(&token_hash(token)).cloned()
    }

    pub(crate) fn signup_allowed(&self, client: IpAddr, now: i64) -> bool {
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

pub(crate) fn clean_display(display: Option<&str>, user: &str) -> Option<String> {
    let display = display
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .unwrap_or(user);
    (display.chars().count() <= MAX_DISPLAY && !display.chars().any(char::is_control))
        .then(|| display.to_string())
}

pub(crate) fn clean_device(device: Option<&str>) -> String {
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

/// Hashes a password with a salt of argon2's own choosing.
///
/// This used to draw sixteen bytes itself and encode them as a `SaltString`.
/// password-hash 0.6 takes the salt out of the caller's hands: `hash_password`
/// generates a large one from the system CSPRNG, and `SaltString` is gone.
/// Hashes written by the old code still verify, because the salt and the
/// parameters are read back out of the PHC string rather than supplied.
pub fn hash_password(password: &str) -> Result<String, Error> {
    Ok(Argon2::default()
        .hash_password(password.as_bytes())
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
    hex(&Sha256::digest(token.as_bytes()))
}

fn new_token() -> Result<String, Error> {
    Ok(hex(&random_bytes("token randomness unavailable")?))
}

/// Saves a new account; an empty `hash` means it has no password and signs in another way.
pub(crate) fn create(
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

/// Whether the account can sign in with a password.
pub(crate) fn has_password(conn: &Connection, user: &str) -> Result<bool, Error> {
    Ok(stored_hash(conn, user)?.is_some_and(|hash| !hash.is_empty()))
}

impl App {
    /// Whether a new account may not take this name.
    pub(crate) fn name_taken(&self, user: &str) -> bool {
        self.config.users.contains_key(user)
            || self.config.devs.iter().any(|dev| dev == user)
            || self.accounts.contains(user)
    }

    /// A new device token for an existing account, saved before it is handed out.
    pub(crate) fn issue_token(&self, user: &str, device: &str) -> Result<String, Error> {
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

pub(crate) fn reply(status: StatusCode, body: serde_json::Value) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

pub(crate) fn refuse(status: StatusCode, message: &str) -> Response {
    reply(status, serde_json::json!({"error": message}))
}

fn internal(error: Error) -> Response {
    log_error!("accounts: {error}");
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
    let user = normalize_user(&request.user);
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
    if app.name_taken(&user) {
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
    app.accounts.insert(&user, &display);
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
        .flatten()
        .filter(|hash| !hash.is_empty());
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
    let user = normalize_user(&request.user);
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
    let Some(token) = bearer(&headers) else {
        return refuse(
            StatusCode::UNAUTHORIZED,
            "Send the device token to sign out.",
        );
    };
    let hash = token_hash(token);
    let Some(owner) = app.accounts.tokens.read().unwrap().get(&hash).cloned() else {
        return refuse(StatusCode::UNAUTHORIZED, "Token not recognized.");
    };
    if let Err(error) = app
        .store
        .lock()
        .unwrap()
        .conn
        .execute("delete from account_tokens where token_hash = ?1", [&hash])
    {
        return internal(error.into());
    }
    app.accounts.tokens.write().unwrap().remove(&hash);
    app.access
        .end_sessions(&owner, |_, token| token == Some(hash.as_str()));
    StatusCode::NO_CONTENT.into_response()
}

/// A signed-in device: an app holding a device token, or a website session.
#[derive(Serialize)]
struct Device {
    id: String,
    kind: &'static str,
    name: Option<String>,
    created_at: i64,
    current: bool,
}

/// A device's public name: the start of its secret's digest, never the secret.
fn device_id(digest: &str) -> String {
    digest[..16].to_string()
}

fn devices(app: &App, user: &str, credential: &Credential) -> Result<Vec<Device>, Error> {
    let mut devices = app
        .store
        .lock()
        .unwrap()
        .conn
        .prepare("select token_hash, device, created_at from account_tokens where user = ?1")?
        .query_map([user], |r| {
            let hash: String = r.get(0)?;
            Ok(Device {
                id: device_id(&hash),
                kind: "app",
                name: Some(r.get(1)?),
                created_at: r.get(2)?,
                current: credential.token.as_deref() == Some(hash.as_str()),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    devices.extend(
        app.access
            .browser_sessions(user, now_ms())
            .into_iter()
            .map(|session| Device {
                id: device_id(&token_hash(&session.key)),
                kind: "web",
                name: None,
                created_at: session.created,
                current: credential.session.as_deref() == Some(session.key.as_str()),
            }),
    );
    devices.sort_by_key(|device| (!device.current, std::cmp::Reverse(device.created_at)));
    Ok(devices)
}

/// Signs one device out, whichever kind it is. False when no device has that id.
fn sign_out(app: &App, user: &str, id: &str) -> Result<bool, Error> {
    let store = app.store.lock().unwrap();
    let hash = store
        .conn
        .prepare("select token_hash from account_tokens where user = ?1")?
        .query_map([user], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|hash| device_id(hash) == id);
    if let Some(hash) = hash {
        store
            .conn
            .execute("delete from account_tokens where token_hash = ?1", [&hash])?;
        drop(store);
        app.accounts.forget_tokens(user, |other| other == hash);
        app.access
            .end_sessions(user, |_, token| token == Some(hash.as_str()));
        return Ok(true);
    }
    drop(store);
    Ok(app.access.end_sessions(user, |key, token| {
        token.is_none() && device_id(&token_hash(key)) == id
    }) > 0)
}

/// Signs out every device of `user` except the one `credential` belongs to.
fn sign_out_others(app: &App, user: &str, credential: &Credential) -> Result<(), Error> {
    let keep = credential.token.as_deref().unwrap_or_default();
    app.store.lock().unwrap().conn.execute(
        "delete from account_tokens where user = ?1 and token_hash != ?2",
        params![user, keep],
    )?;
    app.accounts.forget_tokens(user, |hash| hash != keep);
    app.access.end_sessions(user, |key, token| {
        credential.session.as_deref() != Some(key) && (keep.is_empty() || token != Some(keep))
    });
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceChange {
    action: String,
    #[serde(default)]
    id: Option<String>,
}

fn device_list(app: &App, user: &str, headers: &HeaderMap) -> Response {
    match devices(app, user, &crate::access::credential(app, headers)) {
        Ok(devices) => reply(StatusCode::OK, serde_json::json!({"devices": devices})),
        Err(error) => internal(error),
    }
}

/// The apps and browsers signed in as the player, the requesting one first.
pub async fn list_devices(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !crate::access::authorized(&app, &user, &headers) {
        return refuse(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    device_list(&app, &user, &headers)
}

/// `{"action": "sign_out", "id": "…"}` signs one device out;
/// `{"action": "sign_out_others"}` signs out every device but the one asking.
pub async fn change_devices(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !crate::access::authorized(&app, &user, &headers) {
        return refuse(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    let Ok(change) = serde_json::from_slice::<DeviceChange>(&body) else {
        return refuse(StatusCode::BAD_REQUEST, "Send an action.");
    };
    let done = match (change.action.as_str(), change.id.as_deref()) {
        ("sign_out", Some(id)) => match sign_out(&app, &user, id) {
            Ok(true) => Ok(()),
            Ok(false) => {
                return refuse(StatusCode::NOT_FOUND, "That device is already signed out.");
            }
            Err(error) => Err(error),
        },
        ("sign_out_others", None) => {
            sign_out_others(&app, &user, &crate::access::credential(&app, &headers))
        }
        _ => return refuse(StatusCode::BAD_REQUEST, "Unknown action."),
    };
    match done {
        Ok(()) => device_list(&app, &user, &headers),
        Err(error) => internal(error),
    }
}

/// Sets a new password. One the account already has must be given as `current`;
/// an account that signs in with a provider can set its first one without.
/// `others` also signs out every other device. Returns the refusal, if the password was not saved.
pub(crate) async fn change_password(
    app: &App,
    user: &str,
    headers: &HeaderMap,
    client: IpAddr,
    current: Option<&str>,
    password: &str,
    others: bool,
) -> Option<Response> {
    if !app.accounts.contains(user) {
        return Some(refuse(
            StatusCode::CONFLICT,
            "Players from the server configuration sign in with their token.",
        ));
    }
    if !(MIN_PASSWORD..=MAX_PASSWORD).contains(&password.chars().count()) {
        return Some(refuse(
            StatusCode::BAD_REQUEST,
            "Passwords need at least 10 characters.",
        ));
    }
    let now = now_ms();
    if app.access.throttled(client, now) {
        return Some(refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many attempts. Try again in one minute.",
        ));
    }
    let has_password = match has_password(&app.store.lock().unwrap().conn, user) {
        Ok(has) => has,
        Err(error) => return Some(internal(error)),
    };
    if has_password {
        let Some(current) = current else {
            return Some(refuse(
                StatusCode::BAD_REQUEST,
                "Enter your current password.",
            ));
        };
        if !password_matches(app, user, current).await {
            app.access.failure(client, now);
            return Some(refuse(StatusCode::FORBIDDEN, "That password is not right."));
        }
    }
    let password = password.to_string();
    let Ok(Ok(hash)) = tokio::task::spawn_blocking(move || hash_password(&password)).await else {
        return Some(internal("password hashing failed".into()));
    };
    if let Err(error) = app.store.lock().unwrap().conn.execute(
        "update accounts set password_hash = ?2 where user = ?1",
        params![user, hash],
    ) {
        return Some(internal(error.into()));
    }
    if others {
        return sign_out_others(app, user, &crate::access::credential(app, headers))
            .err()
            .map(internal);
    }
    None
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
                standings: Default::default(),
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

    async fn sign_up(app: &Arc<App>, user: &str, device: &str) -> String {
        let body =
            serde_json::json!({"user": user, "password": "long enough pw", "device": device});
        let (status, created) = call(
            app,
            "POST",
            "/api/accounts",
            &json_headers(),
            &body.to_string(),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        created["token"].as_str().unwrap().to_string()
    }

    async fn sign_in(app: &Arc<App>, user: &str, password: &str, device: &str) -> Option<String> {
        let body = serde_json::json!({"user": user, "password": password, "device": device});
        let (status, signed_in) = call(
            app,
            "POST",
            "/api/accounts/tokens",
            &json_headers(),
            &body.to_string(),
            "192.0.2.1",
        )
        .await;
        (status == StatusCode::OK).then(|| signed_in["token"].as_str().unwrap().to_string())
    }

    /// A website session cookie, from a password or, with `token`, from a device token.
    async fn web_session(app: &Arc<App>, user: &str, token: Option<&str>) -> String {
        let mut request = Request::builder()
            .method("POST")
            .uri("/auth/session")
            .header("x-ankiquest-csrf", "1");
        let body = match token {
            Some(token) => {
                request = request.header("authorization", format!("Bearer {token}"));
                String::new()
            }
            None => {
                request = request.header("content-type", "application/json");
                serde_json::json!({"user": user, "password": "long enough pw"}).to_string()
            }
        };
        let response = crate::router(app.clone())
            .layer(MockConnectInfo(
                "192.0.2.1:40000".parse::<SocketAddr>().unwrap(),
            ))
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    fn with_token(token: &str) -> [(&'static str, String); 2] {
        [
            ("authorization", format!("Bearer {token}")),
            ("content-type", "application/json".into()),
        ]
    }

    fn with_cookie(cookie: &str) -> [(&'static str, String); 3] {
        [
            ("cookie", cookie.to_string()),
            ("x-ankiquest-csrf", "1".into()),
            ("content-type", "application/json".into()),
        ]
    }

    async fn post(
        app: &Arc<App>,
        path: &str,
        headers: &[(&'static str, String)],
        body: serde_json::Value,
        ip: &str,
    ) -> (StatusCode, serde_json::Value) {
        let headers: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        call(app, "POST", path, &headers, &body.to_string(), ip).await
    }

    async fn list(
        app: &Arc<App>,
        user: &str,
        headers: &[(&'static str, String)],
    ) -> serde_json::Value {
        let headers: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let (status, devices) = call(
            app,
            "GET",
            &format!("/api/devices/{user}"),
            &headers,
            "",
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{devices}");
        devices
    }

    async fn web_works(app: &Arc<App>, user: &str, cookie: &str) -> bool {
        call(
            app,
            "GET",
            &format!("/api/activity/{user}"),
            &[("cookie", cookie)],
            "",
            "192.0.2.1",
        )
        .await
        .0 == StatusCode::OK
    }

    #[tokio::test]
    async fn a_password_changes_with_the_current_one_and_keeps_devices_unless_asked() {
        let (app, path) = fixture(true);
        let phone = sign_up(&app, "ana", "AnkiDroid (Pixel)").await;
        let laptop = sign_in(&app, "ana", "long enough pw", "Anki desktop (Linux)")
            .await
            .unwrap();
        let browser = web_session(&app, "ana", None).await;
        let bo = sign_up(&app, "bea", "Phone").await;
        let change = |current: &str, password: &str, others: bool| serde_json::json!({"action": "password", "current": current, "password": password, "sign_out_others": others});

        let (status, _) = post(
            &app,
            "/api/sign-in/ana",
            &[("content-type", "application/json".into())],
            change("long enough pw", "a new long password", false),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "signed out");
        let (status, _) = post(
            &app,
            "/api/sign-in/ana",
            &with_token(&bo),
            change("long enough pw", "a new long password", false),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "another player");
        let (status, _) = post(
            &app,
            "/api/sign-in/ana",
            &with_token(&phone),
            change("wrong password", "a new long password", false),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "wrong current password");
        let (status, _) = post(
            &app,
            "/api/sign-in/ana",
            &with_token(&phone),
            serde_json::json!({"action": "password", "password": "a new long password"}),
            "192.0.2.1",
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "the current password is needed"
        );
        let (status, _) = post(
            &app,
            "/api/sign-in/ana",
            &with_token(&phone),
            change("long enough pw", "too short", false),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            sign_in(&app, "ana", "long enough pw", "Check")
                .await
                .is_some()
        );

        let (status, methods) = post(
            &app,
            "/api/sign-in/ana",
            &with_token(&phone),
            change("long enough pw", "a new long password", false),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{methods}");
        assert_eq!(methods["password"], true);
        assert!(
            sign_in(&app, "ana", "long enough pw", "Old")
                .await
                .is_none()
        );
        assert!(
            sign_in(&app, "ana", "a new long password", "New")
                .await
                .is_some()
        );
        for token in [&phone, &laptop] {
            assert_eq!(app.accounts.token_owner(token).as_deref(), Some("ana"));
        }
        assert!(
            web_works(&app, "ana", &browser).await,
            "no one is signed out by default"
        );

        let (status, _) = post(
            &app,
            "/api/sign-in/ana",
            &with_token(&phone),
            change("a new long password", "a third long password", true),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(app.accounts.token_owner(&phone).as_deref(), Some("ana"));
        assert!(app.accounts.token_owner(&laptop).is_none());
        assert!(!web_works(&app, "ana", &browser).await);
        assert_eq!(
            list(&app, "ana", &with_token(&phone)).await["devices"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            app.accounts.token_owner(&bo).as_deref(),
            Some("bea"),
            "other players keep their devices"
        );
        let reloaded = Directory::load(&app.store.lock().unwrap().conn).unwrap();
        assert!(reloaded.token_owner(&laptop).is_none());
        assert_eq!(reloaded.token_owner(&phone).as_deref(), Some("ana"));
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn wrong_current_passwords_are_throttled_like_sign_in() {
        let (app, path) = fixture(true);
        let token = sign_up(&app, "ana", "Phone").await;
        let client: IpAddr = "192.0.2.9".parse().unwrap();
        for _ in 1..crate::access::MAX_FAILURES {
            app.access.failure(client, now_ms());
        }
        let change = |current: &str| serde_json::json!({"action": "password", "current": current, "password": "a new long password"});
        let (status, _) = post(
            &app,
            "/api/sign-in/ana",
            &with_token(&token),
            change("wrong password"),
            "192.0.2.9",
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = post(
            &app,
            "/api/sign-in/ana",
            &with_token(&token),
            change("long enough pw"),
            "192.0.2.9",
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(
            sign_in(&app, "ana", "long enough pw", "Check")
                .await
                .is_some(),
            "unchanged"
        );
        let (status, _) = post(
            &app,
            "/api/sign-in/ana",
            &with_token(&token),
            change("long enough pw"),
            "192.0.2.10",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "only that address waits");
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn an_account_without_a_password_sets_one_and_configured_players_cannot() {
        let (app, path) = fixture(true);
        create(&app.store.lock().unwrap().conn, "ana", "Ana", "", now_ms()).unwrap();
        app.accounts.insert("ana", "Ana");
        let token = app.issue_token("ana", "Phone").unwrap();
        assert!(
            sign_in(&app, "ana", "a new long password", "Check")
                .await
                .is_none()
        );
        let (status, methods) = post(
            &app,
            "/api/sign-in/ana",
            &with_token(&token),
            serde_json::json!({"action": "password", "password": "a new long password"}),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{methods}");
        assert_eq!(methods["password"], true);
        assert!(
            sign_in(&app, "ana", "a new long password", "Laptop")
                .await
                .is_some()
        );
        let (status, _) = post(
            &app,
            "/api/sign-in/ana",
            &with_token(&token),
            serde_json::json!({"action": "password", "password": "a third long password"}),
            "192.0.2.1",
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "now the current one is needed"
        );

        let (status, _) = post(
            &app,
            "/api/sign-in/alice",
            &with_token("alice-token"),
            serde_json::json!({"action": "password", "password": "a new long password"}),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn devices_are_listed_and_signed_out_one_by_one_or_all_but_this_one() {
        let (app, path) = fixture(true);
        let phone = sign_up(&app, "ana", "AnkiDroid (Pixel)").await;
        let laptop = sign_in(&app, "ana", "long enough pw", "Anki desktop (Linux)")
            .await
            .unwrap();
        let phone_web = web_session(&app, "ana", Some(&phone)).await;
        let browser = web_session(&app, "ana", None).await;
        let bo = sign_up(&app, "bea", "Phone").await;
        let bo_web = web_session(&app, "bea", None).await;

        let (status, _) = call(&app, "GET", "/api/devices/ana", &[], "", "192.0.2.1").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = post(
            &app,
            "/api/devices/ana",
            &with_token(&bo),
            serde_json::json!({"action": "sign_out_others"}),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "another player");
        let (status, _) = call(
            &app,
            "POST",
            "/api/devices/ana",
            &[("cookie", &browser), ("content-type", "application/json")],
            r#"{"action":"sign_out_others"}"#,
            "192.0.2.1",
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "cookie changes need the site header"
        );

        let listed = list(&app, "ana", &with_token(&phone)).await;
        let devices = listed["devices"].as_array().unwrap();
        assert_eq!(
            devices.len(),
            3,
            "the phone's web view is the phone: {listed}"
        );
        assert_eq!(devices[0]["name"], "AnkiDroid (Pixel)");
        assert_eq!(devices[0]["current"], true);
        assert_eq!(devices[0]["kind"], "app");
        assert!(devices[1..].iter().all(|device| device["current"] == false));
        let web = devices
            .iter()
            .find(|device| device["kind"] == "web")
            .unwrap();
        assert_eq!(web["name"], serde_json::Value::Null);
        let text = listed.to_string();
        for secret in [
            &phone,
            &laptop,
            &browser[browser.find('=').unwrap() + 1..].to_string(),
        ] {
            assert!(!text.contains(secret.as_str()), "no secrets in the list");
        }
        assert!(
            !text.contains(&token_hash(&phone)),
            "nor whole token digests"
        );
        assert_eq!(
            list(&app, "ana", &with_cookie(&phone_web)).await["devices"][0]["name"],
            "AnkiDroid (Pixel)",
            "the web view counts as its app"
        );
        let laptop_id = devices
            .iter()
            .find(|d| d["name"] == "Anki desktop (Linux)")
            .unwrap()["id"]
            .clone();

        let bo_id = list(&app, "bea", &with_token(&bo)).await["devices"][0]["id"].clone();
        let (status, _) = post(
            &app,
            "/api/devices/ana",
            &with_token(&phone),
            serde_json::json!({"action": "sign_out", "id": bo_id}),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "another player's device");
        assert_eq!(app.accounts.token_owner(&bo).as_deref(), Some("bea"));

        let (status, after) = post(
            &app,
            "/api/devices/ana",
            &with_token(&phone),
            serde_json::json!({"action": "sign_out", "id": laptop_id}),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(after["devices"].as_array().unwrap().len(), 2);
        assert!(app.accounts.token_owner(&laptop).is_none());
        let (status, _) = post(
            &app,
            "/api/devices/ana",
            &with_token(&phone),
            serde_json::json!({"action": "sign_out", "id": laptop_id}),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = post(
            &app,
            "/api/devices/ana",
            &with_token(&phone),
            serde_json::json!({"action": "sign_out"}),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = post(
            &app,
            "/api/devices/ana",
            &with_cookie(&browser),
            serde_json::json!({"action": "sign_out", "id": web["id"]}),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "a browser can sign itself out");
        assert!(!web_works(&app, "ana", &browser).await);

        let browser = web_session(&app, "ana", None).await;
        let (status, after) = post(
            &app,
            "/api/devices/ana",
            &with_cookie(&browser),
            serde_json::json!({"action": "sign_out_others"}),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(after["devices"].as_array().unwrap().len(), 1);
        assert_eq!(after["devices"][0]["kind"], "web");
        assert_eq!(after["devices"][0]["current"], true);
        assert!(web_works(&app, "ana", &browser).await);
        assert!(app.accounts.token_owner(&phone).is_none());
        assert!(
            !web_works(&app, "ana", &phone_web).await,
            "the phone's web view went with its token"
        );
        assert_eq!(app.accounts.token_owner(&bo).as_deref(), Some("bea"));
        assert!(
            web_works(&app, "bea", &bo_web).await,
            "other players stay signed in"
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn signing_a_token_out_ends_the_web_view_it_opened() {
        let (app, path) = fixture(true);
        let phone = sign_up(&app, "ana", "Phone").await;
        let phone_web = web_session(&app, "ana", Some(&phone)).await;
        let browser = web_session(&app, "ana", None).await;
        assert!(web_works(&app, "ana", &phone_web).await);
        let (status, _) = call(
            &app,
            "DELETE",
            "/api/accounts/tokens",
            &[("authorization", &format!("Bearer {phone}"))],
            "",
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(!web_works(&app, "ana", &phone_web).await);
        assert!(web_works(&app, "ana", &browser).await);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn configured_players_see_and_end_their_browser_sessions() {
        let (app, path) = fixture(true);
        let first = web_session(&app, "alice", Some("alice-token")).await;
        let listed = list(&app, "alice", &with_token("alice-token")).await;
        assert_eq!(listed["devices"].as_array().unwrap().len(), 1, "{listed}");
        let (status, _) = post(
            &app,
            "/api/devices/alice",
            &with_token("alice-token"),
            serde_json::json!({"action": "sign_out_others"}),
            "192.0.2.1",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!web_works(&app, "alice", &first).await);
        assert_eq!(
            call(
                &app,
                "GET",
                "/api/activity/alice",
                &[("authorization", "Bearer alice-token")],
                "",
                "192.0.2.1"
            )
            .await
            .0,
            StatusCode::OK,
            "the configured token keeps working"
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
