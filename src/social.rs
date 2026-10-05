//! Who sees and reaches whom: groups joined by invite, mutual friends, and opt-in public profiles.
//! Everyone who is not a self-service account (configured and sync-folder players) keeps
//! sharing the original community, so existing deployments behave as before.

use crate::{App, Error, now_ms};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

pub const MAX_GROUPS: i64 = 20;
pub const MAX_MEMBERS: i64 = 200;
pub const MAX_PENDING: i64 = 50;
const MAX_GROUP_NAME: usize = 40;

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists groups (
             id integer primary key autoincrement,
             name text not null,
             invite text not null unique,
             created_by text not null,
             created_at integer not null
         );
         create table if not exists group_members (
             group_id integer not null references groups (id) on delete cascade,
             user text not null,
             owner integer not null default 0,
             joined_at integer not null,
             primary key (group_id, user)
         ) without rowid;
         create index if not exists group_members_user on group_members (user);
         create table if not exists friendships (
             a text not null,
             b text not null,
             requested_by text not null,
             accepted integer not null default 0,
             created_at integer not null,
             primary key (a, b),
             check (a < b)
         ) without rowid;
         create index if not exists friendships_b on friendships (b);
         create table if not exists visibility (
             user text primary key,
             public integer not null
         ) without rowid;",
    )?;
    Ok(())
}

fn pair<'a>(x: &'a str, y: &'a str) -> (&'a str, &'a str) {
    if x < y { (x, y) } else { (y, x) }
}

pub fn friends(conn: &Connection, user: &str) -> rusqlite::Result<BTreeSet<String>> {
    conn.prepare(
        "select case when a = ?1 then b else a end from friendships
             where accepted = 1 and (a = ?1 or b = ?1)",
    )?
    .query_map([user], |r| r.get(0))?
    .collect::<Result<_, _>>()
}

pub fn group_mates(conn: &Connection, user: &str) -> rusqlite::Result<BTreeSet<String>> {
    conn.prepare(
        "select distinct other.user from group_members mine
             join group_members other on other.group_id = mine.group_id
             where mine.user = ?1",
    )?
    .query_map([user], |r| r.get(0))?
    .collect::<Result<_, _>>()
}

pub fn group_members(conn: &Connection, group: i64) -> rusqlite::Result<BTreeSet<String>> {
    conn.prepare("select user from group_members where group_id = ?1")?
        .query_map([group], |r| r.get(0))?
        .collect::<Result<_, _>>()
}

fn is_member(conn: &Connection, group: i64, user: &str) -> rusqlite::Result<bool> {
    conn.prepare("select 1 from group_members where group_id = ?1 and user = ?2")?
        .exists(params![group, user])
}

fn public_flag(conn: &Connection, user: &str) -> rusqlite::Result<Option<bool>> {
    conn.query_row(
        "select public from visibility where user = ?1",
        [user],
        |r| r.get(0),
    )
    .optional()
}

/// Who is asking: a signed-in player, the shared community password, or nobody.
pub struct Viewer {
    pub member: Option<String>,
    pub community: bool,
}

/// Which players a board covers.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Circle,
    Global,
    Group(i64),
}

impl Scope {
    pub fn parse(value: Option<&str>) -> Option<Option<Scope>> {
        match value {
            None | Some("") => Some(None),
            Some("circle") => Some(Some(Scope::Circle)),
            Some("global") => Some(Some(Scope::Global)),
            Some(other) => other
                .strip_prefix("group:")
                .and_then(|id| id.parse().ok())
                .map(|id| Some(Scope::Group(id))),
        }
    }
}

impl App {
    /// Players from before accounts existed: configured and sync-folder players.
    pub fn legacy(&self, user: &str) -> bool {
        !self.accounts.contains(user)
    }

    pub fn known_users(&self) -> BTreeSet<String> {
        self.config
            .users
            .keys()
            .cloned()
            .chain(self.players.read().unwrap().keys().cloned())
            .chain(self.accounts.users())
            .collect()
    }

    pub fn is_public(&self, conn: &Connection, user: &str) -> rusqlite::Result<bool> {
        Ok(public_flag(conn, user)?.unwrap_or_else(|| self.legacy(user)))
    }

    /// The people a player shares something with, including the player.
    pub fn circle(&self, conn: &Connection, user: &str) -> rusqlite::Result<BTreeSet<String>> {
        let mut circle = friends(conn, user)?;
        circle.extend(group_mates(conn, user)?);
        if self.legacy(user) {
            circle.extend(
                self.known_users()
                    .into_iter()
                    .filter(|other| self.legacy(other)),
            );
        }
        circle.insert(user.to_string());
        Ok(circle)
    }

    pub fn connected(&self, conn: &Connection, a: &str, b: &str) -> rusqlite::Result<bool> {
        Ok(a == b || self.circle(conn, a)?.contains(b))
    }

    /// Everyone who chose, or by default has, a public profile.
    pub fn global(&self, conn: &Connection) -> rusqlite::Result<BTreeSet<String>> {
        let mut users = BTreeSet::new();
        for user in self.known_users() {
            if self.is_public(conn, &user)? {
                users.insert(user);
            }
        }
        Ok(users)
    }

    pub fn visible(
        &self,
        conn: &Connection,
        viewer: &Viewer,
        target: &str,
    ) -> rusqlite::Result<bool> {
        if viewer.member.as_deref() == Some(target) || self.is_public(conn, target)? {
            return Ok(true);
        }
        if viewer.community && self.legacy(target) {
            return Ok(true);
        }
        match &viewer.member {
            Some(member) => self.connected(conn, member, target),
            None => Ok(false),
        }
    }

    /// The players on a board for this viewer, or None when the scope is not theirs to see.
    pub fn board(
        &self,
        conn: &Connection,
        viewer: &Viewer,
        scope: Option<Scope>,
    ) -> rusqlite::Result<Option<BTreeSet<String>>> {
        let scope = scope.unwrap_or(if viewer.member.is_some() {
            Scope::Circle
        } else {
            Scope::Global
        });
        Ok(match (scope, &viewer.member) {
            (Scope::Global, _) => {
                let mut users = self.global(conn)?;
                if viewer.community {
                    users.extend(self.known_users().into_iter().filter(|u| self.legacy(u)));
                }
                Some(users)
            }
            (Scope::Circle, Some(member)) => Some(self.circle(conn, member)?),
            (Scope::Circle, None) => None,
            (Scope::Group(group), Some(member)) => {
                if is_member(conn, group, member)? {
                    Some(group_members(conn, group)?)
                } else {
                    None
                }
            }
            (Scope::Group(_), None) => None,
        })
    }
}

fn new_invite() -> Result<String, Error> {
    let mut random = [0u8; 12];
    getrandom::fill(&mut random).map_err(|_| "invite randomness unavailable")?;
    Ok(random.iter().map(|b| format!("{b:02x}")).collect())
}

fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()
        && name.chars().count() <= MAX_GROUP_NAME
        && !name.chars().any(char::is_control))
    .then(|| name.to_string())
}

#[derive(Debug, Serialize)]
pub struct Person {
    pub user: String,
    pub display: String,
}

#[derive(Debug, Serialize)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub owner: bool,
    /// Only owners see the invite code.
    pub invite: Option<String>,
    pub members: Vec<Person>,
}

#[derive(Debug, Serialize)]
pub struct Overview {
    pub groups: Vec<Group>,
    pub friends: Vec<Person>,
    pub incoming: Vec<Person>,
    pub outgoing: Vec<Person>,
    pub public: bool,
}

fn person(app: &App, user: String) -> Person {
    Person {
        display: app.display(&user),
        user,
    }
}

fn overview(app: &App, conn: &Connection, user: &str) -> rusqlite::Result<Overview> {
    let rows: Vec<(i64, String, String, bool)> = conn
        .prepare(
            "select g.id, g.name, g.invite, m.owner from groups g
             join group_members m on m.group_id = g.id where m.user = ?1 order by g.name, g.id",
        )?
        .query_map([user], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    let mut groups = Vec::new();
    for (id, name, invite, owner) in rows {
        let mut members: Vec<Person> = group_members(conn, id)?
            .into_iter()
            .map(|u| person(app, u))
            .collect();
        members.sort_by(|a, b| a.display.cmp(&b.display));
        groups.push(Group {
            id,
            name,
            owner,
            invite: owner.then_some(invite),
            members,
        });
    }
    let requests = |incoming: bool| -> rusqlite::Result<Vec<Person>> {
        let mut people: Vec<Person> = conn
            .prepare(
                "select case when a = ?1 then b else a end from friendships
                 where accepted = 0 and (a = ?1 or b = ?1) and (requested_by = ?1) = ?2",
            )?
            .query_map(params![user, !incoming], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|u| person(app, u))
            .collect();
        people.sort_by(|a, b| a.display.cmp(&b.display));
        Ok(people)
    };
    let mut friend_list: Vec<Person> = friends(conn, user)?
        .into_iter()
        .map(|u| person(app, u))
        .collect();
    friend_list.sort_by(|a, b| a.display.cmp(&b.display));
    Ok(Overview {
        groups,
        friends: friend_list,
        incoming: requests(true)?,
        outgoing: requests(false)?,
        public: app.is_public(conn, user)?,
    })
}

fn refuse(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({"error": message}))).into_response()
}

fn failed(error: impl std::fmt::Display) -> Response {
    log_error!("social storage failed: {error}");
    refuse(
        StatusCode::INTERNAL_SERVER_ERROR,
        "That could not be saved. Please try again.",
    )
}

fn respond(app: &App, conn: &Connection, user: &str) -> Response {
    match overview(app, conn, user) {
        Ok(overview) => Json(overview).into_response(),
        Err(error) => failed(error),
    }
}

async fn get_overview(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !crate::access::authorized(&app, &user, &headers) {
        return refuse(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    respond(&app, &app.store.lock().unwrap().conn, &user)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Change {
    CreateGroup { name: String },
    RenameGroup { group: i64, name: String },
    NewInvite { group: i64 },
    Join { invite: String },
    Leave { group: i64 },
    Remove { group: i64, member: String },
    Befriend { user: String },
    Unfriend { user: String },
    Visibility { public: bool },
}

async fn change(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(change): Json<Change>,
) -> Response {
    if !crate::access::authorized(&app, &user, &headers) {
        return refuse(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    let store = app.store.lock().unwrap();
    let conn = &store.conn;
    let now = now_ms();
    let outcome: Result<Result<(), (StatusCode, &str)>, Error> = (|| {
        Ok(match change {
            Change::CreateGroup { name } => {
                let Some(name) = clean_name(&name) else {
                    return Ok(Err((
                        StatusCode::BAD_REQUEST,
                        "Group names are 1 to 40 characters.",
                    )));
                };
                let count: i64 = conn.query_row(
                    "select count(*) from group_members where user = ?1",
                    [&user],
                    |r| r.get(0),
                )?;
                if count >= MAX_GROUPS {
                    return Ok(Err((
                        StatusCode::CONFLICT,
                        "You are in as many groups as you can be.",
                    )));
                }
                conn.execute(
                    "insert into groups (name, invite, created_by, created_at) values (?1, ?2, ?3, ?4)",
                    params![name, new_invite()?, user, now],
                )?;
                conn.execute(
                    "insert into group_members (group_id, user, owner, joined_at) values (?1, ?2, 1, ?3)",
                    params![conn.last_insert_rowid(), user, now],
                )?;
                Ok(())
            }
            Change::RenameGroup { group, name } => {
                let Some(name) = clean_name(&name) else {
                    return Ok(Err((
                        StatusCode::BAD_REQUEST,
                        "Group names are 1 to 40 characters.",
                    )));
                };
                if !owns(conn, group, &user)? {
                    return Ok(Err((
                        StatusCode::FORBIDDEN,
                        "Only the group's owner can do that.",
                    )));
                }
                conn.execute(
                    "update groups set name = ?2 where id = ?1",
                    params![group, name],
                )?;
                Ok(())
            }
            Change::NewInvite { group } => {
                if !owns(conn, group, &user)? {
                    return Ok(Err((
                        StatusCode::FORBIDDEN,
                        "Only the group's owner can do that.",
                    )));
                }
                conn.execute(
                    "update groups set invite = ?2 where id = ?1",
                    params![group, new_invite()?],
                )?;
                Ok(())
            }
            Change::Join { invite } => {
                let group: Option<i64> = conn
                    .query_row(
                        "select id from groups where invite = ?1",
                        [invite.trim()],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(group) = group else {
                    return Ok(Err((
                        StatusCode::NOT_FOUND,
                        "That invite link is not valid any more.",
                    )));
                };
                if is_member(conn, group, &user)? {
                    return Ok(Ok(()));
                }
                let mine: i64 = conn.query_row(
                    "select count(*) from group_members where user = ?1",
                    [&user],
                    |r| r.get(0),
                )?;
                let size: i64 = conn.query_row(
                    "select count(*) from group_members where group_id = ?1",
                    [group],
                    |r| r.get(0),
                )?;
                if mine >= MAX_GROUPS || size >= MAX_MEMBERS {
                    return Ok(Err((
                        StatusCode::CONFLICT,
                        "That group, or your list of groups, is full.",
                    )));
                }
                conn.execute(
                    "insert into group_members (group_id, user, owner, joined_at) values (?1, ?2, 0, ?3)",
                    params![group, user, now],
                )?;
                Ok(())
            }
            Change::Leave { group } => {
                let owner = owns(conn, group, &user)?;
                conn.execute(
                    "delete from group_members where group_id = ?1 and user = ?2",
                    params![group, user],
                )?;
                let next: Option<String> = conn
                    .query_row("select user from group_members where group_id = ?1 order by joined_at limit 1", [group], |r| r.get(0))
                    .optional()?;
                match next {
                    None => {
                        conn.execute("delete from groups where id = ?1", [group])?;
                    }
                    Some(next)
                        if owner
                            && !conn
                                .prepare(
                                    "select 1 from group_members where group_id = ?1 and owner = 1",
                                )?
                                .exists([group])? =>
                    {
                        conn.execute(
                            "update group_members set owner = 1 where group_id = ?1 and user = ?2",
                            params![group, next],
                        )?;
                    }
                    Some(_) => {}
                }
                prune(&app, conn, &user)?;
                Ok(())
            }
            Change::Remove { group, member } => {
                if !owns(conn, group, &user)? || member == user {
                    return Ok(Err((
                        StatusCode::FORBIDDEN,
                        "Only the group's owner can remove others.",
                    )));
                }
                conn.execute(
                    "delete from group_members where group_id = ?1 and user = ?2",
                    params![group, member],
                )?;
                prune(&app, conn, &member)?;
                Ok(())
            }
            Change::Befriend { user: other } => {
                let other = other.trim().to_ascii_lowercase();
                if other == user || !app.is_user(&other) {
                    return Ok(Err((
                        StatusCode::NOT_FOUND,
                        "There is nobody with that username.",
                    )));
                }
                let (a, b) = pair(&user, &other);
                let existing: Option<(String, bool)> = conn
                    .query_row(
                        "select requested_by, accepted from friendships where a = ?1 and b = ?2",
                        params![a, b],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                match existing {
                    Some((_, true)) => {}
                    Some((by, false)) if by != user => {
                        conn.execute(
                            "update friendships set accepted = 1 where a = ?1 and b = ?2",
                            params![a, b],
                        )?;
                    }
                    Some(_) => {}
                    None => {
                        let pending: i64 = conn.query_row(
                            "select count(*) from friendships where requested_by = ?1 and accepted = 0",
                            [&user],
                            |r| r.get(0),
                        )?;
                        if pending >= MAX_PENDING {
                            return Ok(Err((
                                StatusCode::CONFLICT,
                                "Wait for some friend requests to be answered first.",
                            )));
                        }
                        conn.execute(
                            "insert into friendships (a, b, requested_by, accepted, created_at) values (?1, ?2, ?3, 0, ?4)",
                            params![a, b, user, now],
                        )?;
                    }
                }
                Ok(())
            }
            Change::Unfriend { user: other } => {
                let (a, b) = pair(&user, &other);
                conn.execute(
                    "delete from friendships where a = ?1 and b = ?2",
                    params![a, b],
                )?;
                prune(&app, conn, &user)?;
                Ok(())
            }
            Change::Visibility { public } => {
                conn.execute(
                    "insert into visibility (user, public) values (?1, ?2)
                     on conflict (user) do update set public = excluded.public",
                    params![user, public],
                )?;
                Ok(())
            }
        })
    })();
    match outcome {
        Ok(Ok(())) => respond(&app, conn, &user),
        Ok(Err((status, message))) => refuse(status, message),
        Err(error) => failed(error),
    }
}

/// Stops deck notifications between people who no longer share a group or friendship.
fn prune(app: &App, conn: &Connection, user: &str) -> rusqlite::Result<()> {
    let circle = app.circle(conn, user)?;
    let pairs: Vec<(String, String)> = conn
        .prepare("select distinct owner, recipient from deck_recipients where owner = ?1 or recipient = ?1")?
        .query_map([user], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (owner, recipient) in pairs {
        let other = if owner == user { &recipient } else { &owner };
        if !circle.contains(other) {
            conn.execute(
                "delete from deck_recipients where owner = ?1 and recipient = ?2",
                params![owner, recipient],
            )?;
        }
    }
    Ok(())
}

fn owns(conn: &Connection, group: i64, user: &str) -> rusqlite::Result<bool> {
    conn.prepare("select 1 from group_members where group_id = ?1 and user = ?2 and owner = 1")?
        .exists(params![group, user])
}

/// What an invite link shows before joining: the group's name and size.
async fn invite(State(app): State<Arc<App>>, Path(code): Path<String>) -> Response {
    let store = app.store.lock().unwrap();
    let found: Result<Option<(String, i64)>, rusqlite::Error> = store
        .conn
        .query_row(
            "select g.name, count(m.user) from groups g left join group_members m on m.group_id = g.id
             where g.invite = ?1 group by g.id",
            [code.trim()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional();
    match found {
        Ok(Some((name, members))) => {
            Json(serde_json::json!({"name": name, "members": members})).into_response()
        }
        Ok(None) => refuse(
            StatusCode::NOT_FOUND,
            "That invite link is not valid any more.",
        ),
        Err(error) => failed(error),
    }
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/social/{user}", get(get_overview).post(change))
        .route("/api/invites/{code}", get(invite))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::Request;
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::{Mutex, RwLock};
    use tower::ServiceExt;

    fn fixture() -> (Arc<App>, PathBuf) {
        let (store, path) = crate::decks::tests::temporary_store();
        let config = serde_json::from_value(serde_json::json!({
            "registration": true,
            "users": {"alice": {"token": "alice-token"}, "bob": {"token": "bob-token"}}
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
    ) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(if method == "GET" {
                String::new()
            } else {
                body.to_string()
            }))
            .unwrap();
        let response = crate::router(app.clone())
            .layer(MockConnectInfo(
                "192.0.2.1:40000".parse::<SocketAddr>().unwrap(),
            ))
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    async fn sign_up(app: &Arc<App>, user: &str) -> String {
        let request = Request::builder()
            .method("POST")
            .uri("/api/accounts")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"user": user, "password": "long enough pw"}).to_string(),
            ))
            .unwrap();
        let response = crate::router(app.clone())
            .layer(MockConnectInfo(
                format!("192.0.2.{}:40000", user.len())
                    .parse::<SocketAddr>()
                    .unwrap(),
            ))
            .oneshot(request)
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn circle(app: &App, user: &str) -> Vec<String> {
        app.circle(&app.store.lock().unwrap().conn, user)
            .unwrap()
            .into_iter()
            .collect()
    }

    #[tokio::test]
    async fn configured_players_keep_their_community_and_accounts_start_alone() {
        let (app, path) = fixture();
        sign_up(&app, "ana").await;
        assert_eq!(circle(&app, "alice"), ["alice", "bob"]);
        assert_eq!(circle(&app, "ana"), ["ana"]);
        let store = app.store.lock().unwrap();
        let conn = &store.conn;
        assert!(
            app.is_public(conn, "alice").unwrap(),
            "legacy profiles stay as public as before"
        );
        assert!(
            !app.is_public(conn, "ana").unwrap(),
            "accounts start private"
        );
        let anonymous = Viewer {
            member: None,
            community: false,
        };
        assert!(!app.visible(conn, &anonymous, "ana").unwrap());
        assert!(
            app.visible(
                conn,
                &Viewer {
                    member: Some("ana".into()),
                    community: false
                },
                "ana"
            )
            .unwrap()
        );
        assert_eq!(
            app.board(conn, &anonymous, None)
                .unwrap()
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["alice", "bob"]
        );
        drop(store);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn an_invite_link_joins_a_group_whose_members_see_each_other() {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        let bo = sign_up(&app, "bobby").await;
        let (status, overview) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "create_group", "name": "Spanish club"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{overview}");
        let group = overview["groups"][0]["id"].as_i64().unwrap();
        let invite = overview["groups"][0]["invite"]
            .as_str()
            .unwrap()
            .to_string();

        let (status, preview) = call(
            &app,
            "GET",
            &format!("/api/invites/{invite}"),
            &bo,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            (status, preview["name"].as_str()),
            (StatusCode::OK, Some("Spanish club"))
        );
        let (status, joined) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "join", "invite": invite}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            joined["groups"][0]["invite"].is_null(),
            "only the owner sees the invite"
        );
        assert_eq!(circle(&app, "bobby"), ["ana", "bobby"]);
        {
            let conn = &app.store.lock().unwrap().conn;
            let viewer = Viewer {
                member: Some("bobby".into()),
                community: false,
            };
            assert!(app.visible(conn, &viewer, "ana").unwrap());
            assert_eq!(
                app.board(conn, &viewer, Some(Scope::Group(group)))
                    .unwrap()
                    .unwrap()
                    .len(),
                2
            );
            let outsider = Viewer {
                member: Some("alice".into()),
                community: false,
            };
            assert!(
                app.board(conn, &outsider, Some(Scope::Group(group)))
                    .unwrap()
                    .is_none(),
                "other groups' boards stay private"
            );
            assert!(!app.visible(conn, &outsider, "ana").unwrap());
        }

        let (status, _) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "new_invite", "group": group}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = call(
            &app,
            "GET",
            &format!("/api/invites/{invite}"),
            &bo,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "a new link retires the old one"
        );
        let (status, _) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "new_invite", "group": group}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, _) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "leave", "group": group}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, overview) = call(
            &app,
            "GET",
            "/api/social/bobby",
            &bo,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            overview["groups"][0]["owner"], true,
            "the next member inherits the group"
        );
        let (_, overview) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "leave", "group": group}),
        )
        .await;
        assert_eq!(overview["groups"].as_array().unwrap().len(), 0);
        let count: i64 = app
            .store
            .lock()
            .unwrap()
            .conn
            .query_row("select count(*) from groups", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "an empty group is deleted");
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn friends_need_both_sides_and_profiles_can_go_public() {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        let bo = sign_up(&app, "bobby").await;
        let (_, overview) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "befriend", "user": "bobby"}),
        )
        .await;
        assert_eq!(overview["outgoing"][0]["user"], "bobby");
        assert_eq!(
            circle(&app, "ana"),
            ["ana"],
            "a request alone connects nobody"
        );
        let (_, overview) = call(
            &app,
            "GET",
            "/api/social/bobby",
            &bo,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(overview["incoming"][0]["user"], "ana");
        let (_, overview) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "befriend", "user": "ana"}),
        )
        .await;
        assert_eq!(overview["friends"][0]["user"], "ana");
        assert_eq!(circle(&app, "ana"), ["ana", "bobby"]);
        let (status, _) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "befriend", "user": "nobody-here"}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        app.store
            .lock()
            .unwrap()
            .conn
            .execute("insert into deck_recipients (owner, deck_id, recipient) values ('ana', 'spanish', 'bobby')", [])
            .unwrap();
        call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "unfriend", "user": "ana"}),
        )
        .await;
        assert_eq!(circle(&app, "ana"), ["ana"]);
        let left: i64 = app
            .store
            .lock()
            .unwrap()
            .conn
            .query_row("select count(*) from deck_recipients", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "unfriending stops deck notifications");

        let (_, overview) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "visibility", "public": true}),
        )
        .await;
        assert_eq!(overview["public"], true);
        let store = app.store.lock().unwrap();
        let conn = &store.conn;
        assert!(
            app.visible(
                conn,
                &Viewer {
                    member: None,
                    community: false
                },
                "ana"
            )
            .unwrap()
        );
        assert!(app.global(conn).unwrap().contains("ana"));
        drop(store);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn only_the_owner_changes_their_social_life() {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        let (status, _) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &ana,
            serde_json::json!({"action": "visibility", "public": true}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = call(
            &app,
            "GET",
            "/api/social/alice",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    async fn study(app: &Arc<App>, user: &str, token: &str) {
        let now = now_ms();
        let reviews: Vec<_> = (0..5)
            .map(|i| serde_json::json!({"id": now - 60_000 + i, "cid": 1_000 + i, "last_ivl": 1, "time_ms": 8_000, "kind": 1}))
            .collect();
        let body = serde_json::json!({"reviews": reviews, "clock": {"offset_west_min": 0, "rollover_hour": 4}, "decks": null});
        let (status, _) = call(app, "POST", &format!("/api/reviews/{user}"), token, body).await;
        assert_eq!(status, StatusCode::OK);
    }

    fn users(board: &serde_json::Value) -> Vec<&str> {
        let mut users: Vec<&str> = board
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["user"].as_str().unwrap())
            .collect();
        users.sort();
        users
    }

    #[tokio::test]
    async fn strangers_stay_off_each_others_boards_profiles_and_lists() {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        let bo = sign_up(&app, "bobby").await;
        study(&app, "ana", &ana).await;
        study(&app, "bobby", &bo).await;
        study(&app, "alice", "alice-token").await;

        let (_, board) = call(
            &app,
            "GET",
            "/api/leaderboard",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(users(&board), ["ana"]);
        let (_, board) = call(
            &app,
            "GET",
            "/api/leaderboard?scope=global",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            users(&board),
            ["alice"],
            "the global board only lists public profiles"
        );
        let (_, board) = call(
            &app,
            "GET",
            "/api/leaderboard",
            "alice-token",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            users(&board),
            ["alice"],
            "the original community does not see new accounts"
        );
        let (status, _) = call(
            &app,
            "GET",
            "/api/leaderboard?scope=group:1",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = call(
            &app,
            "GET",
            "/api/profile/bobby",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (_, inbox) = call(
            &app,
            "GET",
            "/api/deck-copies/ana",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(inbox["friends"], serde_json::json!([]));
        let (_, nudges) = call(
            &app,
            "GET",
            "/api/friend-nudges/ana",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(nudges["friends"], serde_json::json!([]));
        let (status, _) = call(
            &app,
            "POST",
            "/api/friend-nudges/ana",
            &ana,
            serde_json::json!({"recipient": "bobby"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "befriend", "user": "bobby"}),
        )
        .await;
        call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "befriend", "user": "ana"}),
        )
        .await;
        let (_, board) = call(
            &app,
            "GET",
            "/api/leaderboard",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(users(&board), ["ana", "bobby"]);
        let (status, _) = call(
            &app,
            "GET",
            "/api/profile/bobby",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, inbox) = call(
            &app,
            "GET",
            "/api/deck-copies/ana",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(inbox["friends"][0]["user"], "bobby");
        let (_, records) = call(&app, "GET", "/api/records", &ana, serde_json::Value::Null).await;
        assert!(!records.to_string().contains("alice"));
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn scopes_parse() {
        assert_eq!(Scope::parse(None), Some(None));
        assert_eq!(Scope::parse(Some("circle")), Some(Some(Scope::Circle)));
        assert_eq!(Scope::parse(Some("group:7")), Some(Some(Scope::Group(7))));
        assert_eq!(Scope::parse(Some("group:x")), None);
        assert_eq!(Scope::parse(Some("everyone")), None);
    }
}
