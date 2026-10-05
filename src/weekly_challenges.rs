//! One private, optional friend suggestion per shared competition week.
use crate::{challenges, decks, game::Week, store::Store};
use chrono::{Datelike, Duration};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const TITLE: &str = "A week of steady studying";
pub const DURATION_MS: i64 = 7 * 86_400_000;

#[derive(Serialize)]
pub struct Suggestion {
    pub week_start: i64,
    pub expires_at: i64,
    pub friend: decks::Recipient,
    pub target: u64,
    pub duration_days: u32,
    pub dismissed: bool,
    pub challenge_id: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub week_start: i64,
    pub action: String,
}

pub fn initialize(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "create table if not exists community_weekly_suggestions (
           owner text not null, week_start integer not null, expires_at integer not null,
           friend text not null, dismissed integer not null default 0, challenge integer,
           primary key(owner,week_start)
         ) without rowid;
         create table if not exists community_weekly_goals (
           challenge integer primary key, started integer not null default 0,
           foreign key(challenge) references community_challenges(id)
         );",
    )
}

pub fn suggestion(
    store: &Store,
    user: &str,
    roster: &BTreeMap<String, String>,
    week: &Week,
    now: i64,
) -> Result<Option<Suggestion>, challenges::Error> {
    if !roster.contains_key(user) {
        return Ok(None);
    }
    let friends: Vec<_> = roster.keys().filter(|name| name.as_str() != user).collect();
    if friends.is_empty() {
        return Ok(None);
    }
    let date = week.competition_date(now);
    let monday = date - Duration::days(i64::from(date.weekday().num_days_from_monday()));
    let start = week.boundary_ms(monday);
    let end = week.boundary_ms(monday + Duration::days(7));
    // Stable ordering rotates through everyone without using their private deck history.
    let rank = roster.keys().position(|name| name == user).unwrap_or(0);
    let rotation = i64::from(monday.num_days_from_ce()).div_euclid(7);
    let friend =
        friends[(rotation.rem_euclid(friends.len() as i64) as usize + rank) % friends.len()];
    store.conn.execute(
        "insert or ignore into community_weekly_suggestions(owner,week_start,expires_at,friend)
         values(?1,?2,?3,?4)",
        params![user, start, end, friend],
    )?;
    let (friend, expires_at, dismissed, challenge_id): (String, i64, bool, Option<i64>) =
        store.conn.query_row(
            "select friend,expires_at,dismissed,challenge from community_weekly_suggestions
         where owner=?1 and week_start=?2",
            params![user, start],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
    // Configuration changes never silently substitute a different invitation recipient.
    let Some(display) = roster.get(&friend) else {
        return Ok(None);
    };
    Ok(Some(Suggestion {
        week_start: start,
        expires_at,
        friend: decks::Recipient {
            user: friend,
            display: display.clone(),
        },
        target: 3,
        duration_days: 7,
        dismissed,
        challenge_id,
    }))
}

pub fn act(
    store: &mut Store,
    user: &str,
    action: &Action,
    roster: &BTreeMap<String, String>,
    week: &Week,
    now: i64,
) -> Result<(), challenges::Error> {
    let suggestion = suggestion(store, user, roster, week, now)?.ok_or(
        challenges::Error::Invalid("No weekly friend suggestion is available."),
    )?;
    if suggestion.week_start != action.week_start {
        return Err(challenges::Error::Invalid(
            "Your weekly suggestion changed. Refresh and try again.",
        ));
    }
    if !matches!(action.action.as_str(), "invite" | "dismiss") {
        return Err(challenges::Error::Invalid("Choose invite or dismiss."));
    }
    if action.action == "invite" && suggestion.challenge_id.is_some() {
        return Ok(()); // A lost response cannot create or restart another goal.
    }
    if action.action == "dismiss" && suggestion.challenge_id.is_some() {
        return Err(challenges::Error::Invalid(
            "This suggestion already has an invitation.",
        ));
    }
    if suggestion.dismissed && action.action == "invite" {
        return Err(challenges::Error::Invalid(
            "You skipped this week's suggestion. You can still create a custom goal.",
        ));
    }
    let tx = store.conn.transaction()?;
    if action.action == "dismiss" {
        tx.execute(
            "update community_weekly_suggestions set dismissed=1 where owner=?1 and week_start=?2",
            params![user, suggestion.week_start],
        )?;
    } else {
        let open: i64 = tx.query_row(
            "select count(*) from community_challenges where creator=?1 and cancelled=0 and end_at>?2",
            params![user,now], |r| r.get(0),
        )?;
        if open >= 10 {
            return Err(challenges::Error::Invalid(
                "You already have 10 open challenges. Close or cancel one first.",
            ));
        }
        tx.execute("insert into community_challenges(creator,title,kind,cooperative,target,start_at,end_at)
                    values(?1,?2,'study_days',0,3,?3,?4)", params![user,TITLE,now,suggestion.expires_at])?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "insert into community_weekly_goals(challenge) values(?1)",
            [id],
        )?;
        tx.execute("insert into community_members(challenge,user,status) values(?1,?2,'accepted'),(?1,?3,'invited')",
            params![id,user,suggestion.friend.user])?;
        tx.execute(
            "update community_weekly_suggestions set challenge=?3 where owner=?1 and week_start=?2",
            params![user, suggestion.week_start, id],
        )?;
        challenges::notice(
            &tx,
            &suggestion.friend.user,
            user,
            id,
            TITLE,
            "challenge_invite",
            now,
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn started(conn: &Connection, id: i64) -> rusqlite::Result<Option<bool>> {
    conn.query_row(
        "select started from community_weekly_goals where challenge=?1",
        [id],
        |r| r.get(0),
    )
    .optional()
}

pub fn cancel_stale_invites(conn: &Connection, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "update notifications set push_cancelled=1 where kind='challenge_invite'
                  and challenge_id in (select w.challenge from community_weekly_goals w
                  join community_challenges c on c.id=w.challenge
                  where w.started=1 or c.cancelled=1 or c.end_at<=?1)",
        [now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        competition::Participant,
        game::{Clock, FreezePolicy, Review},
    };
    const NOW: i64 = 1_800_000_000_000;
    fn roster() -> BTreeMap<String, String> {
        ["owner", "friend", "other"]
            .into_iter()
            .map(|name| (name.into(), format!("Display {name}")))
            .collect()
    }
    fn action(start: i64, value: &str) -> Action {
        Action {
            week_start: start,
            action: value.into(),
        }
    }
    fn participant(user: &str, reviews: &[i64]) -> Participant {
        Participant {
            user: user.into(),
            display: user.into(),
            clock: Clock::default(),
            freeze_policy: FreezePolicy::default(),
            reviews: reviews
                .iter()
                .map(|id| Review {
                    id: *id,
                    cid: 1,
                    last_ivl: 0,
                    time_ms: 1000,
                    kind: 1,
                })
                .collect::<Vec<_>>()
                .into(),
        }
    }

    #[test]
    fn suggestions_rotate_are_private_stable_and_persist_skipping() {
        let (mut store, path) = crate::decks::tests::temporary_store();
        let week = Week::default();
        let roster = roster();
        let first = suggestion(&store, "owner", &roster, &week, NOW)
            .unwrap()
            .unwrap();
        let same = suggestion(&store, "owner", &roster, &week, NOW + 1000)
            .unwrap()
            .unwrap();
        assert_eq!(first.friend.user, same.friend.user);
        assert_ne!(first.friend.user, "owner");
        assert!(
            suggestion(&store, "stranger", &roster, &week, NOW)
                .unwrap()
                .is_none()
        );
        assert!(
            suggestion(
                &store,
                "owner",
                &BTreeMap::from([("owner".into(), "Owner".into())]),
                &week,
                NOW
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(
            store
                .conn
                .query_row("select count(*) from notifications", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        act(
            &mut store,
            "owner",
            &action(first.week_start, "dismiss"),
            &roster,
            &week,
            NOW,
        )
        .unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(
            suggestion(&store, "owner", &roster, &week, NOW)
                .unwrap()
                .unwrap()
                .dismissed
        );
        assert!(
            act(
                &mut store,
                "owner",
                &action(first.week_start, "invite"),
                &roster,
                &week,
                NOW
            )
            .is_err()
        );
        let next = suggestion(&store, "owner", &roster, &week, first.expires_at)
            .unwrap()
            .unwrap();
        assert!(!next.dismissed);
        assert_ne!(next.friend.user, first.friend.user);
        assert!(
            act(
                &mut store,
                "owner",
                &action(first.week_start, "invite"),
                &roster,
                &week,
                first.expires_at
            )
            .is_err()
        );
    }

    #[test]
    fn invitations_and_delivery_commit_together_and_retries_do_not_duplicate() {
        let (mut store, path) = crate::decks::tests::temporary_store();
        let roster = roster();
        let week = Week::default();
        let draft = suggestion(&store, "owner", &roster, &week, NOW)
            .unwrap()
            .unwrap();
        store.conn.execute_batch("create trigger fail_weekly before insert on notifications begin select raise(abort,'test'); end;").unwrap();
        assert!(
            act(
                &mut store,
                "owner",
                &action(draft.week_start, "invite"),
                &roster,
                &week,
                NOW
            )
            .is_err()
        );
        assert!(
            suggestion(&store, "owner", &roster, &week, NOW)
                .unwrap()
                .unwrap()
                .challenge_id
                .is_none()
        );
        assert_eq!(
            store
                .conn
                .query_row("select count(*) from community_challenges", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        store
            .conn
            .execute_batch("drop trigger fail_weekly")
            .unwrap();
        act(
            &mut store,
            "owner",
            &action(draft.week_start, "invite"),
            &roster,
            &week,
            NOW,
        )
        .unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        act(
            &mut store,
            "owner",
            &action(draft.week_start, "invite"),
            &roster,
            &week,
            NOW + 1,
        )
        .unwrap();
        assert_eq!(
            store
                .notifications(&draft.friend.user, NOW + 1)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .conn
                .query_row("select count(*) from community_challenges", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn both_agree_before_the_timer_or_either_players_progress_starts() {
        let (mut store, _path) = crate::decks::tests::temporary_store();
        let roster = roster();
        let week = Week::default();
        let draft = suggestion(&store, "owner", &roster, &week, NOW)
            .unwrap()
            .unwrap();
        act(
            &mut store,
            "owner",
            &action(draft.week_start, "invite"),
            &roster,
            &week,
            NOW,
        )
        .unwrap();
        let id = suggestion(&store, "owner", &roster, &week, NOW)
            .unwrap()
            .unwrap()
            .challenge_id
            .unwrap();
        let players = [
            participant("owner", &[NOW + 10]),
            participant(&draft.friend.user, &[NOW + 20]),
        ];
        let waiting = challenges::list(&store, "owner", &players, NOW + 50)
            .unwrap()
            .remove(0);
        assert_eq!(waiting.status, "waiting");
        assert_eq!(waiting.progress, 0);
        assert!(!waiting.started);
        assert!(matches!(
            challenges::act(&mut store, "stranger", id, "accept", NOW + 60),
            Err(challenges::Error::Forbidden)
        ));
        challenges::act(&mut store, &draft.friend.user, id, "accept", NOW + 100).unwrap();
        challenges::act(&mut store, &draft.friend.user, id, "accept", NOW + 200).unwrap();
        let goal = challenges::list(&store, "owner", &players, NOW + 300)
            .unwrap()
            .remove(0);
        assert_eq!(goal.start_at, NOW + 100);
        assert_eq!(goal.end_at, NOW + 100 + DURATION_MS);
        assert_eq!(goal.progress, 0);
        assert!(goal.started);
        assert!(goal.weekly);
        let days: Vec<_> = (0..3).map(|d| NOW + 101 + d * 86_400_000).collect();
        let players = [
            participant("owner", &days),
            participant(&draft.friend.user, &days),
        ];
        assert_eq!(
            challenges::list(&store, "owner", &players, NOW + 3 * 86_400_000).unwrap()[0].status,
            "complete"
        );
        assert!(
            store
                .conn
                .query_row(
                    "select push_cancelled from notifications where kind='challenge_invite'",
                    [],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
        );
    }

    #[test]
    fn decline_and_expiry_never_start_a_weekly_goal() {
        let (mut store, _path) = crate::decks::tests::temporary_store();
        let roster = roster();
        let week = Week::default();
        let draft = suggestion(&store, "owner", &roster, &week, NOW)
            .unwrap()
            .unwrap();
        act(
            &mut store,
            "owner",
            &action(draft.week_start, "invite"),
            &roster,
            &week,
            NOW,
        )
        .unwrap();
        let id = suggestion(&store, "owner", &roster, &week, NOW)
            .unwrap()
            .unwrap()
            .challenge_id
            .unwrap();
        challenges::act(&mut store, &draft.friend.user, id, "decline", NOW + 100).unwrap();
        let goal = challenges::list(&store, "owner", &[], NOW + 101)
            .unwrap()
            .remove(0);
        assert!(!goal.started);
        assert_eq!(goal.status, "declined");
        assert!(challenges::act(&mut store, &draft.friend.user, id, "accept", NOW + 102).is_err());
        let next = suggestion(&store, "owner", &roster, &week, draft.expires_at)
            .unwrap()
            .unwrap();
        act(
            &mut store,
            "owner",
            &action(next.week_start, "invite"),
            &roster,
            &week,
            draft.expires_at,
        )
        .unwrap();
        let next_id = suggestion(&store, "owner", &roster, &week, draft.expires_at)
            .unwrap()
            .unwrap()
            .challenge_id
            .unwrap();
        assert!(
            challenges::act(
                &mut store,
                &next.friend.user,
                next_id,
                "accept",
                next.expires_at
            )
            .is_err()
        );
        cancel_stale_invites(&store.conn, next.expires_at).unwrap();
        assert_eq!(store.conn.query_row("select count(*) from notifications where kind='challenge_invite' and push_cancelled=0",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }

    #[test]
    fn failed_acceptance_rolls_back_both_clocks_and_a_leaver_cannot_finish_solo() {
        let (mut store, _path) = crate::decks::tests::temporary_store();
        let roster = roster();
        let week = Week::default();
        let draft = suggestion(&store, "owner", &roster, &week, NOW)
            .unwrap()
            .unwrap();
        act(
            &mut store,
            "owner",
            &action(draft.week_start, "invite"),
            &roster,
            &week,
            NOW,
        )
        .unwrap();
        let id = suggestion(&store, "owner", &roster, &week, NOW)
            .unwrap()
            .unwrap()
            .challenge_id
            .unwrap();
        store.conn.execute_batch("create trigger fail_accept_weekly before insert on notifications when new.kind='challenge_accepted' begin select raise(abort,'test'); end;").unwrap();
        assert!(challenges::act(&mut store, &draft.friend.user, id, "accept", NOW + 100).is_err());
        let goal = challenges::list(&store, "owner", &[], NOW + 101)
            .unwrap()
            .remove(0);
        assert!(!goal.started);
        assert_eq!(goal.end_at, draft.expires_at);
        assert!(
            goal.members
                .iter()
                .any(|m| m.user == draft.friend.user && m.status == "invited")
        );
        store
            .conn
            .execute_batch("drop trigger fail_accept_weekly")
            .unwrap();
        challenges::act(&mut store, &draft.friend.user, id, "accept", NOW + 200).unwrap();
        let days: Vec<_> = (0..3).map(|d| NOW + 201 + d * 86_400_000).collect();
        let players = [
            participant("owner", &days),
            participant(&draft.friend.user, &days),
        ];
        challenges::refresh(&mut store, &players, NOW + 3 * 86_400_000).unwrap();
        challenges::refresh(&mut store, &players, NOW + 3 * 86_400_000 + 1).unwrap();
        assert_eq!(
            store
                .notifications("owner", NOW + 3 * 86_400_000)
                .unwrap()
                .iter()
                .filter(|n| n.kind == "challenge_complete")
                .count(),
            1
        );
        let mut notice = store
            .notifications(&draft.friend.user, NOW + 3 * 86_400_000)
            .unwrap()
            .into_iter()
            .find(|n| n.kind == "challenge_invite")
            .unwrap();
        crate::i18n::notification(&mut notice, "es", "Friend");
        assert_eq!(notice.title, "Invitación al reto semanal");
        assert!(notice.body.contains("3 días cada uno"));
        challenges::act(
            &mut store,
            &draft.friend.user,
            id,
            "leave",
            NOW + 3 * 86_400_000 + 2,
        )
        .unwrap();
        assert_ne!(
            challenges::list(&store, "owner", &players, NOW + 3 * 86_400_000 + 3).unwrap()[0]
                .status,
            "complete"
        );
    }

    #[test]
    fn shared_week_respects_timezone_and_daylight_saving() {
        let (store, _path) = crate::decks::tests::temporary_store();
        let roster = roster();
        let week = Week {
            tz: chrono_tz::Europe::Berlin,
            rollover_hour: 4,
        };
        let at = chrono::DateTime::parse_from_rfc3339("2026-03-23T05:00:00+01:00")
            .unwrap()
            .timestamp_millis();
        let first = suggestion(&store, "owner", &roster, &week, at)
            .unwrap()
            .unwrap();
        assert_eq!(first.expires_at - first.week_start, DURATION_MS - 3_600_000);
        let before = suggestion(&store, "owner", &roster, &week, first.expires_at - 1)
            .unwrap()
            .unwrap();
        let after = suggestion(&store, "owner", &roster, &week, first.expires_at)
            .unwrap()
            .unwrap();
        assert_eq!(before.week_start, first.week_start);
        assert_ne!(after.week_start, first.week_start);
    }
}
