"""Friends/groups/goals -> one owned window -> server answers replace each view."""
from dataclasses import dataclass
from enum import Enum, auto

from aqt.qt import QApplication, QCheckBox, QComboBox, QDialog, QGridLayout, QHBoxLayout, QLabel, QLineEdit, QPushButton, QTabWidget, QTreeWidget, QTreeWidgetItem, QVBoxLayout, QWidget, Qt

from . import inbox, share_ui, social_data
from .language import tr
from .social_data import _Social


class _Phase(Enum):
    OPEN = auto()
    CLOSED = auto()


class _Mode(Enum):
    READ = auto()
    WRITE = auto()


@dataclass
class _Flight:
    mode: _Mode


class _Window:
    def __init__(self, addon, changed, goal=None):
        from . import friend_goals_ui

        self._addon, self._api, self._profile = addon, addon.client(), addon.mw.pm.profile
        self._changed, self._people, self._jobs = changed, {}, {}
        self._phase, self._invite = _Phase.OPEN, None
        self._dialog = QDialog(addon.mw)
        self._dialog.setWindowTitle(tr("Friends and groups"))
        self._dialog.finished.connect(self._finished)
        root = QVBoxLayout(self._dialog)
        self._status = share_ui._plain("")
        root.addWidget(self._status)
        self._tabs = QTabWidget()
        root.addWidget(self._tabs, 1)
        self._build_people()
        self._build_groups()
        self._goals = friend_goals_ui._Goals(self, goal)
        self._tabs.addTab(self._goals._page, tr("Friend goals"))
        bottom = QHBoxLayout()
        self._refresh = QPushButton(tr("Refresh"))
        self._refresh.clicked.connect(self._reload)
        bottom.addWidget(self._refresh)
        bottom.addStretch(1)
        close = QPushButton(tr("Done"))
        close.clicked.connect(self._dialog.accept)
        bottom.addWidget(close)
        root.addLayout(bottom)
        screen = self._dialog.screen().availableGeometry()
        self._dialog.resize(min(700, screen.width()), min(640, screen.height()))
        if goal is not None:
            self._tabs.setCurrentIndex(2)
        self._reload()

    def _owns(self):
        return self._phase is _Phase.OPEN and self._profile is self._addon.mw.pm.profile and share_ui.same_account(self._api, self._addon.client())

    def _finished(self, _result):
        self._phase = _Phase.CLOSED
        self._jobs.clear()
        self._goals._close()

    def _close(self):
        self._dialog.close()

    def _busy(self):
        return any(job.mode is _Mode.WRITE for job in self._jobs.values())

    def _run(self, key, mode, work, apply, status=None, failed=None):
        # Reopening a goal must not replace an in-flight mutation with a GET.
        if not self._owns() or self._busy():
            return
        target = status or self._status
        request = self._jobs[key] = _Flight(mode)
        self._tabs.setEnabled(not self._busy())
        self._refresh.setEnabled(not self._busy())
        target.setText(tr("Saving…") if mode is _Mode.WRITE else tr("Loading…"))

        def done(future):
            if not self._owns() or self._jobs.get(key) is not request:
                return
            self._jobs.pop(key)
            self._tabs.setEnabled(not self._busy())
            self._refresh.setEnabled(not self._busy())
            try:
                answer = future.result()
            except Exception as error:
                target.setText(tr("Could not complete this action (%s).") % inbox.error_text(error))
                if failed is not None:
                    failed(error)
                return
            target.clear()
            apply(answer)

        self._addon.mw.taskman.run_in_background(work, done)

    def _reload(self):
        if self._busy():
            return
        self._run("people", _Mode.READ, lambda: social_data._people(self._api), self._render_people)
        self._goals._load()

    def _build_people(self):
        page = QWidget()
        layout = QVBoxLayout(page)
        self._public = QCheckBox(tr("Public profile"))
        self._public.setEnabled(False)
        self._public.toggled.connect(self._visibility)
        layout.addWidget(self._public)
        self._people_tree = QTreeWidget()
        self._people_tree.setHeaderLabels([tr("Player"), tr("Username")])
        self._people_tree.itemSelectionChanged.connect(self._person_selected)
        layout.addWidget(self._people_tree, 1)
        actions = QHBoxLayout()
        self._person_buttons = []
        for index in range(2):
            button = QPushButton()
            button.hide()
            button.clicked.connect(lambda _checked=False, index=index: self._person_action(index))
            actions.addWidget(button)
            self._person_buttons.append(button)
        actions.addStretch(1)
        layout.addLayout(actions)
        add = QHBoxLayout()
        self._username = QLineEdit()
        self._username.setPlaceholderText(tr("Username"))
        add.addWidget(self._username)
        self._add = QPushButton(tr("Add friend"))
        self._add.clicked.connect(self._add_friend)
        add.addWidget(self._add)
        layout.addLayout(add)
        self._tabs.addTab(page, tr("Friends"))

    def _person(self):
        item = self._people_tree.currentItem()
        return item.data(0, Qt.ItemDataRole.UserRole) if item else None

    def _person_choices(self):
        person = self._person()
        if person is None:
            return []
        section, user = person
        if section == "incoming":
            return [(_Social.BEFRIEND, tr("Accept request")), (_Social.UNFRIEND, tr("Decline request"))]
        if section == "outgoing":
            return [(_Social.UNFRIEND, tr("Cancel request"))]
        if section == "muted":
            return [(_Social.UNMUTE, tr("Unmute"))]
        muted = user in {row["user"] for row in self._people.get("muted", [])}
        return [(_Social.UNFRIEND, tr("Remove friend")),
                (_Social.UNMUTE, tr("Unmute")) if muted else (_Social.MUTE, tr("Mute"))]

    def _person_selected(self):
        choices = self._person_choices()
        for index, button in enumerate(self._person_buttons):
            button.setVisible(index < len(choices))
            if index < len(choices):
                button.setText(choices[index][1])

    def _person_action(self, index):
        choices, person = self._person_choices(), self._person()
        if person is not None and index < len(choices):
            self._change(choices[index][0], user=person[1])

    def _add_friend(self):
        user = self._username.text().strip()
        if user:
            self._change(_Social.BEFRIEND, user=user)

    def _visibility(self, checked):
        if "public" in self._people:
            self._change(_Social.VISIBILITY, public=checked)

    def _render_people(self, people):
        selected = self._person()
        group_id = self._groups.currentData()
        self._people = people
        self._public.blockSignals(True)
        self._public.setChecked(bool(people.get("public")))
        self._public.setEnabled("public" in people)
        self._public.blockSignals(False)
        self._people_tree.clear()
        for key, title in (("friends", tr("Friends")), ("incoming", tr("Incoming requests")),
                           ("outgoing", tr("Outgoing requests")), ("muted", tr("Muted players"))):
            parent = QTreeWidgetItem(["%s (%d)" % (title, len(people.get(key, [])))])
            self._people_tree.addTopLevelItem(parent)
            for person in people.get(key, []):
                item = QTreeWidgetItem([person.get("display") or person["user"], person["user"]])
                item.setData(0, Qt.ItemDataRole.UserRole, (key, person["user"]))
                parent.addChild(item)
                if selected == (key, person["user"]):
                    self._people_tree.setCurrentItem(item)
            parent.setExpanded(True)
        self._people_tree.resizeColumnToContents(0)
        self._groups.blockSignals(True)
        self._groups.clear()
        for group in people.get("groups", []):
            self._groups.addItem(group["name"], group["id"])
        index = self._groups.findData(group_id)
        if index >= 0:
            self._groups.setCurrentIndex(index)
        self._groups.blockSignals(False)
        self._group_selected()
        self._person_selected()

    def _change(self, action, **fields):
        def changed(answer):
            self._render_people(answer)
            self._goals._load()
            self._changed()

        self._run("people", _Mode.WRITE, lambda: social_data._change(self._api, action, **fields), changed,
                  failed=lambda _error: self._render_people(self._people))

    def _build_groups(self):
        page = QWidget()
        layout = QVBoxLayout(page)
        self._groups = QComboBox()
        self._groups.currentIndexChanged.connect(self._group_selected)
        layout.addWidget(self._groups)
        names = QHBoxLayout()
        self._group_name = QLineEdit()
        self._group_name.setMaxLength(40)
        self._group_name.setPlaceholderText(tr("Group name"))
        names.addWidget(self._group_name)
        create = QPushButton(tr("Create group"))
        create.clicked.connect(lambda: self._change(_Social.CREATE_GROUP, name=self._group_name.text().strip()))
        names.addWidget(create)
        self._rename = QPushButton(tr("Rename"))
        self._rename.clicked.connect(lambda: self._group_action(_Social.RENAME_GROUP, name=self._group_name.text().strip()))
        names.addWidget(self._rename)
        layout.addLayout(names)
        self._members = QTreeWidget()
        self._members.setHeaderLabels([tr("Player"), tr("Username")])
        self._members.itemSelectionChanged.connect(self._member_selected)
        layout.addWidget(self._members, 1)
        actions = QGridLayout()
        self._copy = QPushButton(tr("Copy invite link"))
        self._copy.clicked.connect(self._copy_invite)
        self._new_invite = QPushButton(tr("Replace invite link"))
        self._new_invite.clicked.connect(lambda: self._group_action(_Social.NEW_INVITE))
        self._remove = QPushButton(tr("Remove member"))
        self._remove.clicked.connect(self._remove_member)
        self._leave = QPushButton(tr("Leave group"))
        self._leave.clicked.connect(lambda: self._group_action(_Social.LEAVE))
        for index, button in enumerate((self._copy, self._new_invite, self._remove, self._leave)):
            actions.addWidget(button, index // 2, index % 2)
        layout.addLayout(actions)
        join = QHBoxLayout()
        self._invite_field = QLineEdit()
        self._invite_field.setPlaceholderText(tr("Group invitation link or code"))
        self._invite_field.textChanged.connect(self._invite_changed)
        join.addWidget(self._invite_field)
        preview = QPushButton(tr("Preview invitation"))
        preview.clicked.connect(self._preview_invite)
        join.addWidget(preview)
        layout.addLayout(join)
        self._invite_status = share_ui._plain("")
        layout.addWidget(self._invite_status)
        self._join = QPushButton(tr("Join group"))
        self._join.setEnabled(False)
        self._join.clicked.connect(self._join_group)
        layout.addWidget(self._join)
        self._tabs.addTab(page, tr("Groups"))

    def _group(self):
        return next((g for g in self._people.get("groups", []) if g["id"] == self._groups.currentData()), None)

    def _group_selected(self, *_args):
        group = self._group()
        self._members.clear()
        self._group_name.setText(group["name"] if group else "")
        for member in (group or {}).get("members", []):
            item = QTreeWidgetItem([member.get("display") or member["user"], member["user"]])
            item.setData(0, Qt.ItemDataRole.UserRole, member["user"])
            self._members.addTopLevelItem(item)
        self._members.resizeColumnToContents(0)
        owner = bool(group and group["owner"])
        self._rename.setEnabled(owner)
        self._new_invite.setEnabled(owner)
        self._copy.setEnabled(bool(owner and group.get("invite")))
        self._leave.setEnabled(group is not None)
        self._member_selected()

    def _member_selected(self):
        group, selected = self._group(), self._members.currentItem()
        user = selected.data(0, Qt.ItemDataRole.UserRole) if selected else None
        self._remove.setEnabled(bool(group and group["owner"] and user and user != self._api.name))

    def _group_action(self, action, **fields):
        group = self._group()
        if group is not None:
            self._change(action, group=group["id"], **fields)

    def _remove_member(self):
        if self._remove.isEnabled():
            self._group_action(_Social.REMOVE, member=self._members.currentItem().data(0, Qt.ItemDataRole.UserRole))

    def _copy_invite(self):
        if not self._owns():
            return
        group = self._group()
        link = social_data._invite_link(self._api, group) if group and group["owner"] else None
        if link:
            QApplication.clipboard().setText(link)
            self._status.setText(tr("Invitation link copied."))

    def _invite_changed(self, _text):
        self._jobs.pop("invite", None)
        self._invite = None
        self._join.setEnabled(False)
        self._invite_status.clear()

    def _preview_invite(self):
        try:
            code = social_data._invite_code(self._invite_field.text(), self._api.base)
        except ValueError as error:
            self._invite_status.setText(tr(str(error)))
            return

        def previewed(answer):
            self._invite = code
            self._invite_status.setText(tr("%s · %d members") % (answer["name"], answer["members"]))
            self._join.setEnabled(True)

        self._run("invite", _Mode.READ, lambda: social_data._preview(self._api, code), previewed, self._invite_status)

    def _join_group(self):
        if self._invite is not None:
            self._change(_Social.JOIN, invite=self._invite)
