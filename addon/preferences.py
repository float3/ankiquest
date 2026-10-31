"""Account preferences the server keeps: streak protection and the deck alerts I receive."""

import html
from datetime import datetime

from .language import tr

LAST_UPLOAD_KEY = "ankiquestLastUploadAt"


def alert_senders(data):
    """The senders to offer and those shown as unsubscribed, as the web client's `alertSenders`.
    Older mutes count as unsubscribes, and people who left stay listed so they can be cleared."""
    unsubscribed = set(data.get("unsubscribed_senders") or []) | set(data.get("muted_senders") or [])
    relevant = set(data.get("sharing_senders") or []) | unsubscribed
    people = {}
    for person in data.get("senders") or []:
        user = person.get("user")
        if user in relevant and user not in people:
            people[user] = person.get("display") or user
    for user in unsubscribed:
        people.setdefault(user, user)
    senders = sorted(people.items(), key=lambda item: (item[1].lower(), item[0]))
    return senders, unsubscribed


def subscription_body(fresh, enabled, before, after):
    """A full replacement built on a fresh read: only what the player changed is applied, so
    mutes and remote changes they never touched are kept as unsubscribes."""
    _, current = alert_senders(fresh)
    before, after = set(before), set(after)
    wanted = (current | (after - before)) - (before - after)
    return {"enabled": bool(enabled), "unsubscribed_senders": sorted(wanted)}


def freeze_body(enabled):
    return {"enabled": bool(enabled)}


def last_upload_text(at_ms, translate=tr):
    when = datetime.fromtimestamp(at_ms / 1000).strftime("%Y-%m-%d %H:%M") if at_ms else translate("never")
    return translate("Last upload: %s") % when


def subscriptions_path(api):
    return "/api/deck-subscriptions/" + api.user


def freezes_path(api):
    return "/api/streak-freezes/" + api.user


def load(api):
    """Both settings, each None when this server or token does not offer it."""
    found = {}
    for key, path in (("freezes", freezes_path(api)), ("alerts", subscriptions_path(api))):
        try:
            found[key] = api._request(path)
        except Exception as error:
            print("ankiquest %s:" % key, error)
            found[key] = None
    return found


def save(api, loaded, wanted):
    """Writes only what changed; `wanted` holds the dialog's answers."""
    if loaded.get("freezes") is not None and wanted["freezes"] != bool(loaded["freezes"].get("enabled")):
        api._request(freezes_path(api), freeze_body(wanted["freezes"]))
    alerts = loaded.get("alerts")
    if alerts is None:
        return
    before = wanted["before"]
    if wanted["alerts"] == bool(alerts.get("enabled")) and set(wanted["after"]) == set(before):
        return
    fresh = api._request(subscriptions_path(api))
    api._request(subscriptions_path(api), subscription_body(fresh, wanted["alerts"], before, wanted["after"]))


def section(dialog, layout, mw, api):
    """Appends the server-side preferences to the settings dialog and returns the function to run
    when it is saved. Nothing is shown for an account without a token."""
    from aqt.qt import QCheckBox, QLabel

    from .ui import _scroller

    layout.addWidget(QLabel(last_upload_text(mw.pm.profile.get(LAST_UPLOAD_KEY))))
    if not api.configured:
        return None
    freeze = QCheckBox(tr("Streak protection"))
    freeze_help = _note(tr("Finish all three daily quests to earn one freeze per Anki day, up to 3. One is used automatically for each ended day without reviews."))
    alerts = QCheckBox(tr("Receive deck completion notifications"))
    people_title = QLabel("<b>%s</b>" % html.escape(tr("Unsubscribe from specific people")))
    people = {}
    holder = _scroller([])
    holder.setMaximumHeight(140)
    note = _note("")
    status = _note(tr("Loading your notification choices…"))
    for widget in (freeze, freeze_help, alerts, people_title, holder, note, status):
        layout.addWidget(widget)
    for widget in (freeze, freeze_help, alerts, people_title, holder, note):
        widget.setVisible(False)
    loaded = {}

    def shown(future):
        try:
            loaded.update(future.result())
            _fill(loaded, freeze, freeze_help, alerts, people_title, holder, people, note, status, QCheckBox)
        except RuntimeError:
            return

    mw.taskman.run_in_background(lambda: load(api), shown)

    def finish():
        if not loaded:
            return
        wanted = {
            "freezes": freeze.isChecked(),
            "alerts": alerts.isChecked(),
            "before": [user for user, (box, was) in people.items() if was],
            "after": [user for user, (box, _) in people.items() if box.isChecked()],
        }
        mw.taskman.run_in_background(lambda: save(api, dict(loaded), wanted), _saved)

    return finish


def _note(text):
    from aqt.qt import QLabel

    label = QLabel(text)
    label.setWordWrap(True)
    return label


def _fill(loaded, freeze, freeze_help, alerts, people_title, holder, people, note, status, checkbox):
    status.setVisible(False)
    if loaded.get("freezes") is not None:
        freeze.setChecked(bool(loaded["freezes"].get("enabled")))
        freeze.setVisible(True)
        freeze_help.setVisible(True)
    data = loaded.get("alerts")
    if data is None:
        if loaded.get("freezes") is None:
            status.setText(tr("This server does not support these settings yet."))
            status.setVisible(True)
        return
    alerts.setChecked(bool(data.get("enabled")))
    senders, unsubscribed = alert_senders(data)
    inner = holder.widget().layout()
    for user, display in senders:
        box = checkbox(tr("Unsubscribe from %s") % (display if display == user else "%s (%s)" % (display, user)))
        box.setChecked(user in unsubscribed)
        people[user] = (box, user in unsubscribed)
        inner.insertWidget(inner.count() - 1, box)
    note.setText(_alerts_note(data, senders))
    for widget in (alerts, people_title, holder, note):
        widget.setVisible(True)
    holder.setVisible(bool(senders))


def _alerts_note(data, senders):
    if not senders:
        return tr("No one is sharing decks with you yet. You can still turn all deck completion alerts off.")
    if data.get("muted_senders"):
        return tr("People you muted before are shown as unsubscribed. Saving a change here also removes you from their sharing lists.")
    return tr("A checked name removes you from all of their deck-sharing lists. Clear the name and save to subscribe again.")


def _saved(future):
    from aqt.utils import tooltip

    try:
        future.result()
    except Exception as error:
        tooltip(tr("Could not save your notification settings (%s).") % html.escape(str(error)))
