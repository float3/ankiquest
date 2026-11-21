"""Finds newer add-on builds on GitHub: `addon-<n>` releases, or the rolling `addon-nightly` pre-release.
Only builds packaged with `--updates github` use it; see `SOURCE`."""

import json
import re
from enum import Enum
from http import HTTPStatus
from pathlib import Path
from typing import NamedTuple

from .links import REPO

RELEASES_URL = "https://api.github.com/repos/%s/releases?per_page=50" % REPO
NIGHTLY_URL = "https://github.com/%s/releases/download/addon-nightly/" % REPO
TAG = re.compile(r"addon-([1-9][0-9]*)")
DAY_MS = 86_400_000


class Build(NamedTuple):
    """The download names of the builds for one minimum Anki (its point version, as in manifest.json)."""

    minimum: int
    package: str
    metadata: str


# Newest minimum first. Raising the add-on's minimum Anki publishes its builds under new names
# only, so copies that cannot run them keep finding the last build for their Anki under the old
# names. Anki's installer replaces the files before it reports an archive incompatible.
BUILDS = (
    Build(241100, "ankiquest-24.11.ankiaddon", "addon-nightly-24.11.json"),
    Build(231000, "ankiquest.ankiaddon", "addon-nightly.json"),
)


def builds(point):
    """The builds Anki `point` can run, newest minimum first."""
    return [build for build in BUILDS if point >= build.minimum]


class UpdateSource(Enum):
    """Where this build looks for newer builds, as the packager wrote it into build.json."""

    GITHUB = "github"
    # AnkiWeb, local copies and builds without --updates github: never check, download or install.
    NONE = "none"


def source(addon_dir):
    """The build's update source; a missing or unknown build.json is NONE."""
    # Anki's AddonMeta.ankiweb_id() uses the numeric installation directory: AnkiWeb updates it,
    # even if a build meant for GitHub was uploaded there.
    if re.fullmatch(r"\d+", Path(addon_dir).name):
        return UpdateSource.NONE
    try:
        settings = json.loads((Path(addon_dir) / "build.json").read_text("utf-8"))
        return UpdateSource(settings["updates"])
    except (OSError, ValueError, TypeError, KeyError):
        return UpdateSource.NONE


# Read once: the build decides, nothing at run time changes it.
SOURCE = source(Path(__file__).parent)


def fetch(url, timeout=20):
    from .client import open_url

    headers = {"Accept": "application/vnd.github+json", "User-Agent": "ankiquest-addon"}
    with open_url(url, headers=headers, timeout=timeout) as response:
        return response.read()


def installed(addon_dir):
    """The packaged version, or None for a local copy, which never updates itself."""
    try:
        version = json.loads((Path(addon_dir) / "version.json").read_text("utf-8"))
    except (OSError, ValueError):
        return None
    release, nightly = version.get("release"), version.get("nightly", 0)
    if not isinstance(release, int) or release <= 0 or not isinstance(nightly, int) or nightly < 0:
        return None
    return {"release": release, "nightly": nightly, "commit": str(version.get("commit", ""))}


def label(version):
    if version is None:
        return "local copy"
    if version["nightly"]:
        return "addon-%d nightly %d (%s)" % (version["release"], version["nightly"], version["commit"][:7])
    return "addon-%d" % version["release"]


def newer(candidate, current):
    return current is not None and candidate is not None and (
        (candidate["release"], candidate["nightly"]) > (current["release"], current["nightly"])
    )


def _package(release, point):
    """The asset of `release` built for the newest Anki that `point` can run, or None."""
    assets = {asset.get("name"): asset for asset in release.get("assets", [])}
    return next((assets[build.package] for build in builds(point) if build.package in assets), None)


def latest_stable(point, get=fetch):
    best = None
    for release in json.loads(get(RELEASES_URL)):
        match = TAG.fullmatch(release.get("tag_name", ""))
        if not match or release.get("draft") or release.get("prerelease"):
            continue
        asset = _package(release, point)
        if asset and (best is None or int(match.group(1)) > best["release"]):
            best = {"release": int(match.group(1)), "nightly": 0, "commit": "", "url": asset["browser_download_url"]}
    return best


def _metadata(build, get):
    """The nightly metadata published for `build`, or None if it has not been published."""
    from .client import ServerError

    try:
        return get(NIGHTLY_URL + build.metadata)
    except ServerError as error:
        if error.status == HTTPStatus.NOT_FOUND:
            return None
        raise


def _nightly_data(point, get):
    """The newest nightly metadata Anki `point` can run, and its build; (None, None) if it runs none.

    A newer build's metadata may not be published yet (404), so the next one serves. The oldest
    build's metadata is always published, so its errors are reported."""
    runnable = builds(point)
    for build in runnable[:-1]:
        data = _metadata(build, get)
        if data is not None:
            return data, build
    if not runnable:
        return None, None
    return get(NIGHTLY_URL + runnable[-1].metadata), runnable[-1]


def latest_nightly(point, get=fetch):
    data, build = _nightly_data(point, get)
    if data is None:
        return None

    version = json.loads(data)
    release, nightly = version.get("release"), version.get("nightly")
    if not isinstance(release, int) or release <= 0 or not isinstance(nightly, int) or nightly < 0:
        return None
    return {"release": release, "nightly": nightly, "commit": str(version.get("commit", "")), "url": NIGHTLY_URL + build.package}


def available(channel, point, get=fetch):
    """The newest build on `channel` that Anki `point` (anki.utils.int_version()) can run."""
    return latest_nightly(point, get) if channel == "nightly" else latest_stable(point, get)


def due(last_checked_ms, now_ms):
    return not last_checked_ms or now_ms - last_checked_ms >= DAY_MS


def download(url, target, get=fetch):
    data = get(url, timeout=120)
    if not data.startswith(b"PK"):
        raise ValueError("the download is not an add-on package")
    Path(target).write_bytes(data)
    return target
