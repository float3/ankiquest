//! The running server's shared state: configuration, the store, and every player's reviews.

use crate::config::Config;
use crate::game::{self, Clock, ClockHistory, Profile, Review, Week};
use crate::store::{Error, Store};
use crate::{access, accounts, competition, decks};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) struct Player {
    pub(crate) reviews: Arc<Vec<Review>>,
    pub(crate) clock: Clock,
    /// Every clock they have kept, so old reviews keep the days they fell in.
    pub(crate) clock_history: ClockHistory,
    pub(crate) freeze_policy: game::FreezePolicy,
    /// Changes whenever anything above does, so cached standings know they are stale.
    pub(crate) revision: u64,
}

pub(crate) static REVISIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(crate) fn next_revision() -> u64 {
    REVISIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Standings computed recently, so boards and the tick do not recompute every history on every read.
#[derive(Default)]
pub(crate) struct StandingCache(pub(crate) Mutex<HashMap<String, CachedStanding>>);

pub(crate) struct CachedStanding {
    pub(crate) key: (u64, game::ReviewWeighting, u64, i64, i64),
    pub(crate) computed_at: i64,
    pub(crate) recent: bool,
    pub(crate) profile: Arc<Profile>,
}

/// How long a standing stays fresh while its player has studied in the last hour,
/// because the rolling hour board moves with every minute.
const STANDING_TTL_MS: i64 = 60_000;
/// How much study history a cached standing keeps: enough for reminders about recent days.
const STANDING_HISTORY_DAYS: usize = 14;

pub(crate) struct App {
    pub(crate) config: Config,
    pub(crate) access: access::Access,
    pub(crate) accounts: accounts::Directory,
    pub(crate) week: Week,
    pub(crate) store: Mutex<Store>,
    pub(crate) players: RwLock<HashMap<String, Player>>,
    pub(crate) standings: StandingCache,
}

impl App {
    /// Rereads `user`'s reviews and settings into the cached player.
    pub(crate) fn reload_player(&self, store: &Store, user: &str) -> Result<(), Error> {
        let player = load_player(store, user)?;
        self.players
            .write()
            .unwrap()
            .insert(user.to_string(), player);
        Ok(())
    }

    pub(crate) fn recipient(&self, user: String) -> decks::Recipient {
        decks::Recipient {
            display: self.display(&user),
            user,
        }
    }

    /// `users` as recipients, ordered by display name and then user name.
    pub(crate) fn sorted_recipients(
        &self,
        users: impl IntoIterator<Item = String>,
    ) -> Vec<decks::Recipient> {
        let mut recipients: Vec<_> = users.into_iter().map(|user| self.recipient(user)).collect();
        recipients.sort_by(|a, b| a.display.cmp(&b.display).then_with(|| a.user.cmp(&b.user)));
        recipients
    }

    pub(crate) fn display(&self, user: &str) -> String {
        self.config
            .users
            .get(user)
            .and_then(|u| u.display.clone())
            .or_else(|| self.accounts.account(user).map(|account| account.display))
            .unwrap_or_else(|| user.into())
    }

    /// Anyone this server knows: configured users and accounts.
    pub(crate) fn is_user(&self, user: &str) -> bool {
        self.config.users.contains_key(user) || self.accounts.contains(user)
    }

    /// Players who can sign in: configured users with a token, and every account.
    pub(crate) fn members(&self) -> BTreeSet<String> {
        self.config
            .users
            .iter()
            .filter(|(_, config)| {
                config
                    .token
                    .as_deref()
                    .is_some_and(|token| !token.is_empty())
            })
            .map(|(user, _)| user.clone())
            .chain(self.accounts.users())
            .collect()
    }

    pub(crate) fn profile(&self, user: &str) -> Option<Profile> {
        self.preview(user, &[])
    }

    pub(crate) fn preview(&self, user: &str, pending: &[Review]) -> Option<Profile> {
        let players = self.players.read().unwrap();
        let player = players.get(user)?;
        let last = player.reviews.last().map_or(0, |r| r.id);
        let mut fresh: Vec<Review> = pending.iter().filter(|r| r.id > last).copied().collect();
        fresh.sort_by_key(|r| r.id);
        fresh.dedup_by_key(|r| r.id);
        let merged = [player.reviews.as_slice(), &fresh].concat();
        Some(game::compute_with_history(
            user,
            &self.display(user),
            &merged,
            &player.clock_history,
            &self.week,
            now_ms(),
            &player.freeze_policy,
        ))
    }

    /// A player's standing: their profile without the heatmap, achievements and older history.
    /// It is cached until the player uploads, their Anki day or the clock hour changes,
    /// or a minute passes while they have studied in the last hour.
    pub(crate) fn standing(&self, user: &str) -> Option<Arc<Profile>> {
        let now = now_ms();
        let players = self.players.read().unwrap();
        let player = players.get(user)?;
        let key = (
            player.revision,
            game::review_weighting(),
            game::streak_rule_version(),
            player.clock_history.day(now),
            now.div_euclid(3_600_000),
        );
        if let Some(cached) = self.standings.0.lock().unwrap().get(user)
            && cached.key == key
            && (!cached.recent || now - cached.computed_at < STANDING_TTL_MS)
        {
            return Some(cached.profile.clone());
        }
        let recent = player
            .reviews
            .last()
            .is_some_and(|r| r.id > now - 3_600_000);
        let mut profile = game::compute_with_history(
            user,
            &self.display(user),
            &player.reviews,
            &player.clock_history,
            &self.week,
            now,
            &player.freeze_policy,
        );
        drop(players);
        profile.heatmap = Vec::new();
        profile.achievements = Vec::new();
        let older = profile.history.len().saturating_sub(STANDING_HISTORY_DAYS);
        profile.history.drain(..older);
        let profile = Arc::new(profile);
        self.standings.0.lock().unwrap().insert(
            user.to_string(),
            CachedStanding {
                key,
                computed_at: now,
                recent,
                profile: profile.clone(),
            },
        );
        Some(profile)
    }

    pub(crate) fn forget_standing(&self, user: &str) {
        self.standings.0.lock().unwrap().remove(user);
    }

    pub(crate) fn standings_among(&self, users: &BTreeSet<String>) -> Vec<Arc<Profile>> {
        let players: Vec<String> = self
            .players
            .read()
            .unwrap()
            .keys()
            .filter(|user| users.contains(*user))
            .cloned()
            .collect();
        players.iter().filter_map(|u| self.standing(u)).collect()
    }

    /// Players `user` can send things to: members of their circle other than themselves.
    pub(crate) fn reachable(
        &self,
        store: &Store,
        user: &str,
    ) -> rusqlite::Result<BTreeSet<String>> {
        let circle = self.circle(&store.conn, user)?;
        Ok(self
            .members()
            .into_iter()
            .filter(|other| other != user && circle.contains(other))
            .collect())
    }

    /// The monthly competition still covers the original community only.
    pub(crate) fn legacy_participants(&self) -> Vec<competition::Participant> {
        self.participants()
            .into_iter()
            .filter(|participant| self.legacy(&participant.user))
            .collect()
    }

    pub(crate) fn participants(&self) -> Vec<competition::Participant> {
        let players = self.players.read().unwrap();
        let users: BTreeSet<_> = self
            .config
            .users
            .keys()
            .chain(players.keys())
            .cloned()
            .chain(self.accounts.users())
            .collect();
        users
            .into_iter()
            .map(|user| {
                let player = players.get(&user);
                competition::Participant {
                    display: self.display(&user),
                    reviews: player.map_or_else(Default::default, |p| p.reviews.clone()),
                    clock_history: player
                        .map_or_else(ClockHistory::default, |p| p.clock_history.clone()),
                    freeze_policy: player
                        .map_or_else(game::FreezePolicy::default, |p| p.freeze_policy.clone()),
                    user,
                }
            })
            .collect()
    }
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Marks every event `profile` shows as seen, so none of them is announced later.
pub(crate) fn mark_events_seen(store: &Store, user: &str, profile: &Profile) -> Result<(), Error> {
    for event in &profile.events {
        store.mark_seen(user, &event.key)?;
    }
    if let Some(goal) = crate::goals::event(&store.conn, user, profile)? {
        store.mark_seen(user, &goal.key)?;
    }
    Ok(())
}

pub(crate) fn load_player(store: &Store, user: &str) -> Result<Player, Error> {
    Ok(Player {
        reviews: Arc::new(store.reviews(user)?),
        clock: store.clock(user)?,
        clock_history: store.clock_history(user)?,
        freeze_policy: store.freeze_policy(user)?,
        revision: next_revision(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    #[test]
    fn standings_are_cached_until_the_player_changes_and_stay_small() {
        let (app, path) = fixture();
        with_reviews(&app, "cerro", 5);
        let first = app.standing("cerro").unwrap();
        assert!(
            Arc::ptr_eq(&first, &app.standing("cerro").unwrap()),
            "a second read is served from the cache"
        );
        assert!(first.heatmap.is_empty() && first.achievements.is_empty());
        assert!(
            !app.profile("cerro").unwrap().heatmap.is_empty(),
            "the full profile keeps everything"
        );
        with_reviews(&app, "cerro", 9);
        let second = app.standing("cerro").unwrap();
        assert!(
            !Arc::ptr_eq(&first, &second),
            "new reviews replace the cached standing"
        );
        assert!(second.today.reviews > first.today.reviews);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
