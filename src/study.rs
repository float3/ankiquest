//! A player's own study: profile, history, previews, and uploading reviews.

use crate::access::authorized;
use crate::app::{App, Player, mark_events_seen, now_ms};
use crate::game::{Clock, Profile, Review};
use crate::web_util::{failed_for, store_error};
use crate::{access, challenges, decks, feedback, game, i18n};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

const MAX_PENDING: usize = 5000;

pub(crate) async fn profile(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Json<Profile>, StatusCode> {
    let owner = authorized(&app, &user, &headers);
    let language = {
        let store = app.store.lock().unwrap();
        if owner {
            i18n::owned(&headers, &store, &user)
        } else {
            i18n::selected(&headers, &store, &user)
        }
    }
    .map_err(store_error)?;
    let viewer = access::viewer(&app, &headers);
    if !app
        .visible(&app.store.lock().unwrap().conn, &viewer, &user)
        .map_err(store_error)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut profile = app.profile(&user).ok_or(StatusCode::NOT_FOUND)?;
    i18n::profile(&mut profile, &language);
    Ok(Json(profile))
}

#[derive(Deserialize, Default)]
struct StudyQuery {
    year: Option<i32>,
}

#[derive(Serialize)]
struct StudySession {
    started_at: i64,
    ended_at: i64,
    reviews: u64,
    time_ms: i64,
    new_cards: u64,
}

#[derive(Serialize)]
struct StudyHistory {
    year: i32,
    years: Vec<i32>,
    days: Vec<game::HistoryDay>,
    last_review_at: Option<i64>,
    last_received_at: Option<i64>,
    latest_session: Option<StudySession>,
}

fn latest_study_session(player: &Player) -> Option<StudySession> {
    let last = player.reviews.last()?;
    let day = player.clock_history.day(last.id);
    let mut session = StudySession {
        started_at: last.id,
        ended_at: last.id,
        reviews: 0,
        time_ms: 0,
        new_cards: 0,
    };
    let mut next_at = last.id;
    let mut new_cards = HashSet::new();
    for review in player.reviews.iter().rev() {
        if player.clock_history.day(review.id) != day
            || next_at.saturating_sub(review.id) >= game::SESSION_GAP_MS
        {
            break;
        }
        session.started_at = review.id;
        session.reviews += 1;
        session.time_ms = session.time_ms.saturating_add(review.time_ms.max(0));
        if review.kind == 0 && new_cards.insert(review.cid) {
            session.new_cards += 1;
        }
        next_at = review.id;
    }
    Some(session)
}

/// Years the history and community pages can show.
pub(crate) fn valid_year(year: i32) -> bool {
    (1970..=2100).contains(&year)
}

async fn study_history(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    Query(query): Query<StudyQuery>,
    headers: HeaderMap,
) -> Result<Json<StudyHistory>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let (current_year, latest_session) = {
        let players = app.players.read().unwrap();
        let player = players.get(&user);
        let day = player.map_or_else(
            || now_ms().div_euclid(game::DAY_MS),
            |player| player.clock.day(now_ms()),
        );
        let year = game::date_string(day)
            .get(..4)
            .and_then(|value| value.parse().ok())
            .unwrap_or(1970);
        (year, player.and_then(latest_study_session))
    };
    let year = query.year.unwrap_or(current_year);
    if !valid_year(year) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let profile = app.profile(&user);
    let (years, days) = profile.as_ref().map_or_else(
        || (Vec::new(), Vec::new()),
        |profile| {
            let years: BTreeSet<_> = profile
                .history
                .iter()
                .filter_map(|day| day.date.get(..4)?.parse::<i32>().ok())
                .collect();
            let days = profile
                .history
                .iter()
                .filter(|day| day.date.starts_with(&year.to_string()))
                .cloned()
                .collect();
            (years.into_iter().rev().collect(), days)
        },
    );
    let last_received_at = app
        .store
        .lock()
        .unwrap()
        .last_review_sync(&user)
        .map_err(store_error)?;
    Ok(Json(StudyHistory {
        year,
        years,
        days,
        last_review_at: profile
            .as_ref()
            .and_then(|p| (p.last_review_id > 0).then_some(p.last_review_id)),
        last_received_at,
        latest_session,
    }))
}

#[derive(Deserialize)]
pub(crate) struct Pending {
    pub(crate) reviews: Vec<Review>,
    /// The newest review the client already showed feedback for.
    pub(crate) seen_through: Option<i64>,
}

pub(crate) async fn preview(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(pending): Json<Pending>,
) -> Result<Json<UploadResponse>, StatusCode> {
    if pending.reviews.len() > MAX_PENDING {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    if !authorized(&app, &user, &headers)
        && !app
            .visible(
                &app.store.lock().unwrap().conn,
                &access::viewer(&app, &headers),
                &user,
            )
            .map_err(store_error)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut profile = app
        .preview(&user, &pending.reviews)
        .ok_or(StatusCode::NOT_FOUND)?;
    let before = pending.seen_through.and_then(|seen| {
        let shown: Vec<Review> = pending
            .reviews
            .iter()
            .filter(|review| review.id <= seen)
            .copied()
            .collect();
        app.preview(&user, &shown)
    });
    let mut feedback = feedback::feedback(before.as_ref(), &profile, &[]);
    let language =
        i18n::selected(&headers, &app.store.lock().unwrap(), &user).map_err(store_error)?;
    i18n::profile(&mut profile, &language);
    i18n::feedback(&mut feedback, &language);
    Ok(Json(UploadResponse {
        profile,
        announced: Vec::new(),
        feedback,
    }))
}

#[derive(Deserialize)]
pub(crate) struct Upload {
    pub(crate) reviews: Vec<Review>,
    #[serde(default)]
    pub(crate) deleted: Vec<i64>,
    pub(crate) clock: Clock,
    #[serde(default)]
    pub(crate) silent: bool,
    pub(crate) decks: Option<Vec<decks::Snapshot>>,
    #[serde(default)]
    pub(crate) catalog: bool,
}

/// The player's profile, plus any deck completion this upload just announced.
#[derive(Debug, Serialize)]
pub(crate) struct UploadResponse {
    #[serde(flatten)]
    pub(crate) profile: Profile,
    pub(crate) announced: Vec<decks::Announcement>,
    pub(crate) feedback: feedback::Feedback,
}

pub(crate) async fn upload(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(upload): Json<Upload>,
) -> Result<Json<UploadResponse>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if upload.reviews.len() > MAX_PENDING || upload.deleted.len() > MAX_PENDING {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    if upload
        .decks
        .as_ref()
        .is_some_and(|decks| decks.len() > decks::MAX_DECKS)
    {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    if !decks::valid_clock(upload.clock)
        || upload
            .decks
            .as_ref()
            .is_some_and(|decks| !decks::valid_snapshots(decks))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let reviews: Vec<Review> = upload.reviews.into_iter().filter(|r| r.kind < 4).collect();
    let store_error = failed_for("upload for", &user);
    let mut store = app.store.lock().unwrap();
    let silent = upload.silent || !store.is_known(&user).map_err(store_error)?;
    let before = if silent { None } else { app.profile(&user) };
    store
        .upsert(&user, &reviews, &upload.deleted, upload.clock)
        .map_err(store_error)?;
    let mut announced = Vec::new();
    if let Some(decks) = &upload.decks {
        announced = store
            .record_decks(
                &user,
                &app.display(&user),
                decks,
                upload.clock,
                silent,
                now_ms(),
            )
            .map_err(store_error)?;
        if upload.catalog {
            store.prune_decks(&user, decks).map_err(store_error)?;
        }
    }
    app.reload_player(&store, &user).map_err(store_error)?;
    challenges::refresh(&mut store, &app.participants(), now_ms()).map_err(store_error)?;
    let mut profile = app.profile(&user).ok_or(StatusCode::NOT_FOUND)?;
    if silent {
        mark_events_seen(&store, &user, &profile).map_err(store_error)?;
    }
    let mut feedback = feedback::feedback(before.as_ref(), &profile, &announced);
    let language = i18n::owned(&headers, &store, &user).map_err(store_error)?;
    i18n::profile(&mut profile, &language);
    i18n::feedback(&mut feedback, &language);
    Ok(Json(UploadResponse {
        profile,
        announced,
        feedback,
    }))
}
pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/profile/{user}", get(profile))
        .route("/api/study/{user}", get(study_history))
        .route("/api/preview/{user}", post(preview))
        .route("/api/reviews/{user}", post(upload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::load_player;
    use crate::decks::set_decks;
    use crate::inbox::get_notifications;
    use crate::test_support::*;

    #[tokio::test]
    async fn personal_history_requires_owner_and_uses_anki_days_and_review_gaps() {
        let (app, _path) = fixture();
        let at = |hour: &str| {
            chrono::DateTime::parse_from_rfc3339(&format!("2026-09-25T{hour}:00Z"))
                .unwrap()
                .timestamp_millis()
        };
        {
            let mut store = app.store.lock().unwrap();
            let reviews = [
                (at("03:59"), 1, 1),
                (at("04:01"), 2, 1),
                (at("04:10"), 3, 0),
                (at("04:12"), 3, 1),
            ]
            .map(|(id, cid, kind)| Review {
                id,
                cid,
                kind,
                last_ivl: 1,
                ivl: None,
                time_ms: 30_000,
            });
            store
                .upsert("cerro", &reviews, &[], Clock::default())
                .unwrap();
            app.players
                .write()
                .unwrap()
                .insert("cerro".into(), load_player(&store, "cerro").unwrap());
        }
        assert_eq!(
            study_history(
                State(app.clone()),
                UrlPath("cerro".into()),
                Query(StudyQuery { year: Some(2026) }),
                headers("hill")
            )
            .await
            .err(),
            Some(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            study_history(
                State(app.clone()),
                UrlPath("cerro".into()),
                Query(StudyQuery { year: Some(1800) }),
                headers("cerro")
            )
            .await
            .err(),
            Some(StatusCode::BAD_REQUEST)
        );
        let Json(history) = study_history(
            State(app),
            UrlPath("cerro".into()),
            Query(StudyQuery { year: Some(2026) }),
            headers("cerro"),
        )
        .await
        .unwrap();
        assert!(history.last_received_at.is_some());
        assert_eq!(history.last_review_at, Some(at("04:12")));
        assert_eq!(
            history
                .days
                .iter()
                .find(|day| day.date == "2026-09-24")
                .unwrap()
                .reviews,
            1
        );
        assert_eq!(
            history
                .days
                .iter()
                .find(|day| day.date == "2026-09-25")
                .unwrap()
                .reviews,
            3
        );
        let session = history.latest_session.unwrap();
        assert_eq!(
            (session.started_at, session.ended_at),
            (at("04:10"), at("04:12"))
        );
        assert_eq!(
            (session.reviews, session.time_ms, session.new_cards),
            (2, 60_000, 1)
        );
    }

    #[tokio::test]
    async fn the_upload_reports_what_it_just_announced() {
        let (app, path) = fixture();
        let send = |body: Upload| {
            let app = app.clone();
            async move {
                upload(
                    State(app),
                    UrlPath("cerro".into()),
                    headers("cerro"),
                    Json(body),
                )
                .await
                .unwrap()
                .0
            }
        };
        assert!(send(sample_upload(2, false)).await.announced.is_empty());
        let _ = set_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(preferences(&["hill"])),
        )
        .await
        .unwrap();

        let announced = send(sample_upload(0, false)).await.announced;
        assert_eq!(announced.len(), 1);
        assert_eq!(announced[0].deck, "Spanish");
        assert_eq!(announced[0].recipients, 1);
        assert!(
            send(sample_upload(0, false)).await.announced.is_empty(),
            "a deck is only announced once a day"
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn legacy_and_silent_uploads_and_previews_do_not_publish_completion() {
        let (app, path) = fixture();
        let legacy: Upload = serde_json::from_value(serde_json::json!({
            "reviews": [], "clock": {"offset_west_min": 0, "rollover_hour": 4}
        }))
        .unwrap();
        assert!(legacy.decks.is_none());
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(legacy),
        )
        .await
        .unwrap();
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
            Json(preferences(&["hill"])),
        )
        .await
        .unwrap();
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(0, true)),
        )
        .await
        .unwrap();
        let _ = upload(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(sample_upload(0, false)),
        )
        .await
        .unwrap();
        let _ = preview(
            State(app.clone()),
            UrlPath("cerro".into()),
            HeaderMap::new(),
            Json(Pending {
                reviews: vec![],
                seen_through: None,
            }),
        )
        .await
        .unwrap();
        assert!(
            get_notifications(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0
                .is_empty()
        );
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn uploads_describe_what_they_changed() {
        let (app, path) = fixture();
        let start = now_ms() - 60_000;
        let first = upload_as(&app, "hill", plain_upload(reviews_at(start, 2))).await;
        assert_eq!(first.feedback, feedback::Feedback::default());
        let more = upload_as(&app, "hill", plain_upload(reviews_at(start, 4))).await;
        let status = more.feedback.status.unwrap();
        assert!(status.starts_with('+'), "{status}");
        assert!(status.contains("Lv "), "{status}");
        let again = upload_as(&app, "hill", plain_upload(reviews_at(start, 4))).await;
        assert_eq!(again.feedback.status, None);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn announcements_lead_the_upload_feedback() {
        let (app, path) = fixture();
        upload_as(&app, "cerro", sample_upload(2, false)).await;
        let _ = set_decks(
            State(app.clone()),
            UrlPath("cerro".into()),
            headers("cerro"),
            Json(preferences(&["hill"])),
        )
        .await
        .unwrap();
        let done = upload_as(&app, "cerro", sample_upload(0, false)).await;
        assert_eq!(done.feedback.headlines, vec!["📣 Spanish — told 1 friend"]);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn previews_describe_reviews_since_the_last_preview() {
        let (app, path) = fixture();
        let start = now_ms() - 60_000;
        upload_as(&app, "hill", plain_upload(reviews_at(start, 1))).await;
        let pending = reviews_at(start, 3);
        let ask = |seen_through| {
            let app = app.clone();
            let reviews = pending.clone();
            async move {
                preview(
                    State(app),
                    UrlPath("hill".into()),
                    HeaderMap::new(),
                    Json(Pending {
                        reviews,
                        seen_through,
                    }),
                )
                .await
                .unwrap()
                .0
            }
        };
        assert_eq!(ask(None).await.feedback.status, None);
        let status = ask(Some(pending[1].id)).await.feedback.status.unwrap();
        assert!(status.starts_with('+'), "{status}");
        assert_eq!(ask(Some(pending[2].id)).await.feedback.status, None);
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
