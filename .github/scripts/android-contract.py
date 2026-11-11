"""Reject removal or type changes in API examples consumed by Android."""
import argparse
from collections import defaultdict
import json
from pathlib import Path
import re
import sys

_CONSUMED = ("consumed", "boards", "challenges")


def _kind(value):
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, (int, float)):
        return "number"
    if isinstance(value, str):
        return "string"
    return "array" if isinstance(value, list) else "object"


def _paths(value, path=(), result=None):
    if result is None:
        result = defaultdict(set)
    result[path].add(_kind(value))
    if isinstance(value, dict):
        for key, child in value.items():
            _paths(child, path + (key,), result)
    elif isinstance(value, list):
        for child in value:
            _paths(child, path + (None,), result)
    return result


def _where(path):
    return "$" + "".join("[*]" if key is None else "[" + json.dumps(key) + "]" for key in path)


def _changes(before, after):
    old, new = _paths(before), _paths(after)
    errors = []
    for path, kinds in old.items():
        if path not in new:
            errors.append("%s was removed" % _where(path))
        elif kinds - new[path]:
            errors.append("%s lost JSON type(s) %s; now %s" % (
                _where(path), ", ".join(sorted(kinds - new[path])), ", ".join(sorted(new[path]))))
    return errors


def _names(file):
    catalog = json.loads(file.read_text("utf-8"))
    if not isinstance(catalog, dict) or "consumed" not in catalog:
        raise ValueError("names.json must contain a consumed array")
    result = set()
    for group in _CONSUMED:
        names = catalog.get(group, [])
        if not isinstance(names, list) or any(
            not isinstance(name, str) or not re.fullmatch(r"[a-z][a-z0-9_]*", name) or name == "names"
            for name in names
        ):
            raise ValueError("names.json has an invalid %s array" % group)
        result.update(names)
    return sorted(result)


def _check(server, resources):
    examples = resources / "ankiquest-api"
    names = examples / "names.json"
    if not names.exists():
        return None
    pin = (resources / "ankiquest-server.sha").read_text("utf-8").strip()
    if not re.fullmatch(r"[0-9a-f]{40}", pin):
        raise ValueError("Android's ankiquest-server.sha must contain a full commit SHA")
    errors = []
    for name in _names(names):
        old, new = examples / (name + ".json"), server / (name + ".json")
        if not old.is_file():
            errors.append("Android consumes %s but its vendored example is missing" % name)
        elif not new.is_file():
            errors.append("Consumed example %s.json was removed" % name)
        else:
            errors += ["%s.json: %s" % (name, change) for change in _changes(
                json.loads(old.read_text("utf-8")), json.loads(new.read_text("utf-8")))]
    return errors


def _main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server-examples", type=Path, default=Path("docs/api/examples"))
    parser.add_argument("--android-resources", type=Path, required=True)
    args = parser.parse_args()
    try:
        errors = _check(args.server_examples, args.android_resources)
    except (OSError, ValueError) as error:
        print("Android API compatibility check failed: %s" % error, file=sys.stderr)
        return 1
    if errors is None:
        print("::notice::Android names.json is not published yet; compatibility guard awaits Android #203.")
        return 0
    if errors:
        print("Android's consumed API examples are no longer compatible:", file=sys.stderr)
        for error in errors:
            print("- " + error, file=sys.stderr)
        return 1
    print("Android's consumed example names, key paths and JSON types are preserved.")
    return 0


if __name__ == "__main__":
    sys.exit(_main())
