"""Read daily deck progress without changing the selected deck or study queue."""

DAY_MS = 86_400_000


def study_day(now_ms, offset_west_min, rollover_hour):
    return (now_ms - offset_west_min * 60_000 - rollover_hour * 3_600_000) // DAY_MS


def deck_snapshots(col, now_ms, offset_west_min, rollover_hour, only=None):
    """Call on Anki's serialized collection worker, together with review reads.

    The scheduler supplies daily-limit-aware counts, including subdecks. Its
    learning count only looks ahead a short time, so also include steps due
    later today. Cards in filtered decks belong to their original decks.
    """
    day = study_day(now_ms, offset_west_min, rollover_hour)
    start = day * DAY_MS + offset_west_min * 60_000 + rollover_hour * 3_600_000
    end = start + DAY_MS
    names = col.decks.all_names_and_ids(include_filtered=False)
    tree = col.sched.deck_due_tree()
    if tree is None:
        raise ValueError("Deck progress is unavailable")

    nodes = {}
    tree_intraday = {}

    def visit(node):
        intraday = node.intraday_learning + sum(visit(child) for child in node.children)
        nodes[node.deck_id] = node
        tree_intraday[node.deck_id] = intraday
        return intraday

    visit(tree)
    reviewed = dict(col.db.all(
        "select case when c.odid != 0 then c.odid else c.did end, count(*) "
        "from revlog r join cards c on c.id = r.cid "
        "where r.id >= ? and r.id < ? and r.ease > 0 and r.type < 4 group by 1",
        start, end,
    ))
    learning = dict(col.db.all(
        "select did, count(*) from cards "
        "where odid = 0 and queue in (1, 4) and due < ? group by did",
        end // 1000,
    ))
    filtered = dict(col.db.all(
        "select odid, count(*) from cards where odid != 0 and "
        "(queue = 0 or (queue in (1, 4) and due < ?) "
        "or (queue in (2, 3) and due <= ?)) group by odid",
        end // 1000, col.sched.today,
    ))

    # Roll up SQL counts along logical deck names, not filtered deck placement.
    by_name = {deck.name: deck.id for deck in names}
    totals = {deck.id: [0, 0, 0] for deck in names}
    for deck in names:
        values = [reviewed.get(deck.id, 0), learning.get(deck.id, 0), filtered.get(deck.id, 0)]
        parts = deck.name.split("::")
        for length in range(1, len(parts) + 1):
            ancestor = by_name.get("::".join(parts[:length]))
            if ancestor is not None:
                totals[ancestor] = [a + b for a, b in zip(totals[ancestor], values)]

    snapshots = []
    for deck in names:
        if only is not None and str(deck.id) not in only:
            continue
        reviews, intraday, filtered_due = totals[deck.id]
        node = nodes.get(deck.id)
        # Anki omits an empty Default deck from the tree. Keep its catalog entry
        # conservative instead of turning unavailable counts into completion.
        remaining = 1 if node is None else (
            node.new_count + node.review_count
            + max(0, node.learn_count - tree_intraday[deck.id])
            + intraday + filtered_due
        )
        snapshots.append({
            "id": str(deck.id), "name": deck.name, "remaining": remaining,
            "reviewed_today": reviews, "day": day,
        })
    return snapshots
