"""Account-scoped moments. Profile records survive restarts and verified address moves."""

from . import inbox, moods, share

_KEY = "ankiquestMoments"
_PRIORITY = {
    "secured": 110, "streak": 100, "level": 90, "rank": 80, "achievement": 70,
    "record": 60, "leech": 55, "conquered": 50, "goal": 40, "quest": 30, "session": 0,
}
_POSES = {
    "secured": moods.STREAK, "streak": moods.STREAK, "level": moods.WINNER,
    "rank": moods.WINNER, "achievement": moods.WINNER, "quest": moods.WINNER,
    "record": moods.CELEBRATE, "conquered": moods.CELEBRATE, "goal": moods.CELEBRATE,
}


def _record(profile, account):
    records = profile.setdefault(_KEY, [])
    for record in records:
        if record["account"] == list(account):
            return record
    record = {"account": list(account), "pending": [], "seen": [], "quiet": []}
    records.append(record)
    return record


def _offer(profile, account, items):
    """Keep every new moment; a later server row can add sharing details to a local one."""
    record = _record(profile, account)
    seen = set(record["seen"])
    if any(item["topic"] == "session" for item in items):
        record["pending"] = [item for item in record["pending"] if item["topic"] != "session"]
    waiting = {item["key"]: item for item in record["pending"]}
    for item in items:
        key = item["key"]
        if key in seen:
            continue
        if key in waiting:
            if item.get("share"):
                waiting[key]["share"] = item["share"]
            continue
        # The daily streak cue is an explicit exception to quiet rows, shared with Android.
        if item.get("quiet") and item["topic"] != "secured":
            record["quiet"].append(key)
            record["seen"].append(key)
            seen.add(key)
            continue
        saved = dict(item)
        record["pending"].append(saved)
        waiting[key] = saved


def _pending(profile, account):
    items = _record(profile, account)["pending"]
    return sorted(items, key=lambda item: (-_PRIORITY.get(item["topic"], 30), -item["at"], item["key"]))


def _acknowledge(profile, account, keys):
    """Dismissal, not a pause or an account switch, consumes the displayed moments."""
    record = _record(profile, account)
    keys = set(keys)
    record["pending"] = [item for item in record["pending"] if item["key"] not in keys]
    record["seen"].extend(sorted(keys - set(record["seen"])))


def _quiet_count(profile, account):
    return len(_record(profile, account)["quiet"])


def _clear_quiet(profile, account):
    _record(profile, account)["quiet"] = []


def _notice(entry):
    # Cheers and friends' shares may carry our key too; only our own win belongs here.
    if not inbox.shareable(entry):
        return None
    return {
        "key": entry.get("moment") or "notice:%d" % entry["id"],
        "topic": entry.get("topic") or "unlocks",
        "title": entry.get("title") or "", "body": entry.get("body") or "",
        "at": int(entry.get("created_at") or 0) * 1000,
        "quiet": bool(entry.get("quiet")), "share": share.win_from_entry(entry),
    }


def _rank(change, place, week, at):
    return {
        "key": "rank:%s:%d" % (week or 0, place), "topic": "rank", "at": at,
        "title": change.get("title") or "", "body": change.get("body") or "",
        "share": share.win_from_place(change, place) if place <= 3 else None,
    }


def _pose(item, companion):
    return moods.art(companion, _POSES.get(item["topic"], moods.CELEBRATE))


def _conquest(item, translate):
    return {
        "key": "conquered:%d" % item["at"], "topic": "conquered", "at": item["at"],
        "title": translate("Card conquered!"),
        "body": translate("You learned a card that used to be difficult."),
    }
