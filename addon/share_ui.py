"""Sharing a win from Anki: with friends through the server, or anywhere as a picture."""

from pathlib import Path

from aqt.qt import (
    QApplication,
    QColor,
    QDialog,
    QFileDialog,
    QFont,
    QFontMetricsF,
    QHBoxLayout,
    QImage,
    QLabel,
    QMimeData,
    QPainter,
    QPixmap,
    QPushButton,
    QRectF,
    QTextOption,
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


def _draw_lines(painter, text, pixels, bold, color, layout, top, max_lines=None):
    """Draws centred wrapped text from `top` and answers where it ended."""
    font = _font(pixels, bold)
    metrics = QFontMetricsF(font)
    height = metrics.lineSpacing()
    fit = share.lines_that_fit(top, height) if max_lines is None else max_lines
    lines = share.wrap(text, layout["width"], metrics.horizontalAdvance, fit)
    painter.setFont(font)
    painter.setPen(QColor(color))
    option = QTextOption(_enum(Qt, "AlignmentFlag", "AlignHCenter"))
    for line in lines:
        painter.drawText(QRectF(layout["left"], top, layout["width"], height), line, option)
        top += height
    return top


def card(win, companion):
    """The 1080×1080 picture: mascot, title, body and the AnkiQuest footer."""
    image = QImage(share.CARD_SIZE, share.CARD_SIZE, _enum(QImage, "Format", "Format_ARGB32"))
    image.fill(QColor(share.COLORS["background"]))
    asset = art_path(companion)
    pixmap = QPixmap(asset) if asset else None
    layout = share.card_layout(bool(pixmap and not pixmap.isNull()))
    painter = QPainter(image)
    painter.setRenderHint(_enum(QPainter, "RenderHint", "Antialiasing"))
    painter.setRenderHint(_enum(QPainter, "RenderHint", "SmoothPixmapTransform"))
    if layout["art"]:
        left, top, side, _ = layout["art"]
        scaled = pixmap.scaled(
            side, side,
            _enum(Qt, "AspectRatioMode", "KeepAspectRatio"),
            _enum(Qt, "TransformationMode", "SmoothTransformation"),
        )
        painter.drawPixmap(left + (side - scaled.width()) // 2, top + (side - scaled.height()) // 2, scaled)
    top = _draw_lines(painter, win["title"], share.TITLE_PX, True, share.COLORS["text"], layout, layout["title_top"])
    _draw_lines(painter, win["body"], share.BODY_PX, False, share.COLORS["muted"], layout, top + share.TITLE_GAP)
    _draw_lines(painter, share.FOOTER, share.FOOTER_PX, True, share.COLORS["primary"], layout, layout["footer_top"], 1)
    painter.end()
    return image


def copy_image(win, companion, server):
    """Puts the card and the invitation on the clipboard together."""
    data = QMimeData()
    data.setImageData(card(win, companion))
    data.setText(share.invitation(win, server, tr))
    QApplication.clipboard().setMimeData(data)
    return tr("The picture and an invitation are on the clipboard.")


def save_image(parent, win, companion, server):
    """Saves the card where the player picks, with the invitation on the clipboard; None if cancelled."""
    path, _ = QFileDialog.getSaveFileName(parent, tr("Save image…"), "ankiquest-win.png", "PNG (*.png)")
    if not path:
        return None
    if not path.lower().endswith(".png"):
        path += ".png"
    if not card(win, companion).save(path, "PNG"):
        return tr("Could not save the picture.")
    QApplication.clipboard().setText(share.invitation(win, server, tr))
    return tr("Picture saved. An invitation is on the clipboard.")


def same_account(api, current):
    return [api.base, api.user, api.token] == [current.base, current.user, current.token]


def _plain(text, bold=False):
    label = QLabel(text)
    label.setTextFormat(_enum(Qt, "TextFormat", "PlainText"))
    label.setWordWrap(True)
    if bold:
        label.setFont(_font(15, True))
    return label


def win_dialog(parent, api, win, companion, current_client, run):
    """A small non-modal window offering the win to friends or as a picture.
    `current_client()` is the signed-in account now; `run` is mw.taskman.run_in_background."""
    dialog = QDialog(parent)
    dialog.setWindowTitle(tr("Share this win"))
    dialog.setAttribute(_enum(Qt, "WidgetAttribute", "WA_DeleteOnClose"))
    layout = QVBoxLayout(dialog)
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
    layout.addLayout(head)
    status = _plain("")
    layout.addWidget(status)

    friends = QPushButton(tr("Share with friends"))
    copy = QPushButton(tr("Copy as image"))
    save = QPushButton(tr("Save image…"))
    close = QPushButton(tr("Close"))
    close.clicked.connect(dialog.close)

    def owned():
        if same_account(api, current_client()):
            return True
        status.setText(tr("This win belongs to an AnkiQuest account that is no longer signed in."))
        return False

    def sent(future):
        try:
            recipients = future.result()
        except Exception as error:
            friends.setEnabled(True)
            status.setText(tr("Couldn't share it (%s).") % error_text(error))
            return
        status.setText(share.sent_text(recipients, tr))

    def to_friends():
        if not owned():
            return
        friends.setEnabled(False)
        status.setText(tr("Sharing…"))
        run(lambda: share.send(api, win), sent)

    def to_clipboard():
        status.setText(copy_image(win, companion, api.base))

    def to_file():
        said = save_image(dialog, win, companion, api.base)
        if said:
            status.setText(said)

    friends.clicked.connect(to_friends)
    copy.clicked.connect(to_clipboard)
    save.clicked.connect(to_file)
    row = QHBoxLayout()
    for button in (friends, copy, save):
        row.addWidget(button)
    row.addStretch(1)
    row.addWidget(close)
    layout.addLayout(row)
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
