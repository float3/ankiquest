"""Build the same clean archive for PR checks, GitHub releases and AnkiWeb."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import subprocess
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

_EXCLUDED = {"__pycache__", "tests", "meta.json", "version.json", "manifest.json"}
_FORMATS = {".py", ".json", ".png", ".md", ".js", ".html", ".css", ".svg", ".wasm"}
_ANKIWEB_ID = "414948448"


def _git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()


def _version(repo, ref):
    commit = _git(repo, "rev-parse", "HEAD")
    tag = ref.removeprefix("refs/tags/") if ref.startswith("refs/tags/") else _git(repo, "describe", "--tags", "--match", "addon-[0-9]*", "--abbrev=0", "HEAD")
    if not re.fullmatch(r"addon-[1-9][0-9]*", tag):
        raise ValueError("Expected an addon-<number> release tag")
    nightly = 0 if ref.startswith("refs/tags/") else int(_git(repo, "rev-list", "--count", "--first-parent", tag + "..HEAD"))
    return {"release": int(tag[6:]), "nightly": nightly, "commit": commit}


def _label(version):
    label = "addon-%d" % version["release"]
    if version["nightly"]:
        label += " nightly %d (%s)" % (version["nightly"], version["commit"][:7])
    return label


def _build(source, output, version, *, ankiweb=False):
    manifest = json.loads((source / "manifest.json").read_text("utf-8"))
    manifest.update(name="AnkiQuest", human_version=_label(version))
    github_package = manifest["package"]
    # Each channel conflicts with the other, never itself on a later update.
    manifest.update(package=_ANKIWEB_ID if ankiweb else github_package,
                    conflicts=[github_package] if ankiweb else [_ANKIWEB_ID])
    files = {}
    for file in source.rglob("*"):
        relative = file.relative_to(source)
        if file.is_symlink():
            raise ValueError("An add-on archive cannot include symbolic links")
        if not file.is_file() or any(part in _EXCLUDED or part.startswith(".") for part in relative.parts):
            continue
        if file.suffix in _FORMATS:
            files[relative.as_posix()] = file.read_bytes()
    files["manifest.json"] = json.dumps(manifest, indent=2).encode("utf-8")
    files["version.json"] = json.dumps(version, indent=2).encode("utf-8")
    if "__init__.py" not in files:
        raise ValueError("Missing add-on entry point")
    with ZipFile(output, "w", ZIP_DEFLATED) as archive:
        for name, content in sorted(files.items()):
            info = ZipInfo(name)
            info.compress_type = ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            archive.writestr(info, content)


def _main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ref", default="")
    parser.add_argument("--metadata", type=Path)
    parser.add_argument("--ankiweb", action="store_true", help="Build for the published AnkiWeb listing")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    version = _version(repo, args.ref)
    _build(repo / "addon", args.output, version, ankiweb=args.ankiweb)
    if args.metadata:
        metadata = dict(version, built_at=datetime.now(timezone.utc).isoformat())
        args.metadata.write_text(json.dumps(metadata, indent=2) + "\n", "utf-8")
    print(json.dumps(version))


if __name__ == "__main__":
    _main()
