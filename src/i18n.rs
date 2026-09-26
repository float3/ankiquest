//! Device-selected language, with English fallback and a shared Spanish catalog.
use crate::{
    App, authorized,
    store::{Error, Store},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists languages (user text primary key, language text not null);",
    )?;
    Ok(())
}

pub fn normalize(language: &str) -> &'static str {
    match language
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "es" => "es",
        _ => "en",
    }
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

fn catalog() -> &'static BTreeMap<String, String> {
    static CATALOG: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../static/translations-es.json"))
            .expect("valid translation catalog")
    })
}

fn templates() -> &'static Vec<(&'static String, &'static String)> {
    static TEMPLATES: OnceLock<Vec<(&String, &String)>> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        let mut templates: Vec<_> = catalog()
            .iter()
            .filter(|(source, _)| source.contains("{0}"))
            .collect();
        // A broad "Review {0} cards" must not capture "100 mature" as its count.
        templates.sort_by_key(|(source, _)| {
            std::cmp::Reverse(
                source
                    .split('{')
                    .enumerate()
                    .map(|(index, part)| {
                        if index == 0 {
                            part.len()
                        } else {
                            part.split_once('}').map_or(0, |(_, suffix)| suffix.len())
                        }
                    })
                    .sum::<usize>(),
            )
        });
        templates
    })
}

// Templates are matched only against system-authored text. Substitutions stay verbatim.
pub fn text(value: &str, language: &str) -> String {
    if normalize(language) != "es" {
        return value.into();
    }
    if let Some(exact) = catalog().get(value) {
        return exact.clone();
    }
    for (source, target) in templates() {
        if let Some(values) = capture(source, value) {
            // Replace in one pass, so a name containing a placeholder is never interpreted.
            return substitute(target, &values);
        }
    }
    value.into()
}

fn capture<'a>(pattern: &str, mut value: &'a str) -> Option<Vec<&'a str>> {
    let mut parts = pattern.split('{');
    value = value.strip_prefix(parts.next()?)?;
    let mut values = Vec::new();
    for part in parts {
        let (index, suffix) = part.split_once('}')?;
        if index.parse::<usize>().ok()? != values.len() {
            return None;
        }
        let end = if suffix.is_empty() {
            value.len()
        } else {
            value.find(suffix)?
        };
        values.push(&value[..end]);
        value = &value[end + suffix.len()..];
    }
    value.is_empty().then_some(values)
}

fn substitute(pattern: &str, values: &[&str]) -> String {
    let mut parts = pattern.split('{');
    let mut result = parts.next().unwrap_or_default().to_string();
    for part in parts {
        if let Some((index, suffix)) = part.split_once('}')
            && let Some(value) = index
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get(index))
        {
            result.push_str(value);
            result.push_str(suffix);
            continue;
        }
        result.push('{');
        result.push_str(part);
    }
    result
}

pub fn notice(notice: &mut crate::feedback::Notice, language: &str) {
    notice.title = system_text(&notice.title, language);
    notice.body = system_text(&notice.body, language);
}

fn system_text(value: &str, language: &str) -> String {
    if normalize(language) != "es" {
        return value.into();
    }
    if let Some(title) = value.strip_prefix("Achievement: ") {
        return format!("Logro: {}", text(title, language));
    }
    if let Some(title) = value.strip_prefix("Quest complete: ") {
        return format!("Misión completada: {}", text(title, language));
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
    for headline in &mut feedback.headlines {
        if normalize(language) == "es"
            && let Some((deck, people)) = headline.rsplit_once(" — told ")
        {
            let count = people.split_whitespace().next().unwrap_or("");
            *headline = format!(
                "{deck} — se avisó a {count} {}",
                if count == "1" { "amigo" } else { "amigos" }
            );
        } else {
            *headline = system_text(headline, language);
        }
    }
    if normalize(language) == "es"
        && let Some(status) = &mut feedback.status
    {
        *status = status.replace("Lv ", "Nv. ");
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
    if matches!(notice.kind.as_str(), "message" | "reply") || normalize(language) != "es" {
        return;
    }
    if notice.kind == "completion" {
        // Remove the exact sender prefix rather than parsing arbitrary names as prose.
        if let Some(deck) = notice
            .body
            .strip_prefix(&format!("{sender_display} has finished their "))
            .and_then(|body| body.strip_suffix(" studies for today."))
        {
            notice.body = format!("{sender_display} ha terminado su estudio de {deck} por hoy.");
        } else {
            notice.body = text(&notice.body, language);
        }
    } else {
        notice.body = if notice.kind == "event" && notice.title.ends_with(" new unlocks") {
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
    fn language_tags_templates_and_fallback() {
        assert_eq!(normalize("es_MX"), "es");
        assert_eq!(normalize("ES-es"), "es");
        assert_eq!(normalize("fr"), "en");
        assert_eq!(text("Review 15 cards", "es"), "Repasa 15 tarjetas");
        assert_eq!(
            text("Study for 10 minutes", "es-ES"),
            "Estudia durante 10 minutos"
        );
        assert_eq!(text("Review 15 cards", "de"), "Review 15 cards");
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
