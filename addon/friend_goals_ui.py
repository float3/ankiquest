"""Goal selection and creation use the Friends window's account and request ownership."""
from enum import Enum, auto

from aqt.qt import QCheckBox, QComboBox, QDialog, QFormLayout, QHBoxLayout, QLineEdit, QListWidget, QListWidgetItem, QPushButton, QSpinBox, QTreeWidget, QTreeWidgetItem, QVBoxLayout, QWidget, Qt

from . import share_ui, social_data
from .language import tr
from .social_data import _GoalAction, _GoalState, _Kind, _WeeklyAction
from .social_ui import _Mode

_STATES = {
    _GoalState.ACTIVE: "In progress", _GoalState.UPCOMING: "Upcoming", _GoalState.WAITING: "Waiting for everyone",
    _GoalState.COMPLETE: "Goal reached", _GoalState.ENDED: "Ended", _GoalState.CANCELLED: "Ended early",
    _GoalState.DECLINED: "Invitation declined", _GoalState.UNKNOWN: "Unknown goal state",
}
_ACTIONS = {_GoalAction.ACCEPT: "Accept invitation", _GoalAction.DECLINE: "Decline",
            _GoalAction.LEAVE: "Leave goal", _GoalAction.CANCEL: "End goal for everyone"}


class _Together(Enum):
    INDIVIDUAL = auto()
    SHARED = auto()


class _Phase(Enum):
    OPEN = auto()
    CLOSED = auto()


class _Goals:
    def __init__(self, window, goal):
        self._window, self._wanted, self._data, self._creator = window, goal, {}, None
        self._page = QWidget()
        root = QVBoxLayout(self._page)
        self._status = share_ui._plain("")
        root.addWidget(self._status)
        self._weekly_text = share_ui._plain("")
        root.addWidget(self._weekly_text)
        weekly = QHBoxLayout()
        self._weekly_buttons = []
        for action, title in ((_WeeklyAction.INVITE, tr("Invite friend")), (_WeeklyAction.DISMISS, tr("Not this week"))):
            button = QPushButton(title)
            button.clicked.connect(lambda _checked=False, action=action: self._weekly(action))
            button.hide()
            weekly.addWidget(button)
            self._weekly_buttons.append(button)
        weekly.addStretch(1)
        root.addLayout(weekly)
        self._table = QTreeWidget()
        self._table.setHeaderLabels([tr("Friend goal"), tr("State"), tr("Progress")])
        self._table.setRootIsDecorated(False)
        self._table.itemSelectionChanged.connect(self._selected)
        root.addWidget(self._table, 1)
        self._details = share_ui._plain("")
        root.addWidget(self._details)
        actions = QHBoxLayout()
        self._buttons = []
        for index in range(2):
            button = QPushButton()
            button.hide()
            button.clicked.connect(lambda _checked=False, index=index: self._act(index))
            actions.addWidget(button)
            self._buttons.append(button)
        actions.addStretch(1)
        root.addLayout(actions)
        self._create = QPushButton(tr("Start a friend goal"))
        self._create.setEnabled(False)
        self._create.clicked.connect(self._new)
        root.addWidget(self._create)

    def _load(self):
        self._window._run("goals", _Mode.READ, lambda: social_data._goals(self._window._api), self._render, self._status)

    def _goal(self):
        item = self._table.currentItem()
        return item.data(0, Qt.ItemDataRole.UserRole) if item else None

    def _render(self, data):
        self._data = data
        wanted = self._wanted
        self._table.blockSignals(True)
        self._table.clear()
        selected = None
        for goal in data.get("challenges", []):
            own = next((member for member in goal["members"] if member["user"] == self._window._api.name), {})
            progress = goal["progress"] if goal["cooperative"] else own.get("progress", 0)
            unit = tr("reviews") if goal["kind"] == _Kind.REVIEWS.value else tr("study days")
            count = "%d / %d %s" % (progress, goal["target"], unit) if goal.get("started") else "—"
            item = QTreeWidgetItem([goal["title"], tr(_STATES[social_data._state(goal)]), count])
            item.setData(0, Qt.ItemDataRole.UserRole, goal)
            self._table.addTopLevelItem(item)
            if goal["id"] == wanted:
                selected = item
        self._table.setCurrentItem(selected)
        self._table.blockSignals(False)
        for column in range(self._table.columnCount()):
            self._table.resizeColumnToContents(column)
        if wanted is not None and selected is None:
            self._status.setText(tr("This friend goal is no longer available."))
        elif not data.get("challenges"):
            self._status.setText(tr("No friend goals yet."))
        self._create.setEnabled(bool(data.get("recipients")))
        suggestion = social_data._suggestion(data)
        self._weekly_text.setText(tr("%s: %d study days each over %d days.") % (
            suggestion["friend"]["display"], suggestion["target"], suggestion["duration_days"],
        ) if suggestion else "")
        self._weekly_text.setVisible(suggestion is not None)
        for button in self._weekly_buttons:
            button.setVisible(suggestion is not None)
        self._selected()

    def _selected(self):
        goal = self._goal()
        actions = social_data._actions(goal, self._window._api.name) if goal else ()
        self._wanted = goal["id"] if goal else None
        for index, button in enumerate(self._buttons):
            button.setVisible(index < len(actions))
            if index < len(actions):
                button.setText(tr(_ACTIONS[actions[index]]))
        lines = []
        if goal:
            lines.append(tr("One shared total") if goal["cooperative"] else tr("Individual targets"))
            if not goal.get("started"):
                lines.append(tr("Studying before everyone accepts does not count toward this goal."))
            for member in goal["members"]:
                lines.append("%s · %s · %s" % (member.get("display") or member["user"], tr(member["status"]), member["progress"]))
        self._details.setText("\n".join(lines))

    def _act(self, index):
        goal = self._goal()
        if goal is None:
            return
        actions = social_data._actions(goal, self._window._api.name)
        if index < len(actions):
            self._window._run("goals", _Mode.WRITE, lambda: social_data._goal_action(self._window._api, goal["id"], actions[index]), self._render, self._status)

    def _weekly(self, action):
        suggestion = social_data._suggestion(self._data)
        if suggestion:
            self._window._run("goals", _Mode.WRITE,
                              lambda: social_data._weekly(self._window._api, suggestion["week_start"], action), self._render, self._status)

    def _new(self):
        if not self._window._owns() or self._window._busy():
            return
        if self._creator is not None and self._creator._phase is _Phase.OPEN:
            self._creator._dialog.raise_()
            return
        self._creator = _Create(self)
        self._creator._dialog.show()

    def _close(self):
        if self._creator is not None:
            self._creator._dialog.close()


class _Create:
    def __init__(self, goals):
        self._goals, self._window, self._draft = goals, goals._window, social_data._Draft()
        self._phase = _Phase.OPEN
        self._dialog = QDialog(self._window._dialog)
        self._dialog.setWindowTitle(tr("Start a friend goal"))
        self._dialog.finished.connect(lambda _result: setattr(self, "_phase", _Phase.CLOSED))
        root = QVBoxLayout(self._dialog)
        self._fields = QWidget()
        form = QFormLayout(self._fields)
        self._title = QLineEdit(tr("Three study days together"))
        self._title.setMaxLength(80)
        form.addRow(tr("Title"), self._title)
        self._kind = QComboBox()
        self._kind.addItem(tr("study days"), _Kind.STUDY_DAYS)
        self._kind.addItem(tr("reviews"), _Kind.REVIEWS)
        form.addRow(tr("Count"), self._kind)
        self._together = QComboBox()
        self._together.addItem(tr("Individual targets"), _Together.INDIVIDUAL)
        self._together.addItem(tr("One shared total"), _Together.SHARED)
        form.addRow(tr("Goal type"), self._together)
        self._target, self._days = QSpinBox(), QSpinBox()
        self._target.setRange(1, 100_000)
        self._target.setValue(3)
        self._days.setRange(1, 31)
        self._days.setValue(7)
        form.addRow(tr("Target"), self._target)
        form.addRow(tr("Duration in days"), self._days)
        self._ready = QCheckBox(tr("Start after everyone accepts"))
        self._ready.setChecked(True)
        form.addRow(self._ready)
        self._recipients = QListWidget()
        for person in goals._data.get("recipients", []):
            item = QListWidgetItem("%s (%s)" % (person.get("display") or person["user"], person["user"]))
            item.setData(Qt.ItemDataRole.UserRole, person["user"])
            item.setCheckState(Qt.CheckState.Unchecked)
            self._recipients.addItem(item)
        form.addRow(tr("Invite friends"), self._recipients)
        root.addWidget(self._fields)
        self._status = share_ui._plain("")
        root.addWidget(self._status)
        actions = QHBoxLayout()
        self._save = QPushButton(tr("Invite friends"))
        self._save.clicked.connect(self._send)
        actions.addWidget(self._save)
        cancel = QPushButton(tr("Cancel"))
        cancel.clicked.connect(self._dialog.reject)
        actions.addWidget(cancel)
        root.addLayout(actions)
        self._dialog.resize(460, 500)

    def _send(self):
        if self._phase is _Phase.CLOSED or not self._window._owns() or self._window._busy():
            return
        recipients = [self._recipients.item(index).data(Qt.ItemDataRole.UserRole) for index in range(self._recipients.count())
                      if self._recipients.item(index).checkState() == Qt.CheckState.Checked]
        if not 1 <= len(recipients) <= 30:
            self._status.setText(tr("Choose between 1 and 30 friends."))
            return
        fields = dict(title=self._title.text(), kind=self._kind.currentData().value,
                      cooperative=self._together.currentData() is _Together.SHARED,
                      target=self._target.value(), duration_days=self._days.value(),
                      recipients=recipients, start_when_ready=self._ready.isChecked())
        body = self._draft._body(fields)
        self._fields.setEnabled(False)
        self._save.setEnabled(False)

        def created(answer):
            self._goals._render(answer)
            if self._phase is _Phase.OPEN:
                self._dialog.accept()

        def failed(_error):
            if self._phase is _Phase.OPEN:
                self._fields.setEnabled(True)
                self._save.setEnabled(True)

        self._window._run("goals", _Mode.WRITE, lambda: social_data._create(self._window._api, body), created, self._status, failed)
