"""Finds newer add-on builds on GitHub: `addon-<n>` releases, or the rolling `addon-nightly` pre-release."""

import json
import re
import urllib.request
from pathlib import Path

REPO = "float3/ankiquest"
RELEASES_URL = "https://api.github.com/repos/%s/releases?per_page=50" % REPO
NIGHTLY_URL = "https://github.com/%s/releases/download/addon-nightly/" % REPO
PACKAGE = "ankiquest.ankiaddon"
TAG = re.compile(r"addon-([1-9][0-9]*)")
DAY_MS = 86_400_000


def fetch(url, timeout=20):
    request = urllib.request.Request(url, headers={"Accept": "application/vnd.github+json", "User-Agent": "ankiquest-addon"})
    with urllib.request.urlopen(request, timeout=timeout) as response:
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


def latest_stable(get=fetch):
    best = None
    for release in json.loads(get(RELEASES_URL)):
        match = TAG.fullmatch(release.get("tag_name", ""))
        if not match or release.get("draft") or release.get("prerelease"):
            continue
        asset = next((a for a in release.get("assets", []) if a.get("name") == PACKAGE), None)
        if asset and (best is None or int(match.group(1)) > best["release"]):
            best = {"release": int(match.group(1)), "nightly": 0, "commit": "", "url": asset["browser_download_url"]}
    return best


def latest_nightly(get=fetch):
    version = json.loads(get(NIGHTLY_URL + "addon-nightly.json"))
    release, nightly = version.get("release"), version.get("nightly")
    if not isinstance(release, int) or release <= 0 or not isinstance(nightly, int) or nightly < 0:
        return None
    return {"release": release, "nightly": nightly, "commit": str(version.get("commit", "")), "url": NIGHTLY_URL + PACKAGE}


def available(channel, get=fetch):
    return latest_nightly(get) if channel == "nightly" else latest_stable(get)


def due(last_checked_ms, now_ms):
    return not last_checked_ms or now_ms - last_checked_ms >= DAY_MS


def download(url, target, get=fetch):
    data = get(url, timeout=120)
    if not data.startswith(b"PK"):
        raise ValueError("the download is not an add-on package")
    Path(target).write_bytes(data)
    return target
