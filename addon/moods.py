"""The companion's mood for today, and its artwork, chosen like the phone app's Aki."""

WELCOME, REVIEW, CELEBRATE, STREAK, FREEZE, WINNER = "welcome", "review", "celebrate", "streak", "freeze", "winner"

MESSAGES = {
    WELCOME: "Aki is here to help. One card, one small step.",
    REVIEW: "A small step starts with one card.",
    CELEBRATE: "Your cards are done for now. Nice work!",
    STREAK: "You showed up today. That is progress worth keeping.",
    FREEZE: "Your streak is protected by a freeze. A fresh start is waiting.",
    WINNER: "Every quest is done today. Nice work!",
}

COMPANION_WELCOME = "One card, one small step."

ANKILOPE = {
    WELCOME: "ankilope_welcome",
    CELEBRATE: "ankilope_celebrate",
    WINNER: "ankilope_celebrate",
    REVIEW: "ankilope_study",
    STREAK: "ankilope_study",
    FREEZE: "ankilope_study",
}


def current(profile, now_ms):
    """The profile while it still describes today; None once its study day has ended."""
    if not profile:
        return None
    ends = profile.get("day_ends_at") or 0
    return None if ends and ends <= now_ms else profile


def _reviews_today(profile):
    return int(((profile or {}).get("today") or {}).get("reviews") or 0)


def mood(decks_due, focus_due, profile, now_ms):
    """Today's mood; the collection, not the server, says whether cards are still due.

    `decks_due` lists every deck's due count, `focus_due` is the next deck's.
    """
    today = current(profile, now_ms)
    reviews = _reviews_today(today)
    state = (today or {}).get("streak_state")
    if state == "protected" and reviews == 0:
        return FREEZE
    if decks_due and all(due == 0 for due in decks_due) and reviews > 0:
        return CELEBRATE
    if focus_due > 0:
        return REVIEW
    if state == "studied" and reviews > 0:
        return STREAK
    return WELCOME


def art(companion, chosen):
    """The file name, without folder or extension, of `companion`'s art for a mood; None for no companion."""
    if companion == "none":
        return None
    if companion == "ankilope":
        return ANKILOPE.get(chosen, "ankilope_welcome")
    return "aki_" + chosen if chosen in MESSAGES else "aki_welcome"


def message(companion, chosen):
    """The mood line; the welcome names Aki, so other companions get a plainer one."""
    if chosen == WELCOME and companion != "aki":
        return COMPANION_WELCOME
    return MESSAGES.get(chosen, MESSAGES[WELCOME])
