"""Activity -> account queue -> one announcement outside review and quiet hours."""
from enum import Enum, auto

from . import activity, day_verdict, inbox, notify
from .language import tr

_KEY = "ankiquestAnnouncements"
_REMINDER_AGE = 3600
_COPY_KIND = "deck_copy"


class _Mode(Enum):
    QUIET = auto()
    ANNOUNCE = auto()


def _record(profile, api):
    records = profile.setdefault(_KEY, [])
    account = [api.base, api.user]
    for record in records:
        if record["account"] == account:
            return record
    # Only the first account inherits the pre-upgrade profile cursor.
    cursor = profile.get("ankiquestInboxCursor", 0) if not records else 0
    record = {"account": account, "cursor": cursor, "pending": [],
              "copies": list(profile.get("ankiquestSeenDeckCopies") or []) if not records else [],
              "warned": profile.get("ankiquestStreakDay") if not records else None}
    records.append(record)
    return record


def _fetch(api, cursor):
    page = activity._load(api)
    rows, before = list(page.items), page.before
    while before is not None and before > cursor:
        older = activity._load(api, before=before)
        if older.before is not None and older.before >= before:
            raise ValueError("Activity pagination did not advance")
        rows.extend(older.items)
        before = older.before
    return page, rows


def _quiet(settings, now_s, offset):
    start, end = settings.get("quiet_start", 0), settings.get("quiet_end", 0)
    hour = int((now_s - offset * 60) // 3600) % 24
    if start < end:
        return start <= hour < end
    return (hour >= start or hour < end) if start > end else False


def _eligible(entry, now_s, profile, reminders):
    if entry.get("quiet") or entry.get("read_at") or inbox.shareable(entry):
        return False
    kind = entry.get("kind", "")
    if not kind.startswith("reminder_"):
        return True
    # Historical reminders remain in the inbox; only current reminders can alert.
    keys = {"daily": "gentle_daily", "urgent": "urgent_streak"}
    key = keys.get(kind[9:], kind[9:])
    if not reminders.get(key) or not 0 <= now_s - entry.get("created_at", 0) <= _REMINDER_AGE:
        return False
    if kind in ("reminder_daily", "reminder_urgent", "reminder_milestone", "reminder_freeze_refill"):
        ends = (profile.get("day_ends_at") or 0) / 1000
        if not ends - notify.FRESH_SECONDS <= entry.get("created_at", 0) <= now_s < ends:
            return False
    if kind == "reminder_daily":
        return profile.get("_day_status") in (day_verdict._PENDING, day_verdict._PROTECTED)
    if kind == "reminder_urgent":
        return bool(profile.get("streak_warning"))
    return True


class _Delivery:
    def __init__(self, addon):
        self._addon = addon
        self._owner = self._profile = self._tray = None
        self._destination = None

    def _clear(self):
        if self._tray is not None:
            self._tray.hide()
            self._tray.deleteLater()
        self._owner = self._profile = self._tray = None
        self._destination = None

    def _current(self):
        api = self._addon.client()
        profile = self._addon.mw.pm.profile
        owner = (api.base, api.user, api.token)
        if self._owner != owner or self._profile is not profile:
            self._clear()
            self._owner, self._profile = owner, profile
        if not api.configured or profile is None:
            return None
        return _record(profile, api)

    def _receive(self, rows, mode):
        record = self._current()
        if record is None:
            return
        fresh, cursor = notify.fresh_messages(rows, record["cursor"], int(self._addon.time.time()))
        waiting = {row["id"]: row for row in record["pending"]}
        waiting.update({row["id"]: dict(row) for row in fresh})
        # Read receipts on another device also cancel a waiting announcement.
        for row in rows:
            if row.get("read_at"):
                waiting.pop(row["id"], None)
        record.update(cursor=cursor, pending=list(waiting.values()))
        if mode is _Mode.ANNOUNCE:
            self._flush()

    def _flush(self):
        record = self._current()
        addon = self._addon
        if record is None or addon.mw.state not in ("deckBrowser", "overview", "congrats"):
            return
        now_ms = int(addon.time.time() * 1000)
        reminders = record.get("reminders") or {}
        if _quiet(reminders, now_ms // 1000, addon.offset_west_min()):
            return
        profile = addon.home._profile_for_display(addon.state["profile"], now_ms) or {}
        pending = [row for row in record["pending"] if _eligible(row, now_ms // 1000, profile, reminders)]
        record["pending"] = pending
        if pending:
            # One alert at a time; deck offers have their own destination.
            kind = pending[0].get("kind")
            pending = [row for row in pending if (row.get("kind") == _COPY_KIND) == (kind == _COPY_KIND)]
            message = pending[0] if len(pending) == 1 else {
                "title": (tr("%d new deck offers") if kind == _COPY_KIND else tr("%d new messages")) % len(pending),
                "body": tr("Tools → AnkiQuest → Decks offered to me") if kind == _COPY_KIND else tr("Open Tools → AnkiQuest → Inbox to catch up."),
                "kind": kind,
            }
            if self._show(message):
                delivered = {row["id"] for row in pending}
                record["pending"] = [row for row in record["pending"] if row["id"] not in delivered]
                if any(row.get("kind") == "reminder_urgent" for row in pending):
                    record["warned"] = profile.get("day_ends_at")
            return
        warning = notify.streak_warning(profile, int(addon.config().get("streak_hours", 2) or 0), now_ms, record.get("warned"))
        if warning and self._show(profile["streak_warning"]):
            record["warned"] = warning[1]
            self._profile[addon.STREAK_DAY_KEY] = warning[1]

    def _show(self, entry):
        addon, mw = self._addon, self._addon.mw
        foreground = getattr(mw, "isActiveWindow", lambda: True)() and not getattr(mw, "isMinimized", lambda: False)()
        if foreground:
            text = notify.notice(entry)
            if entry.get("kind") == _COPY_KIND:
                text += "<br><i>%s</i>" % notify.rich(tr("Tools → AnkiQuest → Decks offered to me"))
            if inbox.answerable(entry):
                text += "<br><i>%s</i>" % notify.rich(tr("Tools → AnkiQuest → Inbox to reply"))
            addon.tooltip(text, period=6000)
            return True
        if not addon.config().get("system_notifications", False):
            return False
        from aqt.qt import QIcon, QSystemTrayIcon
        from .share import face_path

        if not QSystemTrayIcon.isSystemTrayAvailable() or not QSystemTrayIcon.supportsMessages():
            return False
        self._destination = addon.copies_ui.open_offers if entry.get("kind") == _COPY_KIND else addon.open_inbox
        if self._tray is None:
            self._tray = QSystemTrayIcon(QIcon(face_path(addon.state["companion"])), mw)
            owner, profile = self._owner, self._profile

            def clicked():
                api = addon.client()
                if mw.pm.profile is profile and (api.base, api.user, api.token) == owner and self._destination is not None:
                    self._destination()

            self._tray.messageClicked.connect(clicked)
            self._tray.setToolTip("AnkiQuest")
            self._tray.show()
        self._tray.showMessage(str(entry.get("title", "")), str(entry.get("body", "")), QSystemTrayIcon.MessageIcon.Information, 6000)
        return True

    def _acknowledge(self, ids):
        record = self._current()
        if record is not None:
            record["pending"] = [row for row in record["pending"] if row["id"] not in ids]

    def _receive_copies(self, offers, names, mode):
        record = self._current()
        if record is None:
            return
        known = set(record.get("copies", []))
        offered = {offer["id"] for offer in offers}
        record["pending"] = [row for row in record["pending"] if row.get("kind") != _COPY_KIND or -row["id"] in offered]
        for offer in offers:
            if offer["id"] in known:
                continue
            record["pending"].append({
                "id": -offer["id"], "kind": _COPY_KIND,
                "title": tr("%s shared %s") % (names.get(offer.get("sender"), offer.get("sender", "")), offer.get("deck", "")),
            })
        record["copies"] = sorted(known | offered)
        if mode is _Mode.ANNOUNCE:
            self._flush()

    def _ack_copies(self):
        record = self._current()
        if record is not None:
            record["pending"] = [row for row in record["pending"] if row.get("kind") != _COPY_KIND]
