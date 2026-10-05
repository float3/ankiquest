//! Private, opt-in challenges and cooperative goals. Reviews count only after joining.
use crate::competition::Participant;
use crate::store::Store;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const DAY_MS: i64 = 86_400_000;

#[derive(Debug)]
pub enum Error {
    Invalid(&'static str),
    Forbidden,
    NotFound,
    Storage(rusqlite::Error),
}

impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        Self::Storage(value)
    }
}

#[derive(Clone, Deserialize, Serialize, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    StudyDays,
    Reviews,
}

impl Kind {
    fn key(&self) -> &'static str {
        match self {
            Self::StudyDays => "study_days",
            Self::Reviews => "reviews",
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Create {
    pub title: String,
    pub kind: Kind,
    pub cooperative: bool,
    pub target: u64,
    pub duration_days: u32,
    pub recipients: Vec<String>,
    /// Keep the full goal duration until every invited member agrees.
    #[serde(default)]
    pub start_when_ready: bool,
    /// New clients keep the same key when retrying one deliberate creation.
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub action: String,
}

#[derive(Serialize, Debug)]
pub struct Member {
    pub user: String,
    pub display: String,
    pub status: String,
    pub progress: u64,
}

#[derive(Serialize, Debug)]
pub struct Challenge {
    pub id: i64,
    pub title: String,
    pub kind: Kind,
    pub cooperative: bool,
    pub creator: String,
    pub start_at: i64,
    pub end_at: i64,
    pub target: u64,
    pub members: Vec<Member>,
    pub progress: u64,
    pub status: String,
    pub weekly: bool,
    pub started: bool,
    pub start_when_ready: bool,
}

pub fn initialize(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "create table if not exists community_challenges (
           id integer primary key, creator text not null, title text not null,
           kind text not null, cooperative integer not null, target integer not null,
           start_at integer not null, end_at integer not null, cancelled integer not null default 0
         );
         create table if not exists community_members (
           challenge integer not null, user text not null, status text not null,
           joined_at integer, left_at integer,
           primary key (challenge, user),
           foreign key (challenge) references community_challenges(id)
         );
         create index if not exists community_members_user on community_members(user);
         create table if not exists community_requests (
           creator text not null, request_id text not null, challenge integer not null,
           payload text not null, primary key(creator,request_id)
         ) without rowid;
         create table if not exists community_deferred_goals (
           challenge integer primary key, duration_days integer not null,
           started integer not null default 0,
           foreign key (challenge) references community_challenges(id)
         );",
    )
}

pub fn create(
    store: &mut Store,
    user: &str,
    request: &Create,
    eligible: &BTreeSet<String>,
    now: i64,
) -> Result<i64, Error> {
    if request.request_id.as_ref().is_some_and(|key| {
        key.len() < 8
            || key.len() > 128
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    }) {
        return Err(Error::Invalid(
            "Use an 8–128 character request ID containing letters, numbers, hyphens or underscores.",
        ));
    }
    let title = request.title.trim();
    if title.is_empty() || title.chars().count() > 80 || title.chars().any(char::is_control) {
        return Err(Error::Invalid("Use a title between 1 and 80 characters."));
    }
    if !(1..=31).contains(&request.duration_days) || !(1..=100_000).contains(&request.target) {
        return Err(Error::Invalid(
            "Choose 1–31 days and a target between 1 and 100,000.",
        ));
    }
    let recipients: BTreeSet<_> = request.recipients.iter().cloned().collect();
    if recipients.is_empty()
        || recipients.len() > 30
        || recipients.len() != request.recipients.len()
        || recipients.contains(user)
        || recipients.iter().any(|name| !eligible.contains(name))
    {
        return Err(Error::Invalid(
            "Select 1–30 different people from the available players.",
        ));
    }
    // A rolling duration may straddle an extra local day; targets still stay attainable
    // without relying on that partial day, or on reviews before acceptance.
    let max_days = u64::from(request.duration_days)
        * if request.cooperative {
            recipients.len() as u64 + 1
        } else {
            1
        };
    if request.kind == Kind::StudyDays && request.target > max_days {
        return Err(Error::Invalid(
            "The study-day target exceeds the available participant days.",
        ));
    }
    let mut payload = serde_json::json!({"title":title,"kind":request.kind,
        "cooperative":request.cooperative,"target":request.target,
        "duration_days":request.duration_days,"recipients":recipients});
    // Preserve old request IDs when an older client retries after an upgrade.
    if request.start_when_ready {
        payload["start_when_ready"] = true.into();
    }
    let payload = payload.to_string();
    if let Some(key) = &request.request_id {
        let previous: Option<(i64,String)> = store.conn.query_row(
            "select challenge,payload from community_requests where creator=?1 and request_id=?2",
            params![user,key], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((id, previous)) = previous {
            return if previous == payload {
                Ok(id)
            } else {
                Err(Error::Invalid(
                    "That request ID belongs to a different challenge.",
                ))
            };
        }
    }
    // Completed goals remain open until the creator closes them or their
    // original deadline passes; the UI permits closing a completed goal.
    let active: i64 = store.conn.query_row(
        "select count(*) from community_challenges where creator=?1 and end_at>?2 and cancelled=0",
        params![user, now],
        |row| row.get(0),
    )?;
    if active >= 10 {
        return Err(Error::Invalid(
            "You already have 10 open challenges. Close or cancel one first.",
        ));
    }
    let tx = store.conn.transaction()?;
    tx.execute(
        "insert into community_challenges (creator,title,kind,cooperative,target,start_at,end_at)
         values (?1,?2,?3,?4,?5,?6,?7)",
        params![
            user,
            title,
            request.kind.key(),
            request.cooperative,
            request.target as i64,
            now,
            if request.start_when_ready {
                i64::MAX
            } else {
                now + i64::from(request.duration_days) * DAY_MS
            }
        ],
    )?;
    let id = tx.last_insert_rowid();
    if request.start_when_ready {
        tx.execute(
            "insert into community_deferred_goals(challenge,duration_days) values(?1,?2)",
            params![id, request.duration_days],
        )?;
    }
    tx.execute(
        "insert into community_members (challenge,user,status,joined_at) values (?1,?2,'accepted',?3)",
        params![id, user, now],
    )?;
    for recipient in recipients {
        tx.execute(
            "insert into community_members (challenge,user,status) values (?1,?2,'invited')",
            params![id, recipient],
        )?;
        notice(&tx, &recipient, user, id, title, "challenge_invite", now)?;
    }
    if let Some(key) = &request.request_id {
        tx.execute("insert into community_requests(creator,request_id,challenge,payload) values(?1,?2,?3,?4)",
            params![user,key,id,payload])?;
    }
    tx.commit()?;
    Ok(id)
}

pub fn act(store: &mut Store, user: &str, id: i64, action: &str, now: i64) -> Result<(), Error> {
    let tx = store.conn.transaction()?;
    let row: Option<(String, String, i64, bool)> = tx
        .query_row(
            "select creator,title,end_at,cancelled from community_challenges where id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let (creator, title, end, cancelled) = row.ok_or(Error::NotFound)?;
    let membership: Option<String> = tx
        .query_row(
            "select status from community_members where challenge=?1 and user=?2",
            params![id, user],
            |r| r.get(0),
        )
        .optional()?;
    let status = membership.ok_or(Error::Forbidden)?;
    if (action == "cancel" && creator == user && cancelled)
        || (action == "accept" && creator != user && status == "accepted")
        || (action == "decline" && status == "declined")
        || (action == "leave" && status == "left")
    {
        return Ok(());
    }
    if cancelled || now >= end {
        return Err(Error::Invalid("This challenge has ended."));
    }
    let waiting_weekly = crate::weekly_challenges::started(&tx, id)? == Some(false);
    let deferred = deferred_goal(&tx, id)?;
    let waiting_custom = deferred.is_some_and(|(started, _)| !started);
    let waiting = waiting_weekly || waiting_custom;
    match action {
        "cancel" if creator == user => {
            tx.execute(
                "update community_challenges set cancelled=1,end_at=min(end_at,?2) where id=?1",
                params![id, now],
            )?;
        }
        "accept" if status == "invited" => {
            tx.execute(
                "update community_members set status='accepted',joined_at=?3 where challenge=?1 and user=?2 and status='invited'",
                params![id,user,now],
            )?;
            let all_accepted: bool = tx.query_row(
                "select not exists(select 1 from community_members where challenge=?1 and status='invited')",
                [id],
                |r| r.get(0),
            )?;
            if waiting && all_accepted {
                tx.execute(
                    "update community_challenges set start_at=?2,end_at=?3 where id=?1",
                    params![
                        id,
                        now,
                        now + deferred
                            .map_or(crate::weekly_challenges::DURATION_MS, |(_, days)| days
                                * DAY_MS)
                    ],
                )?;
                tx.execute("update community_members set joined_at=?2 where challenge=?1 and status='accepted'", params![id,now])?;
                if waiting_weekly {
                    tx.execute(
                        "update community_weekly_goals set started=1 where challenge=?1",
                        [id],
                    )?;
                } else {
                    tx.execute(
                        "update community_deferred_goals set started=1 where challenge=?1",
                        [id],
                    )?;
                }
            }
            notice(&tx, &creator, user, id, &title, "challenge_accepted", now)?;
        }
        "decline" if status == "invited" => {
            tx.execute(
                "update community_members set status='declined',left_at=?3 where challenge=?1 and user=?2",
                params![id,user,now],
            )?;
            if waiting {
                tx.execute(
                    "update community_challenges set cancelled=1,end_at=min(end_at,?2) where id=?1",
                    params![id, now],
                )?;
            }
        }
        "leave" if status == "accepted" && creator != user => {
            tx.execute(
                "update community_members set status='left',left_at=?3 where challenge=?1 and user=?2",
                params![id,user,now],
            )?;
        }
        "cancel" => return Err(Error::Forbidden),
        _ => {
            return Err(Error::Invalid(
                "That action is not available for this invitation.",
            ));
        }
    }
    tx.execute(
        "update notifications set read_at=coalesce(read_at,?3)
        where recipient=?1 and challenge_id=?2 and kind='challenge_invite'",
        params![user, id, now],
    )?;
    crate::weekly_challenges::cancel_stale_invites(&tx, now)?;
    cancel_stale_deferred_invites(&tx, now)?;
    tx.commit()?;
    Ok(())
}

/// Commit event identity and delivery in the same transaction as membership changes.
pub(crate) fn notice(
    conn: &Connection,
    to: &str,
    from: &str,
    id: i64,
    title: &str,
    kind: &str,
    now: i64,
) -> rusqlite::Result<()> {
    let key = format!("challenge:{id}:{kind}:{from}");
    if conn.execute(
        "insert or ignore into seen(user,key) values(?1,?2)",
        params![to, key],
    )? == 0
    {
        return Ok(());
    }
    let (heading, body) = if crate::weekly_challenges::started(conn, id)?.is_some() {
        match kind {
            "challenge_invite" => (
                "Weekly challenge invitation",
                format!("{from} invited you to a weekly goal: study on 3 days each."),
            ),
            "challenge_accepted" => (
                "Weekly challenge started",
                format!("{from} accepted your weekly goal. Your seven days start now."),
            ),
            _ => (
                "Weekly challenge complete",
                "You both finished your weekly goal. Well done!".into(),
            ),
        }
    } else {
        match kind {
            "challenge_invite" => (
                "Challenge invitation",
                format!("{from} invited you to {title}."),
            ),
            "challenge_accepted" => ("Invitation accepted", format!("{from} joined {title}.")),
            _ => (
                "Challenge complete",
                format!("You reached the goal in {title}."),
            ),
        }
    };
    conn.execute(
        "insert into notifications(recipient,sender,title,body,day,created_at,kind,challenge_id)
        values(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            to,
            from,
            heading,
            body,
            now.div_euclid(DAY_MS),
            now,
            kind,
            id
        ],
    )?;
    Ok(())
}

fn deferred_goal(conn: &Connection, id: i64) -> rusqlite::Result<Option<(bool, i64)>> {
    conn.query_row(
        "select started,duration_days from community_deferred_goals where challenge=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
}

fn cancel_stale_deferred_invites(conn: &Connection, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "update notifications set push_cancelled=1 where kind='challenge_invite'
         and challenge_id in (select d.challenge from community_deferred_goals d
         join community_challenges c on c.id=d.challenge
         where d.started=1 or c.cancelled=1 or c.end_at<=?1)",
        [now],
    )?;
    Ok(())
}

/// Progress is derived from reviews, so completion events are reconciled after
/// review uploads/polls and before personal challenge or Activity reads.
pub fn refresh(store: &mut Store, players: &[Participant], now: i64) -> Result<(), crate::Error> {
    crate::weekly_challenges::cancel_stale_invites(&store.conn, now)?;
    cancel_stale_deferred_invites(&store.conn, now)?;
    let creators = store
        .conn
        .prepare(
            "select distinct creator from community_challenges
        where cancelled=0 and end_at>=?1",
        )?
        .query_map([now - 90 * DAY_MS], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut complete = Vec::new();
    for creator in creators {
        complete.extend(
            list(store, &creator, players, now)
                .map_err(|error| format!("challenge progress: {error:?}"))?
                .into_iter()
                .filter(|challenge| {
                    challenge.status == "complete" && challenge.end_at >= now - 90 * DAY_MS
                }),
        );
    }
    let tx = store.conn.transaction()?;
    for challenge in complete {
        for member in challenge
            .members
            .iter()
            .filter(|member| member.status == "accepted")
        {
            notice(
                &tx,
                &member.user,
                "",
                challenge.id,
                &challenge.title,
                "challenge_complete",
                now,
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn list(
    store: &Store,
    user: &str,
    players: &[Participant],
    now: i64,
) -> Result<Vec<Challenge>, Error> {
    // Keep every open invitation actionable; only completed history is bounded.
    // Reached goals are still open until cancelled or their deadline passes.
    let mut query = store.conn.prepare(
        "select c.id,c.title,c.kind,c.cooperative,c.creator,c.start_at,c.end_at,c.target,c.cancelled
         from community_challenges c join community_members m on m.challenge=c.id
         where m.user=?1 and m.status in ('accepted','invited')
           and ((c.cancelled=0 and c.end_at>?2) or c.id in (
             select history.id from community_challenges history
             join community_members membership on membership.challenge=history.id
             where membership.user=?1 and membership.status in ('accepted','invited')
               and (history.cancelled<>0 or history.end_at<=?2)
             order by history.id desc limit 100
           ))
         order by c.id desc",
    )?;
    let rows = query
        .query_map(params![user, now], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                u64::from(r.get::<_, u32>(7)?),
                r.get::<_, bool>(8)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut challenges = Vec::new();
    for (id, title, kind, cooperative, creator, start_at, end_at, target, cancelled) in rows {
        let weekly_started = crate::weekly_challenges::started(&store.conn, id)?;
        let weekly = weekly_started.is_some();
        let deferred = deferred_goal(&store.conn, id)?;
        let started = weekly_started
            .or(deferred.map(|(started, _)| started))
            .unwrap_or(true);
        let kind = if kind == "study_days" {
            Kind::StudyDays
        } else {
            Kind::Reviews
        };
        let mut query = store.conn.prepare(
            "select user,status,joined_at from community_members where challenge=?1 order by user",
        )?;
        let members = query
            .query_map([id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let members: Vec<Member> = members
            .into_iter()
            .map(|(member, status, joined)| {
                let player = players.iter().find(|p| p.user == member);
                let mut count = 0;
                let mut days = BTreeSet::new();
                if started
                    && status == "accepted"
                    && let (Some(player), Some(joined)) = (player, joined)
                {
                    for review in player.reviews.iter() {
                        if review.id >= joined.max(start_at)
                            && review.id < end_at
                            && review.id <= now
                        {
                            count += 1;
                            days.insert(player.clock.day(review.id));
                        }
                    }
                }
                Member {
                    display: player.map_or_else(|| member.clone(), |p| p.display.clone()),
                    user: member,
                    status,
                    progress: if kind == Kind::StudyDays {
                        days.len() as u64
                    } else {
                        count
                    },
                }
            })
            .collect();
        let progress = members.iter().map(|m| m.progress).sum();
        let complete = started
            && (!weekly || members.iter().all(|m| m.status == "accepted"))
            && if cooperative {
                progress >= target
            } else {
                !members.iter().any(|m| m.status == "invited")
                    && members
                        .iter()
                        .filter(|m| m.status == "accepted")
                        .all(|m| m.progress >= target)
            };
        let status = if cancelled
            && (weekly || deferred.is_some())
            && !started
            && members.iter().any(|m| m.status == "declined")
        {
            "declined"
        } else if cancelled {
            "cancelled"
        } else if complete {
            "complete"
        } else if now >= end_at {
            "ended"
        } else if !started {
            "waiting"
        } else if now < start_at {
            "upcoming"
        } else {
            "active"
        };
        challenges.push(Challenge {
            id,
            title,
            kind,
            cooperative,
            creator,
            start_at,
            end_at,
            target,
            members,
            progress,
            status: status.into(),
            weekly,
            started,
            start_when_ready: deferred.is_some(),
        });
    }
    Ok(challenges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Clock, FreezePolicy, Review};

    #[test]
    fn challenge_events_are_private_transactional_and_retry_safe() {
        let (mut store, path) = crate::decks::tests::temporary_store();
        let eligible = BTreeSet::from(["friend".into(), "outsider".into()]);
        let mut request = request();
        request.request_id = Some("create-request-1".into());
        store.conn.execute_batch("create trigger fail_invite before insert on notifications when new.kind='challenge_invite' begin select raise(abort,'test'); end;").unwrap();
        assert!(create(&mut store, "owner", &request, &eligible, 100).is_err());
        for table in [
            "community_challenges",
            "community_members",
            "community_requests",
            "seen",
            "notifications",
        ] {
            assert_eq!(
                store
                    .conn
                    .query_row(&format!("select count(*) from {table}"), [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        store
            .conn
            .execute_batch("drop trigger fail_invite")
            .unwrap();
        let id = create(&mut store, "owner", &request, &eligible, 100).unwrap();
        assert_eq!(
            create(&mut store, "owner", &request, &eligible, 110).unwrap(),
            id
        );
        request.title = "Changed request".into();
        assert!(matches!(
            create(&mut store, "owner", &request, &eligible, 110),
            Err(Error::Invalid(_))
        ));
        let invite = store.activity("friend", 110, 90, None, 100).unwrap();
        assert_eq!(
            (invite.unread_count, invite.action_count, invite.items.len()),
            (1, 1, 1)
        );
        assert_eq!(invite.items[0].challenge_id, Some(id));
        assert_eq!(
            invite.items[0].route,
            Some(format!("/community#challenge-{id}"))
        );
        assert!(
            store
                .activity("outsider", 110, 90, None, 100)
                .unwrap()
                .items
                .is_empty()
        );
        assert!(
            store
                .activity("owner", 110, 90, None, 100)
                .unwrap()
                .items
                .is_empty()
        );
        assert!(matches!(
            act(&mut store, "outsider", id, "accept", 120),
            Err(Error::Forbidden)
        ));
        store
            .read_activity("friend", &[invite.items[0].id], 120)
            .unwrap();
        let read = store.activity("friend", 120, 90, None, 100).unwrap();
        assert_eq!(
            (read.unread_count, read.action_count),
            (0, 1),
            "reading does not accept an invitation"
        );
        store.conn.execute_batch("create trigger fail_accept before insert on notifications when new.kind='challenge_accepted' begin select raise(abort,'test'); end;").unwrap();
        assert!(act(&mut store, "friend", id, "accept", 150).is_err());
        assert_eq!(
            store
                .activity("friend", 150, 90, None, 100)
                .unwrap()
                .action_count,
            1
        );
        store
            .conn
            .execute_batch("drop trigger fail_accept")
            .unwrap();
        act(&mut store, "friend", id, "accept", 150).unwrap();
        act(&mut store, "friend", id, "accept", 170).unwrap();
        assert_eq!(store.notifications("owner", 170).unwrap().len(), 1);
        assert_eq!(
            store.notifications("owner", 170).unwrap()[0].kind,
            "challenge_accepted"
        );
        assert_eq!(
            store
                .activity("friend", 170, 90, None, 100)
                .unwrap()
                .action_count,
            0
        );
        let players = [player("owner", &[110]), player("friend", &[140, 160, 165])];
        assert_eq!(
            list(&store, "friend", &players, 180).unwrap()[0]
                .members
                .iter()
                .find(|m| m.user == "friend")
                .unwrap()
                .progress,
            2,
            "accept retry does not reset joined_at"
        );
        store.conn.execute_batch("create trigger fail_complete before insert on notifications when new.kind='challenge_complete' and new.recipient='friend' begin select raise(abort,'test'); end;").unwrap();
        assert!(refresh(&mut store, &players, 180).is_err());
        assert!(
            store
                .notifications("owner", 180)
                .unwrap()
                .iter()
                .all(|n| n.kind != "challenge_complete")
        );
        store
            .conn
            .execute_batch("drop trigger fail_complete")
            .unwrap();
        refresh(&mut store, &players, 180).unwrap();
        refresh(&mut store, &players, 190).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        refresh(&mut store, &players, 200).unwrap();
        for user in ["owner", "friend"] {
            assert_eq!(
                store
                    .notifications(user, 200)
                    .unwrap()
                    .iter()
                    .filter(|n| n.kind == "challenge_complete")
                    .count(),
                1
            );
        }
        assert!(store.notifications("outsider", 200).unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn invitation_actions_and_completion_skip_declined_and_expired_members() {
        let (mut store, path) = crate::decks::tests::temporary_store();
        let eligible = BTreeSet::from(["friend".into(), "outsider".into()]);
        let mut request = request();
        request.recipients.push("outsider".into());
        let id = create(&mut store, "owner", &request, &eligible, 100).unwrap();
        act(&mut store, "friend", id, "decline", 120).unwrap();
        act(&mut store, "friend", id, "decline", 130).unwrap();
        assert_eq!(
            store
                .activity("friend", 130, 90, None, 100)
                .unwrap()
                .action_count,
            0
        );
        refresh(&mut store, &[player("owner", &[110, 120, 130])], 150).unwrap();
        for user in ["friend", "outsider"] {
            assert!(
                store
                    .notifications(user, 150)
                    .unwrap()
                    .iter()
                    .all(|n| n.kind != "challenge_complete")
            );
        }
        assert_eq!(
            store
                .activity("outsider", 100 + 7 * DAY_MS, 90, None, 100)
                .unwrap()
                .action_count,
            0
        );
        assert!(act(&mut store, "outsider", id, "accept", 100 + 7 * DAY_MS).is_err());
        act(&mut store, "owner", id, "cancel", 200).unwrap();
        act(&mut store, "owner", id, "cancel", 201).unwrap();
        assert_eq!(
            store
                .activity("outsider", 201, 90, None, 100)
                .unwrap()
                .action_count,
            0
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    fn player(user: &str, times: &[i64]) -> Participant {
        Participant {
            user: user.into(),
            display: user.into(),
            clock: Clock::default(),
            freeze_policy: FreezePolicy::default(),
            reviews: times
                .iter()
                .enumerate()
                .map(|(i, id)| Review {
                    id: *id,
                    cid: i as i64,
                    last_ivl: 1,
                    time_ms: 5000,
                    kind: 1,
                })
                .collect::<Vec<_>>()
                .into(),
        }
    }

    fn request() -> Create {
        Create {
            request_id: None,
            title: "Study together".into(),
            kind: Kind::Reviews,
            cooperative: true,
            target: 3,
            duration_days: 7,
            recipients: vec!["friend".into()],
            start_when_ready: false,
        }
    }

    #[test]
    fn custom_goal_can_wait_for_every_invitee_before_its_full_timer_starts() {
        let (mut store, _) = crate::decks::tests::temporary_store();
        initialize(&store.conn).unwrap();
        let request: Create = serde_json::from_value(serde_json::json!({
            "title": "Study together", "kind": "reviews", "cooperative": true,
            "target": 3, "duration_days": 7, "recipients": ["friend", "other"],
            "start_when_ready": true
        }))
        .unwrap();
        let eligible = BTreeSet::from(["friend".into(), "other".into()]);
        let start = 100;
        let id = create(&mut store, "owner", &request, &eligible, start).unwrap();
        let players = [
            player("owner", &[101, 201, 302]),
            player("friend", &[250, 303]),
        ];
        let waiting = list(&store, "owner", &players, start + 8 * DAY_MS).unwrap();
        assert_eq!(waiting[0].status, "waiting");
        assert_eq!(waiting[0].progress, 0);
        act(&mut store, "friend", id, "accept", start + 8 * DAY_MS).unwrap();
        assert_eq!(
            list(&store, "owner", &players, start + 8 * DAY_MS).unwrap()[0].status,
            "waiting"
        );
        let accepted_at = start + 10 * DAY_MS;
        act(&mut store, "other", id, "accept", accepted_at).unwrap();
        let running = list(&store, "owner", &players, accepted_at).unwrap();
        assert_eq!(running[0].status, "active");
        assert_eq!(running[0].start_at, accepted_at);
        assert_eq!(running[0].end_at, accepted_at + 7 * DAY_MS);
        assert_eq!(
            running[0].progress, 0,
            "reviews before everyone agrees do not count"
        );
    }

    #[test]
    fn declining_a_waiting_custom_goal_ends_it_without_starting_the_timer() {
        let (mut store, _) = crate::decks::tests::temporary_store();
        initialize(&store.conn).unwrap();
        let mut request = request();
        request.start_when_ready = true;
        let eligible = BTreeSet::from(["friend".into()]);
        let id = create(&mut store, "owner", &request, &eligible, 100).unwrap();
        act(&mut store, "friend", id, "decline", 100 + 9 * DAY_MS).unwrap();
        let goal = &list(&store, "owner", &[], 100 + 9 * DAY_MS).unwrap()[0];
        assert_eq!(goal.status, "declined");
        assert!(!goal.started);
        assert!(matches!(
            act(&mut store, "friend", id, "accept", 100 + 9 * DAY_MS + 1),
            Err(Error::Invalid(_))
        ));
        let cancelled: bool = store.conn.query_row(
            "select push_cancelled from notifications where challenge_id=?1 and kind='challenge_invite'",
            [id],
            |r| r.get(0),
        ).unwrap();
        assert!(cancelled);
    }

    #[test]
    fn invitations_are_private_and_only_post_acceptance_reviews_count() {
        let (mut store, _) = crate::decks::tests::temporary_store();
        initialize(&store.conn).unwrap();
        let eligible = BTreeSet::from(["friend".into(), "other".into()]);
        let id = create(&mut store, "owner", &request(), &eligible, 100).unwrap();
        let players = [
            player("owner", &[90, 101]),
            player("friend", &[110, 201, 202]),
        ];
        assert!(list(&store, "other", &players, 300).unwrap().is_empty());
        assert!(matches!(
            act(&mut store, "other", id, "accept", 150),
            Err(Error::Forbidden)
        ));
        assert_eq!(
            list(&store, "friend", &players, 150).unwrap()[0].progress,
            1
        );
        act(&mut store, "friend", id, "accept", 200).unwrap();
        let result = list(&store, "owner", &players, 300).unwrap();
        assert_eq!(result[0].progress, 3);
        assert_eq!(result[0].status, "complete");
        act(&mut store, "friend", id, "accept", 250).unwrap();
        assert!(matches!(
            act(&mut store, "friend", id, "cancel", 250),
            Err(Error::Forbidden)
        ));
        act(&mut store, "friend", id, "leave", 250).unwrap();
        assert!(list(&store, "friend", &players, 300).unwrap().is_empty());
        assert_eq!(list(&store, "owner", &players, 300).unwrap()[0].progress, 1);
    }

    #[test]
    fn study_days_are_distinct_and_future_and_post_deadline_reviews_do_not_count() {
        let (mut store, _) = crate::decks::tests::temporary_store();
        initialize(&store.conn).unwrap();
        let mut request = request();
        request.kind = Kind::StudyDays;
        request.target = 3;
        let eligible = BTreeSet::from(["friend".into()]);
        let start = DAY_MS + 12 * 3_600_000;
        let id = create(&mut store, "owner", &request, &eligible, start).unwrap();
        let players = [player(
            "owner",
            &[start, start + 100, start + DAY_MS, start + 8 * DAY_MS],
        )];
        assert_eq!(
            list(&store, "owner", &players, start + 1000).unwrap()[0].progress,
            1
        );
        assert_eq!(
            list(&store, "owner", &players, start + 9 * DAY_MS).unwrap()[0].progress,
            2
        );
        assert!(matches!(
            act(&mut store, "friend", id, "accept", start + 7 * DAY_MS),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn cancelling_preserves_prior_progress_and_excludes_later_reviews() {
        let (mut store, _) = crate::decks::tests::temporary_store();
        initialize(&store.conn).unwrap();
        let eligible = BTreeSet::from(["friend".into()]);
        let id = create(&mut store, "owner", &request(), &eligible, 100).unwrap();
        act(&mut store, "owner", id, "cancel", 200).unwrap();
        let players = [player("owner", &[101, 199, 200, 201, 500])];
        let result = list(&store, "owner", &players, 1000).unwrap();
        assert_eq!(result[0].status, "cancelled");
        assert_eq!(result[0].end_at, 200);
        assert_eq!(
            result[0].progress, 2,
            "the cancellation instant is an exclusive cutoff"
        );
        assert!(matches!(
            act(&mut store, "friend", id, "accept", 250),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn creator_can_close_a_completed_goal_to_free_an_open_slot() {
        let (mut store, _) = crate::decks::tests::temporary_store();
        initialize(&store.conn).unwrap();
        let eligible = BTreeSet::from(["friend".into()]);
        let mut ids = Vec::new();
        for _ in 0..10 {
            ids.push(create(&mut store, "owner", &request(), &eligible, 100).unwrap());
        }
        let players = [player("owner", &[101, 102, 103])];
        assert!(
            list(&store, "owner", &players, 150)
                .unwrap()
                .iter()
                .all(|goal| goal.status == "complete")
        );
        assert!(matches!(
            create(&mut store, "owner", &request(), &eligible, 150),
            Err(Error::Invalid(_))
        ));
        act(&mut store, "owner", ids[0], "cancel", 200).unwrap();
        assert!(create(&mut store, "owner", &request(), &eligible, 201).is_ok());
    }

    #[test]
    fn validates_targets_people_and_creator_limit_before_writing() {
        let (mut store, _) = crate::decks::tests::temporary_store();
        initialize(&store.conn).unwrap();
        let eligible = BTreeSet::from(["friend".into()]);
        let mut request = request();
        request.recipients = vec!["stranger".into()];
        assert!(matches!(
            create(&mut store, "owner", &request, &eligible, 100),
            Err(Error::Invalid(_))
        ));
        request.recipients = vec!["friend".into()];
        request.duration_days = 0;
        assert!(matches!(
            create(&mut store, "owner", &request, &eligible, 100),
            Err(Error::Invalid(_))
        ));
        request.duration_days = 7;
        for _ in 0..10 {
            create(&mut store, "owner", &request, &eligible, 100).unwrap();
        }
        assert!(matches!(
            create(&mut store, "owner", &request, &eligible, 100),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn recent_history_does_not_hide_an_older_open_invitation() {
        let (mut store, _) = crate::decks::tests::temporary_store();
        initialize(&store.conn).unwrap();
        let eligible = BTreeSet::from(["friend".into()]);
        let open = create(&mut store, "owner", &request(), &eligible, 100).unwrap();
        for now in 101..206 {
            let closed = create(&mut store, "owner", &request(), &eligible, now).unwrap();
            act(&mut store, "owner", closed, "cancel", now + 1).unwrap();
        }
        let invitations = list(&store, "friend", &[], 300).unwrap();
        assert_eq!(
            invitations.len(),
            101,
            "one open invitation plus 100 historical goals"
        );
        assert!(invitations.iter().any(|goal| goal.id == open));
        act(&mut store, "friend", open, "accept", 300).unwrap();
        assert!(
            list(&store, "friend", &[], 301)
                .unwrap()
                .iter()
                .any(|goal| {
                    goal.id == open
                        && goal
                            .members
                            .iter()
                            .any(|member| member.user == "friend" && member.status == "accepted")
                })
        );
        act(&mut store, "friend", open, "leave", 302).unwrap();
        assert_eq!(list(&store, "friend", &[], 303).unwrap().len(), 100);
        assert!(
            list(&store, "owner", &[], 303)
                .unwrap()
                .iter()
                .any(|goal| goal.id == open)
        );
        act(&mut store, "owner", open, "cancel", 304).unwrap();
        assert_eq!(list(&store, "owner", &[], 305).unwrap().len(), 100);
    }

    #[test]
    fn every_open_invitation_is_visible_even_above_the_history_limit() {
        let (mut store, _) = crate::decks::tests::temporary_store();
        initialize(&store.conn).unwrap();
        let eligible = BTreeSet::from(["friend".into()]);
        for index in 0..105 {
            create(
                &mut store,
                &format!("owner-{index}"),
                &request(),
                &eligible,
                100,
            )
            .unwrap();
        }
        let invitations = list(&store, "friend", &[], 200).unwrap();
        assert_eq!(invitations.len(), 105);
        assert!(invitations.iter().all(|goal| goal.status == "active"));
    }
}
