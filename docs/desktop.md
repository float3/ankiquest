# Desktop compatibility and packages

AnkiQuest supports Anki 23.10 and newer with Qt 6. CI pins the oldest supported
release (23.10, Python 3.9) and the current release (26.09.4, Python 3.12).
The manifest keeps `min_point_version: 231000`; there is no known incompatible
newer release that needs a maximum version.

## Run the checks

Use a separate virtual environment for each Anki version:

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install -r tests/requirements-anki-26.9.4.txt
python -m pyflakes addon
QT_QPA_PLATFORM=offscreen python -m unittest discover -s tests -v
python .github/scripts/package-addon.py --output ankiquest.ankiaddon
```

On Windows, activate `.venv\Scripts\Activate.ps1` and set
`$env:QT_QPA_PLATFORM = "offscreen"` before running the tests. For the oldest
version, create another environment with `tests/requirements-anki-23.10.txt`.
The current `aqt[qt]` extra pins its matching Qt, WebEngine and SIP packages;
the older requirements explicitly pin those packages. Do not install an
unbounded PyQt/WebEngine combination on top of these environments.

The two **Desktop (Anki …)** jobs run independently of Rust. Each checks:

- Every add-on unit test, including real Qt dialogs in separate processes.
- The whole packaged add-on, installed through Anki's real `AddonManager` into
  a temporary profile with a real collection, task manager, progress manager,
  menus and main window. The smoke checks the add-on's `gui_hooks` and direct
  `mw` attribute paths, renders the deck-list entry and calls the scheduler,
  timebox and timer APIs. It does not start Chromium or claim to test a full
  interactive Anki session.
- Recorded server responses in `test_desktop_contract.py`, plus the activity,
  board and social fixture tests. These exercise profile/upload day verdicts,
  rank/share words, every board shape, notifications, activity, goals, decks,
  capabilities and conquered-card actions. `wins.json` cross-checks the win
  IDs used by notification shares and card-image URLs; desktop does not list
  `/api/wins` itself. Rust regenerates and verifies these same recordings.
- Upload/reconcile/refresh regressions in `test_deck_completion.py` and
  `test_addon_uploads.py`; feedback, conquest paths and web navigation in
  `test_desktop_board.py`; clipboard text in `test_addon_sharing.py` and
  `qt_moments.py`.

The **Add-on archive** job publishes `ankiquest-addon` as a downloadable PR
artifact. PR and release builds use the same packager. Archives omit local
`meta.json`, caches, tests and hidden files, and include `version.json` and a
manifest `human_version`. The latter is verified through Anki's installed
metadata, and Settings shows the same version. A source checkout says
**Local copy**. Stable releases still require an owner-created `addon-N` tag;
a PR artifact does not publish a stable release.

When Anki releases a new version, update the current requirements file and CI
matrix together, keeping the oldest job until its support is deliberately
changed. Verify the release's own Qt extra, both full test jobs and a normal
interactive study session before claiming support. Add an upper version
bound only for a reproduced incompatibility.

## API audit

Checked against the installed 23.10 and 26.09.4 APIs:

| API | Finding |
| --- | --- |
| `QNetworkCookie` | Neither `aqt.qt` exports it. Keep the direct `PyQt6.QtNetwork` import; Qt 6 is already required. The whole-add-on smoke imports it on both versions. |
| `mw.progress.timer` | Both accept `repeat` and `parent`. The smoke calls that signature; the old `TypeError` fallbacks are removed. |
| `mw.col.startTimebox()` | Both expose this collection API; the smoke calls it on a real collection. |
| `col.get_config("rollover", 4)` | Both map this legacy key to scheduling preferences. The smoke sets a non-default rollover of 7 through `set_preferences()` and reads 7 through this call. |
| `gui_hooks` / `mw` | The smoke checks names collected from the add-on source against real objects. Timers and collection behavior have additional regression coverage. |

## Update ownership and AnkiWeb

Anki identifies AnkiWeb installations by their numeric add-on directory. Such
installations never query or install a GitHub update, even when packaged
`version.json` is present. Settings replaces the GitHub channel picker with
**Updates are managed by AnkiWeb**. The Tools update command directs the user
to Anki's **Tools → Add-ons → Check for Updates**.

GitHub packages retain the stable/nightly choice and daily checks. Local
source copies do not self-update. Before publishing, use the
[AnkiWeb listing draft](ankiweb-listing.md), supply the owner account and
listing ID, and test installation from that listing. Do not install both
copies together: Anki's numeric AnkiWeb directory and the GitHub package
directory are different. Once the listing ID exists, add the corresponding
package conflicts and test migration before recommending a channel switch.

References: [Anki add-on sharing](https://docs.ankiweb.net/addons/sharing.html),
[Anki 26.09.4](https://github.com/ankitects/anki/releases/tag/26.09.4),
[aqt package metadata](https://pypi.org/project/aqt/26.9.4/).
