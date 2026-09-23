use crate::game::{Clock, Event};
use crate::store::{Error, Store};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const MAX_DECKS: usize = 2000;
const MAX_RECIPIENTS: usize = 100;
const INBOX_AGE_MS: i64 = 7 * 86_400_000;
pub const ACTIVITY_DAYS: u32 = 90;
const ACTIVITY_AGE_MS: i64 = ACTIVITY_DAYS as i64 * 86_400_000;
const PUSH_LEASE_MS: i64 = 60_000;
const MAX_SEPARATE_PUSHES: usize = 3;
pub const MAX_MESSAGE: usize = 200;

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
}

#[derive(Debug, Serialize)]
pub struct Notification {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub day: i64,
    pub created_at: i64,
    /// Who this is about, and so who a reply goes to. Empty for older rows.
    pub sender: String,
    pub replied: bool,
    /// What put this here: "completion", "reply", "nudge" or "message". Clients
    /// choose how loudly to announce each.
    pub kind: String,
    /// Unix seconds, independently of push/delivery state.
    pub read_at: Option<i64>,
    pub challenge_id: Option<i64>,
    pub route: Option<String>,
    pub action_required: bool,
}

#[derive(Debug, Serialize)]
pub struct Activity {
    pub items: Vec<Notification>,
    pub unread_count: u64,
    pub action_count: u64,
    pub retention_days: u32,
    pub window_days: u32,
    pub next_before: Option<i64>,
}

fn notification(row: &rusqlite::Row<'_>) -> rusqlite::Result<Notification> {
    let challenge_id: Option<i64> = row.get(9)?;
    Ok(Notification {
        id: row.get(0)?,
        title: row.get(1)?,
        body: row.get(2)?,
        day: row.get(3)?,
        created_at: row.get(4)?,
        sender: row.get(5)?,
        replied: row.get(6)?,
        kind: row.get(7)?,
        read_at: row.get(8)?,
        challenge_id,
        route: challenge_id.map(|id| format!("/community#challenge-{id}")),
        action_required: row.get(10)?,
    })
}

// Read and actionable are independent: reading an invitation does not answer it.
const NOTICE_COLUMNS: &str =
    "n.id,n.title,n.body,n.day,n.created_at / 1000,n.sender,n.replied,n.kind,
 n.read_at / 1000,n.challenge_id,exists(select 1 from community_members m
 join community_challenges c on c.id=m.challenge where c.id=n.challenge_id
 and n.kind='challenge_invite' and m.user=n.recipient and m.status='invited'
 and c.cancelled=0 and c.end_at>?2)";

/// A completion that was just announced, echoed back so the client can say so locally.
#[derive(Debug, Serialize)]
pub struct Announcement {
    pub deck: String,
    pub recipients: usize,
}

/// A notification written by the server itself: a nudge, or a message sent by hand.
pub struct Outgoing<'a> {
    pub to: &'a str,
    /// Empty when nobody can be answered.
    pub from: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub kind: &'a str,
}

pub struct Delivery {
    pub user: String,
    pub notification: Notification,
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
             nudges integer not null default 0
         ) without rowid;
         create table if not exists incoming_settings (
             user text primary key,
             enabled integer not null default 1
         ) without rowid;
         create table if not exists incoming_muted_senders (
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
    ] {
        let present = conn
            .prepare("select 1 from pragma_table_info('notifications') where name = ?1")?
            .exists([column])?;
        if !present {
            conn.execute(
                &format!("alter table notifications add column {column} {definition}"),
                [],
            )?;
        }
    }
    conn.execute(
        "create index if not exists notifications_pending on notifications (pushed, retry_at, id)",
        [],
    )?;
    // Older servers left nudges queued after opt-out. Retain their inbox history
    // without resuming those pushes when the player next enables nudges.
    conn.execute(
        "update notifications set push_cancelled = 1
         where pushed = 0 and push_cancelled = 0 and kind = 'nudge'
         and not exists (select 1 from player_settings
                         where user = notifications.recipient and nudges = 1)",
        [],
    )?;
    Ok(())
}

fn insert_notification(
    conn: &Connection,
    message: &Outgoing<'_>,
    day: i64,
    now_ms: i64,
    push_only: bool,
) -> Result<i64, Error> {
    // Keep suppressed completions permanently quiet even when another internal
    // producer uses this insertion helper instead of record_decks.
    let cancelled = message.kind == "completion"
        && !conn.query_row(
            "select not exists (select 1 from incoming_settings where user = ?1 and enabled = 0)
             and not exists (select 1 from (select recipient,sender from incoming_muted_senders
                         union all select recipient,sender from sender_unsubscriptions) where recipient = ?1 and sender = ?2)",
            params![message.to, message.from],
            |r| r.get::<_, bool>(0),
        )?;
    conn.execute(
        "insert into notifications (recipient, sender, title, body, day, created_at, kind, push_only,
                                    completion_cancelled, push_cancelled)
         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        params![message.to, message.from, message.title, message.body, day, now_ms, message.kind, push_only, cancelled],
    )?;
    Ok(conn.last_insert_rowid())
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
            .query_map(params![user, crate::now_ms() - INBOX_AGE_MS], |r| r.get(0))?
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
                .prepare(
                    "select recipient from deck_recipients
                     where owner = ?1 and deck_id = ?2 and recipient != ?1
                     and not exists (select 1 from subscription_baselines b
                                     where b.sender=?1 and b.deck_id=?2 and b.recipient=deck_recipients.recipient and b.day=?3)
                     and not exists (select 1 from incoming_settings
                                     where user = deck_recipients.recipient and enabled = 0)
                     and not exists (select 1 from (select recipient,sender from incoming_muted_senders
                         union all select recipient,sender from sender_unsubscriptions)
                                     where recipient = deck_recipients.recipient and sender = ?1)",
                )?
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

    pub fn notifications(&self, user: &str, now_ms: i64) -> Result<Vec<Notification>, Error> {
        let mut stmt = self.conn.prepare(&format!(
            "select {NOTICE_COLUMNS} from notifications n where n.id in (
                 select id from notifications where recipient=?1 and push_only=0
                 and created_at>=?3
                 and (kind != 'completion' or (completion_cancelled = 0
                     and not exists (select 1 from incoming_settings where user = ?1 and enabled = 0)
                     and not exists (select 1 from (select recipient,sender from incoming_muted_senders
                         union all select recipient,sender from sender_unsubscriptions)
                                     where recipient = ?1 and sender = notifications.sender)))
                 order by id desc limit 500) order by n.id"
        ))?;
        Ok(stmt
            .query_map(params![user, now_ms, now_ms - INBOX_AGE_MS], notification)?
            .collect::<Result<_, _>>()?)
    }

    pub fn activity(
        &self,
        user: &str,
        now_ms: i64,
        days: u32,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Activity, Error> {
        let cutoff = now_ms - i64::from(days) * 86_400_000;
        let mut stmt = self.conn.prepare(&format!(
            "select {NOTICE_COLUMNS} from notifications n
            where n.recipient=?1 and n.push_only=0 and n.created_at>=?3 and (?4 is null or n.id<?4)
            and (n.kind != 'completion' or (n.completion_cancelled = 0
                and not exists (select 1 from incoming_settings where user = ?1 and enabled = 0)
                and not exists (select 1 from (select recipient,sender from incoming_muted_senders
                         union all select recipient,sender from sender_unsubscriptions)
                                where recipient = ?1 and sender = n.sender)))
            order by n.id desc limit ?5"
        ))?;
        let mut items = stmt
            .query_map(
                params![user, now_ms, cutoff, before, i64::from(limit) + 1],
                notification,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let next_before = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items.last().map(|item| item.id)
        } else {
            None
        };
        let (unread_count, action_count) = self.conn.query_row(
            "select coalesce(sum(n.read_at is null),0),coalesce(sum(exists(
              select 1 from community_members m join community_challenges c on c.id=m.challenge
              where c.id=n.challenge_id and n.kind='challenge_invite' and m.user=n.recipient
              and m.status='invited' and c.cancelled=0 and c.end_at>?2)),0)
             from notifications n where n.recipient=?1 and n.push_only=0 and n.created_at>=?3
             and (n.kind != 'completion' or (n.completion_cancelled = 0
                 and not exists (select 1 from incoming_settings where user = ?1 and enabled = 0)
                 and not exists (select 1 from (select recipient,sender from incoming_muted_senders
                         union all select recipient,sender from sender_unsubscriptions)
                                 where recipient = ?1 and sender = n.sender)))",
            params![user, now_ms, cutoff],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
        )?;
        Ok(Activity {
            items,
            unread_count,
            action_count,
            retention_days: ACTIVITY_DAYS,
            window_days: days,
            next_before,
        })
    }

    pub fn read_activity(&mut self, user: &str, ids: &[i64], now_ms: i64) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        for id in ids {
            tx.execute(
                "update notifications set read_at=?3 where id=?1 and recipient=?2
                and push_only=0 and read_at is null and created_at>=?4",
                params![id, user, now_ms, now_ms - ACTIVITY_AGE_MS],
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
                 where recipient = ?1 and kind = 'nudge' and pushed = 0",
                [user],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Puts one message straight into a player's inbox, for the `message` command.
    /// The delivery loop pushes it like any other notification.
    pub fn send(&mut self, message: &Outgoing<'_>, day: i64, now_ms: i64) -> Result<i64, Error> {
        insert_notification(&self.conn, message, day, now_ms, false)
    }

    /// The seen key and its durable delivery commit or roll back together.
    pub fn send_once(
        &mut self,
        message: &Outgoing<'_>,
        key: &str,
        day: i64,
        now_ms: i64,
        push_only: bool,
    ) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        if tx.execute(
            "insert or ignore into seen (user, key) values (?1, ?2)",
            [message.to, key],
        )? > 0
        {
            insert_notification(&tx, message, day, now_ms, push_only)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Preserve the existing grouped ntfy announcements without duplicating phone
    /// inbox alerts. Accounts without ntfy retain the existing silent baseline.
    pub fn queue_events(
        &mut self,
        user: &str,
        events: &[Event],
        day: i64,
        now_ms: i64,
        push_enabled: bool,
    ) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        let mut fresh = Vec::new();
        for event in events {
            if tx.execute(
                "insert or ignore into seen (user, key) values (?1, ?2)",
                [user, &event.key],
            )? > 0
            {
                fresh.push(event);
            }
        }
        if push_enabled {
            if fresh.len() > MAX_SEPARATE_PUSHES {
                let titles: Vec<_> = fresh.iter().map(|event| event.title.as_str()).collect();
                insert_notification(
                    &tx,
                    &Outgoing {
                        to: user,
                        from: "",
                        title: &format!("{} new unlocks", fresh.len()),
                        body: &titles.join(", "),
                        kind: "event",
                    },
                    day,
                    now_ms,
                    true,
                )?;
            } else {
                for event in fresh {
                    insert_notification(
                        &tx,
                        &Outgoing {
                            to: user,
                            from: "",
                            title: &event.title,
                            body: &event.body,
                            kind: "event",
                        },
                        day,
                        now_ms,
                        true,
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Answers one notification, once, and lets the answer be answered in turn.
    /// Returns who heard it, or `None` when there is nothing left to reply to.
    pub fn reply(
        &mut self,
        user: &str,
        display: &str,
        id: i64,
        message: &str,
        now_ms: i64,
    ) -> Result<Option<String>, Error> {
        let tx = self.conn.transaction()?;
        let target = tx
            .query_row(
                "select sender, day from notifications
                 where id = ?1 and recipient = ?2 and replied = 0 and sender not in ('', ?2)
                 and completion_cancelled = 0",
                params![id, user],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        let Some((sender, day)) = target else {
            return Ok(None);
        };
        tx.execute(
            "update notifications set replied = 1 where id = ?1",
            params![id],
        )?;
        tx.execute(
            "insert into notifications (recipient, sender, title, body, day, created_at, kind)
             values (?1, ?2, ?3, ?4, ?5, ?6, 'reply')",
            params![sender, user, format!("💬 {display}"), message, day, now_ms],
        )?;
        tx.commit()?;
        Ok(Some(sender))
    }

    /// Read a bounded batch, leaving it pending until ntfy confirms success.
    /// Retry times put newly queued messages ahead of previously failed pushes.
    pub fn take_deck_deliveries(&mut self, now_ms: i64) -> Result<Vec<Delivery>, Error> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "delete from notifications where created_at < ?1",
            [now_ms - ACTIVITY_AGE_MS],
        )?;
        let deliveries = {
            let mut stmt = tx.prepare(
                "with ready as (
                     select *, row_number() over (partition by recipient order by retry_at, id) as turn
                     from notifications where pushed = 0 and push_cancelled = 0 and retry_at <= ?1
                     and (kind != 'completion' or (completion_cancelled = 0
                         and not exists (select 1 from incoming_settings
                                         where user = notifications.recipient and enabled = 0)
                         and not exists (select 1 from (select recipient,sender from incoming_muted_senders
                         union all select recipient,sender from sender_unsubscriptions)
                                         where recipient = notifications.recipient
                                         and sender = notifications.sender)))
                       and created_at >= ?2
                 ), attempted as (
                     select recipient, max(retry_at) as last_retry from notifications
                     where push_attempts > 0 group by recipient
                 )
                 select ready.recipient, id, title, body, day, created_at / 1000, sender, replied, kind, read_at / 1000, challenge_id
                 from ready left join attempted using (recipient)
                 order by turn, coalesce(last_retry, 0), retry_at, id limit 100",
            )?;
            stmt.query_map(params![now_ms, now_ms - INBOX_AGE_MS], |r| {
                Ok(Delivery {
                    user: r.get(0)?,
                    notification: Notification {
                        id: r.get(1)?,
                        title: r.get(2)?,
                        body: r.get(3)?,
                        day: r.get(4)?,
                        created_at: r.get(5)?,
                        sender: r.get(6)?,
                        replied: r.get(7)?,
                        kind: r.get(8)?,
                        read_at: r.get(9)?,
                        challenge_id: r.get(10)?,
                        route: r
                            .get::<_, Option<i64>>(10)?
                            .map(|id| format!("/community#challenge-{id}")),
                        action_required: false,
                    },
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        tx.commit()?;
        Ok(deliveries)
    }

    /// Lease only the message about to be attempted. A stopped worker is retried
    /// after a minute, while unattempted messages stay available to the next tick.
    pub fn begin_push(&mut self, id: i64, now_ms: i64) -> Result<bool, Error> {
        Ok(self.conn.execute(
            "update notifications set retry_at = ?2, push_attempts = min(push_attempts + 1, 31)
             where id = ?1 and pushed = 0 and push_cancelled = 0 and retry_at <= ?3 and created_at>=?4
             and (kind != 'completion' or (completion_cancelled = 0
                 and not exists (select 1 from incoming_settings
                                 where user = notifications.recipient and enabled = 0)
                 and not exists (select 1 from (select recipient,sender from incoming_muted_senders
                         union all select recipient,sender from sender_unsubscriptions)
                                 where recipient = notifications.recipient
                                 and sender = notifications.sender)))
             and (kind != 'nudge' or exists (select 1 from player_settings
                                            where user = notifications.recipient and nudges = 1))",
            params![id, now_ms + PUSH_LEASE_MS, now_ms, now_ms - INBOX_AGE_MS],
        )? > 0)
    }

    pub fn defer_push(&mut self, id: i64, now_ms: i64) -> Result<(), Error> {
        self.conn.execute(
            "update notifications set retry_at = ?2 where id = ?1 and pushed = 0",
            params![id, now_ms + PUSH_LEASE_MS],
        )?;
        Ok(())
    }

    pub fn cancel_streak_warning(&mut self, id: i64) -> Result<(), Error> {
        // These were always ntfy-only. Keep the seen key, and never delete a
        // message from the player's inbox when discarding stale warnings.
        self.conn.execute(
            "delete from notifications where id = ?1 and push_only = 1 and kind = 'risk'",
            [id],
        )?;
        Ok(())
    }

    pub fn finish_push(&mut self, id: i64, succeeded: bool, now_ms: i64) -> Result<(), Error> {
        let attempts: u32 = self.conn.query_row(
            "select push_attempts from notifications where id = ?1",
            [id],
            |row| row.get(0),
        )?;
        let delay = (20_000 * (1_i64 << attempts.saturating_sub(1).min(6))).min(15 * 60_000);
        self.conn.execute(
            "update notifications set pushed = ?2, retry_at = ?3 where id = ?1 and pushed = 0",
            params![id, succeeded, now_ms + delay],
        )?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn activity_retention_pagination_reads_and_delivery_are_independent() {
        let (mut store, path) = temporary_store();
        let send = |store: &mut Store, to: &str, age: i64| {
            store
                .send(
                    &Outgoing {
                        to,
                        from: "sender",
                        title: "Message",
                        body: "Hello",
                        kind: "message",
                    },
                    DAY,
                    NOW - age * 86_400_000,
                )
                .unwrap()
        };
        let recent = send(&mut store, "hill", 0);
        let peer = send(&mut store, "friend", 0);
        let eight = send(&mut store, "hill", 8);
        let forty = send(&mut store, "hill", 40);
        let expired = send(&mut store, "hill", 91);
        store
            .send_once(
                &Outgoing {
                    to: "hill",
                    from: "",
                    title: "Push only",
                    body: "Hidden",
                    kind: "event",
                },
                "test-push",
                DAY,
                NOW,
                true,
            )
            .unwrap();
        let page = store.activity("hill", NOW, 90, None, 1).unwrap();
        assert_eq!(
            (
                page.unread_count,
                page.action_count,
                page.retention_days,
                page.window_days
            ),
            (3, 0, 90, 90)
        );
        assert_eq!(page.items[0].id, forty);
        assert_eq!(page.next_before, Some(forty));
        let next = store
            .activity("hill", NOW, 90, page.next_before, 1)
            .unwrap();
        assert_eq!(next.items[0].id, eight);
        assert_eq!(
            next.unread_count, 3,
            "counts include the whole window, not just this page"
        );
        assert_eq!(
            store
                .activity("hill", NOW, 30, None, 200)
                .unwrap()
                .unread_count,
            2
        );
        assert_eq!(
            store
                .notifications("hill", NOW)
                .unwrap()
                .iter()
                .map(|n| n.id)
                .collect::<Vec<_>>(),
            [recent]
        );
        store
            .read_activity("hill", &[recent, recent, peer, expired, i64::MAX], NOW)
            .unwrap();
        store.read_activity("hill", &[recent], NOW + 1000).unwrap();
        assert_eq!(
            store
                .activity("friend", NOW, 90, None, 100)
                .unwrap()
                .unread_count,
            1
        );
        drop(store);
        let mut store = Store::open(&path).unwrap();
        let activity = store.activity("hill", NOW, 90, None, 100).unwrap();
        assert_eq!(activity.unread_count, 2);
        assert_eq!(
            activity
                .items
                .iter()
                .find(|n| n.id == recent)
                .unwrap()
                .read_at,
            Some(NOW / 1000)
        );
        let deliveries = store.take_deck_deliveries(NOW).unwrap();
        assert!(
            deliveries.iter().any(|d| d.notification.id == recent),
            "reading does not consume a delivery"
        );
        assert!(
            deliveries
                .iter()
                .all(|d| ![eight, forty, expired].contains(&d.notification.id))
        );
        assert!(!store.begin_push(eight, NOW).unwrap());
        assert_eq!(
            store
                .activity("hill", NOW, 90, None, 100)
                .unwrap()
                .items
                .len(),
            3
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "select count(*) from notifications where id=?1",
                    [expired],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
    use std::sync::atomic::{AtomicU64, Ordering};

    const DAY: i64 = 20_000;
    const NOW: i64 = DAY * 86_400_000 + 12 * 3_600_000;

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
        let now = crate::now_ms();
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
    fn taking_a_delivery_does_not_mark_an_unconfirmed_push_delivered() {
        let (mut store, path) = temporary_store();
        let id = store
            .send(
                &Outgoing {
                    to: "hill",
                    from: "",
                    title: "Hello",
                    body: "Try again",
                    kind: "message",
                },
                DAY,
                NOW,
            )
            .unwrap();
        assert_eq!(store.take_deck_deliveries(NOW).unwrap().len(), 1);
        let pushed: bool = store
            .conn
            .query_row(
                "select pushed from notifications where id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!pushed, "selecting a message is not confirmation from ntfy");
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn a_backlogged_recipient_does_not_hide_other_recipients() {
        let (mut store, path) = temporary_store();
        for _ in 0..101 {
            store
                .send(
                    &Outgoing {
                        to: "hill",
                        from: "",
                        title: "Queued",
                        body: "Waiting",
                        kind: "message",
                    },
                    DAY,
                    NOW,
                )
                .unwrap();
        }
        let friend = store
            .send(
                &Outgoing {
                    to: "friend",
                    from: "",
                    title: "Ready",
                    body: "Deliver me",
                    kind: "message",
                },
                DAY,
                NOW,
            )
            .unwrap();
        let batch = store.take_deck_deliveries(NOW).unwrap();
        assert!(
            batch
                .iter()
                .take(2)
                .any(|delivery| delivery.notification.id == friend),
            "take turns between recipients before draining a large backlog"
        );
        assert!(store.begin_push(batch[0].notification.id, NOW).unwrap());
        assert_eq!(
            store.take_deck_deliveries(NOW).unwrap()[0].notification.id,
            friend,
            "a new tick favors recipients that have not had an attempt yet"
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn failed_pushes_back_off_survive_restart_and_stop_after_success() {
        let (mut store, path) = temporary_store();
        let id = store
            .send(
                &Outgoing {
                    to: "hill",
                    from: "",
                    title: "Retry",
                    body: "Keep me",
                    kind: "message",
                },
                DAY,
                NOW,
            )
            .unwrap();
        let mut at = NOW;
        for attempt in 0..9 {
            assert_eq!(
                store.take_deck_deliveries(at).unwrap()[0].notification.id,
                id
            );
            assert!(store.begin_push(id, at).unwrap());
            assert!(
                !store.begin_push(id, at).unwrap(),
                "another worker cannot claim the same attempt"
            );
            store.finish_push(id, false, at).unwrap();
            let delay = (20_000 * (1_i64 << attempt.min(6))).min(15 * 60_000);
            assert!(
                store
                    .take_deck_deliveries(at + delay - 1)
                    .unwrap()
                    .is_empty()
            );
            drop(store);
            store = Store::open(&path).unwrap();
            at += delay;
        }
        assert!(store.begin_push(id, at).unwrap());
        store.finish_push(id, true, at).unwrap();
        assert!(store.take_deck_deliveries(at + 900_000).unwrap().is_empty());
        assert_eq!(
            store.notifications("hill", at).unwrap().len(),
            1,
            "retry never creates a new inbox notification"
        );
        assert!(
            store
                .take_deck_deliveries(NOW + INBOX_AGE_MS + 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .conn
                .query_row("select count(*) from notifications", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1,
            "delivery expiration retains durable Activity history"
        );
        assert!(
            store
                .take_deck_deliveries(NOW + ACTIVITY_AGE_MS + 1)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .activity("hill", NOW + ACTIVITY_AGE_MS + 1, 90, None, 100)
                .unwrap()
                .items
                .is_empty()
        );
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

    #[test]
    fn interrupted_pushes_retry_after_the_lease_and_leave_other_messages_available() {
        let (mut store, path) = temporary_store();
        let first = store
            .send(
                &Outgoing {
                    to: "hill",
                    from: "",
                    title: "First",
                    body: "Retry",
                    kind: "message",
                },
                DAY,
                NOW,
            )
            .unwrap();
        let second = store
            .send(
                &Outgoing {
                    to: "friend",
                    from: "",
                    title: "Second",
                    body: "Ready",
                    kind: "message",
                },
                DAY,
                NOW,
            )
            .unwrap();
        assert!(store.begin_push(first, NOW).unwrap());
        drop(store);
        let mut store = Store::open(&path).unwrap();
        let ready = store.take_deck_deliveries(NOW + PUSH_LEASE_MS - 1).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].notification.id, second);
        assert_eq!(
            store
                .take_deck_deliveries(NOW + PUSH_LEASE_MS)
                .unwrap()
                .len(),
            2
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn seen_keys_and_notifications_commit_together_and_push_only_stays_private() {
        let (mut store, path) = temporary_store();
        let event = Event {
            key: "level:2".into(),
            title: "Level 2".into(),
            body: "Well done".into(),
        };
        let message = Outgoing {
            to: "hill",
            from: "",
            title: "Nudge",
            body: "Almost there",
            kind: "nudge",
        };
        store.conn.execute_batch("create temp trigger reject_notification before insert on notifications begin select raise(abort, 'test failure'); end;").unwrap();
        assert!(
            store
                .queue_events("hill", std::slice::from_ref(&event), DAY, NOW, true)
                .is_err()
        );
        assert!(
            store
                .send_once(&message, "nudge:today", DAY, NOW, false)
                .is_err()
        );
        assert_eq!(
            store
                .conn
                .query_row("select count(*) from seen", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        store
            .conn
            .execute_batch("drop trigger reject_notification")
            .unwrap();
        for _ in 0..2 {
            store
                .queue_events("hill", std::slice::from_ref(&event), DAY, NOW, true)
                .unwrap();
            store
                .send_once(&message, "nudge:today", DAY, NOW, false)
                .unwrap();
        }
        let inbox = store.notifications("hill", NOW).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].kind, "nudge");
        assert_eq!(store.take_deck_deliveries(NOW).unwrap().len(), 2);
        store
            .queue_events("friend", &[event], DAY, NOW, false)
            .unwrap();
        assert!(store.notifications("friend", NOW).unwrap().is_empty());
        assert!(
            !store.mark_seen("friend", "level:2").unwrap(),
            "accounts without ntfy keep their silent event baseline"
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    pub fn temporary_store() -> (Store, std::path::PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ankiquest-decks-{}-{}-{}",
            std::process::id(),
            crate::now_ms(),
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
    fn an_inbox_from_before_replies_keeps_working() {
        let (store, path) = temporary_store();
        drop(store);
        let conn = Connection::open(path.join("ankiquest.db")).unwrap();
        conn.execute_batch(
            "drop table notifications;
             create table notifications (
                 id integer primary key autoincrement,
                 recipient text not null,
                 title text not null,
                 body text not null,
                 day integer not null,
                 created_at integer not null,
                 pushed integer not null default 0
             );
             insert into notifications (recipient, title, body, day, created_at)
             values ('hill', 'Deck complete', 'Cerro finished Spanish.', 20000, 1728000000000);",
        )
        .unwrap();
        drop(conn);

        let mut store = Store::open(&path).unwrap();
        let inbox = store.notifications("hill", 1_728_000_000_000).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].sender, "");
        assert!(!inbox[0].replied);
        assert!(
            store
                .reply("hill", "Hill", inbox[0].id, "Good job!", NOW)
                .unwrap()
                .is_none(),
            "nobody is recorded as the sender of a pre-reply notification"
        );
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
}
