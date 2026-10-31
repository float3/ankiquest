"""The inbox as a native window: recent messages with Reply, Share and Open on the website."""

import time

from aqt.qt import (
    QDialog,
    QFrame,
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

from . import inbox, share, share_ui
from .language import tr
from .ui import _enum

SHORTCUT = "Ctrl+Shift+M"
THUMBNAIL = 180
KEPT_PICTURES = 40

state = {"dialog": None, "pictures": {}}


def _plain(text, bold=False, muted=False):
    label = QLabel(text)
    label.setTextFormat(_enum(Qt, "TextFormat", "PlainText"))
    label.setWordWrap(True)
    if bold:
        font = label.font()
        font.setBold(True)
        label.setFont(font)
    if muted:
        label.setEnabled(False)
    return label


def _meta(entry, legacy):
    parts = []
    if entry.get("sender"):
        parts.append(tr("From %s") % entry["sender"])
    if inbox.when(entry.get("created_at")):
        parts.append(inbox.when(entry.get("created_at")))
    if inbox.unread(entry, legacy):
        parts.append(tr("Unread"))
    if entry.get("replied"):
        parts.append(tr("Reply sent"))
    return "  ·  ".join(parts)


class Context:
    """What every row needs: the account it was loaded for and how to reach Anki."""

    def __init__(self, dialog, api, companion, current_client, run, open_path, on_replied):
        self.dialog, self.api, self.companion = dialog, api, companion
        self.current_client, self.run, self.open_path, self.on_replied = current_client, run, open_path, on_replied
        self.legacy = False

    def owned(self, status):
        if share_ui.same_account(self.api, self.current_client()):
            return True
        status.setText(tr("This inbox belongs to an AnkiQuest account that is no longer signed in."))
        return False

    def mark_read(self, entry, after):
        """Marks a row read in the background; `after()` runs once the server agreed."""
        if self.legacy or entry.get("read_at"):
            return

        def done(future):
            try:
                future.result()
            except Exception as error:
                print("ankiquest mark read:", error)
                return
            entry["read_at"] = int(time.time())
            after()

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

    def go(text):
        message = inbox.clean_reply(text)
        if not message or not context.owned(status):
            return
        panel.setEnabled(False)
        status.setText(tr("Sending…"))
        legacy = context.legacy

        def work():
            who = inbox.reply(context.api, entry["id"], message)
            if not legacy:
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


def _row(entry, context):
    frame = QFrame()
    frame.setFrameShape(_enum(QFrame, "Shape", "StyledPanel"))
    layout = QVBoxLayout(frame)
    title, body = inbox.headline(entry)
    layout.addWidget(_plain(title, bold=True))
    meta = _plain(_meta(entry, context.legacy), muted=True)
    layout.addWidget(meta)
    original = entry.get("reply_to") or {}
    if original.get("body"):
        layout.addWidget(_plain(tr("In reply to: %s") % original["body"], muted=True))
    if body:
        layout.addWidget(_plain(body))
    if entry.get("image") and entry.get("sender"):
        layout.addWidget(_thumbnail(share.absolute(context.api.base, entry["image"]), context))
    status = _plain("")
    buttons = QHBoxLayout()
    panel = None
    read = None

    def refreshed():
        meta.setText(_meta(entry, context.legacy))
        if read is not None:
            read.setVisible(inbox.unread(entry, context.legacy))

    found = inbox.actions(entry, context.legacy)
    if "reply" in found:
        reply = QPushButton(tr("Reply"))

        def answered():
            reply.setVisible(False)
            panel.setVisible(False)
            refreshed()

        panel = _reply_panel(entry, context, status, answered)
        reply.clicked.connect(lambda: panel.setVisible(not panel.isVisible()))
        buttons.addWidget(reply)
    if "share" in found:
        share_button = QPushButton(tr("Share"))
        share_button.clicked.connect(lambda: _share(entry, context, status))
        buttons.addWidget(share_button)
    if "cheer" in found:
        buttons.addWidget(_cheer_button(entry, context, status))
    website = QPushButton(tr("Open on the website"))

    def opened():
        context.open_path(inbox.web_path(entry))
        context.mark_read(entry, refreshed)

    website.clicked.connect(opened)
    buttons.addWidget(website)
    if "read" in found:
        read = QPushButton(tr("Mark read"))
        read.clicked.connect(lambda: context.mark_read(entry, refreshed) if context.owned(status) else None)
        buttons.addWidget(read)
    buttons.addStretch(1)
    layout.addLayout(buttons)
    if panel is not None:
        layout.addWidget(panel)
    layout.addWidget(status)
    return frame


def _cheer_button(entry, context, status):
    button = QPushButton(tr("🎉 Cheered") if entry.get("reacted") else tr("🎉 Cheer"))
    button.setEnabled(not entry.get("reacted"))

    def cheered(future):
        try:
            future.result()
        except Exception as error:
            button.setEnabled(True)
            status.setText(tr("Could not send your cheer (%s).") % inbox.error_text(error))
            return
        entry["reacted"] = True
        button.setText(tr("🎉 Cheered"))
        status.setText(tr("You cheered %s on.") % entry.get("sender", ""))

    def cheer():
        if not context.owned(status):
            return
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
        _enum(Qt, "AspectRatioMode", "KeepAspectRatio"),
        _enum(Qt, "TransformationMode", "SmoothTransformation"),
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
    if existing is not None and share_ui.same_account(existing.context.api, api):
        existing.reload()
        existing.show()
        existing.raise_()
        existing.activateWindow()
        return existing
    if existing is not None:
        existing.close()
    dialog = _build(parent, api, companion, current_client, run, open_path, on_replied)
    state["dialog"] = dialog
    dialog.finished.connect(lambda *_: state.update(dialog=None) if state["dialog"] is dialog else None)
    dialog.reload()
    dialog.show()
    return dialog


def _build(parent, api, companion, current_client, run, open_path, on_replied):
    dialog = QDialog(parent)
    dialog.setWindowTitle(tr("ankiquest inbox"))
    dialog.setAttribute(_enum(Qt, "WidgetAttribute", "WA_DeleteOnClose"))
    dialog.resize(680, 640)
    context = Context(dialog, api, companion, current_client, run, open_path, on_replied)
    view = {"items": [], "loading": False}
    layout = QVBoxLayout(dialog)
    status = _plain("")
    every = QPushButton(tr("Mark all read"))
    again = QPushButton(tr("Refresh"))
    top = QHBoxLayout()
    top.addWidget(status, 1)
    top.addWidget(every)
    top.addWidget(again)
    layout.addLayout(top)
    area, rows = _list_area()
    layout.addWidget(area, 1)
    close = QPushButton(tr("Close"))
    close.clicked.connect(dialog.close)
    bottom = QHBoxLayout()
    bottom.addStretch(1)
    bottom.addWidget(close)
    layout.addLayout(bottom)

    def render():
        _clear(rows)
        for entry in view["items"]:
            rows.addWidget(_row(entry, context))
        if not view["items"]:
            rows.addWidget(_plain(tr("Nothing in your inbox yet."), muted=True))
        rows.addStretch(1)
        every.setVisible(not context.legacy and any(inbox.unread(entry) for entry in view["items"]))

    def loaded(future):
        view["loading"] = False
        again.setEnabled(True)
        try:
            view["items"], context.legacy = future.result()
        except Exception as error:
            status.setText(tr("Could not load your inbox (%s).") % inbox.error_text(error))
            return
        status.setText("")
        render()

    def reload():
        if view["loading"]:
            return
        view["loading"] = True
        again.setEnabled(False)
        status.setText(tr("Loading…"))
        run(lambda: inbox.load(api), loaded)

    def all_read(future):
        every.setEnabled(True)
        try:
            future.result()
        except Exception as error:
            status.setText(tr("Could not mark everything read (%s).") % inbox.error_text(error))
            return
        now = int(time.time())
        for entry in view["items"]:
            entry["read_at"] = entry.get("read_at") or now
        render()

    def mark_all():
        through = max([int(entry.get("id") or 0) for entry in view["items"]] or [0])
        if not through or not context.owned(status):
            return
        every.setEnabled(False)
        run(lambda: inbox.mark_all_read(api, through), all_read)

    every.clicked.connect(mark_all)
    again.clicked.connect(reload)
    dialog.context, dialog.reload = context, reload
    return dialog
