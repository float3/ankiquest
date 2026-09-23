//! Everything the clients say about the game, worded once here so the phone, the desktop
//! add-on and the website cannot drift apart.

use crate::decks::Announcement;
use crate::game::Profile;
use serde::Serialize;
use std::collections::BTreeSet;

const HOUR_MS: i64 = 3_600_000;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// What an upload changed: headlines worth a longer banner, and the XP status line.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Feedback {
    pub headlines: Vec<String>,
    pub status: Option<String>,
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

pub fn streak_warning(streak: u64, freezes: u32, day_ends_at: i64, now_ms: i64) -> Notice {
    let hours = ((day_ends_at - now_ms).max(1) + HOUR_MS - 1) / HOUR_MS;
    Notice {
        title: format!("🔥 Your {streak} day streak ends in {hours}h"),
        body: if freezes > 0 {
            "A freeze is available if you need a break."
        } else {
            "A little study is waiting when you have time. Your previous progress is still yours."
        }
        .into(),
    }
}

fn announcement(item: &Announcement) -> String {
    let people = item.recipients;
    format!(
        "📣 {} — told {people} {}",
        item.deck,
        if people == 1 { "friend" } else { "friends" }
    )
}

fn level_status(profile: &Profile) -> String {
    format!(
        "Lv {}  {}/{}",
        profile.level, profile.xp_into_level, profile.xp_for_next
    )
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
        .map(|achievement| achievement.title)
        .collect()
}

/// Compares the profile before and after an upload or preview. Without a baseline only
/// the deck announcements are reported.
pub fn feedback(before: Option<&Profile>, after: &Profile, announced: &[Announcement]) -> Feedback {
    let mut headlines: Vec<String> = announced.iter().map(announcement).collect();
    let Some(before) = before else {
        return Feedback {
            headlines,
            status: None,
        };
    };
    let combo = after.today.current_combo;
    let status = match after.xp_total.cmp(&before.xp_total) {
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Less => Some(format!(
            "−{} XP  ·  combo {combo}  ·  {}",
            before.xp_total - after.xp_total,
            level_status(after)
        )),
        std::cmp::Ordering::Greater => {
            if after.level > before.level {
                headlines.push(format!("Level {}!", after.level));
            }
            let had = unlocked(before);
            headlines.extend(
                unlocked(after)
                    .difference(&had)
                    .map(|title| format!("Achievement: {title}")),
            );
            let had = done_quests(before);
            headlines.extend(
                done_quests(after)
                    .difference(&had)
                    .map(|title| format!("Quest complete: {title}")),
            );
            if after.streak > before.streak {
                headlines.push(format!("{} day streak", after.streak));
            }
            let mut status = format!("+{} XP", after.xp_total - before.xp_total);
            if combo >= 5 {
                status.push_str(&format!("  ·  combo {combo}"));
            }
            status.push_str(&format!("  ·  {}", level_status(after)));
            Some(status)
        }
    };
    Feedback { headlines, status }
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
    let gap = if rank > 0 {
        let ahead = &board[rank - 1];
        format!(
            " {} XP behind {}.",
            grouped(ahead.week_xp.saturating_sub(board[rank].week_xp)),
            ahead.display
        )
    } else {
        String::new()
    };
    let (title, body) = if rank < before {
        let passed: Vec<&str> = previous[..before]
            .iter()
            .map(String::as_str)
            .filter(|user| now_at(user).is_some_and(|at| at > rank))
            .collect();
        let title = if rank == 0 {
            "👑 You took the crown".to_string()
        } else {
            format!("▲ You're now #{}", rank + 1)
        };
        let passed = if passed.is_empty() {
            String::new()
        } else {
            format!("You passed {}.", names(passed))
        };
        (title, format!("{passed}{gap}"))
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
        let who = if overtakers.is_empty() {
            "Someone".to_string()
        } else {
            names(overtakers)
        };
        let title = if before == 0 {
            format!("👑 {who} took the crown")
        } else {
            format!("▼ {who} passed you")
        };
        (title, format!("You're now #{}.{gap}", rank + 1))
    };
    Some(Notice {
        title,
        body: body.trim().to_string(),
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
        let warning = streak_warning(12, 1, 10 * HOUR_MS, 7 * HOUR_MS + 1);
        assert_eq!(warning.title, "🔥 Your 12 day streak ends in 3h");
        assert_eq!(warning.body, "A freeze is available if you need a break.");
        let warning = streak_warning(3, 0, HOUR_MS, HOUR_MS);
        assert_eq!(warning.title, "🔥 Your 3 day streak ends in 1h");
        assert!(warning.body.starts_with("A little study"));
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1234567), "1,234,567");
    }
}
