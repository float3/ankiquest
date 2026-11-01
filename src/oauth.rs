//! Sign-in with Google, GitHub and GitLab: the OAuth 2.0 authorization code flow with PKCE.
//! Each provider's stable user id is linked to one player. A first sign-in creates an account
//! once the person has picked a username; later ones sign into the linked player.

use crate::accounts::{clean_device, clean_display, refuse, reply, valid_username};
use crate::privacy::escape;
use crate::web_util::{base64url, hex, hex_cookie, normalize_user, random_bytes, set_cookie};
use crate::{Error, access, app::App, app::now_ms};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where a first sign-in picks its username.
pub const SIGNUP_PATH: &str = "/auth/oauth/signup";
const COOKIE: &str = "ankiquest_oauth";
const FLOW_MS: i64 = 10 * 60 * 1000;
const MAX_FLOWS: usize = 4096;
const MAX_SUBJECT: usize = 255;
const MAX_SUGGESTION: usize = 28;
const TIMEOUT: Duration = Duration::from_secs(10);

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists sign_in_links (
             provider text not null,
             subject text not null,
             user text not null,
             created_at integer not null,
             primary key (provider, subject),
             unique (user, provider)
         ) without rowid;",
    )?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Google,
    Github,
    Gitlab,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Google, Provider::Github, Provider::Gitlab];

    /// The name in configuration and URLs.
    pub fn key(self) -> &'static str {
        match self {
            Provider::Google => "google",
            Provider::Github => "github",
            Provider::Gitlab => "gitlab",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::Google => "Google",
            Provider::Github => "GitHub",
            Provider::Gitlab => "GitLab",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|provider| provider.key() == key)
    }

    fn scope(self) -> Option<&'static str> {
        match self {
            Provider::Google | Provider::Gitlab => Some("openid profile"),
            Provider::Github => None,
        }
    }
}

/// The registered apps, one per provider; a provider is offered only when it has one.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default)]
    pub google: Option<Client>,
    #[serde(default)]
    pub github: Option<Client>,
    #[serde(default)]
    pub gitlab: Option<Client>,
}

/// One OAuth app registered with a provider.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Client {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub client_secret_file: Option<PathBuf>,
    /// A self-hosted GitLab, instead of https://gitlab.com.
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub authorize_url: Option<String>,
    #[serde(default)]
    pub token_url: Option<String>,
    #[serde(default)]
    pub userinfo_url: Option<String>,
}

struct Endpoints {
    authorize: String,
    token: String,
    userinfo: String,
}

impl Client {
    fn secret(&self) -> Option<&str> {
        self.client_secret.as_deref().filter(|s| !s.is_empty())
    }

    fn endpoints(&self, provider: Provider) -> Endpoints {
        let (authorize, token, userinfo) = match provider {
            Provider::Google => (
                "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
                "https://oauth2.googleapis.com/token".to_string(),
                "https://openidconnect.googleapis.com/v1/userinfo".to_string(),
            ),
            Provider::Github => (
                "https://github.com/login/oauth/authorize".to_string(),
                "https://github.com/login/oauth/access_token".to_string(),
                "https://api.github.com/user".to_string(),
            ),
            Provider::Gitlab => {
                let base = self
                    .base_url
                    .as_deref()
                    .unwrap_or("https://gitlab.com")
                    .trim_end_matches('/');
                (
                    format!("{base}/oauth/authorize"),
                    format!("{base}/oauth/token"),
                    format!("{base}/oauth/userinfo"),
                )
            }
        };
        Endpoints {
            authorize: self.authorize_url.clone().unwrap_or(authorize),
            token: self.token_url.clone().unwrap_or(token),
            userinfo: self.userinfo_url.clone().unwrap_or(userinfo),
        }
    }
}

impl Settings {
    fn slot(&mut self, provider: Provider) -> &mut Option<Client> {
        match provider {
            Provider::Google => &mut self.google,
            Provider::Github => &mut self.github,
            Provider::Gitlab => &mut self.gitlab,
        }
    }

    pub fn client(&self, provider: Provider) -> Option<&Client> {
        match provider {
            Provider::Google => self.google.as_ref(),
            Provider::Github => self.github.as_ref(),
            Provider::Gitlab => self.gitlab.as_ref(),
        }
        .filter(|client| client.secret().is_some())
    }

    /// The providers people can sign in with here.
    pub fn providers(&self) -> Vec<Provider> {
        Provider::ALL
            .into_iter()
            .filter(|provider| self.client(*provider).is_some())
            .collect()
    }

    /// Reads each client secret from its file, and refuses a half-configured provider.
    pub fn load_secrets(&mut self, public_url: Option<&str>) -> Result<(), Error> {
        for provider in Provider::ALL {
            let key = provider.key();
            let Some(client) = self.slot(provider) else {
                continue;
            };
            if client.client_id.trim().is_empty() {
                return Err(format!("oauth.{key}.client_id must not be empty").into());
            }
            if let Some(file) = &client.client_secret_file {
                let secret = std::fs::read_to_string(file)
                    .map_err(|e| format!("cannot read oauth.{key}.client_secret_file: {e}"))?;
                client.client_secret = Some(secret.trim().to_string());
            }
            if client.secret().is_none() {
                return Err(format!("oauth.{key} needs a nonempty client_secret_file").into());
            }
            if public_url.is_none_or(|url| url.trim().is_empty()) {
                return Err("oauth needs public_url for its callback addresses".into());
            }
        }
        Ok(())
    }
}

trait Expiring {
    fn expires(&self) -> i64;
}

struct Started {
    provider: Provider,
    verifier: String,
    link: Option<String>,
    next: String,
    expires: i64,
}

#[derive(Clone)]
struct Signup {
    provider: Provider,
    subject: String,
    user: String,
    display: String,
    next: String,
    expires: i64,
}

impl Expiring for Started {
    fn expires(&self) -> i64 {
        self.expires
    }
}

impl Expiring for Signup {
    fn expires(&self) -> i64 {
        self.expires
    }
}

/// Sign-ins in progress, each found through the random value in its browser's cookie.
#[derive(Default)]
pub(crate) struct Flows {
    started: Mutex<HashMap<String, Started>>,
    signups: Mutex<HashMap<String, Signup>>,
}

fn keep<T: Expiring>(map: &mut HashMap<String, T>, key: String, value: T, now: i64) {
    map.retain(|_, entry| entry.expires() > now);
    if map.len() >= MAX_FLOWS
        && let Some(oldest) = map
            .iter()
            .min_by_key(|(_, entry)| entry.expires())
            .map(|(key, _)| key.clone())
    {
        map.remove(&oldest);
    }
    map.insert(key, value);
}

impl Flows {
    fn start(&self, state: String, started: Started, now: i64) {
        keep(&mut self.started.lock().unwrap(), state, started, now);
    }

    /// The flow behind a callback, at most once.
    fn take_started(&self, state: &str, now: i64) -> Option<Started> {
        self.started
            .lock()
            .unwrap()
            .remove(state)
            .filter(|started| started.expires > now)
    }

    fn add_signup(&self, key: String, signup: Signup, now: i64) {
        keep(&mut self.signups.lock().unwrap(), key, signup, now);
    }

    fn signup(&self, key: &str, now: i64) -> Option<Signup> {
        self.signups
            .lock()
            .unwrap()
            .get(key)
            .filter(|signup| signup.expires > now)
            .cloned()
    }

    fn finish_signup(&self, key: &str) {
        self.signups.lock().unwrap().remove(key);
    }
}

fn random() -> Result<[u8; 32], Error> {
    random_bytes("randomness unavailable")
}

/// The S256 code challenge for a PKCE verifier.
pub(crate) fn challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

fn flow_cookie(headers: &HeaderMap) -> Option<&str> {
    hex_cookie(headers, COOKIE)
}

fn set_flow(app: &App, value: &str, max_age: i64) -> HeaderValue {
    set_cookie(app, COOKIE, "/auth/oauth/", value, max_age)
}

/// A page on this site to return to, or the start page.
fn local_path(next: Option<&str>) -> String {
    match next {
        Some(next)
            if next.starts_with('/')
                && !next.starts_with("//")
                && next.is_ascii()
                && !next.contains('\\')
                && !next.chars().any(|c| c.is_ascii_control())
                && !next.starts_with("/login")
                && !next.starts_with("/auth/") =>
        {
            next.to_string()
        }
        _ => "/".into(),
    }
}

fn redirect_uri(app: &App, provider: Provider) -> String {
    format!(
        "{}/auth/oauth/{}/callback",
        app.config
            .public_url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/'),
        provider.key()
    )
}

fn see_other(location: &str, cookies: &[HeaderValue]) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(location).unwrap_or(HeaderValue::from_static("/")),
    );
    for cookie in cookies {
        headers.append(header::SET_COOKIE, cookie.clone());
    }
    response
}

fn page(status: StatusCode, body: &str, cookies: &[HeaderValue]) -> Response {
    let mut response = (status, Html(web!("oauth.html").replace("{{body}}", body))).into_response();
    for cookie in cookies {
        response
            .headers_mut()
            .append(header::SET_COOKIE, cookie.clone());
    }
    response
}

fn translated(text: &str) -> String {
    let text = escape(text);
    format!(r#"<span data-i18n="{text}">{text}</span>"#)
}

fn message(status: StatusCode, text: &str, back: &str, cookies: &[HeaderValue]) -> Response {
    let label = if back == "/settings" {
        "Back to settings"
    } else {
        "Back to sign-in"
    };
    page(
        status,
        &format!(
            r#"<p class="oauth-message">{}</p><a class="oauth-button" href="{back}">{}</a>"#,
            translated(text),
            translated(label)
        ),
        cookies,
    )
}

fn form_page(status: StatusCode, user: &str, display: &str, error: Option<&str>) -> Response {
    page(
        status,
        &format!(
            r#"<h1>{title}</h1>
<form method="post" action="{SIGNUP_PATH}">
<label for="user">{username}</label>
<input id="user" name="user" value="{user}" autocomplete="username" autocapitalize="none" spellcheck="false" required minlength="3" maxlength="32" autofocus>
<label for="display">{display_label}</label>
<input id="display" name="display" value="{display}" autocomplete="nickname" maxlength="40">
<button class="oauth-button" type="submit">{create}</button>
<p class="oauth-error" role="status">{error}</p>
<p class="oauth-privacy"><a href="/privacy">{privacy}</a></p>
</form>"#,
            title = translated("Choose a username"),
            username = translated("Username"),
            display_label = translated("Display name (optional)"),
            create = translated("Create account"),
            privacy = translated("How AnkiQuest handles your data"),
            user = escape(user),
            display = escape(display),
            error = error.map(translated).unwrap_or_default(),
        ),
        &[],
    )
}

/// Who someone is at a provider: the stable id, and names that only seed suggestions.
#[derive(Debug, PartialEq)]
struct Identity {
    subject: String,
    handle: Option<String>,
    name: Option<String>,
}

fn identity(provider: Provider, info: &serde_json::Value) -> Result<Identity, Error> {
    let text = |key: &str| {
        info[key]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(String::from)
    };
    let subject = match provider {
        Provider::Github => info["id"].as_u64().map(|id| id.to_string()),
        Provider::Google | Provider::Gitlab => text("sub"),
    }
    .filter(|subject| subject.len() <= MAX_SUBJECT)
    .ok_or("the provider sent no user id")?;
    let handle = match provider {
        Provider::Google => text("given_name"),
        Provider::Github => text("login"),
        Provider::Gitlab => text("nickname").or_else(|| text("preferred_username")),
    };
    Ok(Identity {
        subject,
        handle,
        name: text("name"),
    })
}

fn read(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<serde_json::Value, Error> {
    let mut response = response?;
    Ok(serde_json::from_str(
        &response.body_mut().read_to_string()?,
    )?)
}

/// Trades the code for an access token, then asks who it belongs to.
fn fetch_identity(
    provider: Provider,
    endpoints: &Endpoints,
    client_id: &str,
    secret: &str,
    code: &str,
    verifier: &str,
    redirect: &str,
) -> Result<Identity, Error> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let token = read(
        agent
            .post(&endpoints.token)
            .header("Accept", "application/json")
            .header("User-Agent", "AnkiQuest")
            .send_form([
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect),
                ("client_id", client_id),
                ("client_secret", secret),
                ("code_verifier", verifier),
            ]),
    )?;
    let access = token["access_token"]
        .as_str()
        .ok_or("the provider sent no access token")?;
    let info = read(
        agent
            .get(&endpoints.userinfo)
            .header("Authorization", &format!("Bearer {access}"))
            .header("Accept", "application/json")
            .header("User-Agent", "AnkiQuest")
            .call(),
    )?;
    identity(provider, &info)
}

fn fold(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => "a",
        'æ' => "ae",
        'ç' => "c",
        'è' | 'é' | 'ê' | 'ë' => "e",
        'ì' | 'í' | 'î' | 'ï' => "i",
        'ñ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => "o",
        'ù' | 'ú' | 'û' | 'ü' => "u",
        'ý' | 'ÿ' => "y",
        'ß' => "ss",
        _ => return None,
    })
}

/// A name in the shape usernames take, or "player" when too little of it survives.
fn sanitize(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if let Some(folded) = fold(c) {
            out.push_str(folded);
        } else if matches!(c, '-' | '_' | ' ' | '.')
            && !out.is_empty()
            && !out.ends_with(['-', '_'])
        {
            out.push(if c == '_' { '_' } else { '-' });
        }
    }
    out.truncate(MAX_SUGGESTION);
    let out = out.trim_end_matches(['-', '_']);
    if out.len() < 3 {
        "player".into()
    } else {
        out.into()
    }
}

/// A free username close to what the provider calls the person.
fn suggest(raw: Option<&str>, taken: impl Fn(&str) -> bool) -> String {
    let base = sanitize(raw.unwrap_or_default());
    let free = |candidate: &str| valid_username(candidate) && !taken(candidate);
    if free(&base) {
        return base;
    }
    (2u64..)
        .map(|n| format!("{base}{n}"))
        .find(|candidate| free(candidate))
        .unwrap()
}

fn display_suggestion(identity: &Identity) -> String {
    identity
        .name
        .as_deref()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(40)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Whether removing one sign-in leaves the player a way back in.
fn can_unlink(account: bool, password: bool, other_links: usize) -> bool {
    !account || password || other_links > 0
}

fn exists(app: &App, user: &str) -> bool {
    app.accounts.contains(user)
        || app
            .config
            .users
            .get(user)
            .and_then(|config| config.token.as_deref())
            .is_some_and(|token| !token.is_empty())
}

/// The player linked to this identity, if they can still sign in.
fn owner(
    app: &App,
    conn: &Connection,
    provider: Provider,
    subject: &str,
) -> Result<Option<String>, Error> {
    let linked: Option<String> = conn
        .query_row(
            "select user from sign_in_links where provider = ?1 and subject = ?2",
            params![provider.key(), subject],
            |r| r.get(0),
        )
        .optional()?;
    Ok(linked.filter(|user| exists(app, user)))
}

fn linked_providers(conn: &Connection, user: &str) -> Result<Vec<Provider>, Error> {
    Ok(conn
        .prepare("select provider from sign_in_links where user = ?1")?
        .query_map([user], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .filter_map(|key| Provider::parse(key))
        .collect())
}

#[derive(Debug, PartialEq)]
enum Outcome {
    SignIn(String),
    Linked,
    LinkedElsewhere,
    ProviderTaken,
    Signup,
    Closed,
}

fn decide(
    app: &App,
    provider: Provider,
    subject: &str,
    link: Option<&str>,
) -> Result<Outcome, Error> {
    let store = app.store.lock().unwrap();
    let conn = &store.conn;
    Ok(match (owner(app, conn, provider, subject)?, link) {
        (Some(owner), Some(me)) if owner == me => Outcome::Linked,
        (Some(_), Some(_)) => Outcome::LinkedElsewhere,
        (Some(owner), None) => Outcome::SignIn(owner),
        (None, Some(me)) => {
            let added = conn.execute(
                "insert or ignore into sign_in_links (provider, subject, user, created_at) values (?1, ?2, ?3, ?4)",
                params![provider.key(), subject, me, now_ms()],
            )?;
            if added == 1 {
                Outcome::Linked
            } else {
                Outcome::ProviderTaken
            }
        }
        (None, None) if app.config.registration => Outcome::Signup,
        (None, None) => Outcome::Closed,
    })
}

enum Created {
    Account,
    NameTaken,
    AlreadyLinked(String),
}

fn create_linked(app: &App, signup: &Signup, user: &str, display: &str) -> Result<Created, Error> {
    let store = app.store.lock().unwrap();
    if let Some(owner) = owner(app, &store.conn, signup.provider, &signup.subject)? {
        return Ok(Created::AlreadyLinked(owner));
    }
    let transaction = store.conn.unchecked_transaction()?;
    let now = now_ms();
    if !crate::accounts::create(&transaction, user, display, "", now)? {
        return Ok(Created::NameTaken);
    }
    transaction.execute(
        "insert into sign_in_links (provider, subject, user, created_at) values (?1, ?2, ?3, ?4)",
        params![signup.provider.key(), signup.subject, user, now],
    )?;
    transaction.commit()?;
    drop(store);
    app.accounts.insert(user, display);
    Ok(Created::Account)
}

fn configured<'a>(app: &'a App, key: &str) -> Option<(Provider, &'a Client)> {
    let provider = Provider::parse(key)?;
    Some((provider, app.config.oauth.client(provider)?))
}

fn signed_in(
    app: &App,
    headers: &HeaderMap,
    user: &str,
    next: &str,
    cookies: &[HeaderValue],
) -> Response {
    match access::member_session(app, headers, user) {
        Ok(session) => {
            let mut all = cookies.to_vec();
            all.push(session);
            see_other(next, &all)
        }
        Err(_) => message(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Sign-in failed. Try again.",
            "/login",
            cookies,
        ),
    }
}

#[derive(Deserialize)]
struct StartQuery {
    #[serde(default)]
    next: Option<String>,
    #[serde(default)]
    link: Option<String>,
}

/// Sends the browser to the provider. With `?link=1`, the signed-in player links it instead.
async fn start(
    State(app): State<Arc<App>>,
    Path(key): Path<String>,
    Query(query): Query<StartQuery>,
    headers: HeaderMap,
) -> Response {
    let Some((provider, client)) = configured(&app, &key) else {
        return refuse(
            StatusCode::NOT_FOUND,
            "This server does not offer that sign-in.",
        );
    };
    let link = if query.link.as_deref() == Some("1") {
        match access::viewer(&app, &headers).member {
            Some(member) => Some(member),
            None => return see_other("/login?next=%2Fsettings", &[]),
        }
    } else {
        None
    };
    let (Ok(state), Ok(verifier)) = (random(), random()) else {
        return refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Sign-in failed. Try again.",
        );
    };
    let (state, verifier) = (hex(&state), base64url(&verifier));
    let endpoints = client.endpoints(provider);
    let mut url = format!(
        "{}{}response_type=code&client_id={}&redirect_uri={}&state={state}&code_challenge={}&code_challenge_method=S256",
        endpoints.authorize,
        if endpoints.authorize.contains('?') {
            '&'
        } else {
            '?'
        },
        access::encoded_next(&client.client_id),
        access::encoded_next(&redirect_uri(&app, provider)),
        challenge(&verifier),
    );
    if let Some(scope) = provider.scope() {
        url += &format!("&scope={}", access::encoded_next(scope));
    }
    let next = if link.is_some() {
        "/settings".to_string()
    } else {
        local_path(query.next.as_deref())
    };
    let now = now_ms();
    app.access.oauth.start(
        state.clone(),
        Started {
            provider,
            verifier,
            link,
            next,
            expires: now + FLOW_MS,
        },
        now,
    );
    see_other(&url, &[set_flow(&app, &state, FLOW_MS / 1000)])
}

/// Where the provider sends the browser back, with a code and the state it was given.
async fn callback(
    State(app): State<Arc<App>>,
    Path(key): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some((provider, client)) = configured(&app, &key) else {
        return refuse(
            StatusCode::NOT_FOUND,
            "This server does not offer that sign-in.",
        );
    };
    let clear = set_flow(&app, "", 0);
    let given = query.get("state").map(String::as_str).unwrap_or_default();
    let started = flow_cookie(&headers)
        .filter(|cookie| *cookie == given)
        .and_then(|state| app.access.oauth.take_started(state, now_ms()))
        .filter(|started| started.provider == provider);
    let Some(started) = started else {
        return message(
            StatusCode::BAD_REQUEST,
            "This sign-in expired. Try again.",
            "/login",
            &[clear],
        );
    };
    let back = if started.link.is_some() {
        "/settings"
    } else {
        "/login"
    };
    if query.contains_key("error") {
        return message(StatusCode::OK, "Sign-in was cancelled.", back, &[clear]);
    }
    let Some(code) = query.get("code").cloned() else {
        return message(
            StatusCode::BAD_REQUEST,
            "Sign-in failed. Try again.",
            back,
            &[clear],
        );
    };
    let endpoints = client.endpoints(provider);
    let client_id = client.client_id.clone();
    let secret = client.secret().unwrap_or_default().to_string();
    let redirect = redirect_uri(&app, provider);
    let verifier = started.verifier.clone();
    let fetched = tokio::task::spawn_blocking(move || {
        fetch_identity(
            provider, &endpoints, &client_id, &secret, &code, &verifier, &redirect,
        )
    })
    .await;
    let identity = match fetched {
        Ok(Ok(identity)) => identity,
        Ok(Err(error)) => {
            log_error!("{} sign-in failed: {error}", provider.name());
            return message(
                StatusCode::BAD_GATEWAY,
                "Sign-in failed. Try again.",
                back,
                &[clear],
            );
        }
        Err(_) => {
            return message(
                StatusCode::BAD_GATEWAY,
                "Sign-in failed. Try again.",
                back,
                &[clear],
            );
        }
    };
    if let Some(me) = &started.link
        && access::viewer(&app, &headers).member.as_ref() != Some(me)
    {
        return message(
            StatusCode::UNAUTHORIZED,
            "Sign in again to link this account.",
            "/login",
            &[clear],
        );
    }
    let outcome = match decide(&app, provider, &identity.subject, started.link.as_deref()) {
        Ok(outcome) => outcome,
        Err(error) => {
            log_error!("{} sign-in could not be saved: {error}", provider.name());
            return message(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Sign-in failed. Try again.",
                back,
                &[clear],
            );
        }
    };
    match outcome {
        Outcome::SignIn(user) => signed_in(&app, &headers, &user, &started.next, &[clear]),
        Outcome::Linked => see_other("/settings", &[clear]),
        Outcome::LinkedElsewhere => message(
            StatusCode::CONFLICT,
            "That account is already linked to another player.",
            back,
            &[clear],
        ),
        Outcome::ProviderTaken => message(
            StatusCode::CONFLICT,
            "You already linked another account from this provider. Unlink it first.",
            back,
            &[clear],
        ),
        Outcome::Closed => message(
            StatusCode::FORBIDDEN,
            "No player is linked to this account, and this server does not take new accounts.",
            back,
            &[clear],
        ),
        Outcome::Signup => {
            let Ok(key) = random().map(|bytes| hex(&bytes)) else {
                return message(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Sign-in failed. Try again.",
                    back,
                    &[clear],
                );
            };
            let user = suggest(
                identity.handle.as_deref().or(identity.name.as_deref()),
                |name| app.name_taken(name),
            );
            let now = now_ms();
            app.access.oauth.add_signup(
                key.clone(),
                Signup {
                    provider,
                    display: display_suggestion(&identity),
                    subject: identity.subject,
                    user,
                    next: started.next,
                    expires: now + FLOW_MS,
                },
                now,
            );
            see_other(SIGNUP_PATH, &[set_flow(&app, &key, FLOW_MS / 1000)])
        }
    }
}

async fn signup_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    match flow_cookie(&headers).and_then(|key| app.access.oauth.signup(key, now_ms())) {
        Some(signup) => form_page(StatusCode::OK, &signup.user, &signup.display, None),
        None => see_other("/login", &[]),
    }
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    user: String,
    #[serde(default)]
    display: String,
}

/// Creates the account a first sign-in asked for, with the username the person chose.
async fn create(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(choice): Form<Choice>,
) -> Response {
    if !access::same_origin(&app, &headers) {
        return message(
            StatusCode::FORBIDDEN,
            "Sign up from this site.",
            "/login",
            &[],
        );
    }
    let now = now_ms();
    let Some((key, signup)) = flow_cookie(&headers)
        .and_then(|key| Some((key.to_string(), app.access.oauth.signup(key, now)?)))
    else {
        return message(
            StatusCode::BAD_REQUEST,
            "This sign-in expired. Try again.",
            "/login",
            &[],
        );
    };
    if !app.config.registration {
        return message(
            StatusCode::FORBIDDEN,
            "Sign-ups are closed on this server.",
            "/login",
            &[],
        );
    }
    let again = |status, text| form_page(status, &choice.user, &choice.display, Some(text));
    let user = normalize_user(&choice.user);
    if !valid_username(&user) {
        return again(
            StatusCode::BAD_REQUEST,
            "Usernames are 3 to 32 lowercase letters, digits, - or _, starting with a letter or digit.",
        );
    }
    let Some(display) = clean_display(Some(&choice.display), &user) else {
        return again(
            StatusCode::BAD_REQUEST,
            "Display names are at most 40 characters.",
        );
    };
    if app.name_taken(&user) {
        return again(StatusCode::CONFLICT, "That username is taken.");
    }
    let client = access::client_ip(&app.config, &headers, peer);
    if !app.accounts.signup_allowed(client, now) {
        return again(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many new accounts from here. Try again later.",
        );
    }
    let clear = set_flow(&app, "", 0);
    let user = match create_linked(&app, &signup, &user, &display) {
        Ok(Created::Account) => user,
        Ok(Created::AlreadyLinked(owner)) => owner,
        Ok(Created::NameTaken) => return again(StatusCode::CONFLICT, "That username is taken."),
        Err(error) => {
            log_error!(
                "{} sign-up could not be saved: {error}",
                signup.provider.name()
            );
            return again(
                StatusCode::INTERNAL_SERVER_ERROR,
                "The account could not be saved. Please try again.",
            );
        }
    };
    app.access.oauth.finish_signup(&key);
    signed_in(&app, &headers, &user, &signup.next, &[clear])
}

fn methods_of(app: &App, user: &str) -> Result<serde_json::Value, Error> {
    let store = app.store.lock().unwrap();
    let linked = linked_providers(&store.conn, user)?;
    let account = app.accounts.contains(user);
    let password = account && crate::accounts::has_password(&store.conn, user)?;
    let providers: Vec<_> = app
        .config
        .oauth
        .providers()
        .into_iter()
        .map(|provider| {
            serde_json::json!({
                "provider": provider.key(),
                "name": provider.name(),
                "linked": linked.contains(&provider),
            })
        })
        .collect();
    Ok(serde_json::json!({"account": account, "password": password, "providers": providers}))
}

fn internal(error: Error) -> Response {
    log_error!("sign-in methods: {error}");
    refuse(
        StatusCode::INTERNAL_SERVER_ERROR,
        "That could not be saved. Please try again.",
    )
}

/// The ways a player can sign in: their password, and each provider and whether it is linked.
async fn methods(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !access::authorized(&app, &user, &headers) {
        return refuse(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    match methods_of(&app, &user) {
        Ok(methods) => reply(StatusCode::OK, methods),
        Err(error) => internal(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    action: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    device: Option<String>,
}

enum Unlinked {
    Done,
    NotLinked,
    LastMethod,
}

fn unlink(app: &App, user: &str, provider: Provider) -> Result<Unlinked, Error> {
    let store = app.store.lock().unwrap();
    let linked = linked_providers(&store.conn, user)?;
    if !linked.contains(&provider) {
        return Ok(Unlinked::NotLinked);
    }
    let account = app.accounts.contains(user);
    let password = account && crate::accounts::has_password(&store.conn, user)?;
    let others = linked
        .iter()
        .filter(|other| **other != provider && app.config.oauth.client(**other).is_some())
        .count();
    if !can_unlink(account, password, others) {
        return Ok(Unlinked::LastMethod);
    }
    store.conn.execute(
        "delete from sign_in_links where user = ?1 and provider = ?2",
        params![user, provider.key()],
    )?;
    Ok(Unlinked::Done)
}

/// `{"action": "unlink", "provider": "github"}` removes a linked sign-in;
/// `{"action": "token"}` makes a device token, for accounts that have no password to trade for one.
async fn change(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !access::authorized(&app, &user, &headers) {
        return refuse(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    let Ok(change) = serde_json::from_slice::<Change>(&body) else {
        return refuse(StatusCode::BAD_REQUEST, "Send an action.");
    };
    match change.action.as_str() {
        "unlink" => {
            let Some(provider) = change.provider.as_deref().and_then(Provider::parse) else {
                return refuse(StatusCode::BAD_REQUEST, "Unknown sign-in provider.");
            };
            match unlink(&app, &user, provider) {
                Ok(Unlinked::Done) => match methods_of(&app, &user) {
                    Ok(methods) => reply(StatusCode::OK, methods),
                    Err(error) => internal(error),
                },
                Ok(Unlinked::NotLinked) => {
                    refuse(StatusCode::NOT_FOUND, "That sign-in is not linked.")
                }
                Ok(Unlinked::LastMethod) => refuse(
                    StatusCode::CONFLICT,
                    "Link another sign-in first, or this account cannot be signed into.",
                ),
                Err(error) => internal(error),
            }
        }
        "token" => {
            if !app.accounts.contains(&user) {
                return refuse(
                    StatusCode::CONFLICT,
                    "Players from the server configuration already have a token.",
                );
            }
            let device = clean_device(change.device.as_deref());
            match app.issue_token(&user, &device) {
                Ok(token) => reply(StatusCode::CREATED, serde_json::json!({"token": token})),
                Err(error) => internal(error),
            }
        }
        _ => refuse(StatusCode::BAD_REQUEST, "Unknown action."),
    }
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route(
            SIGNUP_PATH,
            get(signup_page)
                .post(create)
                .layer(DefaultBodyLimit::max(4096)),
        )
        .route("/auth/oauth/{provider}", get(start))
        .route("/auth/oauth/{provider}/callback", get(callback))
        .route(
            "/api/sign-in/{user}",
            get(methods).post(change).layer(DefaultBodyLimit::max(4096)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use axum::routing::post;
    use std::sync::RwLock;
    use tower::ServiceExt;

    const ORIGIN: &str = "https://anki.example.test";
    const FORM: &str = "application/x-www-form-urlencoded";

    #[derive(Default)]
    struct Mock {
        requests: Mutex<Vec<HashMap<String, String>>>,
        subject: Mutex<String>,
    }

    fn authorized_at_provider(headers: &HeaderMap) -> bool {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            == Some("Bearer provider-access")
    }

    async fn token(
        State(mock): State<Arc<Mock>>,
        Form(form): Form<HashMap<String, String>>,
    ) -> Response {
        let good = form.get("code").map(String::as_str) == Some("good-code");
        mock.requests.lock().unwrap().push(form);
        if good {
            axum::Json(
                serde_json::json!({"access_token": "provider-access", "token_type": "bearer"}),
            )
            .into_response()
        } else {
            (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"error": "invalid_grant"})),
            )
                .into_response()
        }
    }

    async fn userinfo(State(mock): State<Arc<Mock>>, headers: HeaderMap) -> Response {
        if !authorized_at_provider(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let subject = mock.subject.lock().unwrap().clone();
        axum::Json(
            serde_json::json!({"sub": subject, "nickname": "Ana.López", "name": "Ana López"}),
        )
        .into_response()
    }

    async fn github_user(headers: HeaderMap) -> Response {
        if !authorized_at_provider(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        axum::Json(serde_json::json!({"id": 4242, "login": "hill", "name": null})).into_response()
    }

    /// A provider on a local port: GitLab's paths, and GitHub's under /gh.
    async fn provider() -> (String, Arc<Mock>) {
        let mock = Arc::new(Mock {
            subject: Mutex::new("1001".into()),
            ..Mock::default()
        });
        let router = Router::new()
            .route("/oauth/token", post(token))
            .route("/oauth/userinfo", get(userinfo))
            .route("/gh/token", post(token))
            .route("/gh/user", get(github_user))
            .with_state(mock.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (base, mock)
    }

    fn fixture(registration: bool, base: &str) -> (Arc<App>, std::path::PathBuf) {
        let (store, path) = crate::decks::tests::temporary_store();
        let config = serde_json::from_value(serde_json::json!({
            "registration": registration,
            "public_url": ORIGIN,
            "devs": ["hill"],
            "users": {"alice": {"token": "alice-token"}},
            "oauth": {
                "gitlab": {"client_id": "gitlab-app", "client_secret": "gitlab-secret", "base_url": base},
                "github": {
                    "client_id": "github-app",
                    "client_secret": "github-secret",
                    "authorize_url": format!("{base}/gh/authorize"),
                    "token_url": format!("{base}/gh/token"),
                    "userinfo_url": format!("{base}/gh/user")
                }
            }
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

    fn cleanup(app: Arc<App>, path: std::path::PathBuf) {
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    async fn send(
        app: &Arc<App>,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Response {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "anki.example.test")
            .extension(ConnectInfo(
                "192.0.2.1:40000".parse::<SocketAddr>().unwrap(),
            ));
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        crate::router(app.clone())
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    fn location(response: &Response) -> String {
        response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_string()
    }

    /// `name=value` from a Set-Cookie that sets a value.
    fn cookie(response: &Response, name: &str) -> Option<String> {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok()?.split(';').next())
            .find(|pair| pair.starts_with(&format!("{name}=")) && pair.len() > name.len() + 1)
            .map(String::from)
    }

    async fn text(response: Response) -> String {
        String::from_utf8(
            to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    async fn json(response: Response) -> serde_json::Value {
        serde_json::from_str(&text(response).await).unwrap()
    }

    fn param(url: &str, key: &str) -> String {
        url.split_once('?')
            .unwrap()
            .1
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
            .unwrap_or_default()
            .to_string()
    }

    /// Starts a sign-in, then comes back from the provider with `code`.
    async fn round_trip(
        app: &Arc<App>,
        provider: &str,
        query: &str,
        code: &str,
        session: Option<&str>,
    ) -> Response {
        let headers: Vec<(&str, &str)> = session.map(|s| ("cookie", s)).into_iter().collect();
        let started = send(
            app,
            "GET",
            &format!("/auth/oauth/{provider}{query}"),
            &headers,
            "",
        )
        .await;
        assert_eq!(started.status(), StatusCode::SEE_OTHER);
        let state = param(&location(&started), "state");
        let flow = cookie(&started, COOKIE).unwrap();
        let cookies = match session {
            Some(session) => format!("{flow}; {session}"),
            None => flow,
        };
        send(
            app,
            "GET",
            &format!("/auth/oauth/{provider}/callback?code={code}&state={state}"),
            &[("cookie", &cookies)],
            "",
        )
        .await
    }

    async fn member(app: &Arc<App>, session: &str) -> serde_json::Value {
        json(send(app, "GET", "/auth/status", &[("cookie", session)], "").await).await["member"]["user"]
            .clone()
    }

    /// Signs up through the provider as `user`, and returns the session cookie.
    async fn sign_up(app: &Arc<App>, user: &str) -> String {
        let callback = round_trip(app, "gitlab", "", "good-code", None).await;
        assert_eq!(location(&callback), SIGNUP_PATH);
        let pending = cookie(&callback, COOKIE).unwrap();
        let created = send(
            app,
            "POST",
            SIGNUP_PATH,
            &[
                ("cookie", &pending),
                ("content-type", FORM),
                ("origin", ORIGIN),
            ],
            &format!("user={user}"),
        )
        .await;
        assert_eq!(created.status(), StatusCode::SEE_OTHER);
        cookie(&created, "ankiquest_session").unwrap()
    }

    async fn bearer_session(app: &Arc<App>, token: &str) -> String {
        let response = send(
            app,
            "POST",
            "/auth/session",
            &[("authorization", &format!("Bearer {token}"))],
            "",
        )
        .await;
        cookie(&response, "ankiquest_session").unwrap()
    }

    async fn change(
        app: &Arc<App>,
        user: &str,
        session: &str,
        body: serde_json::Value,
    ) -> Response {
        send(
            app,
            "POST",
            &format!("/api/sign-in/{user}"),
            &[
                ("cookie", session),
                ("x-ankiquest-csrf", "1"),
                ("content-type", "application/json"),
            ],
            &body.to_string(),
        )
        .await
    }

    #[test]
    fn pkce_challenges_follow_rfc_7636() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        let verifier = base64url(&random().unwrap());
        assert_eq!(verifier.len(), 43, "a verifier is 43 to 128 characters");
        assert_ne!(verifier, base64url(&random().unwrap()));
    }

    #[test]
    fn flows_are_single_use_expire_and_are_bounded() {
        let flows = Flows::default();
        let started = |expires| Started {
            provider: Provider::Github,
            verifier: "v".into(),
            link: None,
            next: "/".into(),
            expires,
        };
        flows.start("a".into(), started(100), 0);
        assert!(flows.take_started("a", 50).is_some());
        assert!(flows.take_started("a", 50).is_none(), "a state works once");
        flows.start("b".into(), started(100), 0);
        assert!(
            flows.take_started("b", 100).is_none(),
            "and only until it expires"
        );
        for n in 0..=MAX_FLOWS {
            flows.start(n.to_string(), started(1000 + n as i64), 0);
        }
        assert_eq!(flows.started.lock().unwrap().len(), MAX_FLOWS);
        assert!(flows.take_started("0", 1).is_none(), "the oldest gives way");
    }

    #[test]
    fn usernames_are_suggested_from_the_provider() {
        let free = |_: &str| false;
        assert_eq!(suggest(Some("Ana.López"), free), "ana-lopez");
        assert_eq!(suggest(Some("José María"), free), "jose-maria");
        assert_eq!(suggest(Some("octo_cat"), free), "octo_cat");
        assert_eq!(suggest(Some("--Ünal--"), free), "unal");
        assert_eq!(suggest(Some("李"), free), "player");
        assert_eq!(suggest(Some("Jo"), free), "player");
        assert_eq!(suggest(None, free), "player");
        assert_eq!(suggest(Some(&"x".repeat(60)), free).len(), MAX_SUGGESTION);
        assert_eq!(
            suggest(Some("admin"), free),
            "admin2",
            "reserved names are skipped"
        );
        let taken = |name: &str| ["alice", "alice2"].contains(&name);
        assert_eq!(suggest(Some("Alice"), taken), "alice3");
        assert!(valid_username(&suggest(Some(&"ab-".repeat(20)), free)));
    }

    #[test]
    fn providers_identify_people_by_their_stable_id() {
        let github = identity(
            Provider::Github,
            &serde_json::json!({"id": 4242, "login": "octo", "name": "Octo Cat"}),
        )
        .unwrap();
        assert_eq!(
            github,
            Identity {
                subject: "4242".into(),
                handle: Some("octo".into()),
                name: Some("Octo Cat".into())
            }
        );
        let google = identity(
            Provider::Google,
            &serde_json::json!({"sub": "1099", "given_name": "Ana", "name": "Ana López"}),
        )
        .unwrap();
        assert_eq!(
            (google.subject.as_str(), google.handle.as_deref()),
            ("1099", Some("Ana"))
        );
        assert!(identity(Provider::Github, &serde_json::json!({"login": "octo"})).is_err());
        assert!(identity(Provider::Gitlab, &serde_json::json!({"sub": ""})).is_err());
        assert!(
            identity(
                Provider::Google,
                &serde_json::json!({"sub": "x".repeat(300)})
            )
            .is_err()
        );
    }

    #[test]
    fn the_last_way_to_sign_in_stays() {
        assert!(!can_unlink(true, false, 0));
        assert!(can_unlink(true, true, 0));
        assert!(can_unlink(true, false, 1));
        assert!(
            can_unlink(false, false, 0),
            "configured players keep their token"
        );
    }

    #[test]
    fn only_local_pages_are_returned_to() {
        assert_eq!(local_path(Some("/today?x=1#y")), "/today?x=1#y");
        for bad in [
            "//evil.test",
            "https://evil.test",
            "/\\evil",
            "/login",
            "/auth/oauth/github",
            "/é",
            "/a\nb",
        ] {
            assert_eq!(local_path(Some(bad)), "/", "{bad}");
        }
        assert_eq!(local_path(None), "/");
    }

    #[test]
    fn secrets_come_from_files_and_need_a_public_url() {
        let dir = std::env::temp_dir().join(format!("ankiquest-oauth-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("secret");
        std::fs::write(&file, " s3cret \n").unwrap();
        let settings = |file: &std::path::Path| -> Settings {
            serde_json::from_value(
                serde_json::json!({"google": {"client_id": "id", "client_secret_file": file}}),
            )
            .unwrap()
        };
        let mut loaded = settings(&file);
        loaded.load_secrets(Some(ORIGIN)).unwrap();
        assert_eq!(
            loaded.client(Provider::Google).unwrap().secret(),
            Some("s3cret")
        );
        assert_eq!(loaded.providers(), [Provider::Google]);
        assert!(settings(&file).load_secrets(None).is_err());
        std::fs::write(&file, "\n").unwrap();
        assert!(settings(&file).load_secrets(Some(ORIGIN)).is_err());
        assert!(
            settings(&dir.join("missing"))
                .load_secrets(Some(ORIGIN))
                .is_err()
        );
        let mut none = Settings::default();
        none.load_secrets(None).unwrap();
        assert!(none.providers().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_first_sign_in_creates_an_account_and_later_ones_sign_into_it() {
        let (base, mock) = provider().await;
        let (app, path) = fixture(true, &base);
        let status = json(send(&app, "GET", "/auth/status", &[], "").await).await;
        assert_eq!(status["oauth"], serde_json::json!(["github", "gitlab"]));

        let started = send(&app, "GET", "/auth/oauth/gitlab?next=%2Ftoday", &[], "").await;
        assert_eq!(started.status(), StatusCode::SEE_OTHER);
        let url = location(&started);
        assert!(
            url.starts_with(&format!("{base}/oauth/authorize?")),
            "{url}"
        );
        assert_eq!(param(&url, "client_id"), "gitlab-app");
        assert_eq!(
            param(&url, "redirect_uri"),
            "https%3A%2F%2Fanki.example.test%2Fauth%2Foauth%2Fgitlab%2Fcallback"
        );
        assert_eq!(param(&url, "scope"), "openid%20profile");
        assert_eq!(param(&url, "code_challenge_method"), "S256");
        let set = started.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(set.contains("HttpOnly") && set.contains("SameSite=Lax") && set.contains("Secure"));
        let flow = cookie(&started, COOKIE).unwrap();
        let callback = send(
            &app,
            "GET",
            &format!(
                "/auth/oauth/gitlab/callback?code=good-code&state={}",
                param(&url, "state")
            ),
            &[("cookie", &flow)],
            "",
        )
        .await;
        assert_eq!(callback.status(), StatusCode::SEE_OTHER);
        assert_eq!(location(&callback), SIGNUP_PATH);
        let sent = mock.requests.lock().unwrap()[0].clone();
        assert_eq!(
            challenge(&sent["code_verifier"]),
            param(&url, "code_challenge")
        );
        assert_eq!(
            sent["redirect_uri"],
            format!("{ORIGIN}/auth/oauth/gitlab/callback")
        );
        assert_eq!(
            (
                sent["grant_type"].as_str(),
                sent["client_id"].as_str(),
                sent["client_secret"].as_str()
            ),
            ("authorization_code", "gitlab-app", "gitlab-secret")
        );

        let pending = cookie(&callback, COOKIE).unwrap();
        assert_ne!(pending, flow);
        let form = send(&app, "GET", SIGNUP_PATH, &[("cookie", &pending)], "").await;
        assert_eq!(form.status(), StatusCode::OK);
        let form = text(form).await;
        assert!(
            form.contains(r#"value="ana-lopez""#) && form.contains(r#"value="Ana López""#),
            "{form}"
        );
        assert!(
            !app.accounts.contains("ana-lopez"),
            "nothing is created before the person confirms"
        );

        let headers = [
            ("cookie", pending.as_str()),
            ("content-type", FORM),
            ("origin", ORIGIN),
        ];
        let created = send(
            &app,
            "POST",
            SIGNUP_PATH,
            &headers,
            "user=Ana&display=Ana+L.",
        )
        .await;
        assert_eq!(created.status(), StatusCode::SEE_OTHER);
        assert_eq!(location(&created), "/today");
        let session = cookie(&created, "ankiquest_session").unwrap();
        assert_eq!(member(&app, &session).await, "ana");
        assert_eq!(app.display("ana"), "Ana L.");
        assert_eq!(
            send(&app, "POST", SIGNUP_PATH, &headers, "user=ana2")
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "a sign-up is used once"
        );
        assert!(!crate::accounts::has_password(&app.store.lock().unwrap().conn, "ana").unwrap());
        let account = json(
            send(
                &app,
                "GET",
                "/api/accounts/ana",
                &[("cookie", &session)],
                "",
            )
            .await,
        )
        .await;
        assert_eq!(account["password"], false);
        assert_eq!(
            send(
                &app,
                "POST",
                "/api/accounts/tokens",
                &[("content-type", "application/json")],
                r#"{"user":"ana","password":""}"#
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED,
            "an account without a password cannot sign in with an empty one"
        );

        let later = round_trip(&app, "gitlab", "?next=%2Fhistory", "good-code", None).await;
        assert_eq!(later.status(), StatusCode::SEE_OTHER);
        assert_eq!(location(&later), "/history");
        assert_eq!(
            member(&app, &cookie(&later, "ankiquest_session").unwrap()).await,
            "ana"
        );

        let export = crate::privacy::export(&app.store.lock().unwrap().conn, "ana").unwrap();
        assert_eq!(export["sign_in_links"][0]["provider"], "gitlab");
        assert_eq!(export["sign_in_links"][0]["subject"], "1001");
        let delete = |body: &'static str| {
            let app = app.clone();
            let session = session.clone();
            async move {
                send(
                    &app,
                    "DELETE",
                    "/api/accounts/ana",
                    &[
                        ("cookie", &session),
                        ("x-ankiquest-csrf", "1"),
                        ("content-type", "application/json"),
                    ],
                    body,
                )
                .await
                .status()
            }
        };
        assert_eq!(
            delete(r#"{"password":"anything"}"#).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            delete(r#"{"confirm":"bob"}"#).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(delete(r#"{"confirm":"ana"}"#).await, StatusCode::NO_CONTENT);
        let links: i64 = app
            .store
            .lock()
            .unwrap()
            .conn
            .query_row("select count(*) from sign_in_links", [], |r| r.get(0))
            .unwrap();
        assert_eq!(links, 0, "links go with the account");
        let fresh = round_trip(&app, "gitlab", "", "good-code", None).await;
        assert_eq!(
            location(&fresh),
            SIGNUP_PATH,
            "the identity can sign up again"
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_callback_needs_the_state_and_cookie_of_its_own_flow() {
        let (base, _mock) = provider().await;
        let (app, path) = fixture(true, &base);
        let start = || send(&app, "GET", "/auth/oauth/gitlab", &[], "");
        let callback = |query: String, cookie: String| {
            let app = app.clone();
            async move {
                send(
                    &app,
                    "GET",
                    &format!("/auth/oauth/gitlab/callback?{query}"),
                    &[("cookie", &cookie)],
                    "",
                )
                .await
            }
        };
        let started = start().await;
        let (state, flow) = (
            param(&location(&started), "state"),
            cookie(&started, COOKIE).unwrap(),
        );
        let other = start().await;
        let other_flow = cookie(&other, COOKIE).unwrap();
        assert_eq!(
            callback(
                format!("code=good-code&state={}", "0".repeat(64)),
                flow.clone()
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            callback(format!("code=good-code&state={state}"), String::new())
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            callback(format!("code=good-code&state={state}"), other_flow)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            callback(
                format!("code=good-code&state={state}"),
                format!("{flow}; {flow}")
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        let github = send(
            &app,
            "GET",
            &format!("/auth/oauth/github/callback?code=good-code&state={state}"),
            &[("cookie", &flow)],
            "",
        )
        .await;
        assert_eq!(
            github.status(),
            StatusCode::BAD_REQUEST,
            "a state belongs to one provider"
        );
        assert_eq!(
            callback(format!("code=good-code&state={state}"), flow)
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "and is spent by any callback"
        );

        let started = start().await;
        let (state, flow) = (
            param(&location(&started), "state"),
            cookie(&started, COOKIE).unwrap(),
        );
        let cancelled = callback(format!("error=access_denied&state={state}"), flow).await;
        assert_eq!(cancelled.status(), StatusCode::OK);
        assert!(text(cancelled).await.contains("Sign-in was cancelled."));

        let started = start().await;
        let (state, flow) = (
            param(&location(&started), "state"),
            cookie(&started, COOKIE).unwrap(),
        );
        assert_eq!(
            callback(format!("code=bad-code&state={state}"), flow)
                .await
                .status(),
            StatusCode::BAD_GATEWAY
        );

        let started = start().await;
        let (state, flow) = (
            param(&location(&started), "state"),
            cookie(&started, COOKIE).unwrap(),
        );
        assert_eq!(
            callback(format!("code=good-code&state={state}"), flow.clone())
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(
            callback(format!("code=good-code&state={state}"), flow)
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "no replays"
        );

        for unknown in [
            "/auth/oauth/google",
            "/auth/oauth/nope",
            "/auth/oauth/google/callback?code=a&state=b",
        ] {
            assert_eq!(
                send(&app, "GET", unknown, &[], "").await.status(),
                StatusCode::NOT_FOUND,
                "{unknown}"
            );
        }
        cleanup(app, path);
    }

    #[tokio::test]
    async fn the_chosen_username_follows_the_sign_up_rules() {
        let (base, _mock) = provider().await;
        let (app, path) = fixture(true, &base);
        let callback = round_trip(&app, "gitlab", "", "good-code", None).await;
        let pending = cookie(&callback, COOKIE).unwrap();
        let post = |body: &'static str, origin: &'static str, cookie: String| {
            let app = app.clone();
            async move {
                let response = send(
                    &app,
                    "POST",
                    SIGNUP_PATH,
                    &[
                        ("cookie", &cookie),
                        ("content-type", FORM),
                        ("origin", origin),
                    ],
                    body,
                )
                .await;
                (response.status(), text(response).await)
            }
        };
        let (status, page) = post("user=alice", ORIGIN, pending.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "configured players are taken");
        assert!(page.contains("That username is taken.") && page.contains(r#"value="alice""#));
        assert_eq!(
            post("user=hill", ORIGIN, pending.clone()).await.0,
            StatusCode::CONFLICT,
            "so are devs"
        );
        assert_eq!(
            post("user=ab", ORIGIN, pending.clone()).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post("user=admin", ORIGIN, pending.clone()).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post("user=fine&display=a%07b", ORIGIN, pending.clone())
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post("user=fine", "https://evil.test", pending.clone())
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            post("user=fine", ORIGIN, String::new()).await.0,
            StatusCode::BAD_REQUEST
        );
        let page = send(&app, "GET", SIGNUP_PATH, &[], "").await;
        assert_eq!(location(&page), "/login");

        let alice = bearer_session(&app, "alice-token").await;
        let signed_in_elsewhere = send(
            &app,
            "POST",
            SIGNUP_PATH,
            &[
                ("cookie", &format!("{pending}; {alice}")),
                ("content-type", FORM),
                ("origin", ORIGIN),
            ],
            "user=fine",
        )
        .await;
        assert_eq!(
            signed_in_elsewhere.status(),
            StatusCode::SEE_OTHER,
            "a browser signed in as someone else can still finish the form"
        );
        assert_eq!(
            member(
                &app,
                &cookie(&signed_in_elsewhere, "ankiquest_session").unwrap()
            )
            .await,
            "fine"
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn players_link_and_unlink_providers_and_keep_a_way_in() {
        let (base, mock) = provider().await;
        let (app, path) = fixture(true, &base);
        assert_eq!(
            location(&send(&app, "GET", "/auth/oauth/github?link=1", &[], "").await),
            "/login?next=%2Fsettings",
            "linking needs a signed-in player"
        );
        let alice = bearer_session(&app, "alice-token").await;
        let linked = round_trip(&app, "github", "?link=1", "good-code", Some(&alice)).await;
        assert_eq!(linked.status(), StatusCode::SEE_OTHER);
        assert_eq!(location(&linked), "/settings");
        let methods = json(
            send(
                &app,
                "GET",
                "/api/sign-in/alice",
                &[("authorization", "Bearer alice-token")],
                "",
            )
            .await,
        )
        .await;
        assert_eq!(
            methods,
            serde_json::json!({"account": false, "password": false, "providers": [
                {"provider": "github", "name": "GitHub", "linked": true},
                {"provider": "gitlab", "name": "GitLab", "linked": false}
            ]})
        );
        assert_eq!(
            send(
                &app,
                "GET",
                "/api/sign-in/alice",
                &[("authorization", "Bearer wrong")],
                ""
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
        let signed_in = round_trip(&app, "github", "", "good-code", None).await;
        assert_eq!(location(&signed_in), "/");
        assert_eq!(
            member(&app, &cookie(&signed_in, "ankiquest_session").unwrap()).await,
            "alice"
        );

        let created = send(
            &app,
            "POST",
            "/api/accounts",
            &[("content-type", "application/json")],
            r#"{"user":"ana","password":"long enough pw"}"#,
        )
        .await;
        let ana_token = json(created).await["token"].as_str().unwrap().to_string();
        let ana = bearer_session(&app, &ana_token).await;
        let taken = round_trip(&app, "github", "?link=1", "good-code", Some(&ana)).await;
        assert_eq!(
            taken.status(),
            StatusCode::CONFLICT,
            "one identity, one player"
        );
        assert_eq!(
            location(&round_trip(&app, "gitlab", "?link=1", "good-code", Some(&ana)).await),
            "/settings"
        );
        *mock.subject.lock().unwrap() = "1002".into();
        let second = round_trip(&app, "gitlab", "?link=1", "good-code", Some(&ana)).await;
        assert_eq!(
            second.status(),
            StatusCode::CONFLICT,
            "one link per provider"
        );
        let unlinked = change(
            &app,
            "ana",
            &ana,
            serde_json::json!({"action": "unlink", "provider": "gitlab"}),
        )
        .await;
        assert_eq!(
            unlinked.status(),
            StatusCode::OK,
            "ana still has her password"
        );
        assert_eq!(json(unlinked).await["providers"][1]["linked"], false);
        assert_eq!(
            change(
                &app,
                "ana",
                &ana,
                serde_json::json!({"action": "unlink", "provider": "gitlab"})
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            change(
                &app,
                "alice",
                &ana,
                serde_json::json!({"action": "unlink", "provider": "github"})
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED,
            "only the player changes their own sign-in"
        );

        *mock.subject.lock().unwrap() = "2000".into();
        let bo = sign_up(&app, "bo1").await;
        let last = change(
            &app,
            "bo1",
            &bo,
            serde_json::json!({"action": "unlink", "provider": "gitlab"}),
        )
        .await;
        assert_eq!(last.status(), StatusCode::CONFLICT, "the only way in stays");
        let made = change(
            &app,
            "bo1",
            &bo,
            serde_json::json!({"action": "token", "device": "Anki"}),
        )
        .await;
        assert_eq!(made.status(), StatusCode::CREATED);
        let token = json(made).await["token"].as_str().unwrap().to_string();
        assert_eq!(app.accounts.token_owner(&token).as_deref(), Some("bo1"));
        assert_eq!(
            change(
                &app,
                "alice",
                &alice,
                serde_json::json!({"action": "token"})
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
        let unlinked = change(
            &app,
            "alice",
            &alice,
            serde_json::json!({"action": "unlink", "provider": "github"}),
        )
        .await;
        assert_eq!(
            unlinked.status(),
            StatusCode::OK,
            "configured players keep their token"
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn without_registration_only_linked_identities_sign_in() {
        let (base, _mock) = provider().await;
        let (app, path) = fixture(false, &base);
        let closed = round_trip(&app, "gitlab", "", "good-code", None).await;
        assert_eq!(closed.status(), StatusCode::FORBIDDEN);
        assert!(text(closed).await.contains("does not take new accounts"));
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "insert into sign_in_links (provider, subject, user, created_at) values ('gitlab', '1001', 'alice', 0), ('github', '4242', 'ghost', 0)",
                [],
            )
            .unwrap();
        let signed_in = round_trip(&app, "gitlab", "", "good-code", None).await;
        assert_eq!(signed_in.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            member(&app, &cookie(&signed_in, "ankiquest_session").unwrap()).await,
            "alice"
        );
        let ghost = round_trip(&app, "github", "", "good-code", None).await;
        assert_eq!(
            ghost.status(),
            StatusCode::FORBIDDEN,
            "a link to a removed player signs no one in"
        );
        let left: i64 = app
            .store
            .lock()
            .unwrap()
            .conn
            .query_row(
                "select count(*) from sign_in_links where user = 'ghost'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            left, 1,
            "a lookup leaves the link for the account's own cleanup"
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn servers_without_providers_say_nothing_about_them() {
        let (store, path) = crate::decks::tests::temporary_store();
        let config = serde_json::from_value(
            serde_json::json!({"users": {"alice": {"token": "alice-token"}}}),
        )
        .unwrap();
        let app = Arc::new(App {
            config,
            access: crate::access::Access::default(),
            accounts: Default::default(),
            week: crate::game::Week::default(),
            store: Mutex::new(store),
            players: RwLock::new(HashMap::new()),
            standings: Default::default(),
        });
        let status = json(send(&app, "GET", "/auth/status", &[], "").await).await;
        assert!(status.get("oauth").is_none(), "{status}");
        assert_eq!(
            send(&app, "GET", "/auth/oauth/github", &[], "")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        let methods = json(
            send(
                &app,
                "GET",
                "/api/sign-in/alice",
                &[("authorization", "Bearer alice-token")],
                "",
            )
            .await,
        )
        .await;
        assert_eq!(methods["providers"], serde_json::json!([]));
        cleanup(app, path);
    }
}
