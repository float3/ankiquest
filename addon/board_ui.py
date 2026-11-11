"""Scope selection -> owned reads -> native tables; hiding changes only the viewer's Global board."""
import base64
from enum import Enum, auto

from aqt.qt import QColor, QComboBox, QDialog, QGridLayout, QHBoxLayout, QIcon, QLabel, QPainter, QPixmap, QPushButton, QTabWidget, QTreeWidget, QTreeWidgetItem, QVBoxLayout, QWidget, Qt

from . import board, board_data, progress_ui, share_ui
from .language import tr


class _Phase(Enum):
    OPEN = auto()
    CLOSED = auto()


def _initial(name):
    picture = QPixmap(32, 32)
    picture.fill(Qt.GlobalColor.transparent)
    painter = QPainter(picture)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    painter.setPen(Qt.PenStyle.NoPen)
    painter.setBrush(QColor(board.AVATAR_COLOURS[sum(map(ord, name)) % len(board.AVATAR_COLOURS)]))
    painter.drawEllipse(0, 0, 32, 32)
    painter.setPen(Qt.GlobalColor.white)
    painter.drawText(picture.rect(), Qt.AlignmentFlag.AlignCenter, name[:1].upper() or "?")
    painter.end()
    return QIcon(picture)


class _Window:
    def __init__(self, addon, pictures, choose):
        self._addon, self._api, self._profile = addon, addon.client(), addon.mw.pm.profile
        self._pictures, self._choose = pictures, choose
        self._phase, self._requests, self._board = _Phase.OPEN, {}, None
        self._query = board_data._chosen(self._profile, self._api)
        self._dialog = QDialog(addon.mw)
        self._dialog.setWindowTitle(tr("Leaderboard"))
        self._dialog.finished.connect(self._finished)
        root = QVBoxLayout(self._dialog)
        controls = QGridLayout()
        self._scopes, self._periods = QComboBox(), QComboBox()
        self._scopes.addItem(tr("Friends"), "friends")
        for key, title in board.PERIOD_LABELS:
            self._periods.addItem(tr(title), key)
        self._periods.setCurrentIndex(self._periods.findData(self._query.period))
        controls.addWidget(QLabel(tr("Leaderboard scope")), 0, 0)
        controls.addWidget(self._scopes, 0, 1)
        controls.addWidget(QLabel(tr("Which leaderboard?")), 1, 0)
        controls.addWidget(self._periods, 1, 1)
        self._refresh = QPushButton(tr("Refresh"))
        self._refresh.clicked.connect(self._load)
        controls.addWidget(self._refresh, 1, 2)
        root.addLayout(controls)
        self._status = share_ui._plain("")
        root.addWidget(self._status)
        self._tabs = QTabWidget()
        root.addWidget(self._tabs, 1)
        self._tables, self._notes = {}, {}
        for key, title, headers in (
            ("board", tr("Leaderboard"), [tr("Place"), tr("Player"), "XP", tr("Level"), tr("Streak")]),
            ("records", tr("Records"), [tr("Record"), tr("Player"), tr("Value")]),
            ("hidden", tr("Hidden players"), [tr("Player")]),
        ):
            page = QWidget()
            layout = QVBoxLayout(page)
            note = self._notes[key] = share_ui._plain("")
            note.hide()
            layout.addWidget(note)
            table = self._tables[key] = QTreeWidget()
            table.setHeaderLabels(headers)
            table.setRootIsDecorated(False)
            table.setAlternatingRowColors(True)
            table.setUniformRowHeights(True)
            layout.addWidget(table, 1)
            self._tabs.addTab(page, title)
        self._tabs.setTabVisible(2, False)
        self._tabs.currentChanged.connect(self._tab_changed)
        self._tables["board"].itemSelectionChanged.connect(self._selection)
        self._tables["hidden"].itemSelectionChanged.connect(self._selection)
        self._tables["board"].verticalScrollBar().valueChanged.connect(self._paint_faces)
        actions = QHBoxLayout()
        self._hide = QPushButton(tr("Hide for me"))
        self._hide.setEnabled(False)
        self._hide.clicked.connect(self._change_hidden)
        actions.addWidget(self._hide)
        actions.addStretch(1)
        done = QPushButton(tr("Done"))
        done.clicked.connect(self._dialog.accept)
        actions.addWidget(done)
        root.addLayout(actions)
        self._scopes.currentIndexChanged.connect(self._selected)
        self._periods.currentIndexChanged.connect(self._selected)
        screen = self._dialog.screen().availableGeometry()
        self._dialog.resize(min(680, screen.width()), min(600, screen.height()))
        self._load()

    def _owns(self):
        return self._phase is _Phase.OPEN and self._profile is self._addon.mw.pm.profile and share_ui.same_account(self._api, self._addon.client())

    def _finished(self, _result):
        self._phase = _Phase.CLOSED
        self._requests.clear()

    def _close(self):
        self._dialog.close()

    def _selected(self, _index):
        if not self._owns():
            self._close()
            return
        self._query = board_data._Query(self._periods.currentData(), self._scopes.currentData())
        self._choose(self._query)
        self._load()

    def _load(self):
        if not self._owns():
            self._close()
            return
        # One generation covers all tabs; changing scope invalidates every old read.
        self._requests = {key: value for key, value in self._requests.items() if key == "change"}
        request = self._requests["board"] = object()
        query = self._query
        self._board = None
        self._hide.setEnabled(False)
        self._refresh.setEnabled(False)
        self._status.setText(tr("Loading leaderboard…"))
        for key, table in self._tables.items():
            table.clear()
            self._note(key, "")

        def done(future):
            if not self._owns() or self._requests.get("board") is not request:
                return
            self._requests.pop("board")
            self._refresh.setEnabled(True)
            try:
                self._board = future.result()
            except Exception as error:
                self._status.setText(tr("Could not load the leaderboard (%s).") % str(error))
                return
            self._query = self._board.query
            board_data._remember(self._profile, self._api, self._query)
            self._status.clear()
            self._scopes.blockSignals(True)
            self._scopes.clear()
            for key, title in self._board.scopes:
                self._scopes.addItem(tr(title) if key in ("friends", "global") else title, key)
            self._scopes.setCurrentIndex(self._scopes.findData(self._query.scope))
            self._scopes.blockSignals(False)
            self._tabs.setTabVisible(2, self._board.can_hide)
            table = self._tables["board"]
            for place, entry, mine in board_data._rows(self._board, self._api.name):
                name = entry.get("display") or entry["user"]
                title = name + (" · " + tr("you, not shown to others") if mine and entry.get("shown") is False else "")
                item = QTreeWidgetItem([str(place), title, str(entry["xp"]), str(entry.get("level", "")), str(entry.get("streak", ""))])
                item.setData(1, Qt.ItemDataRole.UserRole, entry)
                table.addTopLevelItem(item)
            if not table.topLevelItemCount():
                self._note("board", tr("No uploaded players on this board yet."))
            self._resize(table)
            self._paint_faces()
            self._tab_changed(self._tabs.currentIndex())

        self._addon.mw.taskman.run_in_background(lambda: board_data._fetch(self._api, query), done)

    def _resize(self, table):
        for column in range(table.columnCount()):
            table.resizeColumnToContents(column)

    def _note(self, key, text):
        self._notes[key].setText(text)
        self._notes[key].setVisible(bool(text))

    def _paint_faces(self, *_args):
        if not self._owns() or self._tabs.currentIndex() != 0:
            return
        table = self._tables["board"]
        users = []
        for index in range(table.topLevelItemCount()):
            item = table.topLevelItem(index)
            if not table.visualItemRect(item).intersects(table.viewport().rect()):
                continue
            entry = item.data(1, Qt.ItemDataRole.UserRole)
            uri = self._pictures._get(entry["user"])
            name = entry.get("display") or entry["user"]
            if uri:
                pixmap = QPixmap()
                pixmap.loadFromData(base64.b64decode(uri.split(",", 1)[1]))
                item.setIcon(1, QIcon(pixmap))
            else:
                item.setIcon(1, _initial(name))
            users.append(entry["user"])
        table.resizeColumnToContents(1)
        self._pictures._load(users, self._paint_faces)

    def _tab_changed(self, index):
        self._selection()
        if not self._owns() or self._board is None:
            return
        if index == 0:
            self._paint_faces()
            return
        key = "records" if index == 1 else "hidden"
        if key in self._requests:
            return
        request = self._requests[key] = object()
        query = self._query
        self._note(key, tr("Loading leaderboard…"))

        def done(future):
            if not self._owns() or self._requests.get(key) is not request:
                return
            self._requests.pop(key)
            table = self._tables[key]
            table.clear()
            try:
                data = future.result()
            except Exception as error:
                self._note(key, tr("Could not load the leaderboard (%s).") % str(error))
                return
            self._note(key, "")
            if key == "records":
                for record in data:
                    title = tr(progress_ui._RECORD_NAMES.get(record.get("window"), "Record"))
                    for holder in record.get("holders", []):
                        value = int(holder.get("value", 0))
                        text = tr("%d days") % value if record.get("unit") == "days" else "%d XP" % value
                        table.addTopLevelItem(QTreeWidgetItem([title, holder.get("display") or holder["user"], text]))
                if not table.topLevelItemCount():
                    self._note(key, tr("No records yet."))
            else:
                for entry in data:
                    item = QTreeWidgetItem([entry.get("display") or entry["user"]])
                    item.setData(0, Qt.ItemDataRole.UserRole, entry)
                    table.addTopLevelItem(item)
                self._note(key, tr("Hidden only from your Global board. Select a player to show them again."))
            self._resize(table)
            self._selection()

        self._addon.mw.taskman.run_in_background(
            lambda: board_data._records(self._api, query) if key == "records" else board_data._hidden(self._api), done,
        )

    def _selection(self):
        hidden = self._tabs.currentIndex() == 2
        table = self._tables["hidden" if hidden else "board"]
        item = table.currentItem()
        row = item.data(0 if hidden else 1, Qt.ItemDataRole.UserRole) if item else None
        allowed = self._board is not None and row is not None and self._board.can_hide
        if allowed and not hidden:
            allowed = self._tabs.currentIndex() == 0 and board_data._can_hide(self._board, row, self._api.name)
        self._hide.setText(tr("Show again") if hidden else tr("Hide for me"))
        self._hide.setEnabled(bool(allowed and "change" not in self._requests))

    def _change_hidden(self):
        if not self._owns() or not self._hide.isEnabled():
            return
        hidden = self._tabs.currentIndex() == 2
        table = self._tables["hidden" if hidden else "board"]
        row = table.currentItem().data(0 if hidden else 1, Qt.ItemDataRole.UserRole)
        request = self._requests["change"] = object()
        self._hide.setEnabled(False)
        self._status.setText(tr("Saving…"))

        def done(future):
            if not self._owns() or self._requests.get("change") is not request:
                return
            self._requests.pop("change")
            try:
                future.result()
            except Exception as error:
                self._status.setText(tr("Could not save this choice (%s).") % str(error))
                self._selection()
                return
            self._choose(self._query)
            self._load()

        choice = board_data._Visibility.SHOWN if hidden else board_data._Visibility.HIDDEN
        self._addon.mw.taskman.run_in_background(lambda: board_data._hide(self._api, choice, row["user"]), done)
