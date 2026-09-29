//! Device-selected language, with English fallback and shared interface catalogs.
use crate::{
    App, authorized,
    store::{Error, Store},
};
pub use ankiquest_i18n::{normalize, substitute, text};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists languages (user text primary key, language text not null);",
    )?;
    Ok(())
}

impl Store {
    pub fn language(&self, user: &str) -> Result<String, Error> {
        Ok(self
            .conn
            .query_row(
                "select language from languages where user=?1",
                [user],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "en".into()))
    }

    pub fn set_language(&self, user: &str, language: &str) -> Result<(), Error> {
        self.conn.execute("insert into languages (user,language) values (?1,?2) on conflict(user) do update set language=excluded.language", params![user, normalize(language)])?;
        Ok(())
    }
}

pub fn selected(headers: &HeaderMap, store: &Store, user: &str) -> Result<String, Error> {
    if let Some(value) = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
    {
        return Ok(normalize(value.split([',', ';']).next().unwrap_or("en").trim()).into());
    }
    store.language(user)
}

/// Call only after authenticating the owner. Public profile views never change preferences.
pub fn owned(headers: &HeaderMap, store: &Store, user: &str) -> Result<String, Error> {
    let language = selected(headers, store, user)?;
    if headers.contains_key(header::ACCEPT_LANGUAGE) {
        store.set_language(user, &language)?;
    }
    Ok(language)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Preference {
    pub language: String,
}

pub async fn get(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Preference>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(Preference {
        language: app
            .store
            .lock()
            .unwrap()
            .language(&user)
            .map_err(crate::store_error)?,
    }))
}

pub async fn set(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(preference): Json<Preference>,
) -> Result<Json<Preference>, StatusCode> {
    if !authorized(&app, &user, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if preference.language.is_empty()
        || preference.language.len() > 64
        || !preference
            .language
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let language = normalize(&preference.language).to_string();
    app.store
        .lock()
        .unwrap()
        .set_language(&user, &language)
        .map_err(crate::store_error)?;
    Ok(Json(Preference { language }))
}

pub fn notice(notice: &mut crate::feedback::Notice, language: &str) {
    notice.title = system_text(&notice.title, language);
    notice.body = system_text(&notice.body, language);
}

fn system_text(value: &str, language: &str) -> String {
    let language = normalize(language);
    if language == "en" {
        return value.into();
    }
    if let Some(title) = value.strip_prefix("Achievement: ") {
        let title = text(title, language);
        return if language == "es" {
            format!("Logro: {title}")
        } else {
            substitute(&text("Achievement: {0}", language), &[&title])
        };
    }
    if let Some(title) = value.strip_prefix("Quest complete: ") {
        let title = text(title, language);
        return if language == "es" {
            format!("Misión completada: {title}")
        } else {
            substitute(&text("Quest complete: {0}", language), &[&title])
        };
    }
    if let Some((description, xp)) = value.rsplit_once(" (+") {
        return format!("{} (+{xp}", text(description, language));
    }
    // Rank updates combine independently generated sentences, each with its own names.
    if let Some((placing, gap)) = value.rsplit_once(". ")
        && gap.ends_with('.')
        && gap.contains(" XP behind ")
    {
        return format!(
            "{}. {}",
            text(&format!("{placing}."), language).trim_end_matches('.'),
            text(gap, language)
        );
    }
    text(value, language)
}

pub fn community(value: &mut serde_json::Value, language: &str) {
    fn titles(items: &mut serde_json::Value, language: &str) {
        if let Some(items) = items.as_array_mut() {
            for item in items {
                if let Some(title) = item["title"].as_str() {
                    item["title"] = text(title, language).into();
                }
            }
        }
    }
    titles(&mut value["awards"], language);
    if let Some(players) = value["players"].as_array_mut() {
        for player in players {
            titles(&mut player["trophies"], language);
        }
    }
    if let Some(note) = value["meta"]["scoring_note"].as_str() {
        value["meta"]["scoring_note"] = text(note, language).into();
    }
}

pub fn feedback(feedback: &mut crate::feedback::Feedback, language: &str) {
    let language = normalize(language);
    for headline in &mut feedback.headlines {
        if language != "en"
            && let Some((deck, people)) = headline.rsplit_once(" — told ")
        {
            let count = people.split_whitespace().next().unwrap_or("");
            *headline = if language == "es" {
                format!(
                    "{deck} — se avisó a {count} {}",
                    if count == "1" { "amigo" } else { "amigos" }
                )
            } else {
                let key = if count == "1" {
                    "{0} — told {1} friend"
                } else {
                    "{0} — told {1} friends"
                };
                substitute(&text(key, language), &[deck, count])
            };
        } else {
            *headline = system_text(headline, language);
        }
    }
    if let Some(status) = &mut feedback.status {
        let prefix = match language {
            "es" | "pt" => "Nv. ",
            "fr" => "Niv. ",
            "de" => "Stufe ",
            _ => "Lv ",
        };
        if prefix != "Lv " {
            *status = status.replacen("Lv ", prefix, 1);
        }
    }
}

pub fn profile(profile: &mut crate::game::Profile, language: &str) {
    for quest in &mut profile.quests {
        quest.title = text(&quest.title, language);
    }
    for achievement in &mut profile.achievements {
        achievement.title = text(&achievement.title, language);
        achievement.description = text(&achievement.description, language);
    }
    if let Some(warning) = &mut profile.streak_warning {
        notice(warning, language);
    }
}

pub fn notification(notice: &mut crate::decks::Notification, language: &str, sender_display: &str) {
    let language = normalize(language);
    if matches!(notice.kind.as_str(), "message" | "reply") || language == "en" {
        return;
    }
    if notice.kind == "completion" {
        // Remove the exact sender prefix rather than parsing arbitrary names as prose.
        if let Some(deck) = notice
            .body
            .strip_prefix(&format!("{sender_display} has finished their "))
            .and_then(|body| body.strip_suffix(" studies for today."))
        {
            notice.body = if language == "es" {
                format!("{sender_display} ha terminado su estudio de {deck} por hoy.")
            } else {
                substitute(
                    &text("{0} has finished their {1} studies for today.", language),
                    &[sender_display, deck],
                )
            };
        } else {
            notice.body = text(&notice.body, language);
        }
    } else {
        notice.body = if matches!(notice.kind.as_str(), "event" | "celebration")
            && notice.title.ends_with(" new unlocks")
        {
            notice
                .body
                .split(", ")
                .map(|title| text(title, language))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            system_text(&notice.body, language)
        };
    }
    notice.title = system_text(&notice.title, language);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn celebrations_speak_spanish() {
        let mut notice = crate::decks::Notification {
            id: 1,
            title: "New best 24 hours!".into(),
            body: "1200 XP in 24 hours, beating your old best of 900 XP.".into(),
            day: 0,
            created_at: 0,
            sender: String::new(),
            replied: false,
            kind: "celebration".into(),
            read_at: None,
            challenge_id: None,
            route: None,
            action_required: false,
            reply_to: None,
        };
        notification(&mut notice, "es", "");
        assert_eq!(notice.title, "¡Nuevo récord en 24 horas!");
        assert_eq!(
            notice.body,
            "1200 XP en 24 horas: has superado tu récord anterior de 900 XP."
        );
        assert_eq!(text("50-day streak!", "es"), "¡Racha de 50 días!");
    }

    #[test]
    fn additional_anki_languages_translate_authored_text() {
        assert_eq!(normalize("fr-FR"), "fr");
        assert_eq!(normalize("de-DE"), "de");
        assert_eq!(normalize("pt-BR"), "pt");
        assert_eq!(text("Friends", "fr"), "Amis");
        assert_eq!(text("Friends", "de"), "Freunde");
        assert_eq!(text("Friends", "pt"), "Amigos");
    }

    #[test]
    fn completion_notifications_keep_names_and_deck_titles_verbatim() {
        for (language, title, body) in [
            (
                "fr",
                "Paquet terminé",
                "Aldana a terminé ses révisions du paquet Spanish::{0} pour aujourd’hui.",
            ),
            (
                "de",
                "Stapel abgeschlossen",
                "Aldana hat die Wiederholungen für Spanish::{0} heute abgeschlossen.",
            ),
            (
                "pt",
                "Baralho concluído",
                "Aldana terminou as revisões de Spanish::{0} por hoje.",
            ),
        ] {
            let mut notice = crate::decks::Notification {
                id: 1,
                title: "Deck complete".into(),
                body: "Aldana has finished their Spanish::{0} studies for today.".into(),
                day: 0,
                created_at: 0,
                sender: "Aldana".into(),
                replied: false,
                kind: "completion".into(),
                read_at: None,
                challenge_id: None,
                route: None,
                action_required: false,
                reply_to: None,
            };
            notification(&mut notice, language, "Aldana");
            assert_eq!(notice.title, title);
            assert_eq!(notice.body, body);
        }
    }

    #[test]
    fn language_tags_templates_and_fallback() {
        assert_eq!(normalize("es_MX"), "es");
        assert_eq!(normalize("ES-es"), "es");
        assert_eq!(normalize("fr"), "fr");
        assert_eq!(text("Review 15 cards", "es"), "Repasa 15 tarjetas");
        assert_eq!(
            text("Study for 10 minutes", "es-ES"),
            "Estudia durante 10 minutos"
        );
        assert_eq!(text("Review 15 cards", "de"), "Wiederhole 15 Karten");
        assert_eq!(
            text("Unknown future wording", "es"),
            "Unknown future wording"
        );
        assert_eq!(
            substitute("{1} / {0}", &["{1}", "Spanish"]),
            "Spanish / {1}"
        );
    }
    #[test]
    fn specific_achievement_templates_translate_the_metric_not_just_the_card_count() {
        assert_eq!(
            text("Review 100 mature cards", "es"),
            "Repasa 100 tarjetas maduras"
        );
        assert_eq!(
            text("Review 1000 different cards", "es"),
            "Repasa 1000 tarjetas distintas"
        );
    }
    #[test]
    fn saved_language_is_per_owner_and_persists() {
        let (store, path) = crate::decks::tests::temporary_store();
        assert_eq!(store.language("hill").unwrap(), "en");
        store.set_language("hill", "es-MX").unwrap();
        assert_eq!(store.language("friend").unwrap(), "en");
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.language("hill").unwrap(), "es");
    }
}
