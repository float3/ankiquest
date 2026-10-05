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


def welcome(translate=lambda value: value):
    """Shown under the deck list until the add-on is connected to a server."""
    return (
        "<div id=ankiquest style='max-width:600px;margin:2em auto 0;font-size:13px;text-align:center'>"
        "<b>ankiquest</b> <a href=# onclick=\"pycmd('ankiquest:account');return false\">%s</a></div>"
        % escape(translate("Sign in or create an account"))
    )


def html(profile, place=None, unread=0, translate=lambda value: value, mascot=None):
    """A block for `deck_browser_will_render_content`, so it lands under the stats."""
    parts = []
    if place:
        parts.append(translate("#%d this week") % place)
    if profile:
        parts.append(
            translate("Lv %d  %s/%s XP")
            % (
                profile.get("level", 1),
                "{:,}".format(profile.get("xp_into_level", 0)),
                "{:,}".format(profile.get("xp_for_next", 0)),
            )
        )
        if profile.get("streak"):
            parts.append(translate("\U0001f525 %d day streak") % profile["streak"])
        if profile.get("at_risk"):
            parts.append(translate("streak at risk today"))
    links = link("board", translate("Leaderboard")) + link("challenges", translate("Friends"))
    if unread:
        links += link("inbox", translate("\U0001f4ec %d new") % unread)
    mascot_html = (
        '<img src="%s" width=40 height=40 alt="" style="object-fit:contain">'
        % escape(mascot)
        if mascot else ""
    )
    return (
        "<div id=ankiquest style='max-width:600px;margin:2em auto 0;font-size:13px;"
        "display:flex;flex-wrap:wrap;justify-content:space-between;gap:.3em'>"
        "<div style='display:flex;align-items:center;gap:.5em'>%s<div><b>ankiquest</b> <span style='opacity:.7'>%s</span></div></div><div>%s</div></div>"
        % (mascot_html, escape("  ·  ".join(parts)), links)
    )
