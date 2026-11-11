"""The conquest controls live inside the shared moments card."""

import time
from http import HTTPStatus

from aqt.qt import QApplication, QHBoxLayout, QLineEdit, QPushButton, QTimer, QVBoxLayout, QWidget

from . import conquests, inbox, share, share_ui
from .language import tr


class _Conquest:
    def __init__(self, card, at):
        self._card, self._api, self._at = card, card._api, at
        self._found, self._deck = None, ""
        self._suggestion, self._busy = "", False
        self._widget = QWidget()
        layout = QVBoxLayout(self._widget)
        layout.setContentsMargins(0, 0, 0, 0)
        self._stats = share_ui._plain(tr("Loading card…"))
        layout.addWidget(self._stats)
        layout.addWidget(share_ui._plain(tr("Card label")))
        self._label = QLineEdit()
        self._label.setMaxLength(120)
        self._label.setPlaceholderText(tr("Choose what to share about this card"))
        layout.addWidget(self._label)
        layout.addWidget(share_ui._plain(tr("Your suggestion stays on this computer until you save it.")))
        edits = QHBoxLayout()
        self._save = self._button(tr("Save label"), lambda: self._save_label(self._label.text()))
        self._remove = self._button(tr("Remove label"), lambda: self._save_label(""))
        edits.addWidget(self._save)
        edits.addWidget(self._remove)
        layout.addLayout(edits)
        row = QHBoxLayout()
        self._tell = self._button(tr("Tell friends"), self._send)
        self._undo = self._button(tr("Undo"), self._retract)
        row.addWidget(self._tell)
        row.addWidget(self._undo)
        layout.addLayout(row)
        self._share = self._button(tr("Share"), lambda: self._options.setVisible(self._share.isChecked()))
        self._share.setCheckable(True)
        layout.addWidget(self._share)
        self._options = QWidget()
        options = QVBoxLayout(self._options)
        options.setContentsMargins(0, 0, 0, 0)
        self._copy = self._button(tr("Copy text"), self._copy_text)
        self._image = self._button(tr("Copy as image"), lambda: self._picture(False))
        self._file = self._button(tr("Save image…"), lambda: self._picture(True))
        self._link = self._button(tr("Copy public link"), self._public_link)
        self._link.setToolTip(tr("Makes a public page for this card. Anyone with the link can see it."))
        self._retry = self._button(tr("Check for picture"), self._load)
        for button in (self._copy, self._image, self._file, self._link, self._retry):
            options.addWidget(button)
        layout.addWidget(self._options)
        self._options.hide()
        self._timer = QTimer(self._widget)
        self._timer.timeout.connect(self._buttons)
        self._timer.start(1000)
        self._buttons()

    def _button(self, text, action):
        button = QPushButton(text)
        button.clicked.connect(action)
        return button

    def _buttons(self):
        ready = self._found is not None and not self._busy
        for button in (self._save, self._share, self._copy):
            button.setEnabled(ready)
        self._remove.setEnabled(ready and bool(self._found.get("label")))
        self._tell.setEnabled(ready and not self._found.get("told"))
        self._undo.setVisible(bool(self._found and self._found.get("told")))
        self._undo.setEnabled(ready and conquests._can_undo(self._found, int(time.time() * 1000)))
        has_picture = bool(ready and share.win_id(self._found))
        for button in (self._image, self._file, self._link):
            button.setEnabled(has_picture)
        self._retry.setVisible(not has_picture)
        self._retry.setEnabled(not self._busy)

    def _load(self):
        if self._busy:
            return
        self._busy = True
        self._buttons()
        col = self._card._addon.mw.col

        def local(future):
            self._suggestion, self._deck = future.result()
            if not self._label.isModified() and not (self._found or {}).get("label"):
                self._label.setText(self._suggestion)

        self._card._run(lambda: conquests._local(col, self._at), local, uses_collection=True)
        self._card._run(lambda: conquests._load(self._api, self._at), self._loaded)

    def _loaded(self, future):
        self._busy = False
        try:
            self._found = future.result()
            self._stats.setText(conquests._stats(self._found, tr))
            if not self._label.isModified():
                self._label.setText(self._found.get("label") or self._suggestion)
        except Exception as error:
            self._card._status.setText(tr("Could not load the card (%s).") % inbox.error_text(error))
            self._stats.setText(tr("Card unavailable"))
            self._options.show()
        self._buttons()

    def _change(self, work, message, then=None):
        if self._busy or not self._card._owned():
            return
        self._busy = True
        self._buttons()

        def changed(future):
            self._busy = False
            try:
                result = future.result()
            except Exception as error:
                if getattr(error, "status", None) == HTTPStatus.CONFLICT:
                    self._found["undo_until"] = None
                    self._card._status.setText(tr("It is too late to undo this share."))
                else:
                    self._card._status.setText(tr("Could not update the card (%s).") % inbox.error_text(error))
                self._buttons()
                return
            if then:
                then()
            self._card._status.setText(message(result))
            self._load()

        self._card._run(work, changed)

    def _save_label(self, text):
        self._change(lambda: conquests._save(self._api, self._at, text),
                     lambda _result: tr("Label saved.") if text.strip() else tr("Label removed."),
                     lambda: self._label.setModified(False))

    def _send(self):
        self._change(lambda: conquests._tell(self._api, self._at), lambda result: share.sent_text(result.get("told", 0), tr))

    def _retract(self):
        if conquests._can_undo(self._found or {}, int(time.time() * 1000)):
            self._change(lambda: conquests._undo(self._api, self._at), lambda _result: tr("Share taken back where possible."))

    def _win(self):
        return conquests._win(self._api, self._at, self._found, self._deck, tr)

    def _copy_text(self):
        self._card._status.setText(share_ui.copy_text(self._win(), self._api.base))

    def _picture(self, save):
        sharing = share_ui.Sharing(self._card._dialog, self._api, self._win(), self._card._addon.client,
                                   self._card._run, self._card._status)
        sharing.to_file() if save else sharing.to_clipboard()

    def _public_link(self):
        if self._busy or not self._card._owned():
            return
        self._busy = True
        self._buttons()
        win = self._win()

        def linked(future):
            self._busy = False
            try:
                found = future.result()
                QApplication.clipboard().setText(share.invitation(win, self._api.base, tr) + "\n" + found["url"])
                self._card._status.setText(tr("A public link is on the clipboard. Anyone with it can see this card."))
            except Exception as error:
                self._card._status.setText(tr("Couldn't make a link (%s).") % inbox.error_text(error))
            self._buttons()

        self._card._run(lambda: share.link(self._api, win), linked)
