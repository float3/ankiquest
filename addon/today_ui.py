"""Qt pieces of the Today features: the session recap and the Today settings."""

from aqt.qt import (
    QCheckBox,
    QDialog,
    QHBoxLayout,
    QLabel,
    QPixmap,
    QPushButton,
    QVBoxLayout,
    Qt,
)

from .language import tr

def _enum(owner, group, name):
    """Qt 5 keeps these constants on the class, Qt 6 inside a nested enum."""
    return getattr(getattr(owner, group, owner), name)


def _mascot(path):
    image = QLabel()
    image.setPixmap(QPixmap(path).scaled(
        112, 112,
        _enum(Qt, "AspectRatioMode", "KeepAspectRatio"),
        _enum(Qt, "TransformationMode", "SmoothTransformation"),
    ))
    image.setAlignment(_enum(Qt, "AlignmentFlag", "AlignHCenter"))
    return image


def _button(text, dialog, action=None):
    button = QPushButton(text)

    def clicked():
        dialog.accept()
        if action is not None:
            action()

    button.clicked.connect(clicked)
    return button


def session_dialog(parent, title, text, art, on_continue, on_progress, never_again=None, done_text=None):
    """A small, non-blocking recap with the companion, Continue and progress buttons."""
    dialog = QDialog(parent)
    dialog.setWindowTitle(title)
    dialog.setAttribute(_enum(Qt, "WidgetAttribute", "WA_DeleteOnClose"))
    layout = QVBoxLayout(dialog)
    if art:
        layout.addWidget(_mascot(art))
    heading = QLabel(title)
    heading.setStyleSheet("font-weight:bold;font-size:15px")
    heading.setAlignment(_enum(Qt, "AlignmentFlag", "AlignHCenter"))
    layout.addWidget(heading)
    message = QLabel(text)
    message.setWordWrap(True)
    message.setTextFormat(_enum(Qt, "TextFormat", "PlainText"))
    layout.addWidget(message)
    quiet = None
    if never_again is not None:
        quiet = QCheckBox(tr("Don’t show this again"))
        layout.addWidget(quiet)
    row = QHBoxLayout()
    row.addWidget(_button(tr("View progress"), dialog, on_progress))
    row.addStretch(1)
    finished = _button(done_text or tr("Done"), dialog)
    row.addWidget(finished)
    if on_continue is not None:
        go = _button(tr("Continue studying"), dialog, on_continue)
        go.setDefault(True)
        row.addWidget(go)
    else:
        finished.setDefault(True)
    layout.addLayout(row)
    if quiet is not None:
        dialog.finished.connect(lambda _result: never_again() if quiet.isChecked() else None)
    dialog.setMinimumWidth(360)
    dialog.show()
    return dialog


def settings_section(layout, config, on_goal=None):
    """Appends the Today settings to a settings dialog; returns a function that reads them back.
    `on_goal()` opens the daily goal dialog, which saves on its own."""
    heading = QLabel(tr("Today"))
    heading.setStyleSheet("font-weight:bold;margin-top:8px")
    layout.addWidget(heading)
    if on_goal is not None:
        goal = QPushButton(tr("Daily goal…"))
        goal.clicked.connect(on_goal)
        row = QHBoxLayout()
        row.addWidget(goal)
        row.addStretch(1)
        layout.addLayout(row)
    toggles = {}
    for key, text in (
        ("deck_status", "Mark decks finished today, and learning due later today"),
        ("session_summary", "Show a recap when you leave a study session"),
        ("goal_celebrations", "Celebrate when today counts for your streak and when you reach your daily goal"),
    ):
        toggles[key] = QCheckBox(tr(text))
        toggles[key].setChecked(bool(config.get(key, True)))
        layout.addWidget(toggles[key])

    def values():
        return {key: box.isChecked() for key, box in toggles.items()}

    return values
