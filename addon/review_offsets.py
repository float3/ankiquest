"""The UTC offset each review answered on this computer was made on.

An upload's `clock` says where the player is when it is sent. A review answered after a
time-zone change but uploaded later (Anki was offline, or a daylight-saving change fell
in between) would otherwise be read on the old clock. The server takes an optional
`offset_west_min` on each review row for that ("review_offsets" in /api/capabilities;
older servers ignore it), so the add-on remembers the offset of every answer it sees,
keyed by revlog id, and stamps only those rows. Rows that reached the collection through
AnkiWeb from another device are never in the journal and go up unstamped.

The journal lives in the Anki profile (not the collection, so it never syncs), and an
entry is dropped once its review has been uploaded, or after KEEP_MS in any case."""

from datetime import datetime

JOURNAL_KEY = "ankiquestReviewOffsets"
KEEP_MS = 30 * 86_400_000
# The revlog row of an answer is written just before the reviewer hook runs; anything
# older than this is not the answer the hook is about.
ANSWER_WINDOW_MS = 10 * 60_000


def offset_at(at_ms):
    """Minutes west of UTC on this computer at `at_ms`, like the upload's clock."""
    offset = datetime.fromtimestamp(at_ms / 1000).astimezone().utcoffset()
    return -int(offset.total_seconds() // 60) if offset else 0


def journal(profile):
    """The profile's journal as {revlog id: offset_west_min}, ignoring anything malformed."""
    entries = profile.get(JOURNAL_KEY) if profile is not None else None
    if not isinstance(entries, dict):
        return {}
    return {int(k): int(v) for k, v in entries.items() if str(k).isdigit() and isinstance(v, int)}


def record(profile, revlog_id, now_ms, offset=None):
    """Remembers the offset the answer `revlog_id` was made on. A row that is not a fresh
    answer (none found, or older than ANSWER_WINDOW_MS) is left alone."""
    if profile is None or not revlog_id or not now_ms - ANSWER_WINDOW_MS <= revlog_id <= now_ms + ANSWER_WINDOW_MS:
        return
    entries = journal(profile)
    entries[int(revlog_id)] = offset_at(revlog_id) if offset is None else offset
    profile[JOURNAL_KEY] = entries


def stamp(reviews, entries):
    """`reviews` (upload rows) with `offset_west_min` on the rows found in `entries`."""
    for review in reviews:
        if review["id"] in entries:
            review["offset_west_min"] = entries[review["id"]]
    return reviews


def prune(profile, uploaded_through, now_ms):
    """Drops the entries of reviews uploaded through `uploaded_through`, and any older than KEEP_MS."""
    if profile is None:
        return
    entries = journal(profile)
    kept = {k: v for k, v in entries.items() if k > uploaded_through and k > now_ms - KEEP_MS}
    if kept or JOURNAL_KEY in profile:
        profile[JOURNAL_KEY] = kept
