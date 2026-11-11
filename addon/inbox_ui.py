"""The inbox as a native window: recent messages with Reply, Share and Open on the website."""

import time
from copy import copy
from dataclasses import dataclass
from enum import Enum, auto

from aqt.qt import (
    QDialog,
    QCheckBox,
    QComboBox,
    QFrame,
    QGridLayout,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QPixmap,
    QPushButton,
    QScrollArea,
    QVBoxLayout,
    QWidget,
    Qt,
)

from . import activity, inbox, share, share_ui
from .language import tr

SHORTCUT = "Ctrl+Shift+M"
THUMBNAIL = 180
KEPT_PICTURES = 40

state = {"dialog": None, "pictures": {}}


class _Phase(Enum):
    OPEN = auto()
    CLOSED = auto()


class _Action(Enum):
    READ = auto()
    REPLY = auto()
    CHEER = auto()


@dataclass
class _Scope:
    phase: _Phase = _Phase.OPEN
    revision: int = 0


def clear_account():
    dialog = state["dialog"]
    state.update(dialog=None, pictures={})
    if dialog is not None:
        dialog.close()


def _plain(text, bold=False, muted=False):
    label = QLabel(text)
    label.setTextFormat(Qt.TextFormat.PlainText)
    label.setWordWrap(True)
    if bold:
        font = label.font()
        font.setBold(True)
        label.setFont(font)
    if muted:
        font = label.font()
        font.setPointSize(max(8, font.pointSize() - 1))
        label.setFont(font)
    return label


def _meta(entry):
    parts = []
    if entry.get("sender"):
        parts.append(tr("From %s") % entry["sender"])
    if inbox.when(entry.get("created_at")):
        parts.append(inbox.when(entry.get("created_at")))
    if inbox.unread(entry):
        parts.append(tr("Unread"))
    if entry.get("replied"):
        parts.append(tr("Reply sent"))
    return "  ·  ".join(parts)


class Context:
    """What every row needs: the account it was loaded for and how to reach Anki."""

    def __init__(self, dialog, api, companion, current_client, run, open_path, on_replied):
        self.dialog, self.api, self.companion = dialog, api, companion
        self.current_client, self.run, self.open_path, self.on_replied = current_client, run, open_path, on_replied
        self._scope, self._revision, self._pending = _Scope(), None, set()
        self._pm = getattr(dialog.parentWidget(), "pm", None)
        self._profile = getattr(self._pm, "profile", None)
        self._root = self
        self._after_change, self._reload_inbox = lambda: None, lambda: None
        dialog.finished.connect(self._closed)

    def _closed(self, _result):
        self._scope.phase = _Phase.CLOSED

    def _alive(self):
        return (self._scope.phase is _Phase.OPEN and getattr(self._pm, "profile", None) is self._profile
                and (self._revision is None or self._revision == self._scope.revision))

    def _owns(self):
        return self._alive() and share_ui.same_account(self.api, self.current_client())

    def _row_context(self):
        child = copy(self)
        child._revision, child._pending = self._scope.revision, set()
        return child

    def _refresh_badge(self):
        self._root._after_change()
        self._root._reload_inbox()

    def _arrived(self, future):
        if not self._root._owns():
            return False
        if self._owns():
            return True
        # A write can finish after Refresh replaced its row; refresh the current view.
        try:
            future.result()
        except Exception:
            return False
        self._root._refresh_badge()
        return False

    def owned(self, status):
        if not self._alive():
            return False
        if share_ui.same_account(self.api, self.current_client()):
            return True
        status.setText(tr("This inbox belongs to an AnkiQuest account that is no longer signed in."))
        return False

    def mark_read(self, entry, after, status=None):
        """Marks a row read in the background; `after()` runs once the server agreed."""
        if not self._owns() or entry.get("read_at") or _Action.READ in self._pending:
            return
        self._pending.add(_Action.READ)

        def done(future):
            if not self._arrived(future):
                return
            self._pending.discard(_Action.READ)
            try:
                future.result()
            except Exception as error:
                if status is not None:
                    status.setText(tr("Could not mark this message read (%s).") % inbox.error_text(error))
                return
            entry["read_at"] = int(time.time())
            after()
            self._refresh_badge()

        self.run(lambda: inbox.mark_read(self.api, [entry["id"]]), done)


def _reply_panel(entry, context, status, finished):
    panel = QWidget()
    layout = QVBoxLayout(panel)
    layout.setContentsMargins(0, 0, 0, 0)
    picks = QHBoxLayout()
    field = QLineEdit()
    field.setMaxLength(inbox.MAX_MESSAGE)
    field.setPlaceholderText(tr("Say something nice"))
    send = QPushButton(tr("Send"))

    def sent(future, message):
        if not context._arrived(future):
            return
        context._pending.discard(_Action.REPLY)
        try:
            who = future.result()
        except Exception as error:
            panel.setEnabled(True)
            status.setText(tr("Could not send “%s” (%s).") % (message, inbox.error_text(error)))
            return
        entry["replied"] = True
        entry["read_at"] = entry.get("read_at") or int(time.time())
        status.setText(tr("Sent to %s: %s") % (who or entry.get("sender", ""), message))
        finished()
        context.on_replied(entry["id"])
        context._refresh_badge()

    def go(text):
        message = inbox.clean_reply(text)
        if not message or not context.owned(status) or _Action.REPLY in context._pending:
            return
        context._pending.add(_Action.REPLY)
        panel.setEnabled(False)
        status.setText(tr("Sending…"))

        def work():
            who = inbox.reply(context.api, entry["id"], message)
            try:
                inbox.mark_read(context.api, [entry["id"]])
            except Exception as error:
                print("ankiquest mark read:", error)
            return who

        context.run(work, lambda future: sent(future, message))

    for choice in inbox.reply_choices(entry.get("kind"), tr):
        button = QPushButton(choice)
        button.clicked.connect(lambda _=False, text=choice: go(text))
        picks.addWidget(button)
    picks.addStretch(1)
    send.clicked.connect(lambda: go(field.text()))
    field.returnPressed.connect(lambda: go(field.text()))
    layout.addLayout(picks)
    layout.addLayout(_line(field, send))
    panel.setVisible(False)
    return panel


def _line(*widgets):
    row = QHBoxLayout()
    for widget in widgets:
        row.addWidget(widget)
    return row


def _add_button(layout, button):
    # Two columns keep translated actions visible in a narrow inbox.
    index = layout.count()
    layout.addWidget(button, index // 2, index % 2)


def _row(entry, context):
    frame = QFrame()
    frame.setFrameShape(QFrame.Shape.StyledPanel)
    layout = QVBoxLayout(frame)
    title, body = inbox.headline(entry)
    layout.addWidget(_plain(title, bold=True))
    meta = _plain(_meta(entry), muted=True)
    layout.addWidget(meta)
    original = entry.get("reply_to") or {}
    if original.get("body"):
        layout.addWidget(_plain(tr("In reply to: %s") % original["body"], muted=True))
    if body:
        layout.addWidget(_plain(body))
    if entry.get("image") and entry.get("sender"):
        layout.addWidget(_thumbnail(share.absolute(context.api.base, entry["image"]), context))
    status = _plain("")
    buttons = QGridLayout()
    panel = None
    read = None

    def refreshed():
        meta.setText(_meta(entry))
        if read is not None:
            read.setVisible(inbox.unread(entry))

    found = inbox.actions(entry)
    if "reply" in found:
        reply = QPushButton(tr("Reply"))

        def answered():
            reply.setVisible(False)
            panel.setVisible(False)
            refreshed()

        panel = _reply_panel(entry, context, status, answered)
        reply.clicked.connect(lambda: panel.setVisible(not panel.isVisible()))
        _add_button(buttons, reply)
    if "share" in found:
        share_button = QPushButton(tr("Share"))
        share_button.clicked.connect(lambda: _share(entry, context, status))
        _add_button(buttons, share_button)
    if "cheer" in found:
        _add_button(buttons, _cheer_button(entry, context, status))
    goal_label = tr("View invitation") if entry.get("action_required") else tr("View friend goal")
    website = QPushButton(goal_label if inbox.challenge_target(entry) is not None else tr("Open on the website"))

    def opened():
        if not context.owned(status):
            return
        context.open_path(inbox.web_path(entry))
        context.mark_read(entry, refreshed, status)

    website.clicked.connect(opened)
    _add_button(buttons, website)
    if "read" in found:
        read = QPushButton(tr("Mark read"))
        read.clicked.connect(lambda: context.mark_read(entry, refreshed, status) if context.owned(status) else None)
        _add_button(buttons, read)
    buttons.setColumnStretch(0, 1)
    buttons.setColumnStretch(1, 1)
    layout.addLayout(buttons)
    if panel is not None:
        layout.addWidget(panel)
    layout.addWidget(status)
    return frame


def _cheer_button(entry, context, status):
    button = QPushButton(tr("🎉 Cheered") if entry.get("reacted") else tr("🎉 Cheer"))
    button.setEnabled(not entry.get("reacted"))

    def cheered(future):
        if not context._arrived(future):
            return
        context._pending.discard(_Action.CHEER)
        try:
            future.result()
        except Exception as error:
            button.setEnabled(True)
            status.setText(tr("Could not send your cheer (%s).") % inbox.error_text(error))
            return
        entry["reacted"] = True
        button.setText(tr("🎉 Cheered"))
        status.setText(tr("You cheered %s on.") % entry.get("sender", ""))
        context._refresh_badge()

    def cheer():
        if not context.owned(status) or _Action.CHEER in context._pending or entry.get("reacted"):
            return
        context._pending.add(_Action.CHEER)
        button.setEnabled(False)
        context.run(lambda: inbox.react(context.api, entry["id"]), cheered)

    button.clicked.connect(cheer)
    return button


def _keep_picture(url, png):
    pictures = state["pictures"]
    pictures[url] = png
    while len(pictures) > KEPT_PICTURES:
        pictures.pop(next(iter(pictures)))


def _show_picture(label, png):
    pixmap = QPixmap()
    if not pixmap.loadFromData(png, "PNG"):
        label.setVisible(False)
        return
    label.setPixmap(pixmap.scaled(
        THUMBNAIL, THUMBNAIL,
        Qt.AspectRatioMode.KeepAspectRatio,
        Qt.TransformationMode.SmoothTransformation,
    ))


def _thumbnail(url, context):
    """The friend's card, fetched in the background and kept for the next time the inbox opens."""
    label = QLabel()
    label.setFixedSize(THUMBNAIL, THUMBNAIL)
    known = state["pictures"].get(url)
    if known is not None:
        _show_picture(label, known)
        return label

    def fetched(future):
        if not context._owns():
            return
        try:
            png = future.result()
        except Exception as error:
            print("ankiquest card picture:", error)
            png = None
        try:
            if png is None:
                label.setVisible(False)
                return
            _keep_picture(url, png)
            _show_picture(label, png)
        except RuntimeError:
            return

    context.run(lambda: share.download_image(url), fetched)
    return label


def _share(entry, context, status):
    if not context.owned(status):
        return
    win = share.win_from_entry(entry)
    share_ui.win_dialog(context.dialog, context.api, win, context.companion, context.current_client, context.run).show()


def _list_area():
    inner = QWidget()
    rows = QVBoxLayout(inner)
    area = QScrollArea()
    area.setWidgetResizable(True)
    area.setWidget(inner)
    return area, rows


def _clear(rows):
    while rows.count():
        item = rows.takeAt(0)
        if item.widget() is not None:
            item.widget().deleteLater()


def inbox_dialog(parent, api, companion, current_client, run, open_path, on_replied):
    """Opens the inbox for `api`'s account, or brings the open one forward and reloads it."""
    existing = state["dialog"]
    if existing is not None and existing.context._owns() and share_ui.same_account(existing.context.api, api):
        existing.reload()
        existing.show()
        existing.raise_()
        existing.activateWindow()
        return existing
    if existing is not None:
        existing.close()
        state["pictures"] = {}
    dialog = _build(parent, api, companion, current_client, run, open_path, on_replied)
    state["dialog"] = dialog
    dialog.finished.connect(lambda *_: state.update(dialog=None) if state["dialog"] is dialog else None)
    dialog.reload()
    dialog.show()
    return dialog


def _build(parent, api, companion, current_client, run, open_path, on_replied):
    return _Inbox(parent, api, companion, current_client, run, open_path, on_replied)._dialog


class _Job(Enum):
    LOAD = auto()
    MARK = auto()


class _Inbox:
    def __init__(self, parent, api, companion, current_client, run, open_path, on_replied):
        self._dialog = dialog = QDialog(parent)
        dialog.setWindowTitle(tr("AnkiQuest inbox"))
        dialog.setAttribute(Qt.WidgetAttribute.WA_DeleteOnClose)
        dialog.resize(680, 640)
        self._context = Context(dialog, api, companion, current_client, run, open_path, on_replied)
        self._context._reload_inbox = self._reload
        self._page, self._jobs = None, {}
        root = QVBoxLayout(dialog)
        self._counts, self._status = _plain(""), _plain("")
        root.addWidget(self._counts)
        self._category = QComboBox()
        for category, label in ((activity._Category.ALL, tr("All activity")), (activity._Category.NEEDS_ACTION, tr("Needs action")),
                                (activity._Category.MESSAGES, tr("Messages")), (activity._Category.CHALLENGES, tr("Friend goals")),
                                (activity._Category.DECK_COMPLETIONS, tr("Deck completions")), (activity._Category.STUDY_UPDATES, tr("Study updates"))):
            self._category.addItem(label, category)
        self._unread = QCheckBox(tr("Unread only"))
        self._category.currentIndexChanged.connect(self._filter_changed)
        self._unread.toggled.connect(self._filter_changed)
        root.addLayout(_line(self._category, self._unread))
        self._every = QPushButton(tr("Mark all read"))
        self._every.clicked.connect(self._mark_all)
        self._again = QPushButton(tr("Refresh"))
        self._again.clicked.connect(self._reload)
        root.addLayout(_line(self._status, self._every, self._again))
        self._area, self._rows = _list_area()
        root.addWidget(self._area, 1)
        self._more = QPushButton(tr("Load more"))
        self._more.clicked.connect(self._load_more)
        self._more.hide()
        close = QPushButton(tr("Close"))
        close.clicked.connect(dialog.close)
        root.addLayout(_line(self._more, close))
        dialog.context, dialog.reload, dialog._inbox = self._context, self._reload, self
        self._controls()

    def _controls(self):
        self._again.setEnabled(_Job.LOAD not in self._jobs)
        self._more.setEnabled(_Job.LOAD not in self._jobs)
        self._every.setEnabled(_Job.MARK not in self._jobs and _Job.LOAD not in self._jobs)
        self._every.setVisible(bool(self._page and self._page.unread))

    def _filter_changed(self, *_args):
        self._page = None
        self._render()
        self._reload()

    def _reload(self):
        self._load(None)

    def _load_more(self):
        if self._page and self._page.before is not None and _Job.LOAD not in self._jobs:
            self._load(self._page.before)

    def _load(self, before):
        if not self._context._owns():
            return
        request = self._jobs[_Job.LOAD] = object()
        category = self._category.currentData()
        read = activity._Read.UNREAD if self._unread.isChecked() else activity._Read.ALL
        self._status.setText(tr("Loading…"))
        self._controls()

        def loaded(future):
            if not self._context._owns() or self._jobs.get(_Job.LOAD) is not request:
                return
            self._jobs.pop(_Job.LOAD)
            self._controls()
            try:
                answer = future.result()
            except Exception as error:
                self._status.setText(tr("Could not load your inbox (%s).") % inbox.error_text(error))
                return
            self._page = activity._append(self._page, answer) if before is not None and self._page else answer
            self._status.clear()
            if answer.source is activity._Source.UNFILTERED:
                self._category.blockSignals(True)
                self._unread.blockSignals(True)
                self._category.setCurrentIndex(0)
                self._unread.setChecked(False)
                self._category.blockSignals(False)
                self._unread.blockSignals(False)
                self._status.setText(tr("This server shows all activity without filters."))
            self._render()

        self._context.run(lambda: activity._load(self._context.api, category, read, before), loaded)

    def _render(self):
        # New rows invalidate callbacks for widgets the previous rendering removed.
        self._context._scope.revision += 1
        _clear(self._rows)
        page = self._page
        self._counts.setText(tr("%d unread · %d awaiting action") % (page.unread, page.actions) if page else "")
        if page:
            legacy = page.source is activity._Source.LEGACY
            self._category.setVisible(not legacy)
            self._unread.setVisible(not legacy)
            if legacy:
                self._counts.setText(tr("This server provides recent messages without read counts or filters."))
            attention = page.attention if self._category.currentData() is activity._Category.ALL else []
            if attention:
                self._rows.addWidget(_plain(tr("Needs action"), bold=True))
            wanted = {row["id"] for row in attention}
            for entry in attention + [row for row in page.items if row["id"] not in wanted]:
                self._rows.addWidget(_row(entry, self._context._row_context()))
            if not page.items and not attention:
                self._rows.addWidget(_plain(tr("Nothing in your inbox yet."), muted=True))
        self._rows.addStretch(1)
        self._more.setVisible(bool(page and page.before is not None))
        self._controls()

    def _mark_all(self):
        page = self._page
        through = (page.latest or max((row["id"] for row in page.items), default=0)) if page else 0
        if not through or not self._context._owns() or _Job.MARK in self._jobs:
            return
        request = self._jobs[_Job.MARK] = object()
        self._controls()

        def marked(future):
            if not self._context._owns() or self._jobs.get(_Job.MARK) is not request:
                return
            self._jobs.pop(_Job.MARK)
            self._controls()
            try:
                future.result()
            except Exception as error:
                self._status.setText(tr("Could not mark everything read (%s).") % inbox.error_text(error))
                return
            self._context._refresh_badge()

        self._context.run(lambda: inbox.mark_all_read(self._context.api, through), marked)
