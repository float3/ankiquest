"""The ankiquest block under the deck list: a one-line summary, or the Today panel."""

from html import escape

PAGES = {
    "board": "/week",
    "profile": "/#{user}",
    "inbox": "/community#activity",
    "challenges": "/community#challenges",
    "today": "/today",
    "history": "/history",
    "settings": "/settings",
    "reminders": "/community#reminders",
}

PERIOD_LABELS = (
    ("hour", "this hour"),
    ("day", "today"),
    ("week", "this week"),
    ("month", "this month"),
    ("year", "this year"),
    ("all", "all time"),
)
MEDALS = {1: "👑", 2: "🥈", 3: "🥉"}
STATUS_CLASS = {"done": "aq-done-today", "learning_later": "aq-learning-later"}

STYLE = """<style>
#ankiquest.aq-today{max-width:640px;margin:2em auto 0;font-size:13px;text-align:left}
#ankiquest .aq-head{display:flex;align-items:center;gap:.8em;margin-bottom:.6em}
#ankiquest .aq-mood{font-size:14px}
#ankiquest .aq-muted{opacity:.72}
#ankiquest .aq-small{font-size:12px}
#ankiquest .aq-cards{display:flex;flex-wrap:wrap;gap:.6em}
#ankiquest .aq-card{flex:1 1 260px;box-sizing:border-box;border:1px solid rgba(128,128,128,.32);
border-radius:8px;padding:.6em .8em;background:rgba(128,128,128,.06)}
#ankiquest .aq-card h3{font-size:11px;font-weight:600;text-transform:uppercase;letter-spacing:.04em;opacity:.7;margin:0 0 .35em}
#ankiquest .aq-deck{font-weight:600;font-size:14px;word-break:break-word}
#ankiquest .aq-meter{height:6px;border-radius:3px;background:rgba(128,128,128,.25);overflow:hidden;margin:.25em 0 .45em}
#ankiquest .aq-meter span{display:block;height:100%;background:#4a8fe0}
#ankiquest .aq-meter.aq-full span{background:#2e9e5b}
#ankiquest .aq-row{display:flex;justify-content:space-between;align-items:baseline;gap:.5em}
#ankiquest .aq-board{width:100%;border-collapse:collapse}
#ankiquest .aq-board td{padding:2px 4px}
#ankiquest .aq-board td:last-child{text-align:right;white-space:nowrap}
#ankiquest .aq-me td{background:rgba(74,143,224,.2);font-weight:600}
#ankiquest .aq-done-today{color:#2e9e5b}
#ankiquest .aq-learning-later{color:#b7791f}
#ankiquest .aq-links{margin-top:.6em;text-align:center}
#ankiquest button{margin-top:.3em}
</style>"""


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


def summary_parts(profile, place, translate):
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
    return parts


def html(profile, place=None, unread=0, translate=lambda value: value, mascot=None, today=None):
    """A block for `deck_browser_will_render_content`, so it lands under the stats.

    Given `today`, the view home.today_view builds, it is the Today panel instead.
    """
    if today is not None:
        return panel(profile, place, unread, translate, today)
    parts = summary_parts(profile, place, translate)
    mascot_html = (
        '<img src="%s" width=40 height=40 alt="" style="object-fit:contain">'
        % escape(mascot)
        if mascot else ""
    )
    return (
        "<div id=ankiquest style='max-width:600px;margin:2em auto 0;font-size:13px;"
        "display:flex;flex-wrap:wrap;justify-content:space-between;gap:.3em'>"
        "<div style='display:flex;align-items:center;gap:.5em'>%s<div><b>ankiquest</b> <span style='opacity:.7'>%s</span></div></div><div>%s</div></div>"
        % (mascot_html, escape("  ·  ".join(parts)), links(unread, translate))
    )


def links(unread, translate, more=False):
    found = link("board", translate("Leaderboard")) + link("challenges", translate("Friends"))
    if more:
        found = (
            link("today", translate("Today")) + link("history", translate("History"))
            + found + link("reminders", translate("Reminders")) + link("settings", translate("Website settings"))
        )
    if unread:
        found += link("inbox", translate("\U0001f4ec %d new") % unread)
    return found


def meter(value, target):
    """A progress bar that turns green when full."""
    value, target = max(0, int(value or 0)), max(0, int(target or 0))
    share = 0 if target == 0 else min(100, value * 100 // target)
    return "<div class='aq-meter%s' role=progressbar aria-valuenow=%d aria-valuemax=%d><span style='width:%d%%'></span></div>" % (
        " aq-full" if target and value >= target else "", value, target, share,
    )


def card(title, body):
    return "<div class=aq-card><h3>%s</h3>%s</div>" % (escape(title), body)


def head(today, profile, place, translate):
    art = today.get("art")
    image = '<img src="%s" width=64 height=64 alt="" style="object-fit:contain;flex:none">' % escape(art) if art else ""
    mood = "<div class=aq-mood>%s</div>" % escape(today["mood"]) if today.get("mood") else ""
    line = "<div class='aq-muted aq-small'><b>ankiquest</b> %s</div>" % escape("  ·  ".join(summary_parts(profile, place, translate)))
    return "<div class=aq-head>%s<div>%s%s</div></div>" % (image, mood, line)


def next_deck(today, translate):
    deck = today.get("focus")
    if deck is None:
        return card(translate("Your next step"), "<div class=aq-muted>%s</div>" % escape(translate("Your study starts with a deck.")))
    body = "<div class=aq-deck>%s</div>" % escape(deck["name"])
    body += "<div class='aq-muted aq-small'>%s</div>" % escape(
        translate("%d reviews · %d learning · %d new") % (deck["review"], deck["learning"], deck["new"])
    )
    body += "<div class='aq-small %s'>%s</div>" % (STATUS_CLASS.get(deck["status"], "aq-muted"), escape(deck["label"]))
    if deck["status"] == "review_now":
        body += "<button onclick=\"pycmd('ankiquest:study:%d');return false\">%s</button>" % (
            int(deck["id"]), escape(translate("Study %d due") % deck["ready"]),
        )
    return card(translate("Your next step"), body)


def goal_card(today, translate):
    goal = today.get("goal")
    if not goal:
        return ""
    body = "<div class=aq-small>%s</div>" % escape(goal["minimum"])
    if goal.get("minimum_meter"):
        body += meter(*goal["minimum_meter"])
    if goal.get("daily"):
        body += "<div class=aq-small style='margin-top:.3em'>%s</div>" % escape(goal["daily"])
        body += meter(*goal["daily_meter"])
    if goal.get("backlog"):
        body += "<div class='aq-muted aq-small'>%s</div>" % escape(goal["backlog"])
    return card(translate("Today’s goal"), body)


def quest_line(quest, translate):
    progress, target = int(quest.get("progress") or 0), int(quest.get("target") or 0)
    status = translate("Complete · %d XP") % int(quest.get("reward") or 0) if quest.get("done") else "%d / %d" % (progress, target)
    return "<div class='aq-row aq-small'><span>%s</span><span class=aq-muted>%s</span></div>%s" % (
        escape(str(quest.get("title") or "")), escape(status), meter(progress, target),
    )


def quests_card(profile, today, translate):
    quests = [quest for quest in (profile or {}).get("quests") or [] if isinstance(quest, dict)]
    if not quests:
        return ""
    body = ""
    if all(quest.get("done") for quest in quests) and today.get("winner"):
        body += '<img src="%s" width=48 height=48 alt="" style="object-fit:contain;float:right">' % escape(today["winner"])
    body += "".join(quest_line(quest, translate) for quest in quests)
    return card(translate("Daily quests"), body)


def period_picker(chosen, translate):
    options = "".join(
        "<option value=%s%s>%s</option>" % (key, " selected" if key == chosen else "", escape(translate(text)))
        for key, text in PERIOD_LABELS
    )
    return (
        "<select aria-label='%s' onchange=\"pycmd('ankiquest:period:'+this.value)\" style='font-size:11px'>%s</select>"
        % (escape(translate("Which leaderboard?"), True), options)
    )


def board_row(place, entry, mine):
    return "<tr%s><td>%s</td><td>%s</td><td>%s XP</td></tr>" % (
        " class=aq-me" if mine else "",
        MEDALS.get(place, "#%d" % place),
        escape(str(entry.get("display") or entry.get("user") or "")),
        "{:,}".format(int(entry.get("xp") or 0)),
    )


def board_card(today, translate):
    standing = today.get("board")
    if standing is None:
        return ""
    rows = "".join(board_row(*row) for row in standing["rows"])
    if not rows:
        rows = "<tr><td class=aq-muted>%s</td></tr>" % escape(translate("Nobody else plays yet."))
    title = "<div class=aq-row><h3>%s</h3>%s</div>" % (escape(translate("Leaderboard")), period_picker(standing["period"], translate))
    return "<div class=aq-card>%s<table class='aq-board aq-small'>%s</table></div>" % (title, rows)


def session_line(today, translate):
    if not today.get("session"):
        return ""
    return "<div class='aq-muted aq-small' style='margin-top:.5em'>%s: %s</div>" % (
        escape(translate("Your last study session")), escape(today["session"]),
    )


def panel(profile, place, unread, translate, today):
    cards = next_deck(today, translate) + goal_card(today, translate) + quests_card(profile, today, translate) + board_card(today, translate)
    return (
        "%s<div id=ankiquest class=aq-today>%s<div class=aq-cards>%s</div>%s<div class='aq-links aq-small'>%s</div></div>"
        % (STYLE, head(today, profile, place, translate), cards, session_line(today, translate), links(unread, translate, more=True))
    )
