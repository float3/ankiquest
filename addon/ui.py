"""The dialogs behind the Tools menu: settings and deck sharing."""

import html

from aqt.qt import (
    QCheckBox,
    QComboBox,
    QDialog,
    QGroupBox,
    QHBoxLayout,
    QIcon,
    QLabel,
    QLineEdit,
    QPushButton,
    QScrollArea,
    QSpinBox,
    QTreeWidget,
    QTreeWidgetItem,
    QVBoxLayout,
    QPixmap,
    QWidget,
)

from aqt.qt import Qt

from .client import DEFAULT_SERVER
from .decks import label, ordered
from .language import tr
from .share import face_path
from .today_ui import settings_section


def _scroller(widgets):
    inner = QWidget()
    layout = QVBoxLayout(inner)
    for widget in widgets:
        layout.addWidget(widget)
    layout.addStretch(1)
    area = QScrollArea()
    area.setWidgetResizable(True)
    area.setWidget(inner)
    return area


def _row(*widgets):
    holder = QWidget()
    layout = QHBoxLayout(holder)
    layout.setContentsMargins(0, 0, 0, 0)
    for widget in widgets:
        layout.addWidget(widget)
    return holder


def _buttons(dialog, accept_text, extra=()):
    row = QHBoxLayout()
    for widget in extra:
        row.addWidget(widget)
    row.addStretch(1)
    cancel = QPushButton(tr("Cancel"))
    cancel.clicked.connect(dialog.reject)
    accept = QPushButton(accept_text)
    accept.setDefault(True)
    accept.clicked.connect(dialog.accept)
    row.addWidget(cancel)
    row.addWidget(accept)
    return row


def _brand(dialog, layout, companion="aki"):
    """Local art works even before a server is configured or while offline."""
    if companion == "none":
        return
    asset = face_path(companion)
    dialog.setWindowIcon(QIcon(asset))
    image = QLabel()
    image.setPixmap(QPixmap(asset).scaled(
        48, 48,
        Qt.AspectRatioMode.KeepAspectRatio,
        Qt.TransformationMode.SmoothTransformation,
    ))
    layout.addWidget(_row(image, QLabel(tr("Your study companion."))))


def account_dialog(parent, url, on_submit, companion="aki", dismiss=None):
    """Signs in or creates an account. `on_submit(values, finish)` talks to the server and calls
    `finish(account, error)`; the dialog stays open with the error until it succeeds."""
    dialog = QDialog(parent)
    dialog.setWindowTitle("AnkiQuest")
    layout = QVBoxLayout(dialog)
    _brand(dialog, layout, companion)

    server = QLineEdit(url or DEFAULT_SERVER)
    user = QLineEdit()
    password = QLineEdit()
    password.setEchoMode(QLineEdit.EchoMode.Password)
    display = QLineEdit()
    display_label = QLabel(tr("Display name (optional)"))
    for title, field in ((tr("Server"), server), (tr("Username"), user), (tr("Password"), password)):
        layout.addWidget(QLabel(title))
        layout.addWidget(field)
    layout.addWidget(display_label)
    layout.addWidget(display)
    new = QCheckBox(tr("Create a new account"))
    layout.addWidget(new)
    hint = QLabel(tr("Usernames use lowercase letters, numbers, - and _. Passwords need at least 10 characters."))
    hint.setWordWrap(True)
    layout.addWidget(hint)
    status = QLabel()
    status.setTextFormat(Qt.TextFormat.PlainText)
    status.setWordWrap(True)
    layout.addWidget(status)

    row = QHBoxLayout()
    row.addStretch(1)
    cancel = QPushButton(dismiss or tr("Cancel"))
    cancel.clicked.connect(dialog.reject)
    submit = QPushButton()
    submit.setDefault(True)
    row.addWidget(cancel)
    row.addWidget(submit)
    layout.addLayout(row)

    def toggle():
        creating = new.isChecked()
        for widget in (display_label, display, hint):
            widget.setVisible(creating)
        submit.setText(tr("Create account") if creating else tr("Sign in"))

    new.toggled.connect(toggle)
    toggle()
    result = {}

    def finish(account, error):
        submit.setEnabled(True)
        if error:
            status.setText(error)
            return
        result.update(account)
        dialog.accept()

    def go():
        values = {
            "url": server.text().strip().rstrip("/"),
            "user": user.text().strip().lower(),
            "password": password.text(),
            "display": display.text().strip(),
            "new": new.isChecked(),
        }
        if not values["url"] or not values["user"] or not values["password"]:
            status.setText(tr("Fill in the server, username and password."))
            return
        submit.setEnabled(False)
        status.setText(tr("Connecting…"))
        on_submit(values, finish)

    submit.clicked.connect(go)
    if not dialog.exec():
        return None
    return result


def _folded(title, layout):
    """Keep occasional controls reachable without making the main dialog longer."""
    toggle = QPushButton(title)
    toggle.setCheckable(True)
    holder = QWidget()
    contents = QVBoxLayout(holder)
    contents.setContentsMargins(0, 0, 0, 0)
    toggle.toggled.connect(holder.setVisible)
    holder.hide()
    layout.addWidget(toggle)
    layout.addWidget(holder)
    return contents


def settings_dialog(
    parent, config, on_test, on_upload_all, companion="aki", on_account=None, *,
    extra=None, on_goal=None, on_manage=None, on_decks=None, on_privacy=None,
):
    """One scrollable dialog. Server sections save only for the account they loaded."""
    dialog = QDialog(parent)
    dialog.setWindowTitle("AnkiQuest")
    screen = dialog.screen().availableGeometry()
    dialog.resize(min(640, screen.width() - 80), min(640, screen.height() - 80))
    layout = QVBoxLayout(dialog)
    _brand(dialog, layout, companion)
    scroll = _scroller([])
    scroll.setObjectName("ankiquest_settings_scroll")
    layout.addWidget(scroll, 1)
    contents = scroll.widget().layout()
    groups = {}
    for key, title in (
        ("account", tr("Account")), ("notify", tr("Notify me about")),
        ("sharing", tr("Sharing")), ("app", tr("App and privacy")),
    ):
        group = QGroupBox(title)
        groups[key] = QVBoxLayout(group)
        contents.insertWidget(contents.count() - 1, group)
    account_layout = groups["account"]

    # Connection edits are staged until Save; server choices keep their original owner.
    url = QLineEdit(config.get("url", ""))
    url.setPlaceholderText(DEFAULT_SERVER)
    user = QLineEdit(config.get("user", ""))
    token = QLineEdit(config.get("token", ""))
    token.setEchoMode(QLineEdit.EchoMode.Password)
    original = tuple(field.text().strip() for field in (url, user, token))
    signed_in = all(original)
    signed_server = url.text().strip() if signed_in else None
    identity = QLabel(tr("Signed in as %s on %s") % (user.text(), signed_server) if signed_in else "")
    identity.setTextFormat(Qt.TextFormat.PlainText)
    identity.setWordWrap(True)
    identity.setVisible(signed_in)
    account_layout.addWidget(identity)
    manage = QPushButton(tr("Manage account on the website…")) if on_manage is not None else None
    sign_out = QPushButton(tr("Sign out"))
    if on_account is not None:
        account = QPushButton(tr("Switch account…") if signed_in else tr("Sign in or create account…"))

        def change_account():
            nonlocal signed_server
            answer = on_account(url.text().strip())
            if not answer:
                return
            url.setText(answer["url"])
            user.setText(answer["user"])
            token.setText(answer["token"])
            signed_server = answer["url"]
            identity.setText(tr("Signed in as %s on %s") % (answer["user"], signed_server))
            identity.show()
            account.setText(tr("Switch account…"))
            sign_out.show()
            if manage is not None:
                manage.show()

        account.clicked.connect(change_account)
        account_layout.addWidget(account)
    if manage is not None:
        manage.clicked.connect(lambda: on_manage(signed_server))
        manage.setVisible(signed_in)
        account_layout.addWidget(manage)

    def forget_account():
        for field in (url, user, token):
            field.clear()
        dialog.accept()

    sign_out.clicked.connect(forget_account)
    sign_out.setVisible(signed_in)
    account_layout.addWidget(sign_out)
    fields = _folded(tr("Manual connection"), account_layout)
    for title, field in ((tr("Server"), url), (tr("Player"), user), (tr("Token"), token)):
        fields.addWidget(QLabel(title))
        fields.addWidget(field)
    data = _folded(tr("Data"), account_layout)
    upload = QPushButton(tr("Upload everything again"))
    upload.clicked.connect(on_upload_all)
    data.addWidget(upload)

    rank = QCheckBox(tr("Tell me when my place on the leaderboard changes"))
    rank.setChecked(bool(config.get("notify_rank", True)))
    groups["notify"].addWidget(rank)
    hours = QSpinBox()
    hours.setRange(0, 12)
    hours.setValue(int(config.get("streak_hours", 2) or 0))
    hours.setSuffix(" h")
    groups["notify"].addWidget(_row(QLabel(tr("Warn me before my streak ends")), hours))

    channel = QComboBox()
    for value, title in (("stable", tr("Stable releases")), ("nightly", tr("Nightly builds (untested)"))):
        channel.addItem(title, value)
    channel.setCurrentIndex(1 if config.get("update_channel") == "nightly" else 0)
    groups["app"].addWidget(_row(QLabel(tr("Update channel")), channel))
    today_values = settings_section(groups["app"], config, notify_layout=groups["notify"])
    if on_privacy is not None:
        privacy = QPushButton(tr("Privacy notice"))
        privacy.clicked.connect(on_privacy)
        groups["app"].addWidget(privacy)

    # Local settings stay editable while account-bound sections wait for a saved connection.
    server_groups = {"data": data}
    server_panels = []
    for key, group_layout in groups.items():
        panel = QWidget()
        server_panels.append(panel)
        server_groups[key] = QVBoxLayout(panel)
        server_groups[key].setContentsMargins(0, 0, 0, 0)
        group_layout.addWidget(panel)
    for key, title, callback in (
        ("sharing", tr("Deck completion notifications"), on_decks),
        ("app", tr("Daily goal…"), on_goal),
    ):
        if callback is not None:
            button = QPushButton(title)
            button.clicked.connect(callback)
            server_groups[key].addWidget(button)
    finish = extra(dialog, server_groups) if extra else None
    account_note = QLabel(tr("Save this account, then reopen settings to load its choices."))
    account_note.setWordWrap(True)
    account_layout.addWidget(account_note)

    def account_changed():
        changed = original != tuple(field.text().strip() for field in (url, user, token))
        for panel in server_panels:
            panel.setEnabled(signed_in and not changed)
        upload.setEnabled(signed_in and not changed)
        account_note.setVisible(changed)

    for field in (url, user, token):
        field.textChanged.connect(account_changed)
    account_changed()
    test = QPushButton(tr("Test connection"))
    test.clicked.connect(lambda: on_test(_values(url, user, token, rank, hours, channel)))
    fields.addWidget(test)
    layout.addLayout(_buttons(dialog, tr("Save")))

    if not dialog.exec():
        return None
    if finish and original == tuple(field.text().strip() for field in (url, user, token)):
        finish()
    return dict(_values(url, user, token, rank, hours, channel), **today_values())


def _values(url, user, token, rank, hours, channel):
    return {
        "update_channel": channel.currentData() or "stable",
        "url": url.text().strip(),
        "user": user.text().strip(),
        "token": token.text().strip(),
        "notify_rank": rank.isChecked(),
        "streak_hours": hours.value(),
    }


def deck_dialog(parent, settings, companion="aki"):
    """A tree of decks and a list of recipients; ticking a deck ticks its subdecks."""
    decks = ordered(settings.get("decks") or [])
    people = settings.get("recipients") or []
    dialog = QDialog(parent)
    dialog.setWindowTitle(tr("Deck completion notifications"))
    dialog.resize(680, 520)
    layout = QVBoxLayout(dialog)
    _brand(dialog, layout, companion)
    layout.addWidget(
        QLabel(tr("The people you pick hear once a day when you finish a shared deck."))
    )

    tree = QTreeWidget()
    tree.setHeaderHidden(True)
    items = []
    for index, deck in enumerate(decks):
        item = QTreeWidgetItem([label(deck)])
        item.setCheckState(0, Qt.CheckState.Checked if deck.get("enabled") else Qt.CheckState.Unchecked)
        item.setToolTip(0, "<p>%s</p>" % html.escape(deck["name"]))
        items.append(item)
        parts = deck["name"].split("::")
        owner = None
        for other in range(index - 1, -1, -1):
            if decks[other]["name"] == "::".join(parts[:-1]):
                owner = items[other]
                break
        if owner is None:
            tree.addTopLevelItem(item)
        else:
            owner.addChild(item)
    tree.expandAll()

    guard = {"busy": False}

    def spread(item, _column):
        """A branch shares one answer, without the change bouncing back up."""
        if guard["busy"]:
            return
        guard["busy"] = True
        state = item.checkState(0)
        stack = [item.child(i) for i in range(item.childCount())]
        while stack:
            child = stack.pop()
            child.setCheckState(0, state)
            stack.extend(child.child(i) for i in range(child.childCount()))
        guard["busy"] = False

    tree.itemChanged.connect(spread)

    chosen = set()
    for deck in decks:
        if deck.get("enabled"):
            chosen.update(deck.get("recipients") or [])
    recipients = {}
    for person in people:
        box = QCheckBox("%s (%s)" % (person.get("display") or person["user"], person["user"]))
        box.setChecked(person["user"] in chosen)
        recipients[person["user"]] = box

    columns = QHBoxLayout()
    columns.addWidget(_titled(tr("Decks"), tree), 2)
    columns.addWidget(
        _titled(tr("Notify"), _scroller(list(recipients.values()) or [QLabel(tr("Nobody else plays yet."))])),
        1,
    )
    layout.addLayout(columns)

    every = QPushButton(tr("All / none"))

    def toggle_all():
        checked = Qt.CheckState.Checked
        select = any(item.checkState(0) != checked for item in items)
        for item in items:
            item.setCheckState(0, Qt.CheckState.Checked if select else Qt.CheckState.Unchecked)

    every.clicked.connect(toggle_all)
    layout.addLayout(_buttons(dialog, tr("Save"), (every,)))

    if not dialog.exec():
        return None
    checked = Qt.CheckState.Checked
    shared = [deck["id"] for deck, item in zip(decks, items) if item.checkState(0) == checked]
    unshared = [deck["id"] for deck, item in zip(decks, items) if item.checkState(0) != checked]
    picked = [user for user, box in recipients.items() if box.isChecked()]
    return shared, unshared, picked, None, None


def _titled(title, widget):
    holder = QWidget()
    layout = QVBoxLayout(holder)
    layout.setContentsMargins(0, 0, 0, 0)
    label = QLabel("<b>%s</b>" % title)
    layout.addWidget(label)
    layout.addWidget(widget)
    return holder
