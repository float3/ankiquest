use crate::game::Clock;
use crate::store::{Error, Store};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_DECKS: usize = 2000;
const MAX_RECIPIENTS: usize = 100;
const INBOX_AGE_MS: i64 = 7 * 86_400_000;

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
}

#[derive(Debug, Serialize)]
pub struct Notification {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub day: i64,
    pub created_at: i64,
}

pub struct Delivery {
    pub user: String,
    pub notification: Notification,
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
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
         create index if not exists notifications_recipient on notifications (recipient, id);",
    )?;
    Ok(())
}

impl Store {
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
        }
        Ok(decks)
    }

    pub fn set_deck_preferences(
        &mut self,
        user: &str,
        preferences: &[Preference],
    ) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        for deck in preferences {
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
    ) -> Result<(), Error> {
        let today = clock.day(now_ms);
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
            let body = format!(
                "{display} has finished their {} studies for today.",
                deck.name
            );
            tx.execute(
                "insert into notifications (recipient, title, body, day, created_at)
                 select recipient, 'Deck complete', ?3, ?4, ?5 from deck_recipients
                 where owner = ?1 and deck_id = ?2 and recipient != ?1",
                params![user, deck.id, body, today, now_ms],
            )?;
        }
        tx.commit()?;
        Ok(())
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
        let mut stmt = self.conn.prepare(
            "select id, title, body, day, created_at / 1000 from (
                 select id, title, body, day, created_at from notifications
                 where recipient = ?1 and created_at >= ?2 order by id desc limit 500
             ) order by id",
        )?;
        Ok(stmt
            .query_map(params![user, now_ms - INBOX_AGE_MS], |r| {
                Ok(Notification {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    body: r.get(2)?,
                    day: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    /// Claim each push before network I/O, matching existing notification behavior:
    /// retries never spam recipients; the private inbox remains durable on failure.
    pub fn take_deck_deliveries(&mut self, now_ms: i64) -> Result<Vec<Delivery>, Error> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "delete from notifications where created_at < ?1",
            [now_ms - INBOX_AGE_MS],
        )?;
        let deliveries = {
            let mut stmt = tx.prepare(
                "select recipient, id, title, body, day, created_at / 1000 from notifications where pushed = 0 order by id limit 100",
            )?;
            stmt.query_map([], |r| {
                Ok(Delivery {
                    user: r.get(0)?,
                    notification: Notification {
                        id: r.get(1)?,
                        title: r.get(2)?,
                        body: r.get(3)?,
                        day: r.get(4)?,
                        created_at: r.get(5)?,
                    },
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        for delivery in &deliveries {
            tx.execute(
                "update notifications set pushed = 1 where id = ?1",
                [delivery.notification.id],
            )?;
        }
        tx.commit()?;
        Ok(deliveries)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const DAY: i64 = 20_000;
    const NOW: i64 = DAY * 86_400_000 + 12 * 3_600_000;

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
