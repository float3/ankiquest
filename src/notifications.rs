//! The durable inbox: notifications, the activity feed, replies, and the push queue.

use crate::decks::completion_allowed_sql;
use crate::game::{DAY_MS, Event};
use crate::store::{Error, Store};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub(crate) const INBOX_AGE_MS: i64 = 7 * DAY_MS;
pub const ACTIVITY_DAYS: u32 = 90;
const ACTIVITY_AGE_MS: i64 = ACTIVITY_DAYS as i64 * DAY_MS;
pub(crate) const PUSH_LEASE_MS: i64 = 60_000;
const MAX_SEPARATE_PUSHES: usize = 3;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityCategory {
    Messages,
    Challenges,
    DeckCompletions,
    StudyUpdates,
    NeedsAction,
}

impl ActivityCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::Challenges => "challenges",
            Self::DeckCompletions => "deck_completions",
            Self::StudyUpdates => "study_updates",
            Self::NeedsAction => "needs_action",
        }
    }
}

#[derive(Clone, Copy)]
pub struct ActivityOptions {
    pub days: u32,
    pub before: Option<i64>,
    pub limit: u32,
    pub category: Option<ActivityCategory>,
    pub unread_only: bool,
}

const ACTIVITY_CATEGORY_SQL: &str = "case
    when n.kind in ('message','reply','nudge') then 'messages'
    when n.kind glob 'challenge_*' then 'challenges'
    when n.kind in ('completion','conquest') then 'deck_completions'
    else 'study_updates' end";

macro_rules! action_required_sql {
    () => {
        "exists(select 1 from community_members m
    join community_challenges c on c.id=m.challenge where c.id=n.challenge_id
    and n.kind='challenge_invite' and m.user=n.recipient and m.status='invited'
    and c.cancelled=0 and c.end_at>?2)"
    };
}

const ACTION_REQUIRED_SQL: &str = action_required_sql!();

/// A nudge row of `notifications` that its recipient still wants pushed.
pub(crate) const NUDGE_ALLOWED_SQL: &str = "((sender = '' and exists (select 1 from player_settings
                         where user = notifications.recipient and nudges = 1))
    or (sender != '' and exists (select 1 from player_settings
                         where user = notifications.recipient and friend_nudges = 1)
        and not exists (select 1 from friend_nudge_muted_senders
                         where recipient = notifications.recipient and sender = notifications.sender)))";
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
    /// What put this here: "completion", "conquest", "reply", "nudge" or "message". Clients
    /// choose how loudly to announce each.
    pub kind: String,
    /// Unix seconds, independently of push/delivery state.
    pub read_at: Option<i64>,
    pub challenge_id: Option<i64>,
    pub route: Option<String>,
    pub action_required: bool,
    /// The exact alert this response answered, retained even if the older row expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<ReplyContext>,
    /// What a celebration is about ("achievement", "level", "streak", "record" or
    /// "conquered"), or "place:N" for a weekly recap, so clients can pick matching art.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ReplyContext {
    pub title: String,
    pub body: String,
}

/// A reply or friend nudge this player wrote; `read_at` is when the recipient read it.
#[derive(Debug, Serialize)]
pub struct Sent {
    pub recipient: String,
    #[serde(flatten)]
    pub notice: Notification,
}

#[derive(Debug, Serialize)]
pub struct SentPage {
    pub items: Vec<Sent>,
    pub next_before: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct Activity {
    pub items: Vec<Notification>,
    pub attention: Vec<Notification>,
    pub unread_count: u64,
    pub action_count: u64,
    pub latest_id: Option<i64>,
    pub retention_days: u32,
    pub window_days: u32,
    pub next_before: Option<i64>,
}

fn notification(row: &rusqlite::Row<'_>) -> rusqlite::Result<Notification> {
    let challenge_id: Option<i64> = row.get(9)?;
    let reply_title: Option<String> = row.get(11)?;
    let reply_body: Option<String> = row.get(12)?;
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
        reply_to: reply_title
            .zip(reply_body)
            .map(|(title, body)| ReplyContext { title, body }),
        topic: row.get(13)?,
    })
}

// Read and actionable are independent: reading an invitation does not answer it.
const NOTICE_COLUMNS: &str = concat!(
    "n.id,n.title,n.body,n.day,n.created_at / 1000,n.sender,n.replied,n.kind,
 n.read_at / 1000,n.challenge_id,",
    action_required_sql!(),
    ",n.reply_to_title,n.reply_to_body,n.topic"
);
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
fn is_celebration(key: &str) -> bool {
    ["achievement:", "level:", "streak:", "record:", "conquered:"]
        .iter()
        .any(|prefix| key.starts_with(prefix))
}

pub(crate) fn insert_notification(
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
            &format!("select {}", completion_allowed_sql("?1", "?2")),
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

/// The part of an event key before its first colon, e.g. "streak" for "streak:30".
fn topic_of(key: &str) -> &str {
    key.split_once(':').map_or(key, |(topic, _)| topic)
}

fn set_topic(conn: &Connection, id: i64, topic: Option<&str>) -> Result<(), Error> {
    if topic.is_some() {
        conn.execute(
            "update notifications set topic = ?1 where id = ?2",
            params![topic, id],
        )?;
    }
    Ok(())
}

impl Store {
    pub fn notifications(&self, user: &str, now_ms: i64) -> Result<Vec<Notification>, Error> {
        let mut stmt = self.conn.prepare(&format!(
            "select {NOTICE_COLUMNS} from notifications n where n.id in (
                 select id from notifications where recipient=?1 and push_only=0
                 and created_at>=?3
                 and (kind != 'completion' or (completion_cancelled = 0 and {}))
                 order by id desc limit 500) order by n.id",
            completion_allowed_sql("?1", "notifications.sender")
        ))?;
        Ok(stmt
            .query_map(params![user, now_ms, now_ms - INBOX_AGE_MS], notification)?
            .collect::<Result<_, _>>()?)
    }

    #[cfg(test)]
    pub fn activity(
        &self,
        user: &str,
        now_ms: i64,
        days: u32,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Activity, Error> {
        self.activity_filtered(
            user,
            now_ms,
            ActivityOptions {
                days,
                before,
                limit,
                category: None,
                unread_only: false,
            },
        )
    }

    pub fn activity_filtered(
        &self,
        user: &str,
        now_ms: i64,
        options: ActivityOptions,
    ) -> Result<Activity, Error> {
        let ActivityOptions {
            days,
            before,
            limit,
            category,
            unread_only,
        } = options;
        let cutoff = now_ms - i64::from(days) * DAY_MS;
        let category = category.map(ActivityCategory::as_str);
        let mut stmt = self.conn.prepare(&format!(
            "select {NOTICE_COLUMNS} from notifications n
            where n.recipient=?1 and n.push_only=0 and n.created_at>=?3 and (?4 is null or n.id<?4)
            and (n.kind != 'completion' or (n.completion_cancelled = 0 and {}))
            and (?6 is null or (?6='needs_action' and {ACTION_REQUIRED_SQL})
                or (?6!='needs_action' and ({ACTIVITY_CATEGORY_SQL})=?6))
            and (?7=0 or n.read_at is null)
            order by n.id desc limit ?5",
            completion_allowed_sql("?1", "n.sender")
        ))?;
        let mut items = stmt
            .query_map(
                params![
                    user,
                    now_ms,
                    cutoff,
                    before,
                    i64::from(limit) + 1,
                    category,
                    unread_only
                ],
                notification,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let next_before = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items.last().map(|item| item.id)
        } else {
            None
        };
        let mut attention_stmt = self.conn.prepare(&format!(
            "select {NOTICE_COLUMNS} from notifications n
             where n.recipient=?1 and n.push_only=0 and n.created_at>=?3
             and {ACTION_REQUIRED_SQL}
             order by n.id desc limit 3"
        ))?;
        let attention = attention_stmt
            .query_map(params![user, now_ms, cutoff], notification)?
            .collect::<Result<Vec<_>, _>>()?;
        let (unread_count, action_count, latest_id) = self.conn.query_row(
            &format!(
                "select coalesce(sum(n.read_at is null),0),coalesce(sum({ACTION_REQUIRED_SQL}),0),max(n.id)
                 from notifications n where n.recipient=?1 and n.push_only=0 and n.created_at>=?3
                 and (n.kind != 'completion' or (n.completion_cancelled = 0 and {}))",
                completion_allowed_sql("?1", "n.sender")
            ),
            params![user, now_ms, cutoff],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, i64>(1)? as u64,
                    r.get(2)?,
                ))
            },
        )?;
        Ok(Activity {
            items,
            attention,
            unread_count,
            action_count,
            latest_id,
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

    /// Marks everything up to `through` read, so items that arrived after the
    /// reader's last refresh stay unread.
    pub fn read_all_activity(
        &mut self,
        user: &str,
        through: i64,
        now_ms: i64,
    ) -> Result<(), Error> {
        self.conn.execute(
            "update notifications set read_at=?3 where recipient=?1 and id<=?2
            and push_only=0 and read_at is null and created_at>=?4",
            params![user, through, now_ms, now_ms - ACTIVITY_AGE_MS],
        )?;
        Ok(())
    }

    pub fn sent(
        &self,
        user: &str,
        now_ms: i64,
        before: Option<i64>,
        limit: u32,
    ) -> Result<SentPage, Error> {
        let mut stmt = self.conn.prepare(&format!(
            "select {NOTICE_COLUMNS},n.recipient from notifications n
            where n.sender=?1 and n.kind in ('reply','nudge') and n.push_only=0
            and n.created_at>=?3 and (?4 is null or n.id<?4)
            order by n.id desc limit ?5"
        ))?;
        let mut items = stmt
            .query_map(
                params![
                    user,
                    now_ms,
                    now_ms - ACTIVITY_AGE_MS,
                    before,
                    i64::from(limit) + 1
                ],
                |row| {
                    Ok(Sent {
                        notice: notification(row)?,
                        recipient: row.get(14)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let next_before = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items.last().map(|item| item.notice.id)
        } else {
            None
        };
        Ok(SentPage { items, next_before })
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
        self.send_once_about(message, None, key, day, now_ms, push_only)
    }

    /// [`Store::send_once`] with a [`Notification::topic`].
    pub fn send_once_about(
        &mut self,
        message: &Outgoing<'_>,
        topic: Option<&str>,
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
            let id = insert_notification(&tx, message, day, now_ms, push_only)?;
            set_topic(&tx, id, topic)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Celebrations (achievements, levels, streak milestones, personal bests) become
    /// inbox items every client shows, unless the player turned them off. Other events
    /// keep the grouped ntfy-only announcements. A player's first run, with nothing
    /// seen yet, only records history instead of celebrating all of it at once.
    pub fn queue_events(
        &mut self,
        user: &str,
        events: &[Event],
        day: i64,
        now_ms: i64,
        push_enabled: bool,
    ) -> Result<(), Error> {
        let celebrate = self.celebrations_enabled(user)?;
        let tx = self.conn.transaction()?;
        let baseline = !tx.query_row(
            "select exists (select 1 from seen where user = ?1)",
            [user],
            |r| r.get::<_, bool>(0),
        )?;
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
        let (celebrations, others): (Vec<_>, Vec<_>) = fresh
            .into_iter()
            .partition(|event| is_celebration(&event.key));
        let announce = |events: Vec<&Event>, kind: &str, push_only: bool| -> Result<(), Error> {
            if events.len() > MAX_SEPARATE_PUSHES {
                let titles: Vec<_> = events.iter().map(|event| event.title.as_str()).collect();
                let topic = topic_of(&events[0].key);
                let shared = events.iter().all(|event| topic_of(&event.key) == topic);
                let id = insert_notification(
                    &tx,
                    &Outgoing {
                        to: user,
                        from: "",
                        title: &format!("{} new unlocks", events.len()),
                        body: &titles.join(", "),
                        kind,
                    },
                    day,
                    now_ms,
                    push_only,
                )?;
                set_topic(&tx, id, shared.then_some(topic))?;
            } else {
                for event in events {
                    let id = insert_notification(
                        &tx,
                        &Outgoing {
                            to: user,
                            from: "",
                            title: &event.title,
                            body: &event.body,
                            kind,
                        },
                        day,
                        now_ms,
                        push_only,
                    )?;
                    set_topic(&tx, id, Some(topic_of(&event.key)))?;
                }
            }
            Ok(())
        };
        if !baseline && celebrate && !celebrations.is_empty() {
            announce(celebrations, "celebration", false)?;
        }
        if push_enabled && !others.is_empty() {
            announce(others, "event", true)?;
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
                "select sender, day, title, body from notifications
                 where id = ?1 and recipient = ?2 and replied = 0 and sender not in ('', ?2)
                 and completion_cancelled = 0",
                params![id, user],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((sender, day, title, body)) = target else {
            return Ok(None);
        };
        tx.execute(
            "update notifications set replied = 1 where id = ?1",
            params![id],
        )?;
        tx.execute(
            "insert into notifications (recipient, sender, title, body, day, created_at, kind, reply_to_title, reply_to_body)
             values (?1, ?2, ?3, ?4, ?5, ?6, 'reply', ?7, ?8)",
            params![sender, user, format!("💬 {display}"), message, day, now_ms, title, body],
        )?;
        tx.commit()?;
        Ok(Some(sender))
    }

    /// Read a bounded batch, leaving it pending until ntfy confirms success.
    /// Retry times put newly queued messages ahead of previously failed pushes.
    /// Forgets notifications older than the activity feed reaches back.
    pub fn prune_notifications(&mut self, now_ms: i64) -> Result<(), Error> {
        self.conn.execute(
            "delete from notifications where created_at < ?1",
            [now_ms - ACTIVITY_AGE_MS],
        )?;
        Ok(())
    }

    pub fn take_deck_deliveries(&mut self, now_ms: i64) -> Result<Vec<Delivery>, Error> {
        let tx = self.conn.transaction()?;
        let deliveries = {
            let mut stmt = tx.prepare(&format!(
                "with ready as (
                     select *, row_number() over (partition by recipient order by retry_at, id) as turn
                     from notifications where pushed = 0 and push_cancelled = 0 and retry_at <= ?1
                     and (kind != 'completion' or (completion_cancelled = 0 and {}))
                       and created_at >= ?2
                 ), attempted as (
                     select recipient, max(retry_at) as last_retry from notifications
                     where push_attempts > 0 group by recipient
                 )
                 select id, title, body, day, created_at / 1000, sender, replied, kind, read_at / 1000, challenge_id, 0, reply_to_title, reply_to_body, topic, ready.recipient
                 from ready left join attempted using (recipient)
                 order by turn, coalesce(last_retry, 0), retry_at, id limit 100",
                completion_allowed_sql("notifications.recipient", "notifications.sender")
            ))?;
            stmt.query_map(params![now_ms, now_ms - INBOX_AGE_MS], |r| {
                Ok(Delivery {
                    user: r.get(14)?,
                    notification: notification(r)?,
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
            &format!(
                "update notifications set retry_at = ?2, push_attempts = min(push_attempts + 1, 31)
                 where id = ?1 and pushed = 0 and push_cancelled = 0 and retry_at <= ?3 and created_at>=?4
                 and (kind != 'completion' or (completion_cancelled = 0 and {}))
                 and (kind != 'nudge' or {NUDGE_ALLOWED_SQL})",
                completion_allowed_sql("notifications.recipient", "notifications.sender")
            ),
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

    /// Stops pushing a notification that has nowhere to go. An inbox message
    /// stays in the inbox; a push-only one is dropped, keeping its seen key.
    pub fn cancel_push(&mut self, id: i64) -> Result<(), Error> {
        self.conn.execute(
            "delete from notifications where id = ?1 and pushed = 0 and push_only = 1",
            [id],
        )?;
        self.conn.execute(
            "update notifications set push_cancelled = 1 where id = ?1 and pushed = 0",
            [id],
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
mod tests {
    use super::*;
    use crate::decks::tests::*;
    #[test]
    fn activity_categories_filter_before_pagination_and_keep_global_unread_count() {
        let (mut store, _path) = temporary_store();
        let mut sent = Vec::new();
        for kind in [
            "message",
            "completion",
            "reply",
            "challenge_complete",
            "reminder_daily",
            "message",
        ] {
            sent.push(
                store
                    .send(
                        &Outgoing {
                            to: "hill",
                            from: "friend",
                            title: kind,
                            body: kind,
                            kind,
                        },
                        DAY,
                        NOW,
                    )
                    .unwrap(),
            );
        }
        store.read_activity("hill", &[sent[2]], NOW).unwrap();
        let options = ActivityOptions {
            days: 90,
            before: None,
            limit: 1,
            category: Some(ActivityCategory::Messages),
            unread_only: false,
        };
        let page = store.activity_filtered("hill", NOW, options).unwrap();
        assert_eq!(page.items[0].id, sent[5]);
        assert_eq!(page.next_before, Some(sent[5]));
        assert_eq!(page.unread_count, 5);
        assert_eq!(page.latest_id, Some(sent[5]));
        let older = store
            .activity_filtered(
                "hill",
                NOW,
                ActivityOptions {
                    before: page.next_before,
                    ..options
                },
            )
            .unwrap();
        assert_eq!(older.items[0].id, sent[2]);
        let unread = store
            .activity_filtered(
                "hill",
                NOW,
                ActivityOptions {
                    unread_only: true,
                    limit: 100,
                    ..options
                },
            )
            .unwrap();
        assert_eq!(
            unread.items.iter().map(|n| n.id).collect::<Vec<_>>(),
            [sent[5], sent[0]]
        );
        for (category, expected) in [
            (ActivityCategory::DeckCompletions, sent[1]),
            (ActivityCategory::Challenges, sent[3]),
            (ActivityCategory::StudyUpdates, sent[4]),
        ] {
            let page = store
                .activity_filtered(
                    "hill",
                    NOW,
                    ActivityOptions {
                        category: Some(category),
                        ..options
                    },
                )
                .unwrap();
            assert_eq!(
                page.items.iter().map(|n| n.id).collect::<Vec<_>>(),
                [expected]
            );
        }
        assert!(page.attention.is_empty());
    }

    #[test]
    fn read_invitation_stays_actionable_until_the_member_responds() {
        let (mut store, _path) = temporary_store();
        store
            .conn
            .execute(
                "insert into community_challenges (id,creator,title,kind,cooperative,target,start_at,end_at)
                 values (1,'friend','Study together','reviews',0,10,?1,?2)",
                params![NOW, NOW + 86_400_000],
            )
            .unwrap();
        store
            .conn
            .execute(
                "insert into community_members (challenge,user,status) values (1,'hill','invited')",
                [],
            )
            .unwrap();
        let id = store
            .send(
                &Outgoing {
                    to: "hill",
                    from: "friend",
                    title: "Join me",
                    body: "Study together",
                    kind: "challenge_invite",
                },
                DAY,
                NOW,
            )
            .unwrap();
        store
            .conn
            .execute("update notifications set challenge_id=1 where id=?1", [id])
            .unwrap();
        store.read_activity("hill", &[id], NOW).unwrap();
        let options = ActivityOptions {
            days: 90,
            before: None,
            limit: 20,
            category: Some(ActivityCategory::NeedsAction),
            unread_only: false,
        };
        let pending = store.activity_filtered("hill", NOW, options).unwrap();
        assert_eq!(pending.items[0].id, id);
        assert_eq!(pending.attention[0].id, id);
        assert_eq!(pending.action_count, 1);
        assert_eq!(pending.unread_count, 0);
        assert!(
            store
                .activity_filtered(
                    "hill",
                    NOW,
                    ActivityOptions {
                        unread_only: true,
                        ..options
                    }
                )
                .unwrap()
                .items
                .is_empty()
        );
        store
            .conn
            .execute(
                "update community_members set status='accepted' where challenge=1 and user='hill'",
                [],
            )
            .unwrap();
        let resolved = store.activity_filtered("hill", NOW, options).unwrap();
        assert!(resolved.items.is_empty());
        assert!(resolved.attention.is_empty());
        assert_eq!(resolved.action_count, 0);
    }

    #[test]
    fn read_all_stops_at_what_the_reader_saw() {
        let (mut store, _path) = temporary_store();
        let mut send = |to: &str| {
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
                    NOW,
                )
                .unwrap()
        };
        send("hill");
        let seen = send("hill");
        send("friend");
        let later = send("hill");
        store.read_all_activity("hill", seen, NOW).unwrap();
        let activity = store.activity("hill", NOW, 90, None, 100).unwrap();
        assert_eq!(activity.unread_count, 1);
        assert!(
            activity
                .items
                .iter()
                .find(|n| n.id == later)
                .unwrap()
                .read_at
                .is_none()
        );
        assert_eq!(
            store
                .activity("friend", NOW, 90, None, 100)
                .unwrap()
                .unread_count,
            1
        );
        store.read_all_activity("hill", i64::MAX, NOW).unwrap();
        assert_eq!(
            store
                .activity("hill", NOW, 90, None, 100)
                .unwrap()
                .unread_count,
            0
        );
    }

    #[test]
    fn sent_lists_only_handwritten_messages_with_when_they_were_read() {
        let (mut store, _path) = temporary_store();
        let mut send = |to: &str, from: &str, kind: &str, age_days: i64| {
            store
                .send(
                    &Outgoing {
                        to,
                        from,
                        title: "Title",
                        body: "Body",
                        kind,
                    },
                    DAY,
                    NOW - age_days * 86_400_000,
                )
                .unwrap()
        };
        let nudge = send("friend", "hill", "nudge", 1);
        let reply = send("friend", "hill", "reply", 0);
        send("friend", "hill", "completion", 0);
        send("friend", "", "nudge", 0);
        send("hill", "friend", "reply", 0);
        send("friend", "hill", "reply", 91);
        store.read_activity("friend", &[reply], NOW).unwrap();
        let page = store.sent("hill", NOW, None, 1).unwrap();
        assert_eq!(page.items[0].notice.id, reply);
        assert_eq!(page.items[0].recipient, "friend");
        assert_eq!(page.items[0].notice.read_at, Some(NOW / 1000));
        let next = store.sent("hill", NOW, page.next_before, 1).unwrap();
        assert_eq!(
            next.items.iter().map(|s| s.notice.id).collect::<Vec<_>>(),
            [nudge]
        );
        assert!(next.items[0].notice.read_at.is_none());
        assert_eq!(next.next_before, None);
        assert!(
            store
                .sent("friend", NOW, None, 100)
                .unwrap()
                .items
                .iter()
                .all(|s| s.recipient == "hill")
        );
    }

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
        store.prune_notifications(NOW).unwrap();
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
    fn celebrations_reach_the_inbox_after_the_first_run_unless_turned_off() {
        let (mut store, _path) = temporary_store();
        let event = |key: &str| Event {
            key: key.into(),
            title: format!("Title {key}"),
            body: "Body".into(),
        };
        store
            .queue_events("hill", &[event("achievement:old")], DAY, NOW, false)
            .unwrap();
        assert!(
            store.notifications("hill", NOW).unwrap().is_empty(),
            "history is recorded, not celebrated, on a player's first run"
        );
        store
            .queue_events(
                "hill",
                &[
                    event("achievement:old"),
                    event("record:day:1"),
                    event("quest:1:0"),
                ],
                DAY,
                NOW,
                false,
            )
            .unwrap();
        let inbox = store.notifications("hill", NOW).unwrap();
        assert_eq!(inbox.len(), 1, "quests stay push-only events: {inbox:?}");
        assert_eq!(
            (inbox[0].kind.as_str(), inbox[0].title.as_str()),
            ("celebration", "Title record:day:1")
        );
        assert_eq!(inbox[0].topic.as_deref(), Some("record"));

        let many: Vec<Event> = (0..5).map(|i| event(&format!("level:{i}"))).collect();
        store.queue_events("hill", &many, DAY, NOW, false).unwrap();
        let inbox = store.notifications("hill", NOW).unwrap();
        assert_eq!(inbox.len(), 2);
        assert_eq!(inbox[1].title, "5 new unlocks");
        assert_eq!(
            inbox[1].topic.as_deref(),
            Some("level"),
            "a group of one kind keeps its topic"
        );
        store
            .queue_events(
                "hill",
                &[
                    event("level:9"),
                    event("streak:1:7"),
                    event("achievement:a"),
                    event("achievement:b"),
                    event("achievement:c"),
                ],
                DAY,
                NOW,
                false,
            )
            .unwrap();
        let inbox = store.notifications("hill", NOW).unwrap();
        assert_eq!(inbox[2].topic, None, "a mixed group has no single topic");

        store.set_celebrations("hill", false).unwrap();
        assert!(!store.celebrations_enabled("hill").unwrap());
        store
            .queue_events("hill", &[event("streak:1:50")], DAY, NOW, false)
            .unwrap();
        assert_eq!(store.notifications("hill", NOW).unwrap().len(), 3);
        store.set_celebrations("hill", true).unwrap();
        store
            .queue_events("hill", &[event("streak:1:50")], DAY, NOW, false)
            .unwrap();
        assert_eq!(
            store.notifications("hill", NOW).unwrap().len(),
            3,
            "turning them back on does not replay what was skipped"
        );
    }

    #[test]
    fn seen_keys_and_notifications_commit_together_and_push_only_stays_private() {
        let (mut store, path) = temporary_store();
        let event = Event {
            key: "quest:1:0".into(),
            title: "Quest complete".into(),
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
            !store.mark_seen("friend", "quest:1:0").unwrap(),
            "accounts without ntfy keep their silent event baseline"
        );
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
    fn replies_keep_the_message_they_answer_for_inbox_sent_and_push() {
        let (mut store, path) = temporary_store();
        let original = store
            .send(
                &Outgoing {
                    to: "hill",
                    from: "cerro",
                    title: "Deck complete",
                    body: "Cerro finished Spanish.",
                    kind: "completion",
                },
                DAY,
                NOW,
            )
            .unwrap();
        assert_eq!(
            store
                .reply("hill", "Hill", original, "Good job!", NOW + 1)
                .unwrap(),
            Some("cerro".into())
        );
        let answer = store.notifications("cerro", NOW + 1).unwrap().remove(0);
        let expected =
            serde_json::json!({"title":"Deck complete","body":"Cerro finished Spanish."});
        assert_eq!(serde_json::to_value(&answer).unwrap()["reply_to"], expected);
        assert_eq!(
            serde_json::to_value(&store.sent("hill", NOW + 1, None, 20).unwrap().items[0]).unwrap()
                ["reply_to"],
            expected
        );
        let pushed = store.take_deck_deliveries(NOW + 1).unwrap();
        let reply = pushed
            .into_iter()
            .find(|delivery| delivery.notification.id == answer.id)
            .unwrap();
        assert_eq!(
            serde_json::to_value(reply.notification).unwrap()["reply_to"],
            expected
        );
        assert_eq!(
            store
                .reply("cerro", "Cerro", answer.id, "Thanks!", NOW + 2)
                .unwrap(),
            Some("hill".into())
        );
        let second = store
            .notifications("hill", NOW + 2)
            .unwrap()
            .into_iter()
            .find(|n| n.kind == "reply")
            .unwrap();
        assert_eq!(
            serde_json::to_value(second).unwrap()["reply_to"],
            serde_json::json!({"title":"💬 Hill","body":"Good job!"})
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
}
