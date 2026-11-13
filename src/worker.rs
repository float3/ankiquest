//! The background work: syncing collections, refreshing standings, and pushing notifications.

use crate::app::{App, mark_events_seen, now_ms};
use crate::competition::refresh_community;
use crate::config::Config;
use crate::game::{Clock, Profile};
use crate::store::{self, Error, Store};
use crate::{
    admin, challenges, competition, freezes, game, goals, i18n, notifications, privacy, reminders,
    shares, social, wins,
};
use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_secs(20);
const PUSH_BUDGET: Duration = Duration::from_secs(20);
const STALE_AFTER_MS: i64 = 5 * POLL.as_millis() as i64;
const HEALTH_LOCK_WAIT: Duration = Duration::from_secs(2);

/// When the worker last finished a tick, in milliseconds since the epoch. It starts at the
/// time it is made, so a server that has not finished its first tick yet counts as fresh.
pub(crate) struct Heartbeat(AtomicI64);

impl Default for Heartbeat {
    fn default() -> Self {
        Self(AtomicI64::new(now_ms()))
    }
}

impl Heartbeat {
    pub(crate) fn beat(&self, at: i64) {
        self.0.store(at, Ordering::Relaxed);
    }

    pub(crate) fn last(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Whether the store can be locked within [`HEALTH_LOCK_WAIT`]; never once it is poisoned.
async fn store_available(store: &Mutex<Store>) -> bool {
    let deadline = Instant::now() + HEALTH_LOCK_WAIT;
    loop {
        let available = match store.try_lock() {
            Ok(_) => Some(true),
            Err(TryLockError::Poisoned(_)) => Some(false),
            Err(TryLockError::WouldBlock) => None,
        };
        match available {
            Some(available) => return available,
            None if Instant::now() >= deadline => return false,
            None => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
}

/// `GET /api/health`: 200 while the store can be locked and the worker has ticked within
/// five polls, 503 otherwise, with `{ "ok", "worker_age_s" }` and nothing about players.
pub(crate) async fn health(State(app): State<Arc<App>>) -> Response {
    let age_ms = (now_ms() - app.heartbeat.last()).max(0);
    let ok = age_ms <= STALE_AFTER_MS && store_available(&app.store).await;
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({ "ok": ok, "worker_age_s": age_ms / 1000 })),
    )
        .into_response()
}

/// How close to the end of the player's day securing the streak counts as saving it.
const LATE_SAVE_MS: i64 = 3 * 3_600_000;

/// Whether "Streak secured!" deserves a push: the streak was at risk and saved in the
/// last hours of the day, or after an urgent streak reminder. Securing it on an ordinary
/// study day only reaches the inbox; milestone streaks push their own celebration.
fn streak_saved_late(
    store: &Store,
    user: &str,
    profile: &Profile,
    now_ms: i64,
) -> Result<bool, Error> {
    if profile.streak <= 1 {
        return Ok(false);
    }
    Ok(profile.day_ends_at - now_ms <= LATE_SAVE_MS
        || store.is_seen(user, &format!("reminder:urgent:{}", profile.day))?)
}

pub(crate) fn push_destination<'a>(config: &'a Config, user: &str) -> Option<(&'a str, &'a str)> {
    let base = config.ntfy.as_deref()?.trim();
    let topic = config.users.get(user)?.ntfy_topic.as_deref()?.trim();
    (!base.is_empty() && !topic.is_empty()).then_some((base, topic))
}

fn push(
    config: &Config,
    user: &str,
    title: &str,
    body: &str,
    tag: &str,
    priority: &str,
    timeout: Duration,
) -> Result<(), Error> {
    let (base, topic) = push_destination(config, user).ok_or("no ntfy destination")?;
    let mut request = ureq::post(&format!("{}/{topic}", base.trim_end_matches('/')))
        .config()
        .timeout_global(Some(timeout))
        .build()
        .header("Title", title)
        .header("Priority", priority)
        .header("Tags", tag);
    if let Some(url) = &config.public_url {
        request = request.header("Click", format!("{}/#{user}", url.trim_end_matches('/')));
    }
    let response = request.send(body)?;
    if !response.status().is_success() {
        return Err(format!("ntfy returned {}", response.status()).into());
    }
    Ok(())
}

pub(crate) fn sync_users(base: &Path) -> Result<Vec<String>, Error> {
    let mut users = Vec::new();
    let mut errors = Vec::new();
    for entry in std::fs::read_dir(base)? {
        let result = (|| -> Result<Option<String>, Error> {
            let entry = entry?;
            if !std::fs::metadata(entry.path())?.is_dir() {
                return Ok(None);
            }
            let collection = entry.path().join("collection.anki2");
            match std::fs::metadata(&collection) {
                Ok(metadata) if metadata.is_file() => entry
                    .file_name()
                    .into_string()
                    .map(Some)
                    .map_err(|name| format!("invalid sync user name: {name:?}").into()),
                Ok(_) => Err(format!("{} is not a collection file", collection.display()).into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(format!("{}: {error}", collection.display()).into()),
            }
        })();
        match result {
            Ok(Some(user)) => users.push(user),
            Ok(None) => {}
            Err(error) => errors.push(error.to_string()),
        }
    }
    if errors.is_empty() {
        Ok(users)
    } else {
        Err(errors.join("; ").into())
    }
}

/// Imports run without the store lock but copy into the same scratch files, so one at a time.
static IMPORTING: Mutex<()> = Mutex::new(());

/// Imports every changed collection from the sync server. Copying and reading a collection
/// is the slow part, so the store is locked only to look up what the last import saw and
/// to record the result.
pub(crate) fn import(app: &App, base: &Path) -> Result<(), Error> {
    let _importing = IMPORTING.lock().unwrap();
    let scratch = app.store.lock().unwrap().scratch().to_path_buf();
    let mut errors = Vec::new();
    for user in sync_users(base)? {
        let result = (|| -> Result<(), Error> {
            let (first_import, last) = {
                let store = app.store.lock().unwrap();
                (!store.is_known(&user)?, store.sync_signature(&user)?)
            };
            if let Some(synced) = store::read_sync(&scratch, base, &user, last.as_ref())? {
                let mut store = app.store.lock().unwrap();
                store.record_sync(&user, &synced)?;
                app.reload_player(&store, &user)?;
            }
            if first_import && let Some(profile) = app.profile(&user) {
                mark_events_seen(&app.store.lock().unwrap(), &user, &profile)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(format!("ingest for {user} failed: {error}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; ").into())
    }
}

pub(crate) fn tick(app: &App) -> Result<(), Error> {
    let now = now_ms();
    {
        let store = app.store.lock().unwrap();
        let settings = app.config.settings().effective(&store.conn)?;
        if settings.review_weighting != game::review_weighting() {
            game::set_review_weighting(settings.review_weighting);
            log_info!("review weighting is now {:?}", settings.review_weighting);
        }
        game::set_streak_rule(admin::streak_rule(
            &store.conn,
            settings.streak_minutes,
            now,
        )?);
        admin::flush(&store.conn, now, settings.log_retention_days)?;
        app.accounts.save_uses(&store.conn)?;
        privacy::carry_out_removals(app, &store.conn)?;
        for user in freezes::grant_reloads(&store.conn)? {
            if app.players.read().unwrap().contains_key(&user) {
                app.reload_player(&store, &user)?;
            }
            freezes::grant_reloaded(&store.conn, &user)?;
        }
    }
    // Copying collections is slow, so the import takes the store only for each player's result.
    let import_complete =
        app.config
            .sync_base
            .as_ref()
            .is_none_or(|base| match import(app, base) {
                Ok(()) => true,
                Err(error) => {
                    log_error!("competition refresh deferred until sync import succeeds: {error}");
                    false
                }
            });
    let participants = {
        let mut store = app.store.lock().unwrap();
        let participants = app.participants();
        if import_complete {
            refresh_community(app, &mut store, &app.legacy_participants(), now)?;
        }
        challenges::refresh(&mut store, &participants, now)?;
        participants
    };
    // Standings are the expensive part, so they are worked out without holding the database.
    let users: Vec<String> = app.players.read().unwrap().keys().cloned().collect();
    let ranking: HashMap<&str, Arc<Profile>> = users
        .iter()
        .filter_map(|user| app.standing(user).map(|profile| (user.as_str(), profile)))
        .collect();
    let mut legacy: Vec<(u64, &str)> = ranking
        .iter()
        .filter(|(user, _)| app.legacy(user))
        .map(|(user, profile)| (profile.week_xp, *user))
        .collect();
    legacy.sort_unstable();
    let clocks: HashMap<&str, Clock> = participants
        .iter()
        .map(|participant| {
            (
                participant.user.as_str(),
                participant.clock_history.current(),
            )
        })
        .collect();
    for user in &users {
        let Some(profile) = ranking.get(user.as_str()) else {
            continue;
        };
        // One player at a time, so requests can take the database in between.
        let mut store = app.store.lock().unwrap();
        let result = (|| -> Result<(), Error> {
            let push_enabled = push_destination(&app.config, user).is_some();
            let mut events = profile.events.clone();
            events.extend(goals::event(&store.conn, user, profile)?);
            let push_secured = streak_saved_late(&store, user, profile, now_ms())?;
            store.queue_moments(
                user,
                &events,
                profile.day,
                now_ms(),
                push_enabled,
                push_secured,
                &wins::stats(profile),
            )?;
            if store.nudges_enabled(user)? {
                let ahead = just_ahead(app, &store.conn, user, profile, &ranking, &legacy)?
                    .map(|other| (other.display.clone(), other.week_xp));
                for nudge in game::nudges(profile, ahead.as_ref().map(|(w, xp)| (w.as_str(), *xp)))
                {
                    store.send_once(
                        &notifications::Outgoing {
                            to: user,
                            from: "",
                            title: &nudge.title,
                            body: &nudge.body,
                            kind: "nudge",
                        },
                        &nudge.key,
                        profile.day,
                        now_ms(),
                        false,
                    )?;
                }
            }
            if let Some(clock) = clocks.get(user.as_str()) {
                let recap = competition::weekly_recap(&store, user, &app.week, now)?;
                let place = recap
                    .as_ref()
                    .map(|recap| place_win(&store.conn, user, profile, recap, now))
                    .transpose()?
                    .flatten();
                reminders::tick(&mut store, user, profile, clock, &app.week, now, recap)?;
                if let Some((win, topic, since)) = place {
                    wins::attach_recap(&store.conn, user, win, &topic, since)?;
                }
            }
            shares::share_automatically(app, &mut store, user, now_ms())?;
            Ok(())
        })();
        if let Err(error) = result {
            log_error!(user: user, "worker tick for {user} failed: {error}");
        }
    }
    deliver_notifications(app, PUSH_BUDGET)
}

/// The win for a top-three place in last week's finished competition, made once, with the
/// recap topic and the week's end so the recap that announces it can point to it.
fn place_win(
    conn: &rusqlite::Connection,
    user: &str,
    profile: &Profile,
    recap: &competition::WeeklyRecap,
    now: i64,
) -> Result<Option<(i64, String, i64)>, Error> {
    // A place is a win only once late syncs can no longer change it.
    let Some(rank) = recap
        .rank
        .filter(|rank| recap.final_result && (1..=3).contains(rank))
    else {
        return Ok(None);
    };
    if recap.week_end > now || now >= recap.valid_until {
        return Ok(None);
    }
    let key = format!("place:{}:{rank}", recap.week_start.div_euclid(game::DAY_MS));
    let mut stats = wins::stats(profile);
    stats["place"] = rank.into();
    stats["week_xp"] = recap.xp.into();
    wins::create(
        conn,
        user,
        &key,
        profile.day,
        now,
        &format!("#{rank} on the weekly board"),
        &format!(
            "{} XP over {} study days last week.",
            recap.xp, recap.days_studied
        ),
        &stats,
    )?;
    Ok(wins::id_of(conn, user, &key)?.map(|win| (win, format!("place:{rank}"), recap.week_end)))
}

/// Whoever in the player's circle is closest above them this week, for "just out of reach" nudges.
/// `legacy` is the original community sorted by week XP, so it needs no scan per player.
fn just_ahead<'a>(
    app: &App,
    conn: &rusqlite::Connection,
    user: &str,
    profile: &Profile,
    ranking: &'a HashMap<&str, Arc<Profile>>,
    legacy: &[(u64, &'a str)],
) -> rusqlite::Result<Option<&'a Profile>> {
    let mut best: Option<&Profile> = None;
    let mut consider = |other: &'a Profile| {
        if other.week_xp > profile.week_xp && best.is_none_or(|best| other.week_xp < best.week_xp) {
            best = Some(other);
        }
    };
    if app.legacy(user) {
        let above = legacy.partition_point(|(xp, _)| *xp <= profile.week_xp);
        if let Some(other) = legacy.get(above).and_then(|(_, other)| ranking.get(other)) {
            consider(other);
        }
    }
    let mut others = social::friends(conn, user)?;
    others.extend(social::group_mates(conn, user)?);
    for other in others {
        if let Some(other) = ranking.get(other.as_str()) {
            consider(other);
        }
    }
    Ok(best)
}

fn deliver_notifications(app: &App, budget: Duration) -> Result<(), Error> {
    app.store.lock().unwrap().prune_notifications(now_ms())?;
    let deliveries = app.store.lock().unwrap().take_deck_deliveries(now_ms())?;
    let started = std::time::Instant::now();
    for mut delivery in deliveries {
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        let id = delivery.notification.id;
        if push_destination(&app.config, &delivery.user).is_none() {
            app.store.lock().unwrap().skip_push(id)?;
            continue;
        }
        let mut reminder_body = None;
        if delivery.notification.kind.starts_with("reminder_") {
            let now = now_ms();
            let profile = app.profile(&delivery.user);
            let clock = app
                .players
                .read()
                .unwrap()
                .get(&delivery.user)
                .map(|p| p.clock);
            let mut store = app.store.lock().unwrap();
            let guard = match (profile, clock) {
                (Some(profile), Some(clock)) => {
                    reminder_body = Some(reminders::current_body(
                        &delivery.notification,
                        &profile,
                        &clock,
                        &app.week,
                        now,
                    ));
                    reminders::delivery_guard(
                        &store,
                        &delivery.user,
                        &delivery.notification,
                        &profile,
                        &clock,
                        &app.week,
                        now,
                    )?
                }
                _ => reminders::DeliveryGuard::Cancel,
            };
            match guard {
                reminders::DeliveryGuard::Cancel => {
                    reminders::cancel(&mut store, id)?;
                    continue;
                }
                reminders::DeliveryGuard::Defer => {
                    store.defer_push(id, now)?;
                    continue;
                }
                reminders::DeliveryGuard::Deliver => {}
            }
        }
        if !app.store.lock().unwrap().begin_push(id, now_ms())? {
            continue;
        }
        let language = app.store.lock().unwrap().language(&delivery.user)?;
        if let Some(body) = reminder_body {
            delivery.notification.body = body;
        }
        let sender_display = app.display(&delivery.notification.sender);
        i18n::notification(&mut delivery.notification, &language, &sender_display);
        crate::shares::localize(
            &app.store.lock().unwrap().conn,
            &mut delivery.notification,
            &language,
        )?;
        let push_body = if let Some(original) = &delivery.notification.reply_to {
            format!(
                "{}\n\n{}: {}",
                delivery.notification.body,
                i18n::text("In reply to", &language),
                original.body
            )
        } else {
            delivery.notification.body.clone()
        };
        let result = push(
            &app.config,
            &delivery.user,
            &delivery.notification.title,
            &push_body,
            match delivery.notification.kind.as_str() {
                "event" => "tada",
                "reminder_urgent" => "fire",
                _ => "white_check_mark",
            },
            if delivery.notification.kind.starts_with("reminder_")
                && delivery.notification.kind != "reminder_urgent"
            {
                "default"
            } else {
                "high"
            },
            remaining.min(Duration::from_secs(10)),
        );
        app.store
            .lock()
            .unwrap()
            .finish_push(id, result.is_ok(), now_ms())?;
        if let Err(e) = result {
            log_error!(
                user: delivery.user,
                "ntfy push for {} failed; queued for retry: {e}",
                delivery.user
            );
        }
    }
    Ok(())
}
/// Aborts the process when dropped by a panicking thread, so a worker that dies takes the
/// server with it and the service manager restarts both.
struct AbortOnPanic;

impl Drop for AbortOnPanic {
    fn drop(&mut self) {
        if std::thread::panicking() {
            std::process::abort();
        }
    }
}

/// Runs [`tick`] every [`POLL`] on its own thread for as long as the server runs.
pub(crate) fn spawn(app: Arc<App>) {
    std::thread::spawn(move || {
        let _abort = AbortOnPanic;
        loop {
            if let Err(e) = tick(&app) {
                log_error!("tick failed: {e}");
            }
            app.heartbeat.beat(now_ms());
            std::thread::sleep(POLL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::app::load_player;
    use crate::decks::set_decks;
    use crate::test_support::*;

    fn queued_message(app: &App, user: &str) -> i64 {
        app.store
            .lock()
            .unwrap()
            .send(
                &notifications::Outgoing {
                    to: user,
                    from: "",
                    title: "Hello",
                    body: "Do not lose this",
                    kind: "message",
                },
                Clock::default().day(now_ms()),
                now_ms(),
            )
            .unwrap()
    }

    fn was_pushed(app: &App, id: i64) -> bool {
        app.store
            .lock()
            .unwrap()
            .conn
            .query_row(
                "select pushed from notifications where id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn local_push(status: u16) -> (String, std::thread::JoinHandle<String>) {
        local_push_with_check(status, || {})
    }

    fn local_push_with_check(
        status: u16,
        check: impl FnOnce() + Send + 'static,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "no push request arrived"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("accept push: {e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 2048];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            check();
            let _ = write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            String::from_utf8(request).unwrap()
        });
        (address, server)
    }

    #[test]
    fn streak_secured_is_pushed_only_when_saved_late() {
        let (app, path) = fixture();
        with_reviews(&app, "hill", 1);
        let mut profile = app.profile("hill").unwrap();
        let store = app.store.lock().unwrap();
        profile.streak = 5;
        let early = profile.day_ends_at - 10 * 3_600_000;
        assert!(!streak_saved_late(&store, "hill", &profile, early).unwrap());
        let late = profile.day_ends_at - 3_600_000;
        assert!(streak_saved_late(&store, "hill", &profile, late).unwrap());
        store
            .mark_seen("hill", &format!("reminder:urgent:{}", profile.day))
            .unwrap();
        assert!(
            streak_saved_late(&store, "hill", &profile, early).unwrap(),
            "saved after an urgent reminder"
        );
        profile.streak = 1;
        assert!(
            !streak_saved_late(&store, "hill", &profile, late).unwrap(),
            "a first day had nothing at risk"
        );
        drop(store);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn a_tick_turns_moments_into_wins_and_sends_automatic_ones_as_one_digest() {
        // Keep the fixture's last ten minutes within one study day.
        let study_time = 1_793_183_400_000;
        crate::app::fix_now_ms(Some(study_time));

        let (app, path) = fixture();
        with_reviews(&app, "hill", 1);
        tick(&app).unwrap();
        {
            let store = app.store.lock().unwrap();
            wins::update_sharing(
                &store.conn,
                "hill",
                wins::SharingUpdate {
                    policies: [
                        ("daily_goal".to_string(), "auto".to_string()),
                        ("streak_secured".to_string(), "auto".to_string()),
                    ]
                    .into(),
                    daily_cap: None,
                },
            )
            .unwrap();
            store
                .conn
                .execute(
                    "insert into goals (user, mode, value) values ('hill', 'cards', 3)",
                    [],
                )
                .unwrap();
            store
                .conn
                .execute(
                    "update seen set key = 'history' where key glob 'secured:*'",
                    [],
                )
                .unwrap();
        }
        with_reviews(&app, "hill", 4);
        tick(&app).unwrap();
        tick(&app).unwrap();
        let store = app.store.lock().unwrap();
        let kinds: Vec<String> = store
            .conn
            .prepare("select kind from wins where user = 'hill' order by id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(kinds, ["streak_secured", "daily_goal"]);
        for friend in ["cerro", "friend"] {
            let shares: Vec<_> = store
                .notifications(friend, now_ms())
                .unwrap()
                .into_iter()
                .filter(|n| n.kind == "share")
                .collect();
            assert_eq!(shares.len(), 1, "{friend}: {shares:?}");
            assert_eq!(shares[0].title, "🎉 Hill: 2 wins");
            assert_eq!(shares[0].body, "Streak secured! · Daily goal reached!");
        }
        let own = store.notifications("hill", now_ms()).unwrap();
        assert!(
            own.iter()
                .all(|n| n.kind != "celebration" || n.win.is_some())
        );
        drop(store);
        cleanup(app, path);
        crate::app::fix_now_ms(None);
    }

    #[test]
    fn one_players_failure_does_not_stop_the_others_or_delivery() {
        // Keep the fixture's last ten minutes within one study day.
        let study_time = 1_793_183_400_000;
        crate::app::fix_now_ms(Some(study_time));

        let (mut app, path) = fixture();
        let (base, server) = local_push(200);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("cerro").unwrap().ntfy_topic = Some("cerro-topic".into());
        for user in ["hill", "cerro", "friend"] {
            with_reviews(&app, user, 3);
        }
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "insert into player_settings (user, nudges) values ('hill', 'broken')",
                [],
            )
            .unwrap();
        let id = queued_message(&app, "cerro");
        tick(&app).unwrap();
        assert!(server.join().unwrap().starts_with("POST /cerro-topic "));
        assert!(was_pushed(&app, id));
        let seen = |user: &str| {
            app.store
                .lock()
                .unwrap()
                .conn
                .query_row("select count(*) from seen where user = ?1", [user], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
        };
        assert!(seen("cerro") > 0 && seen("friend") > 0);
        cleanup(app, path);
        crate::app::fix_now_ms(None);
    }

    async fn health_of(app: &Arc<App>) -> (StatusCode, serde_json::Value) {
        let response = health(State(app.clone())).await;
        let status = response.status();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    // The clock is pinned on this thread, which the current-thread runtime polls `health` on,
    // so the ages it reports don't drift with however long the steps take.
    #[tokio::test(flavor = "current_thread")]
    async fn health_needs_a_recent_tick_and_a_usable_store() {
        crate::app::fix_now_ms(Some(1_750_000_000_000));
        let (app, path) = fixture();
        app.heartbeat.beat(now_ms() - 30_000);
        assert_eq!(
            health_of(&app).await,
            (
                StatusCode::OK,
                serde_json::json!({"ok": true, "worker_age_s": 30})
            )
        );
        app.heartbeat.beat(now_ms() - STALE_AFTER_MS - 1000);
        let (status, body) = health_of(&app).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["ok"], false);
        app.heartbeat.beat(now_ms());
        let poisoner = app.clone();
        std::thread::spawn(move || {
            let _store = poisoner.store.lock().unwrap();
            panic!("poison the store");
        })
        .join()
        .unwrap_err();
        let (status, body) = health_of(&app).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, serde_json::json!({"ok": false, "worker_age_s": 0}));
        crate::app::fix_now_ms(None);
        cleanup(app, path);
    }

    #[test]
    fn missing_push_configuration_does_not_consume_a_queued_message() {
        let (app, path) = fixture();
        let id = queued_message(&app, "hill");
        tick(&app).unwrap();
        assert!(
            !was_pushed(&app, id),
            "an inbox-only delivery has not been pushed"
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn disabling_nudges_cancels_failed_pushes_without_losing_inbox_history() {
        for enable_again in [false, true] {
            let (mut app, path) = fixture();
            let (base, failed) = local_push(503);
            let config = &mut Arc::get_mut(&mut app).unwrap().config;
            config.ntfy = Some(base);
            config.users.get_mut("hill").unwrap().ntfy_topic = Some("hill-topic".into());
            let id = {
                let mut store = app.store.lock().unwrap();
                store.set_nudges("hill", true).unwrap();
                store
                    .send_once(
                        &notifications::Outgoing {
                            to: "hill",
                            from: "",
                            title: "Almost there",
                            body: "Level 2 is 90 XP away.",
                            kind: "nudge",
                        },
                        "nudge:test",
                        Clock::default().day(now_ms()),
                        now_ms(),
                        false,
                    )
                    .unwrap();
                store.conn.last_insert_rowid()
            };
            deliver_notifications(&app, PUSH_BUDGET).unwrap();
            assert!(failed.join().unwrap().ends_with("Level 2 is 90 XP away."));
            assert!(!was_pushed(&app, id));
            let settings = set_decks(
                State(app.clone()),
                UrlPath("hill".into()),
                headers("hill"),
                Json(decks::SettingsUpdate {
                    decks: vec![],
                    nudges: Some(false),
                    celebrations: None,
                    global_leaderboard: None,
                }),
            )
            .await
            .unwrap()
            .0;
            assert!(!settings.nudges);
            if enable_again {
                let settings = set_decks(
                    State(app.clone()),
                    UrlPath("hill".into()),
                    headers("hill"),
                    Json(decks::SettingsUpdate {
                        decks: vec![],
                        nudges: Some(true),
                        celebrations: None,
                        global_leaderboard: None,
                    }),
                )
                .await
                .unwrap()
                .0;
                assert!(settings.nudges);
            }
            drop(app);
            let mut store = Store::open(&path).unwrap();
            let retry_at = now_ms() + 60_000;
            assert!(
                store.take_deck_deliveries(retry_at).unwrap().is_empty(),
                "opting out must cancel a failed nudge even after restart and re-enabling"
            );
            assert!(!store.begin_push(id, retry_at).unwrap());
            let inbox = store.notifications("hill", retry_at).unwrap();
            assert_eq!(inbox.len(), 1);
            assert_eq!(inbox[0].id, id);
            assert_eq!(inbox[0].body, "Level 2 is 90 XP away.");
            assert!(!store.mark_seen("hill", "nudge:test").unwrap());
            cleanup(store, path);
        }
    }

    #[test]
    fn failed_http_push_is_not_acknowledged() {
        let (mut app, path) = fixture();
        let (base, server) = local_push(500);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("hill").unwrap().ntfy_topic = Some("hill-topic".into());
        let id = queued_message(&app, "hill");
        tick(&app).unwrap();
        assert!(server.join().unwrap().starts_with("POST /hill-topic "));
        assert!(
            !was_pushed(&app, id),
            "an HTTP error must leave the message retryable"
        );
        cleanup(app, path);
    }

    #[test]
    fn ntfy_pushes_request_high_priority_for_vibration_and_pop_up_alerts() {
        let (mut app, path) = fixture();
        let (base, server) = local_push(200);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("hill").unwrap().ntfy_topic = Some("hill-topic".into());
        let id = queued_message(&app, "hill");
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        let request = server.join().unwrap();
        assert!(
            request
                .lines()
                .any(|line| line.eq_ignore_ascii_case("priority: high")),
            "ntfy needs high priority to request vibration and a pop-up"
        );
        assert!(was_pushed(&app, id));
        cleanup(app, path);
    }

    #[test]
    fn ntfy_reply_includes_the_message_it_answers() {
        let (mut app, path) = fixture();
        let (base, server) = local_push(200);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("cerro").unwrap().ntfy_topic = Some("cerro-topic".into());
        let now = now_ms();
        let reply_id = {
            let mut store = app.store.lock().unwrap();
            let original = store
                .send(
                    &notifications::Outgoing {
                        to: "hill",
                        from: "cerro",
                        title: "Deck complete",
                        body: "Cerro finished Spanish.",
                        kind: "completion",
                    },
                    Clock::default().day(now),
                    now,
                )
                .unwrap();
            store
                .reply("hill", "Hill", original, "Good job!", now + 1)
                .unwrap();
            store.conn.last_insert_rowid()
        };
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        let request = server.join().unwrap();
        assert!(
            request.ends_with("Good job!\n\nIn reply to: Cerro finished Spanish."),
            "{request}"
        );
        assert!(was_pushed(&app, reply_id));
        assert_eq!(
            app.store
                .lock()
                .unwrap()
                .notifications("cerro", now + 1)
                .unwrap()[0]
                .body,
            "Good job!"
        );
        cleanup(app, path);
    }

    #[test]
    fn successful_retry_acknowledges_the_same_message_without_holding_the_store_lock() {
        let (mut app, path) = fixture();
        let (base, failed) = local_push(503);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("hill").unwrap().ntfy_topic = Some("hill-topic".into());
        let id = queued_message(&app, "hill");
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        failed.join().unwrap();
        assert!(!was_pushed(&app, id));
        let app_for_check = Arc::new(Mutex::new(std::sync::Weak::<App>::new()));
        let check = app_for_check.clone();
        let (base, success) = local_push_with_check(200, move || {
            let app = check.lock().unwrap().upgrade().unwrap();
            assert!(
                app.store.try_lock().is_ok(),
                "HTTP must not block uploads or inbox reads"
            );
        });
        Arc::get_mut(&mut app).unwrap().config.ntfy = Some(base);
        *app_for_check.lock().unwrap() = Arc::downgrade(&app);
        app.store
            .lock()
            .unwrap()
            .conn
            .execute("update notifications set retry_at = 0 where id = ?1", [id])
            .unwrap();
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        let request = success.join().unwrap();
        assert!(request.starts_with("POST /hill-topic "));
        assert!(request.ends_with("Do not lose this"));
        assert!(was_pushed(&app, id));
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        let store = app.store.lock().unwrap();
        assert_eq!(store.notifications("hill", now_ms()).unwrap()[0].id, id);
        assert_eq!(
            store
                .conn
                .query_row(
                    "select push_attempts from notifications where id = ?1",
                    [id],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        drop(store);
        cleanup(app, path);
    }

    #[test]
    fn push_uses_the_recipients_saved_language_without_rewriting_the_stored_notice() {
        let (mut app, path) = fixture();
        let (base, server) = local_push(200);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("hill").unwrap().ntfy_topic = Some("hill-topic".into());
        let id = {
            let mut store = app.store.lock().unwrap();
            store.set_language("hill", "es-AR").unwrap();
            store
                .send(
                    &notifications::Outgoing {
                        to: "hill",
                        from: "cerro",
                        title: "Deck complete",
                        body: "Cerro has finished their Friends studies for today.",
                        kind: "completion",
                    },
                    1,
                    now_ms(),
                )
                .unwrap()
        };
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        let request = server.join().unwrap();
        assert!(request.ends_with("Cerro ha terminado su estudio de Friends por hoy."));
        assert!(was_pushed(&app, id));
        assert!(
            app.store
                .lock()
                .unwrap()
                .notifications("hill", now_ms())
                .unwrap()[0]
                .body
                .contains("has finished")
        );
        cleanup(app, path);
    }

    #[test]
    fn unconfigured_front_rows_do_not_block_a_configured_recipient() {
        let (mut app, path) = fixture();
        let (base, server) = local_push(200);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("friend").unwrap().ntfy_topic = Some("friend-topic".into());
        for _ in 0..101 {
            queued_message(&app, "hill");
        }
        let id = queued_message(&app, "friend");
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        assert!(server.join().unwrap().starts_with("POST /friend-topic "));
        assert!(was_pushed(&app, id));
        assert_eq!(app.store.lock().unwrap().conn.query_row("select sum(push_attempts + pushed) from notifications where recipient = 'hill'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        cleanup(app, path);
    }

    #[test]
    fn a_recipient_without_a_topic_keeps_the_inbox_copy_marked_as_having_no_destination() {
        let (mut app, path) = fixture();
        let id = queued_message(&app, "hill");
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        let state = |app: &App| {
            app.store
                .lock()
                .unwrap()
                .conn
                .query_row(
                    "select push_cancelled, pushed from notifications where id = ?1",
                    [id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap()
        };
        assert_eq!(state(&app), (notifications::NO_PUSH_DESTINATION, 0));
        assert_eq!(
            app.store
                .lock()
                .unwrap()
                .notifications("hill", now_ms())
                .unwrap()[0]
                .id,
            id
        );
        // A topic added later does not push the message that was already skipped.
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some("http://127.0.0.1:9".into());
        config.users.get_mut("hill").unwrap().ntfy_topic = Some("hill-topic".into());
        deliver_notifications(&app, PUSH_BUDGET).unwrap();
        assert_eq!(state(&app), (notifications::NO_PUSH_DESTINATION, 0));
        assert!(
            app.store
                .lock()
                .unwrap()
                .take_deck_deliveries(now_ms())
                .unwrap()
                .is_empty()
        );
        cleanup(app, path);
    }

    #[test]
    fn network_budget_leaves_unattempted_messages_ready_for_the_next_tick() {
        let (mut app, path) = fixture();
        // A listener nobody accepts on: the system finishes the connection and takes the
        // request, but no answer ever comes, so the push only ends when its timeout does.
        let stalled = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(format!("http://{}", stalled.local_addr().unwrap()));
        for user in ["hill", "friend"] {
            config.users.get_mut(user).unwrap().ntfy_topic = Some(user.into());
        }
        let first = queued_message(&app, "hill");
        let second = queued_message(&app, "friend");
        let attempts = |id: i64| {
            app.store
                .lock()
                .unwrap()
                .conn
                .query_row(
                    "select push_attempts + retry_at from notifications where id = ?1",
                    [id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
        };
        deliver_notifications(&app, Duration::ZERO).unwrap();
        assert_eq!(attempts(first), 0, "no budget means no attempt");
        let start = std::time::Instant::now();
        deliver_notifications(&app, Duration::from_secs(1)).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the request timeout respects the remaining budget, not the 10 second cap"
        );
        assert!(attempts(first) > 0, "the first message is attempted");
        assert!(!was_pushed(&app, first));
        assert_eq!(
            attempts(second),
            0,
            "skipped messages must not be leased or marked delivered"
        );
        drop(stalled);
        cleanup(app, path);
    }

    #[test]
    fn failed_server_game_events_remain_durable_without_duplicating_the_inbox() {
        let (mut app, path) = fixture();
        let (base, server) = local_push(500);
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        config.users.get_mut("cerro").unwrap().ntfy_topic = Some("cerro-topic".into());
        let now = now_ms();
        let reviews: Vec<_> = (0..100)
            .map(|i| Review {
                id: now - (100 - i) * 10_000,
                cid: i,
                last_ivl: 30,
                ivl: None,
                time_ms: 5000,
                kind: 0,
            })
            .collect();
        {
            let mut store = app.store.lock().unwrap();
            store
                .upsert("cerro", &reviews, &[], Clock::default())
                .unwrap();
            app.players
                .write()
                .unwrap()
                .insert("cerro".into(), load_player(&store, "cerro").unwrap());
        }
        assert!(!app.profile("cerro").unwrap().events.is_empty());
        tick(&app).unwrap();
        server.join().unwrap();
        let count: i64 = app
            .store
            .lock()
            .unwrap()
            .conn
            .query_row(
                "select count(*) from notifications where recipient = 'cerro' and pushed = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(count > 0, "failed server unlocks need a durable retry");
        tick(&app).unwrap();
        assert!(
            app.store
                .lock()
                .unwrap()
                .notifications("cerro", now_ms())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            app.store
                .lock()
                .unwrap()
                .conn
                .query_row(
                    "select count(*) from notifications where recipient = 'cerro'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            count
        );
        cleanup(app, path);
    }

    #[test]
    fn the_nudge_target_is_the_closest_player_above_within_the_circle() {
        let (store, path) = crate::test_support::temporary_store();
        store
            .conn
            .execute_batch("insert into accounts (user, display, password_hash, created_at) values ('ana', 'Ana', 'x', 0)")
            .unwrap();
        let app = app_from(
            store,
            serde_json::from_value(
                serde_json::json!({"users": {"cerro": {}, "hill": {}, "friend": {}}}),
            )
            .unwrap(),
        );
        for (user, count) in [("friend", 2), ("ana", 4), ("cerro", 6), ("hill", 20)] {
            with_reviews(&app, user, count);
        }
        let users = ["friend", "ana", "cerro", "hill"];
        let ranking: HashMap<&str, Arc<Profile>> = users
            .iter()
            .map(|user| (*user, app.standing(user).unwrap()))
            .collect();
        let mut legacy: Vec<(u64, &str)> = ranking
            .iter()
            .filter(|(user, _)| app.legacy(user))
            .map(|(user, profile)| (profile.week_xp, *user))
            .collect();
        legacy.sort_unstable();
        let ahead = |user: &str| {
            let store = app.store.lock().unwrap();
            just_ahead(&app, &store.conn, user, &ranking[user], &ranking, &legacy)
                .unwrap()
                .map(|profile| profile.user.clone())
        };
        assert_eq!(
            ahead("friend").as_deref(),
            Some("cerro"),
            "a stranger's account is not in reach"
        );
        assert_eq!(ahead("cerro").as_deref(), Some("hill"));
        assert_eq!(ahead("hill"), None);
        assert_eq!(
            ahead("ana"),
            None,
            "an account with no friends has nobody to chase"
        );
        app.store
            .lock()
            .unwrap()
            .conn
            .execute("insert into friendships (a, b, requested_by, accepted, created_at) values ('ana', 'friend', 'ana', 1, 0)", [])
            .unwrap();
        assert_eq!(
            ahead("friend").as_deref(),
            Some("ana"),
            "a friend counts once connected"
        );
        assert_eq!(
            ahead("ana").as_deref(),
            None,
            "the friend is behind, and ana is not in the original community"
        );
        cleanup(app, path);
    }
}
