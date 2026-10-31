use crate::access::authorized;
use crate::app::App;
use crate::game::Clock;
use crate::notifications::{INBOX_AGE_MS, NUDGE_ALLOWED_SQL, Outgoing, insert_notification};
use crate::store::{Error, Store, has_column};
use crate::web_util::failed_for;
use crate::worker::push_destination;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

pub const MAX_DECKS: usize = 2000;
const MAX_RECIPIENTS: usize = 100;
pub const MAX_MESSAGE: usize = 200;

/// Whether `recipient` muted or unsubscribed from `sender`; both are SQL expressions.
pub(crate) fn sender_muted_sql(recipient: &str, sender: &str) -> String {
    format!(
        "exists (select 1 from (select recipient,sender from incoming_muted_senders
                         union all select recipient,sender from sender_unsubscriptions)
                         where recipient = {recipient} and sender = {sender})"
    )
}

/// Whether `recipient` still hears deck completions from `sender`; both are SQL expressions.
pub(crate) fn completion_allowed_sql(recipient: &str, sender: &str) -> String {
    format!(
        "not exists (select 1 from incoming_settings where user = {recipient} and enabled = 0)
         and not {}",
        sender_muted_sql(recipient, sender)
    )
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    pub remaining: u64,
    pub reviewed_today: u64,
    pub day: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preference {
    pub id: String,
    pub enabled: bool,
    pub recipients: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsUpdate {
    pub decks: Vec<Preference>,
    /// Left out by clients that predate nudges, which then keep their setting.
    #[serde(default)]
    pub nudges: Option<bool>,
    #[serde(default)]
    pub celebrations: Option<bool>,
}

/// Preferences owned by the recipient, independent of what senders choose to share.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IncomingSettings {
    pub enabled: bool,
    pub muted_senders: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Deck {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub recipients: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Recipient {
    pub user: String,
    pub display: String,
}

#[derive(Debug, Serialize)]
pub struct Settings {
    pub decks: Vec<Deck>,
    pub recipients: Vec<Recipient>,
    pub nudges: bool,
    pub celebrations: bool,
}

/// A completion that was just announced, echoed back so the client can say so locally.
#[derive(Debug, Serialize)]
pub struct Announcement {
    pub deck: String,
    pub recipients: usize,
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

pub fn valid_message(message: &str) -> bool {
    valid_text(message, MAX_MESSAGE)
}

pub fn valid_incoming_settings(settings: &IncomingSettings) -> bool {
    let mut senders = HashSet::new();
    settings.muted_senders.len() <= MAX_RECIPIENTS
        && settings
            .muted_senders
            .iter()
            .all(|sender| valid_text(sender, 128) && senders.insert(sender))
}

pub fn valid_clock(clock: Clock) -> bool {
    (-1440..=1440).contains(&clock.offset_west_min) && (0..=23).contains(&clock.rollover_hour)
}

pub fn valid_snapshots(snapshots: &[Snapshot]) -> bool {
    let mut ids = HashSet::new();
    snapshots.len() <= MAX_DECKS
        && snapshots.iter().all(|deck| {
            valid_text(&deck.id, 128)
                && valid_text(&deck.name, 512)
                && ids.insert(&deck.id)
                && deck.remaining <= i64::MAX as u64
                && deck.reviewed_today <= i64::MAX as u64
        })
}

pub fn valid_preferences(
    preferences: &[Preference],
    known_decks: &[Deck],
    recipients: &[Recipient],
) -> bool {
    let deck_ids: HashSet<_> = known_decks.iter().map(|d| d.id.as_str()).collect();
    let users: HashSet<_> = recipients.iter().map(|r| r.user.as_str()).collect();
    let mut ids = HashSet::new();
    preferences.len() <= MAX_DECKS
        && preferences.iter().all(|deck| {
            let unique: HashSet<_> = deck.recipients.iter().collect();
            valid_text(&deck.id, 128)
                && ids.insert(&deck.id)
                && deck_ids.contains(deck.id.as_str())
                && deck.recipients.len() <= MAX_RECIPIENTS
                && unique.len() == deck.recipients.len()
                && (!deck.enabled || !deck.recipients.is_empty())
                && deck
                    .recipients
                    .iter()
                    .all(|user| users.contains(user.as_str()))
        })
}

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists decks (
             owner text not null,
             id text not null,
             name text not null,
             enabled integer not null default 0,
             primary key (owner, id)
         ) without rowid;
         create table if not exists deck_recipients (
             owner text not null,
             deck_id text not null,
             recipient text not null,
             primary key (owner, deck_id, recipient)
         ) without rowid;
         create table if not exists deck_completions (
             owner text not null,
             deck_id text not null,
             day integer not null,
             primary key (owner, deck_id, day)
         ) without rowid;
         create table if not exists notifications (
             id integer primary key autoincrement,
             recipient text not null,
             title text not null,
             body text not null,
             day integer not null,
             created_at integer not null,
             pushed integer not null default 0
         );
         create index if not exists notifications_recipient on notifications (recipient, id);
         create table if not exists player_settings (
             user text primary key,
             nudges integer not null default 0,
             friend_nudges integer not null default 0
         ) without rowid;
         create table if not exists incoming_settings (
             user text primary key,
             enabled integer not null default 1
         ) without rowid;
         create table if not exists incoming_muted_senders (
             recipient text not null,
             sender text not null,
             primary key (recipient, sender)
         ) without rowid;
         create table if not exists friend_nudge_muted_senders (
             recipient text not null,
             sender text not null,
             primary key (recipient, sender)
         ) without rowid;",
    )?;
    // Databases from before replies exist in the wild; adding the columns is the migration.
    for (column, definition) in [
        ("sender", "text not null default ''"),
        ("replied", "integer not null default 0"),
        ("kind", "text not null default 'message'"),
        ("push_only", "integer not null default 0"),
        ("retry_at", "integer not null default 0"),
        ("push_attempts", "integer not null default 0"),
        ("push_cancelled", "integer not null default 0"),
        ("completion_cancelled", "integer not null default 0"),
        ("read_at", "integer"),
        ("challenge_id", "integer"),
        ("reply_to_title", "text"),
        ("reply_to_body", "text"),
        ("topic", "text"),
        ("win_id", "integer"),
    ] {
        if !has_column(conn, "notifications", column)? {
            conn.execute(
                &format!("alter table notifications add column {column} {definition}"),
                [],
            )?;
        }
    }
    if !has_column(conn, "player_settings", "celebrations")? {
        conn.execute(
            "alter table player_settings add column celebrations integer not null default 1",
            [],
        )?;
    }
    if !has_column(conn, "player_settings", "friend_nudges")? {
        // Add the column and copy the prior choice together: a restart must not
        // mistake a half-migrated database for a new user's opt-out.
        conn.execute_batch(
            "begin immediate;
             alter table player_settings add column friend_nudges integer not null default 0;
             update player_settings set friend_nudges = nudges;
             commit;",
        )?;
    }
    conn.execute(
        "create index if not exists notifications_pending on notifications (pushed, retry_at, id)",
        [],
    )?;
    conn.execute(
        "create index if not exists notifications_sender on notifications (sender, id)",
        [],
    )?;
    // Older servers left nudges queued after opt-out. Retain their inbox history
    // without resuming those pushes when the player next enables nudges.
    conn.execute(
        &format!(
            "update notifications set push_cancelled = 1
             where pushed = 0 and push_cancelled = 0 and kind = 'nudge'
             and not {NUDGE_ALLOWED_SQL}"
        ),
        [],
    )?;
    Ok(())
}

impl Store {
    pub fn incoming_settings(&self, user: &str) -> Result<IncomingSettings, Error> {
        let enabled = self
            .conn
            .query_row(
                "select enabled from incoming_settings where user = ?1",
                [user],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(true);
        let muted_senders = self
            .conn
            .prepare(
                "select sender from incoming_muted_senders where recipient = ?1 order by sender",
            )?
            .query_map([user], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(IncomingSettings {
            enabled,
            muted_senders,
        })
    }

    /// Return only sender identities that this recipient already knows about.
    /// Persisted mutes remain editable after a sender stops sharing or history expires.
    pub fn known_incoming_senders(&self, user: &str) -> Result<Vec<String>, Error> {
        Ok(self
            .conn
            .prepare(
                "select owner as sender from deck_recipients where recipient = ?1 and owner != ?1
                 union select sender from notifications where recipient = ?1 and kind = 'completion'
                     and sender not in ('', ?1) and created_at >= ?2
                 union select sender from incoming_muted_senders where recipient = ?1 and sender != ?1
                 order by sender",
            )?
            .query_map(params![user, crate::app::now_ms() - INBOX_AGE_MS], |r| r.get(0))?
            .collect::<Result<_, _>>()?)
    }

    /// Suppression consumes existing history as well as pending pushes. Re-enabling
    /// affects future completions; it never revives announcements cancelled here.
    pub fn set_incoming_settings(
        &mut self,
        user: &str,
        settings: &IncomingSettings,
    ) -> Result<(), Error> {
        if !valid_incoming_settings(settings) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid incoming notification settings",
            )
            .into());
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "insert into incoming_settings (user, enabled) values (?1, ?2)
             on conflict (user) do update set enabled = excluded.enabled",
            params![user, settings.enabled],
        )?;
        tx.execute(
            "delete from incoming_muted_senders where recipient = ?1",
            [user],
        )?;
        for sender in &settings.muted_senders {
            tx.execute(
                "insert into incoming_muted_senders (recipient, sender) values (?1, ?2)",
                params![user, sender],
            )?;
        }
        tx.execute(
            "update notifications set completion_cancelled = 1, push_cancelled = 1
             where recipient = ?1 and kind = 'completion'
             and (?2 = 0 or exists (select 1 from incoming_muted_senders
                                   where recipient = ?1 and sender = notifications.sender))",
            params![user, settings.enabled],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn decks(&self, user: &str) -> Result<Vec<Deck>, Error> {
        let mut stmt = self
            .conn
            .prepare("select id, name, enabled from decks where owner = ?1 order by name, id")?;
        let mut decks = stmt
            .query_map([user], |r| {
                Ok(Deck {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    enabled: r.get(2)?,
                    recipients: Vec::new(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut stmt = self.conn.prepare(
            "select recipient from deck_recipients where owner = ?1 and deck_id = ?2 order by recipient",
        )?;
        for deck in &mut decks {
            deck.recipients = stmt
                .query_map(params![user, deck.id], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            // No active recipients means sharing is off in the editor. Keep the
            // stored switch so a recipient can restore an unchanged selection.
            deck.enabled &= !deck.recipients.is_empty();
        }
        Ok(decks)
    }

    pub fn set_deck_preferences(
        &mut self,
        user: &str,
        preferences: &[Preference],
    ) -> Result<(), Error> {
        let current: HashMap<_, _> = self
            .decks(user)?
            .into_iter()
            .map(|deck| (deck.id.clone(), deck))
            .collect();
        let tx = self.conn.transaction()?;
        for deck in preferences {
            // Clients save the whole form, including untouched decks. Preserve
            // detached selections (and the stored switch) for those decks.
            if current.get(&deck.id).is_some_and(|saved| {
                saved.enabled == deck.enabled
                    && saved.recipients.len() == deck.recipients.len()
                    && saved.recipients.iter().collect::<HashSet<_>>()
                        == deck.recipients.iter().collect::<HashSet<_>>()
            }) {
                continue;
            }
            // Explicit sender edits replace remembered selections too. A later
            // resubscribe must not undo a newer choice made by the sender.
            tx.execute(
                "delete from unsubscribed_decks where sender=?1 and deck_id=?2",
                params![user, deck.id],
            )?;
            tx.execute(
                "update decks set enabled = ?3 where owner = ?1 and id = ?2",
                params![user, deck.id, deck.enabled],
            )?;
            tx.execute(
                "delete from deck_recipients where owner = ?1 and deck_id = ?2",
                params![user, deck.id],
            )?;
            for recipient in &deck.recipients {
                tx.execute(
                    "insert into deck_recipients (owner, deck_id, recipient) values (?1, ?2, ?3)",
                    params![user, deck.id, recipient],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Save discovery and fan-out atomically. A completed baseline is consumed even
    /// when sharing is off or the upload is silent, so opt-in cannot publish history.
    pub fn record_decks(
        &mut self,
        user: &str,
        display: &str,
        snapshots: &[Snapshot],
        clock: Clock,
        silent: bool,
        now_ms: i64,
    ) -> Result<Vec<Announcement>, Error> {
        let today = clock.day(now_ms);
        let mut announced = Vec::new();
        let mut candidates = Vec::new();
        let tx = self.conn.transaction()?;
        for deck in snapshots {
            // Delayed and future snapshots cannot rename a deck or complete today.
            if deck.day != today {
                continue;
            }
            tx.execute(
                "insert into decks (owner, id, name) values (?1, ?2, ?3)
                 on conflict (owner, id) do update set name = excluded.name",
                params![user, deck.id, deck.name],
            )?;
            // A restored recipient may have missed snapshots while sharing was
            // off. Only fresh work (or a later Anki day) ends that baseline.
            tx.execute(
                "delete from subscription_baselines where sender=?1 and deck_id=?2
                        and (day!=?3 or ?4)",
                params![
                    user,
                    deck.id,
                    today,
                    deck.remaining > 0 || deck.reviewed_today == 0
                ],
            )?;
            if deck.remaining != 0 || deck.reviewed_today == 0 {
                continue;
            }
            let fresh = tx.execute(
                "insert or ignore into deck_completions (owner, deck_id, day) values (?1, ?2, ?3)",
                params![user, deck.id, today],
            )? > 0;
            let enabled: bool = tx.query_row(
                "select enabled from decks where owner = ?1 and id = ?2",
                params![user, deck.id],
                |r| r.get(0),
            )?;
            if !fresh || silent || !enabled {
                continue;
            }
            let recipients: HashSet<String> = tx
                .prepare(&format!(
                    "select recipient from deck_recipients
                     where owner = ?1 and deck_id = ?2 and recipient != ?1
                     and not exists (select 1 from subscription_baselines b
                                     where b.sender=?1 and b.deck_id=?2 and b.recipient=deck_recipients.recipient and b.day=?3)
                     and {}",
                    completion_allowed_sql("deck_recipients.recipient", "?1")
                ))?
                .query_map(params![user, deck.id, today], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            candidates.push((deck, recipients));
        }

        // Clients include subdeck reviews in their parents' totals. Equal totals
        // therefore represent the same studied work. Coalesce only fresh eligible
        // completions in this upload, separately for each recipient.
        let mut by_name_and_reviews: HashMap<(&str, u64), Vec<usize>> = HashMap::new();
        let mut deliveries = Vec::new();
        for (index, (deck, recipients)) in candidates.iter().enumerate() {
            by_name_and_reviews
                .entry((&deck.name, deck.reviewed_today))
                .or_default()
                .push(index);
            deliveries.push(recipients.clone());
        }
        for (deck, recipients) in &candidates {
            for (boundary, _) in deck.name.match_indices("::") {
                let parent = &deck.name[..boundary];
                if let Some(ancestors) = by_name_and_reviews.get(&(parent, deck.reviewed_today)) {
                    for &index in ancestors {
                        // Use original recipient sets so a deeper descendant can
                        // suppress every equivalent ancestor in any upload order.
                        deliveries[index].retain(|recipient| !recipients.contains(recipient));
                    }
                }
            }
        }
        for ((deck, _), recipients) in candidates.iter().zip(deliveries) {
            if recipients.is_empty() {
                continue;
            }
            let body = format!(
                "{display} has finished their {} studies for today.",
                deck.name
            );
            for recipient in &recipients {
                insert_notification(
                    &tx,
                    &Outgoing {
                        to: recipient,
                        from: user,
                        title: "Deck complete",
                        body: &body,
                        kind: "completion",
                    },
                    today,
                    now_ms,
                    false,
                )?;
            }
            announced.push(Announcement {
                deck: deck.name.clone(),
                recipients: recipients.len(),
            });
        }
        tx.commit()?;
        Ok(announced)
    }

    /// A full deck list from the client replaces the stored one, so deleted decks disappear.
    pub fn prune_decks(&mut self, user: &str, catalog: &[Snapshot]) -> Result<(), Error> {
        let keep: HashSet<&str> = catalog.iter().map(|deck| deck.id.as_str()).collect();
        let tx = self.conn.transaction()?;
        let stored: Vec<String> = tx
            .prepare("select id from decks where owner = ?1")?
            .query_map([user], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        for id in stored.iter().filter(|id| !keep.contains(id.as_str())) {
            tx.execute(
                "delete from subscription_baselines where sender=?1 and deck_id=?2",
                params![user, id],
            )?;
            tx.execute(
                "delete from unsubscribed_decks where sender=?1 and deck_id=?2",
                params![user, id],
            )?;
            tx.execute(
                "delete from decks where owner = ?1 and id = ?2",
                params![user, id],
            )?;
            tx.execute(
                "delete from deck_recipients where owner = ?1 and deck_id = ?2",
                params![user, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn nudges_enabled(&self, user: &str) -> Result<bool, Error> {
        Ok(self
            .conn
            .query_row(
                "select nudges from player_settings where user = ?1",
                [user],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub fn friend_nudges_receiving(&self, user: &str) -> Result<bool, Error> {
        Ok(self
            .conn
            .query_row(
                "select friend_nudges from player_settings where user = ?1",
                [user],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub fn friend_nudge_muted(&self, recipient: &str, sender: &str) -> Result<bool, Error> {
        Ok(self.conn.query_row(
            "select exists(select 1 from friend_nudge_muted_senders where recipient = ?1 and sender = ?2)",
            params![recipient, sender],
            |row| row.get(0),
        )?)
    }

    pub fn friend_nudges_enabled(&self, recipient: &str, sender: &str) -> Result<bool, Error> {
        Ok(self.friend_nudges_receiving(recipient)?
            && !self.friend_nudge_muted(recipient, sender)?)
    }

    pub fn set_friend_nudges(&mut self, user: &str, enabled: bool) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "insert into player_settings (user, friend_nudges) values (?1, ?2)
             on conflict (user) do update set friend_nudges = excluded.friend_nudges",
            params![user, enabled],
        )?;
        if !enabled {
            tx.execute(
                "update notifications set push_cancelled = 1
                 where recipient = ?1 and kind = 'nudge' and sender != '' and pushed = 0",
                [user],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_friend_nudge_muted(
        &mut self,
        recipient: &str,
        sender: &str,
        muted: bool,
    ) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        if muted {
            tx.execute(
                "insert or ignore into friend_nudge_muted_senders (recipient, sender) values (?1, ?2)",
                params![recipient, sender],
            )?;
            tx.execute(
                "update notifications set push_cancelled = 1
                 where recipient = ?1 and sender = ?2 and kind = 'nudge' and pushed = 0",
                params![recipient, sender],
            )?;
        } else {
            tx.execute(
                "delete from friend_nudge_muted_senders where recipient = ?1 and sender = ?2",
                params![recipient, sender],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn celebrations_enabled(&self, user: &str) -> Result<bool, Error> {
        Ok(self
            .conn
            .query_row(
                "select celebrations from player_settings where user = ?1",
                [user],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(true))
    }

    pub fn set_celebrations(&mut self, user: &str, enabled: bool) -> Result<(), Error> {
        self.conn.execute(
            "insert into player_settings (user, celebrations) values (?1, ?2)
             on conflict (user) do update set celebrations = excluded.celebrations",
            params![user, enabled],
        )?;
        Ok(())
    }

    pub fn set_nudges(&mut self, user: &str, enabled: bool) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "insert into player_settings (user, nudges) values (?1, ?2)
             on conflict (user) do update set nudges = excluded.nudges",
            params![user, enabled],
        )?;
        if !enabled {
            tx.execute(
                "update notifications set push_cancelled = 1
                 where recipient = ?1 and kind = 'nudge' and sender = '' and pushed = 0",
                [user],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

fn deck_settings(app: &App, store: &Store, user: &str) -> Result<Settings, Error> {
    let unsubscribed = store.unsubscribed_recipients(user)?;
    let circle = app.circle(&store.conn, user)?;
    let pushable = app
        .config
        .users
        .keys()
        .filter(|candidate| push_destination(&app.config, candidate).is_some())
        .cloned();
    let users: BTreeSet<String> = app
        .members()
        .into_iter()
        .chain(pushable)
        .filter(|candidate| {
            candidate != user && circle.contains(candidate) && !unsubscribed.contains(candidate)
        })
        .collect();
    Ok(Settings {
        decks: store.decks(user)?,
        recipients: users.into_iter().map(|user| app.recipient(user)).collect(),
        nudges: store.nudges_enabled(user)?,
        celebrations: store.celebrations_enabled(user)?,
    })
}

pub(crate) async fn get_decks(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Json<Settings>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    deck_settings(&app, &app.store.lock().unwrap(), &user)
        .map(Json)
        .map_err(failed_for("deck settings for", &user))
}

pub(crate) async fn set_decks(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(update): Json<SettingsUpdate>,
) -> Result<Json<Settings>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if update.decks.len() > MAX_DECKS {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let store_error = failed_for("deck settings for", &user);
    let mut store = app.store.lock().unwrap();
    let current = deck_settings(&app, &store, &user).map_err(store_error)?;
    if !valid_preferences(&update.decks, &current.decks, &current.recipients) {
        return Err(StatusCode::BAD_REQUEST);
    }
    store
        .set_deck_preferences(&user, &update.decks)
        .map_err(store_error)?;
    if let Some(nudges) = update.nudges {
        store.set_nudges(&user, nudges).map_err(store_error)?;
    }
    if let Some(celebrations) = update.celebrations {
        store
            .set_celebrations(&user, celebrations)
            .map_err(store_error)?;
    }
    deck_settings(&app, &store, &user)
        .map(Json)
        .map_err(store_error)
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/decks/{user}", get(get_decks).post(set_decks))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::UserConfig;
    use crate::notifications::PUSH_LEASE_MS;
    use crate::study::upload;
    use crate::test_support::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(crate) const DAY: i64 = 20_000;
    pub(crate) const NOW: i64 = DAY * 86_400_000 + 12 * 3_600_000;

    fn incoming(enabled: bool, muted_senders: &[&str]) -> IncomingSettings {
        IncomingSettings {
            enabled,
            muted_senders: muted_senders
                .iter()
                .map(|sender| (*sender).into())
                .collect(),
        }
    }

    fn queue_kind(store: &mut Store, to: &str, from: &str, kind: &str, at: i64) -> i64 {
        store
            .send(
                &Outgoing {
                    to,
                    from,
                    title: "Queued",
                    body: "Waiting",
                    kind,
                },
                DAY,
                at,
            )
            .unwrap()
    }

    #[test]
    fn incoming_settings_validate_shape_and_default_to_existing_delivery() {
        let (mut store, path) = temporary_store();
        let settings = store.incoming_settings("hill").unwrap();
        assert!(settings.enabled);
        assert!(settings.muted_senders.is_empty());
        assert!(valid_incoming_settings(&incoming(true, &[])));
        assert!(valid_incoming_settings(&incoming(
            false,
            &["cerro", "friend"]
        )));
        for senders in [
            vec![""],
            vec![" "],
            vec!["bad\nname"],
            vec!["cerro", "cerro"],
        ] {
            assert!(!valid_incoming_settings(&incoming(true, &senders)));
        }
        assert!(!valid_incoming_settings(&incoming(
            true,
            &[&"x".repeat(129)]
        )));
        let too_many = IncomingSettings {
            enabled: true,
            muted_senders: (0..=MAX_RECIPIENTS)
                .map(|id| format!("person{id}"))
                .collect(),
        };
        assert!(!valid_incoming_settings(&too_many));
        assert!(
            serde_json::from_str::<IncomingSettings>(
                r#"{"enabled":true,"muted_senders":[],"recipient":"someone_else"}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<IncomingSettings>(r#"{"enabled":true}"#).is_err());
        assert!(
            serde_json::from_str::<IncomingSettings>(r#"{"enabled":"false","muted_senders":[]}"#)
                .is_err()
        );
        let id = queue_kind(&mut store, "hill", "cerro", "completion", NOW);
        assert_eq!(store.notifications("hill", NOW).unwrap()[0].id, id);
        assert!(store.begin_push(id, NOW).unwrap());
        store
            .set_incoming_settings("hill", &incoming(true, &["cerro"]))
            .unwrap();
        assert!(
            store
                .set_incoming_settings("hill", &incoming(false, &["friend", "friend"]))
                .is_err()
        );
        let unchanged = store.incoming_settings("hill").unwrap();
        assert!(unchanged.enabled);
        assert_eq!(unchanged.muted_senders, ["cerro"]);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn incoming_migration_preserves_delivery_and_saved_choices_survive_restart() {
        let (mut store, path) = temporary_store();
        let id = queue_kind(&mut store, "hill", "cerro", "completion", NOW);
        store
            .conn
            .execute_batch(
                "drop table incoming_settings;
             drop table incoming_muted_senders;
             alter table notifications drop column completion_cancelled;",
            )
            .unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(store.incoming_settings("hill").unwrap().enabled);
        assert_eq!(store.notifications("hill", NOW).unwrap()[0].id, id);
        assert_eq!(
            store.take_deck_deliveries(NOW).unwrap()[0].notification.id,
            id
        );
        store
            .set_incoming_settings("hill", &incoming(false, &["cerro"]))
            .unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        let mut settings = store.incoming_settings("hill").unwrap();
        assert!(!settings.enabled);
        assert_eq!(settings.muted_senders, ["cerro"]);
        settings.enabled = true;
        store.set_incoming_settings("hill", &settings).unwrap();
        assert_eq!(
            store.incoming_settings("hill").unwrap().muted_senders,
            ["cerro"]
        );
        assert!(store.notifications("hill", NOW).unwrap().is_empty());
        assert!(store.take_deck_deliveries(NOW).unwrap().is_empty());
        store
            .set_incoming_settings("hill", &incoming(true, &[]))
            .unwrap();
        assert!(!store.begin_push(id, NOW + PUSH_LEASE_MS).unwrap());
        assert!(store.notifications("hill", NOW).unwrap().is_empty());
        let future = queue_kind(&mut store, "hill", "cerro", "completion", NOW + 1);
        assert_eq!(store.notifications("hill", NOW + 1).unwrap()[0].id, future);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn known_incoming_senders_exposes_only_existing_recipient_relationships() {
        let (mut store, path) = completion_fixture(&[("1", "Private deck name", &["hill"])]);
        let now = crate::app::now_ms();
        queue_kind(&mut store, "hill", "recent", "completion", now);
        queue_kind(
            &mut store,
            "hill",
            "expired",
            "completion",
            now - INBOX_AGE_MS - 1,
        );
        queue_kind(&mut store, "hill", "reply_only", "reply", now);
        queue_kind(&mut store, "stranger", "not_yours", "completion", now);
        queue_kind(&mut store, "hill", "hill", "completion", now);
        store
            .set_incoming_settings("hill", &incoming(true, &["remembered"]))
            .unwrap();
        assert_eq!(
            store.known_incoming_senders("hill").unwrap(),
            ["cerro", "recent", "remembered"]
        );
        assert!(
            store
                .known_incoming_senders("unrelated")
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.decks("cerro").unwrap()[0].recipients, ["hill"]);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn sender_mute_cancels_selected_retry_and_sent_completions_but_keeps_other_kinds() {
        let (mut store, path) = temporary_store();
        store.set_nudges("hill", true).unwrap();
        let selected = queue_kind(&mut store, "hill", "cerro", "completion", NOW);
        let retry = queue_kind(&mut store, "hill", "cerro", "completion", NOW);
        let sent = queue_kind(&mut store, "hill", "cerro", "completion", NOW);
        let other_sender = queue_kind(&mut store, "hill", "friend", "completion", NOW);
        let other_recipient = queue_kind(&mut store, "friend", "cerro", "completion", NOW);
        let unrelated: Vec<_> = ["reply", "message", "nudge", "event", "risk"]
            .iter()
            .map(|kind| queue_kind(&mut store, "hill", "cerro", kind, NOW))
            .collect();
        assert!(store.begin_push(retry, NOW).unwrap());
        store.finish_push(retry, false, NOW).unwrap();
        assert!(store.begin_push(sent, NOW).unwrap());
        store.finish_push(sent, true, NOW).unwrap();
        let selected_batch = store.take_deck_deliveries(NOW).unwrap();
        assert!(
            selected_batch
                .iter()
                .any(|delivery| delivery.notification.id == selected)
        );
        store
            .set_incoming_settings("hill", &incoming(true, &["cerro"]))
            .unwrap();
        assert!(
            !store.begin_push(selected, NOW).unwrap(),
            "a selected batch must recheck opt-out"
        );
        assert!(!store.begin_push(retry, NOW + PUSH_LEASE_MS).unwrap());
        assert!(
            store
                .reply("hill", "Hill", selected, "Hidden", NOW)
                .unwrap()
                .is_none()
        );
        let inbox = store.notifications("hill", NOW).unwrap();
        assert_eq!(inbox.len(), unrelated.len() + 1);
        assert!(inbox.iter().any(|message| message.id == other_sender));
        assert!(
            unrelated
                .iter()
                .all(|id| inbox.iter().any(|message| message.id == *id))
        );
        assert_eq!(
            store.notifications("friend", NOW).unwrap()[0].id,
            other_recipient
        );
        let ready = store.take_deck_deliveries(NOW + PUSH_LEASE_MS).unwrap();
        assert_eq!(ready.len(), unrelated.len() + 2);
        assert!(
            ready
                .iter()
                .all(|delivery| ![selected, retry, sent].contains(&delivery.notification.id))
        );
        store
            .set_incoming_settings("hill", &incoming(true, &[]))
            .unwrap();
        assert!(!store.begin_push(retry, NOW + PUSH_LEASE_MS).unwrap());
        assert_eq!(
            store.notifications("hill", NOW).unwrap().len(),
            unrelated.len() + 1
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn activity_respects_completion_mutes_and_does_not_restore_cancelled_history() {
        let (mut store, path) = temporary_store();
        queue_kind(&mut store, "hill", "cerro", "completion", NOW);
        let allowed = queue_kind(&mut store, "hill", "friend", "completion", NOW);
        let message = queue_kind(&mut store, "hill", "cerro", "message", NOW);
        store
            .set_incoming_settings("hill", &incoming(true, &["cerro"]))
            .unwrap();
        let activity = store.activity("hill", NOW, 30, None, 1).unwrap();
        assert_eq!(activity.unread_count, 2);
        assert_eq!(activity.items[0].id, message);
        let older = store
            .activity("hill", NOW, 30, activity.next_before, 10)
            .unwrap();
        assert_eq!(older.items.len(), 1);
        assert_eq!(older.items[0].id, allowed);
        store
            .set_incoming_settings("hill", &incoming(false, &[]))
            .unwrap();
        store
            .set_incoming_settings("hill", &incoming(true, &[]))
            .unwrap();
        let activity = store.activity("hill", NOW, 30, None, 10).unwrap();
        assert_eq!(activity.unread_count, 1);
        assert_eq!(activity.items.len(), 1);
        assert_eq!(activity.items[0].id, message);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn global_completion_opt_out_cancels_leases_and_keeps_sender_choices_for_future() {
        let (mut store, path) = temporary_store();
        let leased = queue_kind(&mut store, "hill", "friend", "completion", NOW);
        let selected = queue_kind(&mut store, "hill", "other", "completion", NOW);
        let message = queue_kind(&mut store, "hill", "friend", "message", NOW);
        let other_recipient = queue_kind(&mut store, "friend", "cerro", "completion", NOW);
        assert!(store.begin_push(leased, NOW).unwrap());
        store
            .set_incoming_settings("hill", &incoming(false, &["cerro"]))
            .unwrap();
        // A response from a previously leased request must not resurrect retries.
        store.finish_push(leased, false, NOW).unwrap();
        assert!(!store.begin_push(selected, NOW).unwrap());
        let during_opt_out = queue_kind(&mut store, "hill", "future_sender", "completion", NOW);
        assert_eq!(store.notifications("hill", NOW).unwrap().len(), 1);
        assert_eq!(store.notifications("hill", NOW).unwrap()[0].id, message);
        assert_eq!(
            store.notifications("friend", NOW).unwrap()[0].id,
            other_recipient
        );
        let mut settings = store.incoming_settings("hill").unwrap();
        settings.enabled = true;
        store.set_incoming_settings("hill", &settings).unwrap();
        assert_eq!(settings.muted_senders, ["cerro"]);
        for id in [leased, selected, during_opt_out] {
            assert!(!store.begin_push(id, NOW + PUSH_LEASE_MS).unwrap());
        }
        let muted = queue_kind(&mut store, "hill", "cerro", "completion", NOW);
        let allowed = queue_kind(&mut store, "hill", "friend", "completion", NOW);
        assert!(!store.begin_push(muted, NOW).unwrap());
        assert!(store.begin_push(allowed, NOW).unwrap());
        assert_eq!(store.notifications("hill", NOW).unwrap().len(), 2);
        store
            .set_incoming_settings("hill", &incoming(true, &[]))
            .unwrap();
        assert_eq!(store.notifications("hill", NOW).unwrap().len(), 2);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn incoming_filter_precedes_grouping_and_counts_and_consumes_suppressed_completions() {
        let (mut store, path) = completion_fixture(&[
            ("1", "Spanish", &["hill", "parent_only", "all_off", "other"]),
            ("2", "Spanish::Verbs", &["hill", "all_off", "other"]),
        ]);
        store
            .set_incoming_settings("hill", &incoming(true, &["cerro"]))
            .unwrap();
        store
            .set_incoming_settings("parent_only", &incoming(true, &["friend"]))
            .unwrap();
        store
            .set_incoming_settings("all_off", &incoming(false, &["friend"]))
            .unwrap();
        let mut snapshots = [
            named_snapshot("1", "Spanish", 0, 4, DAY),
            named_snapshot("2", "Spanish::Verbs", 0, 4, DAY),
        ];
        assert_eq!(
            record_completions(&mut store, &snapshots),
            [("Spanish".into(), 1), ("Spanish::Verbs".into(), 1),]
        );
        assert_completion_inbox(&store, "hill", &[]);
        assert_completion_inbox(&store, "all_off", &[]);
        assert_completion_inbox(&store, "parent_only", &["Spanish"]);
        assert_completion_inbox(&store, "other", &["Spanish::Verbs"]);
        assert_eq!(
            store.decks("cerro").unwrap()[0].recipients,
            ["all_off", "hill", "other", "parent_only"]
        );
        store
            .set_incoming_settings("hill", &incoming(true, &[]))
            .unwrap();
        store
            .set_incoming_settings("all_off", &incoming(true, &["friend"]))
            .unwrap();
        assert!(record_completions(&mut store, &snapshots).is_empty());
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(record_completions(&mut store, &snapshots).is_empty());
        for snapshot in &mut snapshots {
            snapshot.day += 1;
        }
        let announcements = store
            .record_decks(
                "cerro",
                "Cerro",
                &snapshots,
                Clock::default(),
                false,
                NOW + 86_400_000,
            )
            .unwrap();
        assert_eq!(announcements.len(), 2);
        assert_eq!(announcements[0].recipients, 1);
        assert_eq!(announcements[1].recipients, 3);
        assert_eq!(
            store.notifications("hill", NOW + 86_400_000).unwrap().len(),
            1
        );
        assert_eq!(
            store
                .notifications("all_off", NOW + 86_400_000)
                .unwrap()
                .len(),
            1
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn cancelled_completions_do_not_fill_the_visible_inbox_limit() {
        let (mut store, path) = temporary_store();
        let visible = queue_kind(&mut store, "hill", "cerro", "reply", NOW);
        for _ in 0..501 {
            queue_kind(&mut store, "hill", "cerro", "completion", NOW);
        }
        store
            .set_incoming_settings("hill", &incoming(true, &["cerro"]))
            .unwrap();
        assert_eq!(store.notifications("hill", NOW).unwrap().len(), 1);
        assert_eq!(store.notifications("hill", NOW).unwrap()[0].id, visible);
        store
            .set_incoming_settings("hill", &incoming(true, &[]))
            .unwrap();
        assert_eq!(store.notifications("hill", NOW).unwrap()[0].id, visible);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn inbox_and_pushes_recheck_preferences_even_without_cancellation_flags() {
        let (mut store, path) = temporary_store();
        let muted = queue_kind(&mut store, "hill", "cerro", "completion", NOW);
        let global = queue_kind(&mut store, "friend", "cerro", "completion", NOW);
        let visible = queue_kind(&mut store, "hill", "other", "completion", NOW);
        store
            .conn
            .execute_batch(
                "insert into incoming_muted_senders (recipient, sender) values ('hill', 'cerro');
             insert into incoming_settings (user, enabled) values ('friend', 0);",
            )
            .unwrap();
        assert_eq!(store.notifications("hill", NOW).unwrap()[0].id, visible);
        assert_eq!(store.notifications("hill", NOW).unwrap().len(), 1);
        assert!(store.notifications("friend", NOW).unwrap().is_empty());
        let batch = store.take_deck_deliveries(NOW).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].notification.id, visible);
        assert!(!store.begin_push(muted, NOW).unwrap());
        assert!(!store.begin_push(global, NOW).unwrap());
        assert!(store.begin_push(visible, NOW).unwrap());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn cancellation_failure_rolls_back_incoming_settings_and_sender_mutes() {
        let (mut store, path) = temporary_store();
        let id = queue_kind(&mut store, "hill", "friend", "completion", NOW);
        store
            .set_incoming_settings("hill", &incoming(true, &["cerro"]))
            .unwrap();
        store.conn.execute_batch(
            "create trigger prevent_cancellation before update of completion_cancelled on notifications
             begin select raise(abort, 'failed cancellation'); end;",
        ).unwrap();
        assert!(
            store
                .set_incoming_settings("hill", &incoming(false, &["friend"]))
                .is_err()
        );
        let settings = store.incoming_settings("hill").unwrap();
        assert!(settings.enabled);
        assert_eq!(settings.muted_senders, ["cerro"]);
        assert_eq!(store.notifications("hill", NOW).unwrap()[0].id, id);
        assert!(store.begin_push(id, NOW).unwrap());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn nudge_opt_out_cancels_selected_and_leased_pushes_only_for_that_user() {
        let (mut store, path) = temporary_store();
        store.set_nudges("hill", true).unwrap();
        store.set_nudges("friend", true).unwrap();
        let mut ids = Vec::new();
        for (user, kind) in [
            ("hill", "nudge"),
            ("hill", "nudge"),
            ("hill", "message"),
            ("hill", "completion"),
            ("friend", "nudge"),
        ] {
            ids.push(
                store
                    .send(
                        &Outgoing {
                            to: user,
                            from: "",
                            title: "Notification",
                            body: "Keep this in the inbox",
                            kind,
                        },
                        DAY,
                        NOW,
                    )
                    .unwrap(),
            );
        }
        assert_eq!(store.take_deck_deliveries(NOW).unwrap().len(), 5);
        assert!(store.begin_push(ids[1], NOW).unwrap());
        store.set_nudges("hill", false).unwrap();
        store.finish_push(ids[1], false, NOW).unwrap();
        store.set_nudges("hill", true).unwrap();
        assert!(
            !store.begin_push(ids[0], NOW).unwrap(),
            "a batch selected before opt-out must not send a cancelled nudge"
        );
        drop(store);
        let mut store = Store::open(&path).unwrap();
        let mut ready: Vec<_> = store
            .take_deck_deliveries(NOW + PUSH_LEASE_MS)
            .unwrap()
            .into_iter()
            .map(|delivery| delivery.notification.id)
            .collect();
        ready.sort();
        assert_eq!(ready, ids[2..]);
        assert_eq!(store.notifications("hill", NOW).unwrap().len(), 4);
        assert_eq!(store.notifications("friend", NOW).unwrap().len(), 1);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn friend_and_automatic_nudge_opt_outs_cancel_only_their_own_pending_pushes() {
        let (mut store, path) = temporary_store();
        store.set_nudges("hill", true).unwrap();
        store.set_friend_nudges("hill", true).unwrap();
        let notice = |from| Outgoing {
            to: "hill",
            from,
            title: "Time for Anki?",
            body: "Study today",
            kind: "nudge",
        };
        let automatic = store.send(&notice(""), DAY, NOW).unwrap();
        let friend = store.send(&notice("cerro"), DAY, NOW).unwrap();
        store.set_friend_nudges("hill", false).unwrap();
        assert!(store.begin_push(automatic, NOW).unwrap());
        assert!(!store.begin_push(friend, NOW).unwrap());

        store.set_friend_nudges("hill", true).unwrap();
        let another_friend = store.send(&notice("cerro"), DAY, NOW).unwrap();
        let another_automatic = store.send(&notice(""), DAY, NOW).unwrap();
        store.set_nudges("hill", false).unwrap();
        assert!(store.begin_push(another_friend, NOW).unwrap());
        assert!(!store.begin_push(another_automatic, NOW).unwrap());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn existing_nudge_choices_migrate_to_friend_choices_once() {
        let (store, path) = temporary_store();
        store.conn.execute_batch(
            "drop table player_settings;
             create table player_settings (user text primary key, nudges integer not null default 0) without rowid;
             insert into player_settings (user, nudges) values ('hill', 1), ('cerro', 0);",
        ).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(store.nudges_enabled("hill").unwrap());
        assert!(store.friend_nudges_receiving("hill").unwrap());
        assert!(!store.friend_nudges_receiving("cerro").unwrap());
        store.set_friend_nudges("hill", false).unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert!(store.nudges_enabled("hill").unwrap());
        assert!(!store.friend_nudges_receiving("hill").unwrap());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn old_pending_nudges_are_cancelled_when_the_recipient_has_opted_out() {
        let (mut store, path) = temporary_store();
        for user in ["hill", "friend"] {
            store.set_nudges(user, true).unwrap();
            store
                .send(
                    &Outgoing {
                        to: user,
                        from: "",
                        title: "Old nudge",
                        body: "Already queued before the upgrade",
                        kind: "nudge",
                    },
                    DAY,
                    NOW,
                )
                .unwrap();
        }
        // Simulate an older server's opt-out, which left the delivery pending.
        store
            .conn
            .execute(
                "update player_settings set nudges = 0 where user = 'hill'",
                [],
            )
            .unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        let ready = store.take_deck_deliveries(NOW).unwrap();
        assert_eq!(ready.len(), 1, "old opted-out nudges must not be retried");
        assert_eq!(ready[0].user, "friend");
        store.set_nudges("hill", true).unwrap();
        assert_eq!(store.take_deck_deliveries(NOW).unwrap().len(), 1);
        assert_eq!(store.notifications("hill", NOW).unwrap().len(), 1);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    pub fn temporary_store() -> (Store, std::path::PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ankiquest-decks-{}-{}-{}",
            std::process::id(),
            crate::app::now_ms(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        (Store::open(&path).unwrap(), path)
    }

    fn snapshot(id: &str, remaining: u64, reviewed_today: u64, day: i64) -> Snapshot {
        Snapshot {
            id: id.into(),
            name: "Spanish".into(),
            remaining,
            reviewed_today,
            day,
        }
    }

    fn save(store: &mut Store, deck: Snapshot, silent: bool, at: i64) {
        store
            .record_decks("cerro", "Cerro", &[deck], Clock::default(), silent, at)
            .unwrap();
    }

    fn enable(store: &mut Store, id: &str, recipients: &[&str]) {
        store
            .set_deck_preferences(
                "cerro",
                &[Preference {
                    id: id.into(),
                    enabled: true,
                    recipients: recipients.iter().map(|r| (*r).into()).collect(),
                }],
            )
            .unwrap();
    }

    fn named_snapshot(
        id: &str,
        name: &str,
        remaining: u64,
        reviewed_today: u64,
        day: i64,
    ) -> Snapshot {
        Snapshot {
            name: name.into(),
            ..snapshot(id, remaining, reviewed_today, day)
        }
    }

    fn completion_fixture(decks: &[(&str, &str, &[&str])]) -> (Store, std::path::PathBuf) {
        let (mut store, path) = temporary_store();
        for (id, name, recipients) in decks {
            save(&mut store, named_snapshot(id, name, 1, 0, DAY), false, NOW);
            if !recipients.is_empty() {
                enable(&mut store, id, recipients);
            }
        }
        (store, path)
    }

    fn record_completions(store: &mut Store, snapshots: &[Snapshot]) -> Vec<(String, usize)> {
        let mut announcements: Vec<_> = store
            .record_decks("cerro", "Cerro", snapshots, Clock::default(), false, NOW)
            .unwrap()
            .into_iter()
            .map(|announcement| (announcement.deck, announcement.recipients))
            .collect();
        announcements.sort();
        announcements
    }

    fn assert_completion_inbox(store: &Store, recipient: &str, decks: &[&str]) {
        let mut bodies: Vec<_> = store
            .notifications(recipient, NOW)
            .unwrap()
            .into_iter()
            .map(|notification| {
                assert_eq!(notification.kind, "completion");
                assert_eq!(notification.sender, "cerro");
                notification.body
            })
            .collect();
        bodies.sort();
        let mut expected: Vec<_> = decks
            .iter()
            .map(|deck| format!("Cerro has finished their {deck} studies for today."))
            .collect();
        expected.sort();
        assert_eq!(bodies, expected, "completion inbox for {recipient}");
    }

    #[test]
    fn equivalent_parent_and_child_notify_each_recipient_only_once() {
        let (mut store, path) = completion_fixture(&[
            ("1", "Spanish", &["hill", "friend"]),
            ("2", "Spanish::Verbs", &["hill", "friend"]),
        ]);
        let snapshots = [
            named_snapshot("1", "Spanish", 0, 4, DAY),
            named_snapshot("2", "Spanish::Verbs", 0, 4, DAY),
        ];
        assert_eq!(
            record_completions(&mut store, &snapshots),
            [("Spanish::Verbs".into(), 2)]
        );
        for recipient in ["hill", "friend"] {
            assert_completion_inbox(&store, recipient, &["Spanish::Verbs"]);
        }
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn equivalent_nested_completions_keep_the_deepest_deck_in_every_upload_order() {
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let names = ["Spanish", "Spanish::Verbs", "Spanish::Verbs::Present"];
            let (mut store, path) = completion_fixture(&[
                ("1", names[0], &["hill"]),
                ("2", names[1], &["hill"]),
                ("3", names[2], &["hill"]),
            ]);
            let snapshots: Vec<_> = order
                .into_iter()
                .map(|index| named_snapshot(&(index + 1).to_string(), names[index], 0, 4, DAY))
                .collect();
            assert_eq!(
                record_completions(&mut store, &snapshots),
                [(names[2].into(), 1)],
                "upload order {order:?}"
            );
            assert_completion_inbox(&store, "hill", &[names[2]]);
            drop(store);
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn nested_completion_coalescing_respects_each_decks_recipient_choices() {
        let (mut store, path) = completion_fixture(&[
            ("1", "Spanish", &["hill", "parent_only"]),
            ("2", "Spanish::Verbs", &["hill", "child_only"]),
            ("3", "Spanish::Verbs::Present", &["grandchild_only"]),
        ]);
        let snapshots = [
            named_snapshot("1", "Spanish", 0, 4, DAY),
            named_snapshot("2", "Spanish::Verbs", 0, 4, DAY),
            named_snapshot("3", "Spanish::Verbs::Present", 0, 4, DAY),
        ];
        assert_eq!(
            record_completions(&mut store, &snapshots),
            [
                ("Spanish".into(), 1),
                ("Spanish::Verbs".into(), 2),
                ("Spanish::Verbs::Present".into(), 1),
            ]
        );
        assert_completion_inbox(&store, "hill", &["Spanish::Verbs"]);
        assert_completion_inbox(&store, "parent_only", &["Spanish"]);
        assert_completion_inbox(&store, "child_only", &["Spanish::Verbs"]);
        assert_completion_inbox(&store, "grandchild_only", &["Spanish::Verbs::Present"]);
        assert_completion_inbox(&store, "cerro", &[]);
        assert_completion_inbox(&store, "stranger", &[]);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn a_parent_with_additional_studied_cards_keeps_its_completion() {
        let (mut store, path) = completion_fixture(&[
            ("1", "Spanish", &["hill"]),
            ("2", "Spanish::Verbs", &["hill"]),
        ]);
        assert_eq!(
            record_completions(
                &mut store,
                &[
                    named_snapshot("1", "Spanish", 0, 5, DAY),
                    named_snapshot("2", "Spanish::Verbs", 0, 4, DAY),
                ]
            ),
            [("Spanish".into(), 1), ("Spanish::Verbs".into(), 1)]
        );
        assert_completion_inbox(&store, "hill", &["Spanish", "Spanish::Verbs"]);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn a_child_that_cannot_announce_does_not_suppress_the_parent() {
        for (enabled, remaining, reviewed, day) in [
            (false, 0, 4, DAY),
            (true, 1, 4, DAY),
            (true, 0, 0, DAY),
            (true, 0, 4, DAY - 1),
            (true, 0, 4, DAY + 1),
        ] {
            let (mut store, path) =
                completion_fixture(&[("1", "Spanish", &["hill"]), ("2", "Spanish::Verbs", &[])]);
            if enabled {
                enable(&mut store, "2", &["hill"]);
            }
            assert_eq!(
                record_completions(
                    &mut store,
                    &[
                        named_snapshot("1", "Spanish", 0, 4, DAY),
                        named_snapshot("2", "Spanish::Verbs", remaining, reviewed, day),
                    ]
                ),
                [("Spanish".into(), 1)],
                "child enabled={enabled}, remaining={remaining}, reviewed={reviewed}, day={day}"
            );
            assert_completion_inbox(&store, "hill", &["Spanish"]);
            drop(store);
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn previously_consumed_child_completions_do_not_suppress_a_fresh_parent() {
        for silent in [false, true] {
            let (mut store, path) = completion_fixture(&[
                ("1", "Spanish", &["hill"]),
                ("2", "Spanish::Verbs", &["hill"]),
            ]);
            save(
                &mut store,
                named_snapshot("2", "Spanish::Verbs", 0, 4, DAY),
                silent,
                NOW,
            );
            assert_eq!(
                record_completions(
                    &mut store,
                    &[
                        named_snapshot("1", "Spanish", 0, 4, DAY),
                        named_snapshot("2", "Spanish::Verbs", 0, 4, DAY),
                    ]
                ),
                [("Spanish".into(), 1)]
            );
            let expected = if silent {
                vec!["Spanish"]
            } else {
                vec!["Spanish", "Spanish::Verbs"]
            };
            assert_completion_inbox(&store, "hill", &expected);
            drop(store);
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn unrelated_names_prefixes_and_sibling_decks_keep_separate_completions() {
        for names in [
            ["Spanish", "Spanish 2"],
            ["Spanish", "Spanish:Verbs"],
            ["Spanish", "Spanishish::Verbs"],
            ["Spanish::Verbs", "Spanish::Nouns"],
            ["Spanish", "Geography::Spanish"],
        ] {
            let (mut store, path) =
                completion_fixture(&[("1", names[0], &["hill"]), ("2", names[1], &["hill"])]);
            let announcements = record_completions(
                &mut store,
                &[
                    named_snapshot("1", names[0], 0, 4, DAY),
                    named_snapshot("2", names[1], 0, 4, DAY),
                ],
            );
            assert_eq!(announcements.len(), 2, "deck names {names:?}");
            assert_completion_inbox(&store, "hill", &names);
            drop(store);
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn overlapping_colons_follow_the_clients_left_to_right_hierarchy_boundaries() {
        let (mut store, path) = completion_fixture(&[
            ("1", "Spanish", &["hill"]),
            ("2", "Spanish:", &["hill"]),
            ("3", "Spanish:::Verbs", &["hill"]),
        ]);
        // The clients split "Spanish:::Verbs" into "Spanish" and ":Verbs".
        // "Spanish:" is therefore a separate deck, not this child's parent.
        assert_eq!(
            record_completions(
                &mut store,
                &[
                    named_snapshot("1", "Spanish", 0, 4, DAY),
                    named_snapshot("2", "Spanish:", 0, 4, DAY),
                    named_snapshot("3", "Spanish:::Verbs", 0, 4, DAY),
                ]
            ),
            [("Spanish:".into(), 1), ("Spanish:::Verbs".into(), 1)]
        );
        assert_completion_inbox(&store, "hill", &["Spanish:", "Spanish:::Verbs"]);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn suppressed_parent_completions_stay_consumed_after_replay_and_restart() {
        let (mut store, path) = completion_fixture(&[
            ("1", "Spanish", &["hill"]),
            ("2", "Spanish::Verbs", &["hill"]),
        ]);
        let snapshots = [
            named_snapshot("1", "Spanish", 0, 4, DAY),
            named_snapshot("2", "Spanish::Verbs", 0, 4, DAY),
        ];
        assert_eq!(
            record_completions(&mut store, &snapshots),
            [("Spanish::Verbs".into(), 1)]
        );
        assert!(record_completions(&mut store, &snapshots).is_empty());
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(record_completions(&mut store, &snapshots[..1]).is_empty());
        assert!(record_completions(&mut store, &snapshots).is_empty());
        assert_completion_inbox(&store, "hill", &["Spanish::Verbs"]);
        assert_eq!(store.decks("cerro").unwrap().len(), 2);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn partial_upload_keeps_a_parent_completion_and_the_existing_deck_catalog() {
        let (mut store, path) = completion_fixture(&[
            ("1", "Spanish", &["hill"]),
            ("2", "Spanish::Verbs", &["hill"]),
        ]);
        assert_eq!(
            record_completions(&mut store, &[named_snapshot("1", "Spanish", 0, 4, DAY)]),
            [("Spanish".into(), 1)]
        );
        assert_eq!(
            record_completions(
                &mut store,
                &[named_snapshot("2", "Spanish::Verbs", 0, 4, DAY)]
            ),
            [("Spanish::Verbs".into(), 1)]
        );
        let decks = store.decks("cerro").unwrap();
        assert_eq!(decks.len(), 2);
        assert!(
            decks
                .iter()
                .all(|deck| deck.enabled && deck.recipients == ["hill"])
        );
        assert_completion_inbox(&store, "hill", &["Spanish", "Spanish::Verbs"]);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn full_catalog_removes_deleted_decks_and_their_recipients() {
        let (mut store, path) = temporary_store();
        save(&mut store, snapshot("1", 2, 0, DAY), false, NOW);
        save(&mut store, snapshot("2", 2, 0, DAY), false, NOW);
        enable(&mut store, "2", &["hill"]);
        store
            .prune_decks("cerro", &[snapshot("1", 2, 0, DAY)])
            .unwrap();
        let decks = store.decks("cerro").unwrap();
        assert_eq!(decks.len(), 1);
        assert_eq!(decks[0].id, "1");
        save(&mut store, snapshot("2", 2, 0, DAY), false, NOW);
        assert!(
            store
                .decks("cerro")
                .unwrap()
                .iter()
                .all(|d| !d.enabled && d.recipients.is_empty())
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn completion_is_private_once_per_deck_day_and_survives_restart() {
        let (mut store, path) = temporary_store();
        save(&mut store, snapshot("1", 2, 0, DAY), false, NOW);
        save(&mut store, snapshot("2", 2, 0, DAY), false, NOW);
        enable(&mut store, "1", &["hill", "friend"]);
        enable(&mut store, "2", &["hill"]);
        save(&mut store, snapshot("1", 0, 4, DAY), false, NOW);
        save(&mut store, snapshot("1", 0, 4, DAY), false, NOW);
        save(&mut store, snapshot("1", 1, 3, DAY), false, NOW);
        save(&mut store, snapshot("1", 0, 4, DAY), false, NOW);
        assert_eq!(store.notifications("hill", NOW).unwrap().len(), 1);
        assert_eq!(store.notifications("friend", NOW).unwrap().len(), 1);
        assert!(store.notifications("cerro", NOW).unwrap().is_empty());
        assert!(store.notifications("stranger", NOW).unwrap().is_empty());
        assert_eq!(
            store.notifications("hill", NOW).unwrap()[0].body,
            "Cerro has finished their Spanish studies for today."
        );
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(store.decks("cerro").unwrap()[0].enabled);
        save(&mut store, snapshot("1", 0, 5, DAY), false, NOW);
        save(&mut store, snapshot("2", 0, 3, DAY), false, NOW);
        save(
            &mut store,
            snapshot("1", 0, 4, DAY + 1),
            false,
            NOW + 86_400_000,
        );
        assert_eq!(
            store.notifications("hill", NOW + 86_400_000).unwrap().len(),
            3
        );
        assert_eq!(
            store
                .notifications("friend", NOW + 86_400_000)
                .unwrap()
                .len(),
            2
        );
        let deliveries = store.take_deck_deliveries(NOW + 86_400_000).unwrap();
        assert_eq!(deliveries.len(), 5);
        for delivery in deliveries {
            assert!(
                store
                    .begin_push(delivery.notification.id, NOW + 86_400_000)
                    .unwrap()
            );
            store
                .finish_push(delivery.notification.id, true, NOW + 86_400_000)
                .unwrap();
        }
        assert!(
            store
                .take_deck_deliveries(NOW + 86_400_000)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.notifications("hill", NOW + 86_400_000).unwrap().len(),
            3
        );
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(
            store
                .take_deck_deliveries(NOW + 86_400_000)
                .unwrap()
                .is_empty()
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn silent_disabled_empty_and_old_snapshots_never_publish_later() {
        let (mut store, path) = temporary_store();
        save(&mut store, snapshot("1", 0, 3, DAY), false, NOW);
        enable(&mut store, "1", &["hill"]);
        save(&mut store, snapshot("1", 0, 3, DAY), false, NOW);
        save(&mut store, snapshot("2", 1, 0, DAY), false, NOW);
        enable(&mut store, "2", &["hill"]);
        save(&mut store, snapshot("2", 0, 3, DAY), true, NOW);
        save(&mut store, snapshot("2", 0, 3, DAY), false, NOW);
        save(&mut store, snapshot("3", 1, 0, DAY), false, NOW);
        enable(&mut store, "3", &["hill"]);
        for (remaining, reviewed, day) in
            [(1, 3, DAY), (0, 0, DAY), (0, 3, DAY - 1), (0, 3, DAY + 1)]
        {
            save(
                &mut store,
                snapshot("3", remaining, reviewed, day),
                false,
                NOW,
            );
        }
        assert!(store.notifications("hill", NOW).unwrap().is_empty());
        save(&mut store, snapshot("3", 0, 3, DAY), false, NOW);
        assert_eq!(store.notifications("hill", NOW).unwrap().len(), 1);
        assert!(
            store
                .notifications("hill", NOW + INBOX_AGE_MS + 1)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .take_deck_deliveries(NOW + INBOX_AGE_MS + 1)
                .unwrap()
                .is_empty()
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn completion_uses_owner_clock_rollover_and_deck_identity() {
        let (mut store, path) = temporary_store();
        let clock = Clock {
            offset_west_min: -120,
            rollover_hour: 4,
        };
        let before_rollover = DAY * 86_400_000 + 3_600_000;
        store
            .record_decks(
                "cerro",
                "Cerro",
                &[snapshot("1", 1, 0, DAY - 1)],
                clock,
                false,
                before_rollover,
            )
            .unwrap();
        enable(&mut store, "1", &["hill"]);
        store
            .record_decks(
                "other",
                "Other",
                &[snapshot("1", 0, 3, DAY - 1)],
                clock,
                false,
                before_rollover,
            )
            .unwrap();
        assert!(
            store
                .notifications("hill", before_rollover)
                .unwrap()
                .is_empty()
        );
        for (at, day) in [
            (before_rollover, DAY - 1),
            (before_rollover + 2 * 3_600_000, DAY),
        ] {
            let mut deck = snapshot("1", 0, 3, day);
            deck.name = "Spanish::Verbs".into();
            store
                .record_decks("cerro", "Cerro", &[deck], clock, false, at)
                .unwrap();
        }
        let inbox = store.notifications("hill", NOW).unwrap();
        assert_eq!(inbox.len(), 2);
        assert_eq!(inbox[0].day, DAY - 1);
        assert_eq!(inbox[1].day, DAY);
        assert_eq!(store.decks("cerro").unwrap()[0].name, "Spanish::Verbs");
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn validates_snapshot_shape_clock_and_recipient_choices() {
        let good = snapshot("1", 0, 1, DAY);
        assert!(valid_snapshots(std::slice::from_ref(&good)));
        assert!(!valid_snapshots(&[good.clone(), good.clone()]));
        assert!(!valid_snapshots(&[Snapshot {
            name: "\n".into(),
            ..good.clone()
        }]));
        assert!(!valid_snapshots(&[Snapshot {
            remaining: u64::MAX,
            ..good.clone()
        }]));
        assert!(!valid_clock(Clock {
            offset_west_min: i64::MAX,
            rollover_hour: 4
        }));
        assert!(!valid_clock(Clock {
            offset_west_min: 0,
            rollover_hour: 24
        }));
        let decks = vec![Deck {
            id: "1".into(),
            name: "Spanish".into(),
            enabled: false,
            recipients: vec![],
        }];
        let recipients = vec![Recipient {
            user: "hill".into(),
            display: "Hill".into(),
        }];
        for users in [
            vec![],
            vec!["stranger"],
            vec!["cerro"],
            vec!["hill", "hill"],
        ] {
            assert!(!valid_preferences(
                &[Preference {
                    id: "1".into(),
                    enabled: true,
                    recipients: users.into_iter().map(String::from).collect()
                }],
                &decks,
                &recipients
            ));
        }
        assert!(valid_preferences(
            &[Preference {
                id: "1".into(),
                enabled: true,
                recipients: vec!["hill".into()]
            }],
            &decks,
            &recipients
        ));
        assert!(
            serde_json::from_str::<SettingsUpdate>(
                r#"{"decks":[{"id":"1","enabled":true,"recipients":["hill"],"owner":"other"}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn recipient_choices_require_a_configured_delivery_channel() {
        let (mut app, path) = fixture();
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some("http://localhost:9999".into());
        config.users.insert(
            "push-only".into(),
            UserConfig {
                ntfy_topic: Some("friend-topic".into()),
                ..UserConfig::default()
            },
        );
        config.users.insert(
            "empty-token".into(),
            UserConfig {
                token: Some(String::new()),
                ..UserConfig::default()
            },
        );
        config
            .users
            .insert("display-only".into(), UserConfig::default());
        config.users.insert(
            "empty-topic".into(),
            UserConfig {
                ntfy_topic: Some(String::new()),
                ..UserConfig::default()
            },
        );
        {
            let mut store = app.store.lock().unwrap();
            store
                .upsert("imported", &[], &[], Clock::default())
                .unwrap();
            let settings = deck_settings(&app, &store, "cerro").unwrap();
            assert_eq!(
                settings
                    .recipients
                    .iter()
                    .map(|r| r.user.as_str())
                    .collect::<Vec<_>>(),
                vec!["friend", "hill", "push-only"]
            );
            assert!(settings.decks.is_empty());
        }
        Arc::get_mut(&mut app).unwrap().config.ntfy = None;
        assert_eq!(
            deck_settings(&app, &app.store.lock().unwrap(), "cerro")
                .unwrap()
                .recipients
                .iter()
                .map(|r| r.user.as_str())
                .collect::<Vec<_>>(),
            vec!["friend", "hill"]
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn nudges_are_off_until_asked_for_and_then_stay_on() {
        let (app, path) = fixture();
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(2, false)),
        )
        .await
        .unwrap();
        let save = |nudges: Option<bool>| {
            let app = app.clone();
            async move {
                let mut update = preferences(&["hill"]);
                update.nudges = nudges;
                set_decks(
                    State(app),
                    UrlPath("cerro".into()),
                    headers("cerro"),
                    Json(update),
                )
                .await
                .unwrap()
                .0
            }
        };
        assert!(!save(None).await.nudges, "off until someone asks");
        assert!(save(Some(true)).await.nudges);
        assert!(save(None).await.nudges, "an older client leaves it alone");
        assert!(!save(Some(false)).await.nudges);
        assert!(
            !get_decks(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0
                .nudges,
            "the setting belongs to one player"
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn rejects_invalid_updates_without_changing_settings() {
        let (app, path) = fixture();
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(2, false)),
        )
        .await
        .unwrap();
        for update in [
            preferences(&["cerro"]),
            preferences(&["unknown"]),
            preferences(&[]),
            preferences(&["hill", "hill"]),
            SettingsUpdate {
                decks: vec![Preference {
                    id: "missing".into(),
                    enabled: false,
                    recipients: vec![],
                }],
                nudges: None,
                celebrations: None,
            },
            SettingsUpdate {
                decks: vec![
                    Preference {
                        id: "1".into(),
                        enabled: true,
                        recipients: vec!["hill".into()],
                    },
                    Preference {
                        id: "1".into(),
                        enabled: false,
                        recipients: vec![],
                    },
                ],
                nudges: None,
                celebrations: None,
            },
        ] {
            assert_eq!(
                set_decks(
                    State(app.clone()),
                    UrlPath("cerro".into()),
                    headers("cerro"),
                    Json(update)
                )
                .await
                .unwrap_err(),
                StatusCode::BAD_REQUEST
            );
        }
        let settings = get_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
        )
        .await
        .unwrap()
        .0;
        assert!(!settings.decks[0].enabled);
        assert!(settings.decks[0].recipients.is_empty());
        let mut bad = sample_upload(0, false);
        bad.clock.offset_west_min = i64::MAX;
        assert_eq!(
            upload(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(bad)
            )
            .await
            .unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        let mut duplicate = sample_upload(0, false);
        let repeated = duplicate.decks.as_ref().unwrap()[0].clone();
        duplicate.decks.as_mut().unwrap().push(repeated);
        assert_eq!(
            upload(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(duplicate)
            )
            .await
            .unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        let mut oversized = sample_upload(0, false);
        let deck = oversized.decks.as_ref().unwrap()[0].clone();
        oversized.decks = Some(vec![deck; MAX_DECKS + 1]);
        assert_eq!(
            upload(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(oversized)
            )
            .await
            .unwrap_err(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
