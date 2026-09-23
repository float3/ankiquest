"""One line under the deck list; the leaderboard itself opens from the website."""

PAGES = {
    "board": "/week",
    "profile": "/#{user}",
    "inbox": "/community#activity",
    "challenges": "/community#challenges",
}


def escape(text):
    return (
        str(text)
        .replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
    )


def link(page, text):
    return (
        "<a href=# style='margin-left:.8em' onclick=\"pycmd('ankiquest:web:%s');return false\">%s</a>"
        % (page, escape(text))
    )


def html(profile, place=None, unread=0):
    """A block for `deck_browser_will_render_content`, so it lands under the stats."""
    parts = []
    if place:
        parts.append("#%d this week" % place)
    if profile:
        parts.append(
            "Lv %d  %s/%s XP"
            % (
                profile.get("level", 1),
                "{:,}".format(profile.get("xp_into_level", 0)),
                "{:,}".format(profile.get("xp_for_next", 0)),
            )
        )
        if profile.get("streak"):
            parts.append("\U0001f525 %d day streak" % profile["streak"])
        if profile.get("at_risk"):
            parts.append("streak at risk today")
    links = link("board", "Leaderboard") + link("challenges", "Friends")
    if unread:
        links += link("inbox", "\U0001f4ec %d new" % unread)
    return (
        "<div id=ankiquest style='max-width:600px;margin:2em auto 0;font-size:13px;"
        "display:flex;flex-wrap:wrap;justify-content:space-between;gap:.3em'>"
        "<div><b>ankiquest</b> <span style='opacity:.7'>%s</span></div><div>%s</div></div>"
        % (escape("  ·  ".join(parts)), links)
    )
