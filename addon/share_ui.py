"""Sharing a win from Anki: with friends through the server, or anywhere as the server's card picture."""

from pathlib import Path

from aqt.qt import (
    QApplication,
    QDialog,
    QFileDialog,
    QFont,
    QHBoxLayout,
    QImage,
    QLabel,
    QMimeData,
    QPixmap,
    QPushButton,
    QVBoxLayout,
    Qt,
)

from . import share
from .inbox import error_text
from .language import tr
from .ui import _enum

state = {"popup": None}


def art_path(companion):
    name = share.art_file(companion)
    return str(Path(__file__).with_name(name)) if name else None


def _font(pixels, bold):
    font = QFont()
    font.setPixelSize(pixels)
    font.setBold(bold)
    return font


def picture(png):
    """The PNG bytes as a QImage; None when Qt cannot read them."""
    image = QImage()
    return image if image.loadFromData(png, "PNG") else None


def copy_card(found):
    """Puts the server's card and its invitation on the clipboard together."""
    image = picture(found["png"])
    if image is None:
        return tr("Could not read the picture.")
    data = QMimeData()
    data.setImageData(image)
    data.setText(found["text"])
    QApplication.clipboard().setMimeData(data)
    return tr("The picture and an invitation are on the clipboard.")


def save_card(path, found):
    """Writes the server's card to `path`, with its invitation on the clipboard."""
    try:
        Path(path).write_bytes(found["png"])
    except OSError:
        return tr("Could not save the picture.")
    QApplication.clipboard().setText(found["text"])
    return tr("Picture saved. An invitation is on the clipboard.")


def ask_path(parent):
    path, _ = QFileDialog.getSaveFileName(parent, tr("Save image…"), "ankiquest-win.png", "PNG (*.png)")
    if path and not path.lower().endswith(".png"):
        path += ".png"
    return path or None


def copy_text(win, server):
    QApplication.clipboard().setText(share.invitation(win, server, tr))
    return tr("An invitation is on the clipboard.")


def same_account(api, current):
    return [api.base, api.user, api.token] == [current.base, current.user, current.token]


def _plain(text, bold=False):
    label = QLabel(text)
    label.setTextFormat(_enum(Qt, "TextFormat", "PlainText"))
    label.setWordWrap(True)
    if bold:
        label.setFont(_font(15, True))
    return label


def _head(win, companion):
    head = QHBoxLayout()
    asset = art_path(companion)
    if asset:
        image = QLabel()
        image.setPixmap(QPixmap(asset).scaled(
            56, 56,
            _enum(Qt, "AspectRatioMode", "KeepAspectRatio"),
            _enum(Qt, "TransformationMode", "SmoothTransformation"),
        ))
        head.addWidget(image)
    words = QVBoxLayout()
    words.addWidget(_plain(win["title"], True))
    words.addWidget(_plain(win["body"]))
    head.addLayout(words, 1)
    return head


class Sharing:
    """What the share window's buttons do. The card is fetched once, in the background, and kept."""

    def __init__(self, dialog, api, win, current_client, run, status):
        self.dialog, self.api, self.win = dialog, api, win
        self.current_client, self.run, self.status = current_client, run, status
        self.found = None
        self.waiting = []

    def owned(self):
        if same_account(self.api, self.current_client()):
            return True
        self.status.setText(tr("This win belongs to an AnkiQuest account that is no longer signed in."))
        return False

    def to_friends(self, button):
        if not self.owned():
            return
        button.setEnabled(False)
        self.status.setText(tr("Sharing…"))

        def sent(future):
            try:
                recipients = future.result()
            except Exception as error:
                button.setEnabled(True)
                self.status.setText(tr("Couldn't share it (%s).") % error_text(error))
                return
            self.status.setText(share.sent_text(recipients, tr))

        self.run(lambda: share.send(self.api, self.win), sent)

    def with_card(self, then):
        """Runs `then(found)` once the card is here, fetching it first if needed."""
        if self.found is not None:
            then(self.found)
            return
        if not self.owned():
            return
        self.waiting.append(then)
        if len(self.waiting) > 1:
            return
        self.status.setText(tr("Getting the picture…"))
        self.run(lambda: share.card(self.api, self.win), self.fetched)

    def fetched(self, future):
        waiting, self.waiting = self.waiting, []
        try:
            self.found = future.result()
        except Exception as error:
            self.status.setText(tr("Could not get the picture (%s).") % error_text(error))
            return
        for action in waiting:
            action(self.found)

    def to_clipboard(self):
        self.with_card(lambda found: self.status.setText(copy_card(found)))

    def to_file(self):
        path = ask_path(self.dialog)
        if path:
            self.with_card(lambda found: self.status.setText(save_card(path, found)))

    def to_text(self):
        self.status.setText(copy_text(self.win, self.api.base))


def _buttons(sharing):
    friends = QPushButton(tr("Share with friends"))
    friends.clicked.connect(lambda: sharing.to_friends(friends))
    if not share.has_card(sharing.win):
        text = QPushButton(tr("Copy text"))
        text.clicked.connect(sharing.to_text)
        return [friends, text]
    copy = QPushButton(tr("Copy as image"))
    save = QPushButton(tr("Save image…"))
    copy.clicked.connect(sharing.to_clipboard)
    save.clicked.connect(sharing.to_file)
    return [friends, copy, save]


def win_dialog(parent, api, win, companion, current_client, run):
    """A small non-modal window offering the win to friends or as a picture.
    `current_client()` is the signed-in account now; `run` is mw.taskman.run_in_background."""
    dialog = QDialog(parent)
    dialog.setWindowTitle(tr("Share this win"))
    dialog.setAttribute(_enum(Qt, "WidgetAttribute", "WA_DeleteOnClose"))
    layout = QVBoxLayout(dialog)
    layout.addLayout(_head(win, companion))
    status = _plain("")
    layout.addWidget(status)
    sharing = Sharing(dialog, api, win, current_client, run, status)
    close = QPushButton(tr("Close"))
    close.clicked.connect(dialog.close)
    row = QHBoxLayout()
    for button in _buttons(sharing):
        row.addWidget(button)
    row.addStretch(1)
    row.addWidget(close)
    layout.addLayout(row)
    dialog.sharing = sharing
    dialog.resize(520, dialog.sizeHint().height())
    return dialog


def _corner(dialog, parent):
    """Bottom right of Anki's window, out of the way of the card being studied."""
    if parent is None:
        return
    frame = parent.frameGeometry()
    size = dialog.sizeHint()
    dialog.move(frame.right() - size.width() - 40, frame.bottom() - size.height() - 60)


def offer(parent, api, win, companion, current_client, run):
    """Shows a just-announced win without taking focus; a newer win replaces the one showing."""
    previous = state["popup"]
    if previous is not None:
        try:
            previous.close()
        except RuntimeError:
            pass
    dialog = win_dialog(parent, api, win, companion, current_client, run)
    dialog.setAttribute(_enum(Qt, "WidgetAttribute", "WA_ShowWithoutActivating"))
    dialog.finished.connect(lambda *_: state.update(popup=None) if state["popup"] is dialog else None)
    state["popup"] = dialog
    _corner(dialog, parent)
    dialog.show()
    return dialog
