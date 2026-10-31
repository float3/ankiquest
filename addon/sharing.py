"""Who hears about each kind of win: the server's sharing settings, as a settings dialog section."""

import html

from .language import tr

POLICIES = (
    ("private", "Only me"),
    ("ask", "Ask me"),
    ("auto", "Tell friends automatically"),
)
KINDS = (
    ("daily_goal", "Daily goal reached"),
    ("streak_secured", "Streak secured for the day"),
    ("streak", "Streak milestones"),
    ("level", "New levels"),
    ("achievement", "Achievements"),
    ("record", "Personal bests"),
    ("quest", "Daily quests done"),
    ("conquered", "Conquered cards"),
    ("place", "Top three of the week"),
    ("challenge", "Challenges completed"),
)
DEFAULT_CAP = 3
CAP_RANGE = (1, 10)


def path(api):
    return "/api/sharing/" + api.user


def load(api):
    """The settings, or None when this server does not have them."""
    try:
        answer = api._request(path(api))
    except Exception as error:
        print("ankiquest sharing:", error)
        return None
    return answer if isinstance(answer, dict) and isinstance(answer.get("policies"), dict) else None


def cap(settings):
    try:
        value = int((settings or {}).get("daily_cap"))
    except (TypeError, ValueError):
        return DEFAULT_CAP
    return max(CAP_RANGE[0], min(CAP_RANGE[1], value))


def changes(loaded, policies, daily_cap):
    """The PUT body for what the player changed, or None when nothing did."""
    before = loaded.get("policies") or {}
    known = {value for value, _ in POLICIES}
    changed = {kind: policy for kind, policy in policies.items() if policy in known and before.get(kind) != policy}
    body = {"policies": changed} if changed else {}
    if daily_cap != cap(loaded):
        body["daily_cap"] = int(daily_cap)
    return body or None


def save(api, body):
    return api._request(path(api), body, method="PUT")


def section(layout, mw, api):
    """Appends the sharing choices to the settings dialog, folded away until asked for, and
    returns what to run when it is saved. Nothing is shown for an account without a token."""
    from aqt.qt import QLabel, QPushButton

    if not api.configured:
        return None
    heading = QLabel(tr("Sharing your wins"))
    heading.setStyleSheet("font-weight:bold;margin-top:8px")
    unfold = QPushButton(tr("Choose who hears about each kind of win…"))
    unfold.setCheckable(True)
    status = QLabel(tr("Loading your sharing choices…"))
    status.setWordWrap(True)
    holder, combos, daily = _form()
    for widget in (heading, unfold, status, holder):
        layout.addWidget(widget)
    status.setVisible(False)
    holder.setVisible(False)
    loaded = {}

    def shown():
        open_now = unfold.isChecked()
        holder.setVisible(open_now and bool(loaded))
        status.setVisible(open_now and not loaded)

    def arrived(future):
        try:
            found = future.result()
            if found is None:
                status.setText(tr("This server does not support these settings yet."))
                return
            loaded.update(found)
            _fill(found, combos, daily)
            shown()
        except RuntimeError:
            return

    unfold.toggled.connect(lambda _checked: shown())
    mw.taskman.run_in_background(lambda: load(api), arrived)

    def finish():
        if not loaded:
            return
        body = changes(loaded, {kind: box.currentData() for kind, box in combos.items()}, daily.value())
        if body:
            mw.taskman.run_in_background(lambda: save(api, body), _saved)

    return finish


def _form():
    from aqt.qt import QComboBox, QGridLayout, QLabel, QSpinBox, QWidget

    holder = QWidget()
    grid = QGridLayout(holder)
    grid.setContentsMargins(0, 0, 0, 0)
    combos = {}
    for row, (kind, text) in enumerate(KINDS):
        box = QComboBox()
        for value, label in POLICIES:
            box.addItem(tr(label), value)
        combos[kind] = box
        grid.addWidget(QLabel(tr(text)), row, 0)
        grid.addWidget(box, row, 1)
    daily = QSpinBox()
    daily.setRange(*CAP_RANGE)
    grid.addWidget(QLabel(tr("Automatic shares each friend gets per day")), len(KINDS), 0)
    grid.addWidget(daily, len(KINDS), 1)
    return holder, combos, daily


def _fill(found, combos, daily):
    policies = found.get("policies") or {}
    for kind, box in combos.items():
        index = box.findData(policies.get(kind))
        box.setCurrentIndex(max(0, index))
    daily.setValue(cap(found))


def _saved(future):
    from aqt.utils import tooltip

    try:
        future.result()
    except Exception as error:
        tooltip(tr("Could not save your sharing choices (%s).") % html.escape(str(error)))
