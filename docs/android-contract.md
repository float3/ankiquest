# Keeping released Android clients compatible

The `android-contract` CI job compares this server's API examples with the
examples consumed by `float3/AnkiQuest-Android@main`. It makes a sparse public
checkout without a personal access token and runs a plain Python guard. It
does not run Android code or Gradle.

Android owns the baseline in `AnkiDroid/src/test/resources/`:

- `ankiquest-server.sha` identifies the full server commit that supplied the
  vendored data.
- `ankiquest-api/names.json` lists response names in arrays called `consumed`,
  `boards`, `challenges`, `unused` and `not_yet`. The first three categories
  count as consumed; the last two do not.
- `ankiquest-api/<name>.json` contains that commit's recorded responses.

The companion Android work is [Android #203](https://github.com/float3/AnkiQuest-Android/issues/203).
It verifies the vendored files against the pinned commit and proposes newer
data through a bump PR. Ordinary Android PRs and pushes test their committed
data instead of silently taking new server data.

The server guard fails if a consumed example disappears, a JSON key path is
removed, or a type seen at that path is lost. Array elements share a `[*]`
path, preserving the types and keys represented anywhere in the baseline's
array. Booleans are distinct from numbers; integer and decimal numbers are
the same JSON type. Values, new keys, new array variants and new examples
are allowed. An empty array cannot replace a baseline that exercises member
fields without losing that coverage.

This is a guard for recorded response shapes, not a complete API schema or
a replacement for the real client contract tests. Preserve behavior for
released apps too: a compatible recording cannot prove that every response
or optional-field combination is safe.

Until Android publishes `names.json`, the job emits a notice and succeeds,
as the transition in #230 requires. Once the catalog exists, malformed
catalogs, pins and missing vendored examples fail instead of silently
disabling the check.

Run the guard locally after checking out Android:

```sh
python3 -m unittest discover -s tests -p test_android_contract.py -v
python3 .github/scripts/android-contract.py \
  --android-resources ../AnkiQuest-Android/AnkiDroid/src/test/resources
```

Server PRs changing examples name the Android follow-up or say **picked up
by bump PR**. To retire a consumed example, first land the Android change
that stops reading it and updates the catalog, then wait until released
apps no longer need it before removing the server behavior and recording.

The repository owner must make **android-contract** a required check on
`master`, require PRs and apply those rules to administrators too. A workflow
file does not change branch protection. This is also tracked in the owner
actions of Android #203.
