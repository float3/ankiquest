//! The background work: syncing collections, refreshing standings, and pushing notifications.

use crate::app::{App, mark_events_seen, now_ms};
use crate::config::Config;
use crate::game::{Clock, Profile};
use crate::refresh_community;
use crate::store::{Error, Store};
use crate::{
    admin, challenges, competition, freezes, game, i18n, notifications, privacy, reminders, social,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const POLL: Duration = Duration::from_secs(20);
const PUSH_BUDGET: Duration = Duration::from_secs(20);

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

pub(crate) fn import(app: &App, store: &mut Store, base: &Path) -> Result<(), Error> {
    let mut errors = Vec::new();
    for user in sync_users(base)? {
        let result = (|| -> Result<(), Error> {
            let first_import = !store.is_known(&user)?;
            if store.ingest(base, &user)? {
                app.reload_player(store, &user)?;
            }
            if first_import && let Some(profile) = app.profile(&user) {
                mark_events_seen(store, &user, &profile)?;
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

fn streak_warning_body(profile: &Profile) -> String {
    let minutes = match profile.today.streak_minutes {
        0 => String::new(),
        needed => format!(
            " with {} of {needed} minutes done",
            profile.today.minutes.min(needed)
        ),
    };
    format!(
        "Your {} day streak ends tonight{minutes}. {}",
        profile.streak,
        if !profile.freezes_enabled {
            "Streak freezes are turned off."
        } else if profile.freezes > 0 {
            "A freeze would cover you, but why spend it?"
        } else {
            "No freezes left."
        }
    )
}

pub(crate) fn tick(app: &App) -> Result<(), Error> {
    let now = now_ms();
    let participants =
        {
            let mut store = app.store.lock().unwrap();
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
            privacy::carry_out_removals(app, &store.conn)?;
            for user in freezes::grant_reloads(&store.conn)? {
                if app.players.read().unwrap().contains_key(&user) {
                    app.reload_player(&store, &user)?;
                }
                freezes::grant_reloaded(&store.conn, &user)?;
            }
            let import_complete = app.config.sync_base.as_ref().is_none_or(|base| {
                match import(app, &mut store, base) {
                    Ok(()) => true,
                    Err(error) => {
                        log_error!(
                            "competition refresh deferred until sync import succeeds: {error}"
                        );
                        false
                    }
                }
            });
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
        let push_enabled = push_destination(&app.config, user).is_some();
        store.queue_events(user, &profile.events, profile.day, now_ms(), push_enabled)?;
        if store.nudges_enabled(user)? {
            let ahead = just_ahead(app, &store.conn, user, profile, &ranking, &legacy)?
                .map(|other| (other.display.clone(), other.week_xp));
            for nudge in game::nudges(profile, ahead.as_ref().map(|(w, xp)| (w.as_str(), *xp))) {
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
            reminders::tick(&mut store, user, profile, clock, &app.week, now, recap)?;
        }
    }
    deliver_notifications(app, PUSH_BUDGET)
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
            app.store.lock().unwrap().cancel_push(id)?;
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
        let current_warning = if delivery.notification.kind == "risk" {
            match app.profile(&delivery.user) {
                Some(profile) if profile.day == delivery.notification.day && profile.at_risk => {
                    Some(streak_warning_body(&profile))
                }
                _ => {
                    app.store.lock().unwrap().cancel_streak_warning(id)?;
                    continue;
                }
            }
        } else {
            reminder_body
        };
        if !app.store.lock().unwrap().begin_push(id, now_ms())? {
            continue;
        }
        let language = app.store.lock().unwrap().language(&delivery.user)?;
        if let Some(body) = current_warning {
            delivery.notification.body = body;
        }
        let sender_display = app.display(&delivery.notification.sender);
        i18n::notification(&mut delivery.notification, &language, &sender_display);
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
                "risk" | "reminder_urgent" => "fire",
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
/// Runs [`tick`] every [`POLL`] on its own thread for as long as the server runs.
pub(crate) fn spawn(app: Arc<App>) {
    std::thread::spawn(move || {
        loop {
            if let Err(e) = tick(&app) {
                log_error!("tick failed: {e}");
            }
            std::thread::sleep(POLL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FreezeUpdate;
    use crate::accounts;
    use crate::app::load_player;
    use crate::set_decks;
    use crate::set_freezes;
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
    fn missing_push_configuration_does_not_consume_a_queued_message() {
        let (app, path) = fixture();
        let id = queued_message(&app, "hill");
        tick(&app).unwrap();
        assert!(
            !was_pushed(&app, id),
            "an inbox-only delivery has not been pushed"
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn stale_streak_warnings_are_cancelled_after_rollover_or_studying() {
        for studied in [false, true] {
            let (app, path) = fixture();
            let now = now_ms();
            let day = Clock::default().day(now);
            let review = Review {
                id: if studied { now } else { now - 86_400_000 },
                cid: 1,
                last_ivl: 30,
                ivl: None,
                time_ms: 5000,
                kind: 1,
            };
            {
                let mut store = app.store.lock().unwrap();
                store
                    .upsert("hill", &[review], &[], Clock::default())
                    .unwrap();
                app.players
                    .write()
                    .unwrap()
                    .insert("hill".into(), load_player(&store, "hill").unwrap());
                store
                    .send_once(
                        &notifications::Outgoing {
                            to: "hill",
                            from: "",
                            title: "Streak at risk",
                            body: "Your streak ends tonight",
                            kind: "risk",
                        },
                        "risk:test",
                        if studied { day } else { day - 1 },
                        now,
                        true,
                    )
                    .unwrap();
            }
            deliver_notifications(&app, PUSH_BUDGET).unwrap();
            assert_eq!(
                app.store
                    .lock()
                    .unwrap()
                    .conn
                    .query_row(
                        "select count(*) from notifications where kind = 'risk'",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                0,
                "an obsolete warning must not be sent on a later retry"
            );
            assert!(
                !app.store
                    .lock()
                    .unwrap()
                    .mark_seen("hill", "risk:test")
                    .unwrap(),
                "cancelling a stale warning must not generate it again"
            );
            drop(app);
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[tokio::test]
    async fn queued_streak_warnings_follow_freeze_setting_changes_before_retry() {
        for enabled in [false, true] {
            let (mut app, path) = fixture();
            let (base, failed) = local_push(503);
            let config = &mut Arc::get_mut(&mut app).unwrap().config;
            config.ntfy = Some(base);
            config.users.get_mut("hill").unwrap().ntfy_topic = Some("hill-topic".into());
            let clock = Clock::default();
            let day = clock.day(now_ms());
            let start = (day - 1) * 86_400_000;
            let reviews: Vec<_> = (0..60)
                .map(|i| Review {
                    id: start + 6 * 3_600_000 + i * 10_000 + if i >= 30 { 600_000 } else { 0 },
                    cid: i,
                    last_ivl: 30,
                    ivl: None,
                    time_ms: 10_000,
                    kind: 0,
                })
                .collect();
            {
                let mut store = app.store.lock().unwrap();
                store.upsert("hill", &reviews, &[], clock).unwrap();
                store.set_freezes_enabled("hill", true, start).unwrap();
                app.players
                    .write()
                    .unwrap()
                    .insert("hill".into(), load_player(&store, "hill").unwrap());
            }
            // Both directions retain one earned freeze: only its availability changes.
            let settings = set_freezes(
                State(app.clone()),
                UrlPath("hill".into()),
                headers("hill"),
                Json(FreezeUpdate { enabled: !enabled }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(settings.enabled, !enabled);
            let before = app.profile("hill").unwrap();
            assert_eq!(before.stored_freezes, 1);
            assert_eq!(before.freezes_enabled, !enabled);
            assert!(before.at_risk);
            let old_tail = if enabled {
                "Streak freezes are turned off."
            } else {
                "A freeze would cover you, but why spend it?"
            };
            let old_body = format!("Your {} day streak ends tonight. {old_tail}", before.streak);
            let id = {
                let mut store = app.store.lock().unwrap();
                store
                    .send_once(
                        &notifications::Outgoing {
                            to: "hill",
                            from: "",
                            title: "Streak at risk",
                            body: &old_body,
                            kind: "risk",
                        },
                        &format!("risk:{day}"),
                        day,
                        now_ms(),
                        true,
                    )
                    .unwrap();
                store.conn.last_insert_rowid()
            };
            deliver_notifications(&app, PUSH_BUDGET).unwrap();
            assert!(failed.join().unwrap().ends_with(&old_body));
            assert!(!was_pushed(&app, id));

            let settings = set_freezes(
                State(app.clone()),
                UrlPath("hill".into()),
                headers("hill"),
                Json(FreezeUpdate { enabled }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(settings.enabled, enabled);
            assert_eq!(settings.freezes, 1);
            let current = app.profile("hill").unwrap();
            assert_eq!(current.stored_freezes, 1);
            assert_eq!(current.freezes, u32::from(enabled));
            let (base, success) = local_push(200);
            Arc::get_mut(&mut app).unwrap().config.ntfy = Some(base);
            app.store
                .lock()
                .unwrap()
                .conn
                .execute("update notifications set retry_at = 0 where id = ?1", [id])
                .unwrap();
            deliver_notifications(&app, PUSH_BUDGET).unwrap();
            let request = success.join().unwrap();
            let expected_tail = if enabled {
                "A freeze would cover you, but why spend it?"
            } else {
                "Streak freezes are turned off."
            };
            assert!(
                request.ends_with(expected_tail),
                "retried warning must describe the current setting (enabled={enabled})"
            );
            assert!(!request.ends_with(old_tail));
            assert!(was_pushed(&app, id));
            assert!(
                app.store
                    .lock()
                    .unwrap()
                    .notifications("hill", now_ms())
                    .unwrap()
                    .is_empty()
            );
            drop(app);
            std::fs::remove_dir_all(path).unwrap();
        }
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
            drop(store);
            std::fs::remove_dir_all(path).unwrap();
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
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
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
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
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
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
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
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
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
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
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
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn network_budget_leaves_unattempted_messages_ready_for_the_next_tick() {
        let (mut app, path) = fixture();
        let (base, server) =
            local_push_with_check(200, || std::thread::sleep(Duration::from_millis(500)));
        let config = &mut Arc::get_mut(&mut app).unwrap().config;
        config.ntfy = Some(base);
        for user in ["hill", "friend"] {
            config.users.get_mut(user).unwrap().ntfy_topic = Some(user.into());
        }
        let first = queued_message(&app, "hill");
        let second = queued_message(&app, "friend");
        deliver_notifications(&app, Duration::ZERO).unwrap();
        let start = std::time::Instant::now();
        deliver_notifications(&app, Duration::from_millis(100)).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "the request timeout respects the remaining budget"
        );
        server.join().unwrap();
        assert!(!was_pushed(&app, first));
        let store = app.store.lock().unwrap();
        assert_eq!(
            store
                .conn
                .query_row(
                    "select push_attempts + retry_at from notifications where id = ?1",
                    [second],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0,
            "skipped messages must not be leased or marked delivered"
        );
        drop(store);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
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
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn the_nudge_target_is_the_closest_player_above_within_the_circle() {
        let (store, path) = decks::tests::temporary_store();
        store
            .conn
            .execute_batch("insert into accounts (user, display, password_hash, created_at) values ('ana', 'Ana', 'x', 0)")
            .unwrap();
        let accounts = accounts::Directory::load(&store.conn).unwrap();
        let app = App {
            config: serde_json::from_value(
                serde_json::json!({"users": {"cerro": {}, "hill": {}, "friend": {}}}),
            )
            .unwrap(),
            access: access::Access::default(),
            accounts,
            week: Week::default(),
            store: Mutex::new(store),
            players: RwLock::new(HashMap::new()),
            standings: Default::default(),
        };
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
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
