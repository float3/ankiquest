use crate::decks::{IncomingSettings, valid_incoming_settings};
use crate::store::{Error, Store};
use rusqlite::{Connection, params};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub unsubscribed_senders: Vec<String>,
}

impl Settings {
    pub fn valid(&self, recipient: &str) -> bool {
        !self
            .unsubscribed_senders
            .iter()
            .any(|sender| sender == recipient)
            && valid_incoming_settings(&IncomingSettings {
                enabled: self.enabled,
                muted_senders: self.unsubscribed_senders.clone(),
            })
    }
}

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists sender_unsubscriptions (
             recipient text not null, sender text not null,
             primary key (recipient, sender)
         ) without rowid;
         create index if not exists sender_unsubscriptions_sender on sender_unsubscriptions (sender, recipient);
         create table if not exists unsubscribed_decks (
             recipient text not null, sender text not null, deck_id text not null,
             primary key (recipient, sender, deck_id)
         ) without rowid;
         create table if not exists subscription_baselines (
             recipient text not null, sender text not null, deck_id text not null, day integer not null,
             primary key (recipient, sender, deck_id)
         ) without rowid;
         create trigger if not exists respect_recipient_unsubscribe_insert
         before insert on deck_recipients
         when exists (select 1 from sender_unsubscriptions where recipient=new.recipient and sender=new.owner)
         begin select raise(abort, 'recipient has unsubscribed'); end;
         create trigger if not exists respect_recipient_unsubscribe_update
         before update of recipient, owner on deck_recipients
         when exists (select 1 from sender_unsubscriptions where recipient=new.recipient and sender=new.owner)
         begin select raise(abort, 'recipient has unsubscribed'); end;",
    )?;
    Ok(())
}

impl Store {
    pub fn sharing_senders(&self, recipient: &str) -> Result<Vec<String>, Error> {
        Ok(self
            .conn
            .prepare(
                "select distinct r.owner from deck_recipients r
            join decks d on d.owner=r.owner and d.id=r.deck_id
            where r.recipient=?1 and r.owner!=?1 and d.enabled=1 order by r.owner",
            )?
            .query_map([recipient], |r| r.get(0))?
            .collect::<Result<_, _>>()?)
    }

    pub fn unsubscribed_senders(&self, recipient: &str) -> Result<Vec<String>, Error> {
        Ok(self
            .conn
            .prepare(
                "select sender from sender_unsubscriptions where recipient=?1 order by sender",
            )?
            .query_map([recipient], |r| r.get(0))?
            .collect::<Result<_, _>>()?)
    }

    pub fn unsubscribed_recipients(&self, sender: &str) -> Result<HashSet<String>, Error> {
        Ok(self
            .conn
            .prepare("select recipient from sender_unsubscriptions where sender=?1")?
            .query_map([sender], |r| r.get(0))?
            .collect::<Result<_, _>>()?)
    }

    /// Only the recipient calls this. Remember detached selections so they can
    /// restore them later, without granting access to any additional decks.
    pub fn set_deck_subscriptions(
        &mut self,
        recipient: &str,
        settings: &Settings,
    ) -> Result<(), Error> {
        if !settings.valid(recipient) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid deck subscriptions",
            )
            .into());
        }
        let previous: HashSet<_> = self.unsubscribed_senders(recipient)?.into_iter().collect();
        let next: HashSet<_> = settings.unsubscribed_senders.iter().cloned().collect();
        let now = crate::now_ms();
        let resume_days: HashMap<_, _> = previous
            .difference(&next)
            .map(|sender| Ok((sender.clone(), self.clock(sender)?.day(now))))
            .collect::<Result<_, Error>>()?;
        let tx = self.conn.transaction()?;
        for sender in next.difference(&previous) {
            tx.execute(
                "delete from subscription_baselines where recipient=?1 and sender=?2",
                params![recipient, sender],
            )?;
            tx.execute("insert into unsubscribed_decks (recipient,sender,deck_id)
                        select recipient,owner,deck_id from deck_recipients where recipient=?1 and owner=?2",
                params![recipient, sender])?;
            tx.execute(
                "delete from deck_recipients where recipient=?1 and owner=?2",
                params![recipient, sender],
            )?;
            tx.execute(
                "insert into sender_unsubscriptions (recipient,sender) values (?1,?2)",
                params![recipient, sender],
            )?;
        }
        for sender in previous.difference(&next) {
            tx.execute(
                "delete from sender_unsubscriptions where recipient=?1 and sender=?2",
                params![recipient, sender],
            )?;
            tx.execute(
                "insert or ignore into deck_recipients (owner,deck_id,recipient)
                        select s.sender,s.deck_id,s.recipient from unsubscribed_decks s
                        join decks d on d.owner=s.sender and d.id=s.deck_id
                        where s.recipient=?1 and s.sender=?2",
                params![recipient, sender],
            )?;
            tx.execute(
                "insert or replace into subscription_baselines (recipient,sender,deck_id,day)
                 select s.recipient,s.sender,s.deck_id,?3 from unsubscribed_decks s
                 join decks d on d.owner=s.sender and d.id=s.deck_id
                 where s.recipient=?1 and s.sender=?2",
                params![recipient, sender, resume_days[sender]],
            )?;
            tx.execute(
                "delete from unsubscribed_decks where recipient=?1 and sender=?2",
                params![recipient, sender],
            )?;
        }
        tx.execute(
            "insert into incoming_settings (user,enabled) values (?1,?2)
                    on conflict (user) do update set enabled=excluded.enabled",
            params![recipient, settings.enabled],
        )?;
        // The new editor includes existing mutes in the unsubscribe selection.
        // Old clients can still edit mutes, but cannot change subscriptions.
        tx.execute(
            "delete from incoming_muted_senders where recipient=?1",
            [recipient],
        )?;
        tx.execute("update notifications set completion_cancelled=1,push_cancelled=1
                    where recipient=?1 and kind='completion'
                    and (?2=0 or exists (select 1 from sender_unsubscriptions where recipient=?1 and sender=notifications.sender))",
            params![recipient, settings.enabled])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decks::{Outgoing, Preference, Snapshot, tests::temporary_store};
    use crate::game::Clock;

    fn settings(enabled: bool, senders: &[&str]) -> Settings {
        Settings {
            enabled,
            unsubscribed_senders: senders.iter().map(|s| (*s).into()).collect(),
        }
    }

    fn snapshot(id: &str, remaining: u64, day: i64) -> Snapshot {
        Snapshot {
            id: id.into(),
            name: format!("Private {id}"),
            remaining,
            reviewed_today: 2,
            day,
        }
    }

    fn share(store: &mut Store, id: &str, recipients: &[&str]) {
        let now = crate::now_ms();
        store
            .record_decks(
                "cerro",
                "Cerro",
                &[snapshot(id, 1, Clock::default().day(now))],
                Clock::default(),
                false,
                now,
            )
            .unwrap();
        store
            .set_deck_preferences(
                "cerro",
                &[Preference {
                    id: id.into(),
                    enabled: !recipients.is_empty(),
                    recipients: recipients.iter().map(|s| (*s).into()).collect(),
                }],
            )
            .unwrap();
    }

    fn notice(store: &mut Store, to: &str, from: &str, kind: &str, now: i64) -> i64 {
        store
            .send(
                &Outgoing {
                    to,
                    from,
                    title: "Notice",
                    body: "Body",
                    kind,
                },
                Clock::default().day(now),
                now,
            )
            .unwrap()
    }

    #[test]
    fn unsubscribe_detaches_all_decks_survives_restart_and_cannot_be_undone_by_legacy_settings() {
        let (mut store, path) = temporary_store();
        share(&mut store, "1", &["hill", "friend"]);
        share(&mut store, "2", &["hill"]);
        store
            .set_deck_subscriptions("hill", &settings(true, &["cerro"]))
            .unwrap();
        assert_eq!(store.decks("cerro").unwrap()[0].recipients, ["friend"]);
        assert!(!store.decks("cerro").unwrap()[1].enabled);
        assert_eq!(
            store
                .conn
                .query_row(
                    "select count(*) from deck_recipients where owner='cerro' and recipient='hill'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        store
            .set_incoming_settings(
                "hill",
                &IncomingSettings {
                    enabled: true,
                    muted_senders: vec![],
                },
            )
            .unwrap();
        assert!(
            store
                .set_deck_preferences(
                    "cerro",
                    &[Preference {
                        id: "1".into(),
                        enabled: true,
                        recipients: vec!["hill".into()]
                    }]
                )
                .is_err()
        );
        assert_eq!(
            store.decks("cerro").unwrap()[0].recipients,
            ["friend"],
            "rejected stale save rolls back"
        );
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.unsubscribed_senders("hill").unwrap(), ["cerro"]);
        store
            .set_deck_subscriptions("hill", &settings(true, &["cerro"]))
            .unwrap();
        store
            .set_deck_subscriptions("hill", &settings(true, &[]))
            .unwrap();
        let decks = store.decks("cerro").unwrap();
        assert_eq!(decks[0].recipients, ["friend", "hill"]);
        assert_eq!(decks[1].recipients, ["hill"]);
        assert!(decks.iter().all(|deck| deck.enabled));
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn unsubscribe_cancels_selected_and_future_pushes_without_affecting_other_messages() {
        let (mut store, path) = temporary_store();
        let now = crate::now_ms();
        let old = notice(&mut store, "hill", "cerro", "completion", now);
        let retry = notice(&mut store, "hill", "cerro", "completion", now);
        assert!(store.begin_push(retry, now).unwrap());
        store.finish_push(retry, false, now).unwrap();
        let other = notice(&mut store, "hill", "friend", "completion", now);
        let message = notice(&mut store, "hill", "cerro", "message", now);
        let reply = notice(&mut store, "hill", "cerro", "reply", now);
        let peer = notice(&mut store, "friend", "cerro", "completion", now);
        assert!(
            store
                .take_deck_deliveries(now)
                .unwrap()
                .iter()
                .any(|d| d.notification.id == old)
        );
        store
            .set_deck_subscriptions("hill", &settings(true, &["cerro"]))
            .unwrap();
        let suppressed = notice(&mut store, "hill", "cerro", "completion", now);
        for id in [old, retry, suppressed] {
            assert!(!store.begin_push(id, now + 120_000).unwrap());
        }
        let ids = |store: &Store| {
            store
                .notifications("hill", now)
                .unwrap()
                .iter()
                .map(|n| n.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&store), [other, message, reply]);
        assert_eq!(
            store
                .activity("hill", now, 90, None, 100)
                .unwrap()
                .items
                .len(),
            3
        );
        assert!(store.begin_push(peer, now).unwrap());
        store
            .set_deck_subscriptions("hill", &settings(true, &[]))
            .unwrap();
        assert_eq!(ids(&store), [other, message, reply]);
        let future = notice(&mut store, "hill", "cerro", "completion", now + 1);
        assert!(store.begin_push(future, now + 1).unwrap());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn resubscribe_never_replays_completed_decks_or_shares_additional_decks() {
        let (mut store, path) = temporary_store();
        share(&mut store, "shared", &["hill"]);
        share(&mut store, "private", &["friend"]);
        let now = crate::now_ms();
        let day = Clock::default().day(now);
        store
            .set_deck_subscriptions("hill", &settings(true, &["cerro"]))
            .unwrap();
        let complete = [snapshot("shared", 0, day)];
        assert!(
            store
                .record_decks("cerro", "Cerro", &complete, Clock::default(), false, now)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .conn
                .query_row("select count(*) from notifications", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "nothing is generated for a removed recipient"
        );
        store
            .set_deck_subscriptions("hill", &settings(true, &[]))
            .unwrap();
        assert!(
            store
                .record_decks("cerro", "Cerro", &complete, Clock::default(), false, now)
                .unwrap()
                .is_empty()
        );
        let tomorrow = now + 86_400_000;
        assert_eq!(
            store
                .record_decks(
                    "cerro",
                    "Cerro",
                    &[snapshot("shared", 0, day + 1)],
                    Clock::default(),
                    false,
                    tomorrow
                )
                .unwrap()[0]
                .recipients,
            1
        );
        assert_eq!(
            store
                .decks("cerro")
                .unwrap()
                .iter()
                .find(|d| d.id == "private")
                .unwrap()
                .recipients,
            ["friend"]
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn resubscribe_respects_later_sender_edits_and_deleted_decks() {
        let (mut store, path) = temporary_store();
        for id in ["edited", "deleted", "unchanged"] {
            share(&mut store, id, &["hill", "friend"]);
        }
        store
            .set_deck_subscriptions("hill", &settings(true, &["cerro"]))
            .unwrap();
        share(&mut store, "edited", &["friend", "another"]);
        let day = Clock::default().day(crate::now_ms());
        store
            .prune_decks(
                "cerro",
                &[snapshot("edited", 1, day), snapshot("unchanged", 1, day)],
            )
            .unwrap();
        share(&mut store, "deleted", &["friend"]);
        store
            .set_deck_subscriptions("hill", &settings(true, &[]))
            .unwrap();
        for deck in store.decks("cerro").unwrap() {
            assert_eq!(
                deck.recipients.contains(&"hill".to_string()),
                deck.id == "unchanged"
            );
        }
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn resubscribe_uses_fresh_progress_when_no_snapshots_arrived_during_unsubscribe() {
        let (mut store, path) = temporary_store();
        for id in ["old", "in_progress", "tomorrow"] {
            share(&mut store, id, &["hill", "friend"]);
        }
        let now = crate::now_ms();
        let day = Clock::default().day(now);
        store
            .set_deck_subscriptions("hill", &settings(true, &["cerro"]))
            .unwrap();
        // A client may stop reporting a deck when its recipients disappear.
        store
            .set_deck_subscriptions("hill", &settings(true, &[]))
            .unwrap();
        let old = store
            .record_decks(
                "cerro",
                "Cerro",
                &[snapshot("old", 0, day)],
                Clock::default(),
                false,
                now,
            )
            .unwrap();
        assert_eq!(
            old[0].recipients, 1,
            "only the continuously subscribed friend gets the completed baseline"
        );
        assert!(store.notifications("hill", now).unwrap().is_empty());
        store
            .record_decks(
                "cerro",
                "Cerro",
                &[snapshot("in_progress", 1, day)],
                Clock::default(),
                false,
                now,
            )
            .unwrap();
        let fresh = store
            .record_decks(
                "cerro",
                "Cerro",
                &[snapshot("in_progress", 0, day)],
                Clock::default(),
                false,
                now,
            )
            .unwrap();
        assert_eq!(fresh[0].recipients, 2);
        let tomorrow = store
            .record_decks(
                "cerro",
                "Cerro",
                &[snapshot("tomorrow", 0, day + 1)],
                Clock::default(),
                false,
                now + 86_400_000,
            )
            .unwrap();
        assert_eq!(tomorrow[0].recipients, 2);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn sharing_senders_only_lists_enabled_selected_decks_even_when_receiving_is_paused() {
        let (mut store, path) = temporary_store();
        share(&mut store, "1", &["hill", "friend"]);
        share(&mut store, "2", &["hill"]);
        assert_eq!(store.sharing_senders("hill").unwrap(), ["cerro"]);
        assert!(store.sharing_senders("stranger").unwrap().is_empty());
        store
            .set_deck_subscriptions("hill", &settings(false, &[]))
            .unwrap();
        assert_eq!(store.sharing_senders("hill").unwrap(), ["cerro"]);
        store
            .set_deck_subscriptions("hill", &settings(false, &["cerro"]))
            .unwrap();
        assert!(store.sharing_senders("hill").unwrap().is_empty());
        assert_eq!(store.sharing_senders("friend").unwrap(), ["cerro"]);
        store
            .set_deck_subscriptions("hill", &settings(true, &[]))
            .unwrap();
        store
            .set_deck_preferences(
                "cerro",
                &[
                    Preference {
                        id: "1".into(),
                        enabled: false,
                        recipients: vec!["hill".into()],
                    },
                    Preference {
                        id: "2".into(),
                        enabled: false,
                        recipients: vec!["hill".into()],
                    },
                ],
            )
            .unwrap();
        assert!(store.sharing_senders("hill").unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn saving_other_deck_settings_preserves_unchanged_resubscription_choices() {
        let (mut store, path) = temporary_store();
        share(&mut store, "1", &["hill", "friend"]);
        share(&mut store, "2", &["hill"]);
        share(&mut store, "3", &["hill", "friend"]);
        store
            .set_deck_subscriptions("hill", &settings(true, &["cerro"]))
            .unwrap();
        let preferences = store
            .decks("cerro")
            .unwrap()
            .into_iter()
            .map(|deck| Preference {
                enabled: deck.enabled && deck.id != "3",
                id: deck.id,
                recipients: deck.recipients,
            })
            .collect::<Vec<_>>();
        store.set_deck_preferences("cerro", &preferences).unwrap();
        store
            .set_deck_subscriptions("hill", &settings(true, &[]))
            .unwrap();
        let decks = store.decks("cerro").unwrap();
        assert_eq!(decks[0].recipients, ["friend", "hill"]);
        assert_eq!(decks[1].recipients, ["hill"]);
        assert!(
            decks[1].enabled,
            "the last recipient can restore an unchanged deck"
        );
        assert_eq!(decks[2].recipients, ["friend"]);
        assert!(!decks[2].enabled);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn failed_unsubscribe_rolls_back_deck_selections_and_alert_preferences() {
        let (mut store, path) = temporary_store();
        share(&mut store, "1", &["hill"]);
        notice(&mut store, "hill", "cerro", "completion", crate::now_ms());
        store.conn.execute_batch("create trigger fail_cancel before update on notifications begin select raise(abort,'failed'); end;").unwrap();
        assert!(
            store
                .set_deck_subscriptions("hill", &settings(false, &["cerro"]))
                .is_err()
        );
        assert_eq!(store.decks("cerro").unwrap()[0].recipients, ["hill"]);
        assert!(store.unsubscribed_senders("hill").unwrap().is_empty());
        assert!(store.incoming_settings("hill").unwrap().enabled);
        assert_eq!(
            store
                .conn
                .query_row("select count(*) from unsubscribed_decks", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
}
