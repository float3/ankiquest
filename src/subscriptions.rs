use crate::access::authorized;
use crate::app::App;
use crate::decks::{IncomingSettings, valid_incoming_settings};
use crate::store::{Error, Store};
use crate::web_util::store_error;
use crate::{decks, subscriptions};
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

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
        let mut next: HashSet<_> = settings.unsubscribed_senders.iter().cloned().collect();
        next.extend(self.incoming_settings(recipient)?.muted_senders);
        let now = crate::app::now_ms();
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
        // Mutes carry over into the unsubscribe selection, whichever client saved it.
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

#[derive(Debug, Serialize)]
pub(crate) struct IncomingPreferences {
    #[serde(flatten)]
    settings: decks::IncomingSettings,
    senders: Vec<decks::Recipient>,
    unsubscribed_senders: Vec<String>,
    sharing_senders: Vec<String>,
}

fn incoming_preferences(
    app: &App,
    store: &Store,
    user: &str,
) -> Result<IncomingPreferences, Error> {
    let mut users: BTreeSet<String> = app
        .circle(&store.conn, user)?
        .into_iter()
        .filter(|other| app.is_user(other))
        .collect();
    users.extend(store.known_incoming_senders(user)?);
    let unsubscribed_senders = store.unsubscribed_senders(user)?;
    users.extend(unsubscribed_senders.iter().cloned());
    users.remove(user);
    let senders = app.sorted_recipients(users);
    Ok(IncomingPreferences {
        settings: store.incoming_settings(user)?,
        senders,
        unsubscribed_senders,
        sharing_senders: store.sharing_senders(user)?,
    })
}

pub(crate) async fn get_incoming_preferences(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Json<IncomingPreferences>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    incoming_preferences(&app, &app.store.lock().unwrap(), &user)
        .map(Json)
        .map_err(store_error)
}

pub(crate) async fn set_incoming_preferences(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(update): Json<decks::IncomingSettings>,
) -> Result<Json<IncomingPreferences>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !decks::valid_incoming_settings(&update) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut store = app.store.lock().unwrap();
    let current = incoming_preferences(&app, &store, &user).map_err(store_error)?;
    if update
        .muted_senders
        .iter()
        .any(|user| !current.senders.iter().any(|sender| sender.user == *user))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    store
        .set_incoming_settings(&user, &update)
        .map_err(store_error)?;
    incoming_preferences(&app, &store, &user)
        .map(Json)
        .map_err(store_error)
}

async fn set_deck_subscriptions(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(update): Json<subscriptions::Settings>,
) -> Result<Json<IncomingPreferences>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !update.valid(&user) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut store = app.store.lock().unwrap();
    let current = incoming_preferences(&app, &store, &user).map_err(store_error)?;
    if update
        .unsubscribed_senders
        .iter()
        .any(|sender| !current.senders.iter().any(|person| &person.user == sender))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    store
        .set_deck_subscriptions(&user, &update)
        .map_err(store_error)?;
    incoming_preferences(&app, &store, &user)
        .map(Json)
        .map_err(store_error)
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route(
            "/api/notification-preferences/{user}",
            get(get_incoming_preferences).post(set_incoming_preferences),
        )
        .route(
            "/api/deck-subscriptions/{user}",
            get(get_incoming_preferences).post(set_deck_subscriptions),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decks::tests::temporary_store;
    use crate::decks::{Preference, Snapshot};
    use crate::game::Clock;
    use crate::inbox::get_notifications;
    use crate::notifications::Outgoing;
    use crate::study::upload;
    use crate::test_support::*;
    use crate::{get_decks, router, set_decks};

    fn settings(enabled: bool, senders: &[&str]) -> Settings {
        Settings {
            enabled,
            unsubscribed_senders: senders.iter().map(|s| (*s).into()).collect(),
        }
    }

    #[test]
    fn saving_subscriptions_keeps_an_earlier_mute() {
        let (mut store, _path) = temporary_store();
        share(&mut store, "1", &["hill"]);
        store
            .set_incoming_settings(
                "hill",
                &crate::decks::IncomingSettings {
                    enabled: true,
                    muted_senders: vec!["cerro".into()],
                },
            )
            .unwrap();
        store
            .set_deck_subscriptions("hill", &settings(true, &[]))
            .unwrap();
        assert_eq!(store.unsubscribed_senders("hill").unwrap(), ["cerro"]);
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
        let now = crate::app::now_ms();
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
        let now = crate::app::now_ms();
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
        let now = crate::app::now_ms();
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
        let day = Clock::default().day(crate::app::now_ms());
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
        let now = crate::app::now_ms();
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
        notice(
            &mut store,
            "hill",
            "cerro",
            "completion",
            crate::app::now_ms(),
        );
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

    #[tokio::test]
    async fn recipient_unsubscribe_removes_sender_selections_and_only_recipient_can_restore() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let (app, path) = fixture();
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(2, false)),
        )
        .await
        .unwrap();
        let _ = set_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(preferences(&["hill", "friend"])),
        )
        .await
        .unwrap();
        let save = |owner: &str, unsubscribed: Vec<&str>| {
            Request::post("/api/deck-subscriptions/hill")
                .header("authorization", format!("Bearer {owner}-secret"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"enabled":true,"unsubscribed_senders":unsubscribed})
                        .to_string(),
                ))
                .unwrap()
        };
        let response = router(app.clone())
            .oneshot(save("hill", vec!["cerro"]))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let settings = get_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(settings.decks[0].recipients, ["friend"]);
        assert!(
            !settings
                .recipients
                .iter()
                .any(|person| person.user == "hill")
        );
        assert_eq!(
            set_decks(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(preferences(&["hill", "friend"]))
            )
            .await
            .unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            router(app.clone())
                .oneshot(save("cerro", vec![]))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            router(app.clone())
                .oneshot(save("hill", vec![]))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.store.lock().unwrap().decks("cerro").unwrap()[0].recipients,
            ["friend", "hill"]
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn subscription_api_requires_owner_validates_senders_and_protects_cookie_writes() {
        use axum::body::{Body, to_bytes};
        use axum::http::Request;
        use tower::ServiceExt;

        let (app, path) = fixture();
        let read = || {
            Request::get("/api/deck-subscriptions/hill")
                .body(Body::empty())
                .unwrap()
        };
        for token in [None, Some("cerro-secret")] {
            let mut request = read();
            if let Some(token) = token {
                request
                    .headers_mut()
                    .insert("authorization", format!("Bearer {token}").parse().unwrap());
            }
            assert_eq!(
                router(app.clone()).oneshot(request).await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }
        for (body, status) in [
            (
                serde_json::json!({"enabled":true,"unsubscribed_senders":["hill"]}),
                StatusCode::BAD_REQUEST,
            ),
            (
                serde_json::json!({"enabled":true,"unsubscribed_senders":["unknown"]}),
                StatusCode::BAD_REQUEST,
            ),
            (
                serde_json::json!({"enabled":true,"unsubscribed_senders":["cerro","cerro"]}),
                StatusCode::BAD_REQUEST,
            ),
            (
                serde_json::json!({"enabled":true,"unsubscribed_senders":["bad\nname"]}),
                StatusCode::BAD_REQUEST,
            ),
            (
                serde_json::json!({"enabled":true,"unsubscribed_senders":(0..101).map(|i| format!("sender{i}")).collect::<Vec<_>>()}),
                StatusCode::BAD_REQUEST,
            ),
            (
                serde_json::json!({"enabled":true}),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                serde_json::json!({"enabled":true,"unsubscribed_senders":[],"recipient":"cerro"}),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            let request = Request::post("/api/deck-subscriptions/hill")
                .header("authorization", "Bearer hill-secret")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            assert_eq!(
                router(app.clone()).oneshot(request).await.unwrap().status(),
                status
            );
        }
        let session = access::session(
            State(app.clone()),
            axum::extract::ConnectInfo("127.0.0.1:40123".parse().unwrap()),
            headers("hill"),
            axum::body::Bytes::new(),
        )
        .await;
        assert_eq!(session.status(), StatusCode::NO_CONTENT);
        let cookie = session.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        for csrf in [false, true] {
            let mut request = Request::post("/api/deck-subscriptions/hill")
                .header("cookie", cookie)
                .header("content-type", "application/json");
            if csrf {
                request = request.header("x-ankiquest-csrf", "1");
            }
            let response = router(app.clone())
                .oneshot(
                    request
                        .body(Body::from(
                            r#"{"enabled":true,"unsubscribed_senders":["cerro"]}"#,
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if csrf {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            if csrf {
                let body: serde_json::Value =
                    serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap())
                        .unwrap();
                assert_eq!(body["unsubscribed_senders"], serde_json::json!(["cerro"]));
                assert_eq!(body["sharing_senders"], serde_json::json!([]));
                assert!(!body.to_string().contains("deck_id"));
            }
        }
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn incoming_preferences_are_private_validated_and_available_before_studying() {
        let (app, path) = fixture();
        let settings =
            get_incoming_preferences(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0;
        assert!(settings.settings.enabled);
        assert!(settings.settings.muted_senders.is_empty());
        assert_eq!(
            settings
                .senders
                .iter()
                .map(|p| p.user.as_str())
                .collect::<Vec<_>>(),
            vec!["cerro", "friend"]
        );
        assert!(app.profile("hill").is_none());
        for muted_senders in [
            vec!["hill".into()],
            vec!["unknown".into()],
            vec!["cerro".into(), "cerro".into()],
        ] {
            assert_eq!(
                set_incoming_preferences(
                    State(app.clone()),
                    UrlPath("hill".into()),
                    headers("hill"),
                    Json(decks::IncomingSettings {
                        enabled: false,
                        muted_senders
                    })
                )
                .await
                .unwrap_err(),
                StatusCode::BAD_REQUEST
            );
        }
        assert!(
            app.store
                .lock()
                .unwrap()
                .incoming_settings("hill")
                .unwrap()
                .enabled
        );
        assert!(
            serde_json::from_value::<decks::IncomingSettings>(serde_json::json!({
                "enabled":false,"muted_senders":[],"user":"cerro"
            }))
            .is_err()
        );
        {
            let mut store = app.store.lock().unwrap();
            store
                .send(
                    &notifications::Outgoing {
                        to: "hill",
                        from: "former-player",
                        title: "Deck complete",
                        body: "Private deck name",
                        kind: "completion",
                    },
                    0,
                    now_ms(),
                )
                .unwrap();
        }
        let saved = set_incoming_preferences(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(decks::IncomingSettings {
                enabled: false,
                muted_senders: vec!["former-player".into()],
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(!saved.settings.enabled);
        assert_eq!(saved.settings.muted_senders, vec!["former-player"]);
        assert!(
            !serde_json::to_string(&saved)
                .unwrap()
                .contains("Private deck name")
        );
        assert!(
            app.store
                .lock()
                .unwrap()
                .incoming_settings("cerro")
                .unwrap()
                .enabled
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn recipient_mutes_change_upload_delivery_without_changing_outgoing_settings() {
        let (app, path) = fixture();
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(2, false)),
        )
        .await
        .unwrap();
        let _ = set_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(preferences(&["hill", "friend"])),
        )
        .await
        .unwrap();
        let _ = set_incoming_preferences(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(decks::IncomingSettings {
                enabled: true,
                muted_senders: vec!["cerro".into()],
            }),
        )
        .await
        .unwrap();
        let completion = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(0, false)),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(completion.announced.len(), 1);
        assert_eq!(completion.announced[0].recipients, 1);
        assert!(
            get_notifications(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0
                .is_empty()
        );
        assert_eq!(
            get_notifications(
                State(app.clone()),
                UrlPath("friend".into()),
                headers("friend")
            )
            .await
            .unwrap()
            .0
            .len(),
            1
        );
        let outgoing = get_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(outgoing.decks[0].recipients, vec!["friend", "hill"]);
        let _ = set_incoming_preferences(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(decks::IncomingSettings {
                enabled: true,
                muted_senders: vec![],
            }),
        )
        .await
        .unwrap();
        assert!(
            upload(
                State(app.clone()),
                UrlPath("cerro".into()),
                headers("cerro"),
                Json(sample_upload(0, false))
            )
            .await
            .unwrap()
            .0
            .announced
            .is_empty()
        );
        assert!(
            get_notifications(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0
                .is_empty()
        );
        // Old clients save only outgoing preferences; recipient choices survive.
        let _ = set_incoming_preferences(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(decks::IncomingSettings {
                enabled: false,
                muted_senders: vec!["cerro".into()],
            }),
        )
        .await
        .unwrap();
        let _ = set_decks(
            State(app.clone()),
            UrlPath("hill".into()),
            headers("hill"),
            Json(decks::SettingsUpdate {
                decks: vec![],
                nudges: Some(true),
                celebrations: None,
            }),
        )
        .await
        .unwrap();
        let settings =
            get_incoming_preferences(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0;
        assert!(!settings.settings.enabled);
        assert_eq!(settings.settings.muted_senders, vec!["cerro"]);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
