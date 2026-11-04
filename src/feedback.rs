//! Everything the clients say about the game, worded once here so the phone, the desktop
//! add-on and the website cannot drift apart.
//!
//! Each text is kept as a catalog key and the values for its placeholders, and written in
//! the reader's language only when it is sent, so nothing has to read English back.

use crate::decks::Announcement;
use crate::game::{Conquest, HOUR_MS, Profile};
use crate::i18n;
use serde::{Serialize, Serializer};
use std::collections::BTreeSet;
use std::fmt;

/// A text for a player, sent as a plain string in `language`, English unless localized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    text: Text,
    language: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Text {
    /// A catalog key and its placeholder values.
    Said(&'static str, Vec<Arg>),
    /// Texts shown side by side.
    Joined(&'static str, Vec<Text>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Arg {
    /// Numbers, names and deck titles, shown as they are.
    Value(String),
    /// A title the server wrote, such as an achievement's, translated on its own.
    Title(String),
}

fn say(key: &'static str) -> Text {
    Text::Said(key, Vec::new())
}

impl Text {
    fn value(mut self, value: impl ToString) -> Self {
        if let Text::Said(_, args) = &mut self {
            args.push(Arg::Value(value.to_string()));
        }
        self
    }

    fn title(mut self, title: &str) -> Self {
        if let Text::Said(_, args) = &mut self {
            args.push(Arg::Title(title.into()));
        }
        self
    }

    fn render(&self, language: &str) -> String {
        match self {
            Text::Said(key, args) => {
                let args: Vec<String> = args
                    .iter()
                    .map(|arg| match arg {
                        Arg::Value(value) => value.clone(),
                        Arg::Title(title) => i18n::text(title, language),
                    })
                    .collect();
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                i18n::substitute(i18n::template(key, language), &args)
            }
            Text::Joined(separator, parts) => parts
                .iter()
                .map(|part| part.render(language))
                .collect::<Vec<_>>()
                .join(separator),
        }
    }
}

impl From<Text> for Message {
    fn from(text: Text) -> Self {
        Message {
            text,
            language: "en",
        }
    }
}

impl Message {
    pub fn localize(&mut self, language: &str) {
        self.language = i18n::normalize(language);
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text.render(self.language))
    }
}

impl PartialEq<&str> for Message {
    fn eq(&self, other: &&str) -> bool {
        self.text.render(self.language) == *other
    }
}

impl Serialize for Message {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: Message,
    pub body: Message,
}

impl Notice {
    pub fn localize(&mut self, language: &str) {
        self.title.localize(language);
        self.body.localize(language);
    }
}

/// What an upload changed: headlines worth a longer banner, and the XP status line.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Feedback {
    pub headlines: Vec<Message>,
    pub status: Option<Message>,
}

impl Feedback {
    pub fn localize(&mut self, language: &str) {
        for headline in &mut self.headlines {
            headline.localize(language);
        }
        if let Some(status) = &mut self.status {
            status.localize(language);
        }
    }
}

fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `minutes` is today's review minutes and the minimum, when the server sets one.
pub fn streak_warning(
    streak: u64,
    freezes: u32,
    day_ends_at: i64,
    now_ms: i64,
    minutes: Option<(u64, u64)>,
) -> Notice {
    let hours = ((day_ends_at - now_ms).max(1) + HOUR_MS - 1) / HOUR_MS;
    let title = match minutes {
        Some((done, needed)) => {
            say("🔥 Your {0} day streak ends in {1}h · {2} of {3} minutes today")
                .value(streak)
                .value(hours)
                .value(done.min(needed))
                .value(needed)
        }
        None => say("🔥 Your {0} day streak ends in {1}h")
            .value(streak)
            .value(hours),
    };
    let body = if freezes > 0 {
        say("A freeze is available if you need a break.")
    } else {
        say("A little study is waiting when you have time. Your previous progress is still yours.")
    };
    Notice {
        title: title.into(),
        body: body.into(),
    }
}

fn announcement(item: &Announcement) -> Message {
    let told = if item.recipients == 1 {
        say("📣 {0} — told {1} friend")
    } else {
        say("📣 {0} — told {1} friends")
    };
    told.value(&item.deck).value(item.recipients).into()
}

fn conquest_headline(conquest: &Conquest) -> Message {
    let headline = if conquest.leech {
        say("🏆 Leech tamed after {0} lapses")
    } else {
        say("🏆 Card conquered after {0} lapses")
    };
    headline.value(conquest.lapses).into()
}

fn level_status(profile: &Profile) -> Text {
    say("Lv {0}  {1}/{2}")
        .value(profile.level)
        .value(profile.xp_into_level)
        .value(profile.xp_for_next)
}

fn done_quests(profile: &Profile) -> BTreeSet<&str> {
    profile
        .quests
        .iter()
        .filter(|quest| quest.done)
        .map(|quest| quest.title.as_str())
        .collect()
}

fn unlocked(profile: &Profile) -> BTreeSet<&str> {
    profile
        .achievements
        .iter()
        .filter(|achievement| achievement.unlocked.is_some())
        .map(|achievement| achievement.title.as_str())
        .collect()
}

const STATUS_SEPARATOR: &str = "  ·  ";

/// Compares the profile before and after an upload or preview. Without a baseline only
/// the deck announcements are reported. The profiles' quest and achievement titles must
/// still be the English ones.
pub fn feedback(before: Option<&Profile>, after: &Profile, announced: &[Announcement]) -> Feedback {
    let mut headlines: Vec<Message> = announced.iter().map(announcement).collect();
    let Some(before) = before else {
        return Feedback {
            headlines,
            status: None,
        };
    };
    // Only answers this upload added: resending history with intervals finds old
    // conquests, which the inbox already keeps quiet about.
    headlines.extend(
        after
            .conquests
            .iter()
            .rev()
            .filter(|c| c.at > before.last_review_id)
            .map(conquest_headline),
    );
    let combo = after.today.current_combo;
    let status = match after.xp_total.cmp(&before.xp_total) {
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Less => Some(Text::Joined(
            STATUS_SEPARATOR,
            vec![
                say("−{0} XP").value(before.xp_total - after.xp_total),
                say("combo {0}").value(combo),
                level_status(after),
            ],
        )),
        std::cmp::Ordering::Greater => {
            if after.level > before.level {
                headlines.push(say("Level {0}!").value(after.level).into());
            }
            let had = unlocked(before);
            headlines.extend(
                unlocked(after)
                    .difference(&had)
                    .map(|title| say("Achievement: {0}").title(title).into()),
            );
            let had = done_quests(before);
            headlines.extend(
                done_quests(after)
                    .difference(&had)
                    .map(|title| say("Quest complete: {0}").title(title).into()),
            );
            if after.streak > before.streak {
                headlines.push(say("{0} day streak").value(after.streak).into());
            }
            let mut parts = vec![say("+{0} XP").value(after.xp_total - before.xp_total)];
            if combo >= 5 {
                parts.push(say("combo {0}").value(combo));
            }
            parts.push(level_status(after));
            Some(Text::Joined(STATUS_SEPARATOR, parts))
        }
    };
    Feedback {
        headlines,
        status: status.map(Message::from),
    }
}

/// One row of the weekly board, in board order.
pub struct Place<'a> {
    pub user: &'a str,
    pub display: &'a str,
    pub week_xp: u64,
}

/// How `me` moved on the weekly board since a device last saw `previous`.
pub fn rank_change(previous: &[String], board: &[Place], me: &str) -> Option<Notice> {
    let rank = board.iter().position(|place| place.user == me)?;
    let before = previous.iter().position(|user| user == me)?;
    if rank == before || board[rank].week_xp == 0 {
        return None;
    }
    let display = |user: &str| {
        board
            .iter()
            .find(|place| place.user == user)
            .map_or(user, |place| place.display)
            .to_string()
    };
    let names = |users: Vec<&str>| {
        users
            .into_iter()
            .map(&display)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let now_at = |user: &str| board.iter().position(|place| place.user == user);
    let gap = (rank > 0).then(|| {
        let ahead = &board[rank - 1];
        say("{0} XP behind {1}.")
            .value(grouped(ahead.week_xp.saturating_sub(board[rank].week_xp)))
            .value(ahead.display)
    });
    let (title, mut body) = if rank < before {
        let passed: Vec<&str> = previous[..before]
            .iter()
            .map(String::as_str)
            .filter(|user| now_at(user).is_some_and(|at| at > rank))
            .collect();
        let title = if rank == 0 {
            say("👑 You took the crown")
        } else {
            say("▲ You're now #{0}").value(rank + 1)
        };
        let passed = (!passed.is_empty()).then(|| say("You passed {0}.").value(names(passed)));
        (title, passed.into_iter().collect::<Vec<_>>())
    } else {
        let overtakers: Vec<&str> = board[..rank]
            .iter()
            .map(|place| place.user)
            .filter(|user| {
                previous
                    .iter()
                    .position(|seen| seen == user)
                    .is_some_and(|at| at > before)
            })
            .collect();
        let title = match (overtakers.is_empty(), before == 0) {
            (true, true) => say("👑 Someone took the crown"),
            (true, false) => say("▼ Someone passed you"),
            (false, true) => say("👑 {0} took the crown").value(names(overtakers)),
            (false, false) => say("▼ {0} passed you").value(names(overtakers)),
        };
        (title, vec![say("You're now #{0}.").value(rank + 1)])
    };
    body.extend(gap);
    Some(Notice {
        title: title.into(),
        body: Text::Joined(" ", body).into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place<'a>(user: &'a str, week_xp: u64) -> Place<'a> {
        Place {
            user,
            display: user,
            week_xp,
        }
    }

    fn order(users: &[&str]) -> Vec<String> {
        users.iter().map(|user| user.to_string()).collect()
    }

    #[test]
    fn taking_the_crown_names_who_was_passed() {
        let board = [place("me", 1500), place("ana", 1200), place("bo", 10)];
        let notice = rank_change(&order(&["ana", "me", "bo"]), &board, "me").unwrap();
        assert_eq!(notice.title, "👑 You took the crown");
        assert_eq!(notice.body, "You passed ana.");
    }

    #[test]
    fn climbing_reports_the_gap_to_the_next_player() {
        let board = [place("ana", 3000), place("me", 1500), place("bo", 10)];
        let notice = rank_change(&order(&["ana", "bo", "me"]), &board, "me").unwrap();
        assert_eq!(notice.title, "▲ You're now #2");
        assert_eq!(notice.body, "You passed bo. 1,500 XP behind ana.");
    }

    #[test]
    fn falling_names_the_overtakers() {
        let board = [place("ana", 3000), place("me", 1500)];
        let notice = rank_change(&order(&["me", "ana"]), &board, "me").unwrap();
        assert_eq!(notice.title, "👑 ana took the crown");
        assert_eq!(notice.body, "You're now #2. 1,500 XP behind ana.");
        let board = [place("ana", 30), place("bo", 20), place("me", 10)];
        let notice = rank_change(&order(&["ana", "me", "bo"]), &board, "me").unwrap();
        assert_eq!(notice.title, "▼ bo passed you");
        assert_eq!(notice.body, "You're now #3. 10 XP behind bo.");
    }

    #[test]
    fn no_change_unknown_history_or_no_xp_stays_quiet() {
        let board = [place("ana", 30), place("me", 10)];
        assert!(rank_change(&order(&["ana", "me"]), &board, "me").is_none());
        assert!(rank_change(&[], &board, "me").is_none());
        assert!(rank_change(&order(&["ana"]), &board, "me").is_none());
        let board = [place("ana", 30), place("me", 0)];
        assert!(rank_change(&order(&["me", "ana"]), &board, "me").is_none());
    }

    #[test]
    fn streak_warning_rounds_hours_up_and_mentions_freezes() {
        let warning = streak_warning(12, 1, 10 * HOUR_MS, 7 * HOUR_MS + 1, None);
        assert_eq!(warning.title, "🔥 Your 12 day streak ends in 3h");
        assert_eq!(warning.body, "A freeze is available if you need a break.");
        let warning = streak_warning(3, 0, HOUR_MS, HOUR_MS, None);
        assert_eq!(warning.title, "🔥 Your 3 day streak ends in 1h");
        assert!(warning.body.to_string().starts_with("A little study"));
    }

    #[test]
    fn streak_warning_counts_minutes_toward_a_minimum() {
        let warning = streak_warning(12, 0, 10 * HOUR_MS, 7 * HOUR_MS, Some((6, 10)));
        assert_eq!(
            warning.title,
            "🔥 Your 12 day streak ends in 3h · 6 of 10 minutes today"
        );
        let warning = streak_warning(12, 0, 10 * HOUR_MS, 7 * HOUR_MS, Some((14, 10)));
        assert!(
            warning
                .title
                .to_string()
                .ends_with("10 of 10 minutes today")
        );
    }

    #[test]
    fn names_that_look_like_feedback_stay_verbatim_in_every_language() {
        let deck = "Lv 3 — told 2 friends (+5";
        let mut told = announcement(&Announcement {
            deck: deck.into(),
            recipients: 1,
        });
        assert_eq!(told, "📣 Lv 3 — told 2 friends (+5 — told 1 friend");
        told.localize("es");
        assert_eq!(told, "📣 Lv 3 — told 2 friends (+5 — se avisó a 1 amigo");
        told.localize("de-AT");
        assert_eq!(
            told,
            "📣 Lv 3 — told 2 friends (+5 — 1 Freund benachrichtigt"
        );

        let tricky = "Bo. 5 XP behind Ana.";
        let board = [
            Place {
                user: "ana",
                display: "Ana",
                week_xp: 3000,
            },
            Place {
                user: "me",
                display: "Me",
                week_xp: 1500,
            },
            Place {
                user: "bo",
                display: tricky,
                week_xp: 10,
            },
        ];
        let mut notice = rank_change(&order(&["ana", "bo", "me"]), &board, "me").unwrap();
        notice.localize("de");
        assert_eq!(notice.title, "▲ Du bist jetzt auf Platz 2");
        assert_eq!(
            notice.body,
            "Du hast Bo. 5 XP behind Ana. überholt. 1,500 XP hinter Ana."
        );
    }

    #[test]
    fn someone_unseen_is_named_in_the_readers_language() {
        let board = [place("new", 3000), place("me", 1500)];
        let mut notice = rank_change(&order(&["me"]), &board, "me").unwrap();
        assert_eq!(notice.title, "👑 Someone took the crown");
        notice.localize("fr");
        assert_eq!(notice.title, "👑 Quelqu’un a pris la couronne");
        assert_eq!(notice.body, "Tu es maintenant n° 2. 1,500 XP derrière new.");
    }

    #[test]
    fn messages_are_sent_as_plain_strings() {
        let mut warning = streak_warning(12, 1, 10 * HOUR_MS, 7 * HOUR_MS + 1, None);
        warning.localize("pt-BR");
        assert_eq!(
            serde_json::to_value(&warning).unwrap(),
            serde_json::json!({
                "title": "🔥 A tua sequência de 12 dias termina em 3 h",
                "body": "Tens uma proteção de sequência disponível se precisares de uma pausa.",
            })
        );
        let mut feedback = Feedback {
            headlines: vec![say("Level {0}!").value(4).into()],
            status: Some(
                Text::Joined(
                    STATUS_SEPARATOR,
                    vec![
                        say("+{0} XP").value(30),
                        say("Lv {0}  {1}/{2}").value(4).value(5).value(400),
                    ],
                )
                .into(),
            ),
        };
        assert_eq!(
            serde_json::to_value(&feedback).unwrap(),
            serde_json::json!({"headlines": ["Level 4!"], "status": "+30 XP  ·  Lv 4  5/400"})
        );
        feedback.localize("de");
        assert_eq!(
            serde_json::to_value(&feedback).unwrap(),
            serde_json::json!({"headlines": ["Level 4!"], "status": "+30 XP  ·  Stufe 4  5/400"})
        );
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1234567), "1,234,567");
    }
}
