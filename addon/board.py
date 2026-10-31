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
AVATAR_COLOURS = ("#58cc02", "#1cb0f6", "#ff9600", "#ce82ff", "#ff4b4b", "#2b70c9", "#ffc800")

STYLE = """<style>
#ankiquest.aq-today{--aq-bg:#fff;--aq-line:#e5e5e5;--aq-text:#3c3c3c;--aq-muted:#777;--aq-soft:#f7f7f7;
--aq-green:#58cc02;--aq-green-dark:#46a302;--aq-blue:#1cb0f6;--aq-blue-dark:#1899d6;--aq-orange:#ff9600;
--aq-purple:#ce82ff;--aq-gold:#ffc800;--aq-red:#ff4b4b;--aq-track:#e5e5e5;--aq-me:#ddf4ff;
max-width:680px;margin:2em auto 0;text-align:left;color:var(--aq-text);
font:600 14px/1.35 "Nunito","Varela Round","Segoe UI Variable Display","Segoe UI",system-ui,sans-serif}
.nightMode #ankiquest.aq-today,.night-mode #ankiquest.aq-today,.night_mode #ankiquest.aq-today{
--aq-bg:#2b2d31;--aq-line:#1e1f22;--aq-text:#f2f3f5;--aq-muted:#b5bac1;--aq-soft:#313338;
--aq-track:#404249;--aq-me:#3b4a6b}
#ankiquest .aq-hero{display:flex;align-items:center;gap:14px;margin-bottom:14px}
#ankiquest .aq-hero img{width:96px;height:96px;object-fit:contain;flex:none;filter:drop-shadow(0 4px 0 rgba(0,0,0,.08))}
#ankiquest .aq-bubble{position:relative;background:var(--aq-bg);border:2px solid var(--aq-line);border-radius:16px;
padding:10px 14px;font-weight:800;font-size:15px;margin-bottom:8px;display:inline-block}
#ankiquest .aq-bubble:before{content:"";position:absolute;left:-9px;top:14px;width:14px;height:14px;background:var(--aq-bg);
border-left:2px solid var(--aq-line);border-bottom:2px solid var(--aq-line);transform:rotate(45deg)}
#ankiquest .aq-pills{display:flex;flex-wrap:wrap;gap:6px}
#ankiquest .aq-pill{display:inline-flex;align-items:center;gap:4px;padding:4px 10px;border-radius:999px;
font-weight:800;font-size:13px;color:#fff;box-shadow:inset 0 -3px 0 rgba(0,0,0,.18)}
#ankiquest .aq-pill.aq-streak{background:var(--aq-orange)}
#ankiquest .aq-pill.aq-level{background:var(--aq-purple)}
#ankiquest .aq-pill.aq-rank{background:var(--aq-gold);color:#5c4400}
#ankiquest .aq-pill.aq-risk{background:var(--aq-red)}
#ankiquest .aq-pill.aq-ghost{background:var(--aq-soft);color:var(--aq-muted);box-shadow:inset 0 -3px 0 var(--aq-line)}
#ankiquest .aq-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:12px;align-items:start}
#ankiquest .aq-card{background:var(--aq-bg);border:2px solid var(--aq-line);border-bottom-width:5px;border-radius:18px;padding:14px 16px}
#ankiquest .aq-card h3{margin:0 0 10px;font-size:12px;font-weight:800;letter-spacing:.08em;text-transform:uppercase;color:var(--aq-muted)}
#ankiquest .aq-deck{font-size:19px;font-weight:800;word-break:break-word;margin-bottom:8px}
#ankiquest .aq-counts{display:flex;gap:6px;margin-bottom:8px;flex-wrap:wrap}
#ankiquest .aq-count{padding:2px 9px;border-radius:999px;font-size:12px;font-weight:800;background:var(--aq-soft)}
#ankiquest .aq-count.aq-review{color:var(--aq-blue)}
#ankiquest .aq-count.aq-learn{color:var(--aq-red)}
#ankiquest .aq-count.aq-new{color:var(--aq-green-dark)}
#ankiquest .aq-note{font-size:13px;color:var(--aq-muted);margin:4px 0}
#ankiquest .aq-done-today{color:var(--aq-green-dark)}
#ankiquest .aq-learning-later{color:var(--aq-orange)}
#ankiquest .aq-button{display:block;width:100%;margin-top:10px;padding:12px 16px;border:0;border-radius:14px;cursor:pointer;
background:var(--aq-green);color:#fff;font-family:inherit;font-weight:800;font-size:15px;line-height:1;text-transform:uppercase;letter-spacing:.06em;
box-shadow:0 4px 0 var(--aq-green-dark)}
#ankiquest .aq-button:hover{filter:brightness(1.05)}
#ankiquest .aq-button:active{transform:translateY(4px);box-shadow:none}
#ankiquest .aq-goal-edit{margin-top:8px;padding:4px 12px;border:2px solid var(--aq-track);border-radius:999px;cursor:pointer;background:transparent;color:var(--aq-blue);font-family:inherit;font-weight:800;font-size:12px}
#ankiquest .aq-goal-edit:hover{background:var(--aq-soft)}
#ankiquest .aq-goal{margin-bottom:12px}
#ankiquest .aq-goal:last-child{margin-bottom:0}
#ankiquest .aq-row{display:flex;justify-content:space-between;align-items:baseline;gap:8px;font-weight:800}
#ankiquest .aq-row span:last-child{color:var(--aq-muted);white-space:nowrap}
#ankiquest .aq-meter{position:relative;height:16px;border-radius:999px;background:var(--aq-track);overflow:hidden;margin:6px 0 2px}
#ankiquest .aq-meter span{display:block;height:100%;border-radius:999px;background:var(--aq-gold);
box-shadow:inset 0 -4px 0 rgba(0,0,0,.12);position:relative;min-width:16px}
#ankiquest .aq-meter span:after{content:"";position:absolute;left:8px;right:8px;top:3px;height:4px;border-radius:999px;background:rgba(255,255,255,.45)}
#ankiquest .aq-meter.aq-full span{background:var(--aq-green)}
#ankiquest .aq-meter.aq-empty span{min-width:0;box-shadow:none}
#ankiquest .aq-meter.aq-empty span:after{display:none}
#ankiquest .aq-quest{display:flex;gap:10px;align-items:center;margin-bottom:10px}
#ankiquest .aq-quest:last-child{margin-bottom:0}
#ankiquest .aq-quest-icon{flex:none;width:36px;height:36px;border-radius:12px;display:grid;place-items:center;font-size:18px;
background:var(--aq-soft);box-shadow:inset 0 -3px 0 var(--aq-line)}
#ankiquest .aq-quest-body{flex:1;min-width:0}
#ankiquest .aq-winner{float:right;width:56px;height:56px;object-fit:contain;margin:-6px -4px 0 6px}
#ankiquest .aq-board-head{display:flex;justify-content:space-between;align-items:center;margin-bottom:8px}
#ankiquest .aq-board-head h3{margin:0}
#ankiquest select{font-family:inherit;font-weight:800;font-size:12px;color:var(--aq-text);background:var(--aq-soft);border:2px solid var(--aq-line);
border-radius:999px;padding:3px 8px}
#ankiquest .aq-player{display:flex;align-items:center;gap:10px;padding:6px 8px;border-radius:12px}
#ankiquest .aq-player.aq-me{background:var(--aq-me);box-shadow:inset 0 0 0 2px var(--aq-blue)}
#ankiquest .aq-place{width:26px;text-align:center;font-weight:800;color:var(--aq-muted)}
#ankiquest .aq-avatar{flex:none;width:30px;height:30px;border-radius:50%;display:grid;place-items:center;color:#fff;font-weight:800;
font-size:13px;box-shadow:inset 0 -3px 0 rgba(0,0,0,.18)}
#ankiquest .aq-name{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:800}
#ankiquest .aq-xp{font-weight:800;color:var(--aq-orange);white-space:nowrap}
#ankiquest .aq-empty-note{color:var(--aq-muted);padding:6px 8px}
#ankiquest .aq-session{margin-top:12px;text-align:center}
#ankiquest .aq-links{display:flex;flex-wrap:wrap;justify-content:center;gap:8px;margin-top:14px}
#ankiquest .aq-links a{text-decoration:none;font-weight:800;font-size:13px;color:var(--aq-blue);background:var(--aq-bg);
border:2px solid var(--aq-line);border-bottom-width:4px;border-radius:12px;padding:6px 12px}
#ankiquest .aq-links a:hover{background:var(--aq-soft)}
#ankiquest .aq-links a.aq-unread{background:var(--aq-blue);border-color:var(--aq-blue-dark);color:#fff}
</style>"""


def link(page, text, extra=""):
    return (
        "<a href=#%s onclick=\"pycmd('ankiquest:web:%s');return false\">%s</a>"
        % (extra, page, escape(text))
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
        parts.append(level_text(profile, translate))
        if profile.get("streak"):
            parts.append(translate("\U0001f525 %d day streak") % profile["streak"])
        if profile.get("at_risk"):
            parts.append(translate("streak at risk today"))
    return parts


def level_text(profile, translate):
    return translate("Lv %d  %s/%s XP") % (
        profile.get("level", 1),
        "{:,}".format(profile.get("xp_into_level", 0)),
        "{:,}".format(profile.get("xp_for_next", 0)),
    )


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
        % (mascot_html, escape("  ·  ".join(parts)), plain_links(unread, translate))
    )


def plain_links(unread, translate):
    found = link("board", translate("Leaderboard"), " style='margin-left:.8em'")
    found += link("challenges", translate("Friends"), " style='margin-left:.8em'")
    if unread:
        found += link("inbox", translate("\U0001f4ec %d new") % unread, " style='margin-left:.8em'")
    return found


def links(unread, translate):
    found = "".join(
        link(page, translate(text))
        for page, text in (
            ("today", "Today"), ("history", "History"), ("board", "Leaderboard"),
            ("challenges", "Friends"), ("reminders", "Reminders"), ("settings", "Website settings"),
        )
    )
    if unread:
        found += link("inbox", translate("\U0001f4ec %d new") % unread, " class=aq-unread")
    return found


def meter(value, target):
    """A progress bar that turns green when full."""
    value, target = max(0, int(value or 0)), max(0, int(target or 0))
    share = 0 if target == 0 else min(100, value * 100 // target)
    state = " aq-full" if target and value >= target else " aq-empty" if share == 0 else ""
    return "<div class='aq-meter%s' role=progressbar aria-valuenow=%d aria-valuemax=%d><span style='width:%d%%'></span></div>" % (
        state, value, target, share,
    )


def card(title, body):
    return "<div class=aq-card><h3>%s</h3>%s</div>" % (escape(title), body)


def pill(text, kind):
    return "<span class='aq-pill aq-%s'>%s</span>" % (kind, escape(text))


def pills(profile, place, translate):
    found = ""
    if profile and profile.get("streak"):
        found += pill(translate("\U0001f525 %d day streak") % profile["streak"], "streak")
    if profile:
        found += pill(level_text(profile, translate), "level")
    if place:
        found += pill("\U0001f3c6 " + translate("#%d this week") % place, "rank")
    if profile and profile.get("at_risk"):
        found += pill(translate("streak at risk today"), "risk")
    return "<div class=aq-pills>%s</div>" % found if found else ""


def head(today, profile, place, translate):
    art = today.get("art")
    image = '<img src="%s" alt="">' % escape(art) if art else ""
    mood = "<div class=aq-bubble>%s</div>" % escape(today["mood"]) if today.get("mood") else ""
    return "<div class=aq-hero>%s<div>%s%s</div></div>" % (image, mood, pills(profile, place, translate))


def counts(deck, translate):
    parts = (
        ("review", translate("%d reviews") % deck["review"]),
        ("learn", translate("%d learning") % deck["learning"]),
        ("new", translate("%d new") % deck["new"]),
    )
    return "<div class=aq-counts>%s</div>" % "".join(
        "<span class='aq-count aq-%s'>%s</span>" % (kind, escape(text)) for kind, text in parts
    )


def next_deck(today, translate):
    deck = today.get("focus")
    if deck is None:
        return card(translate("Your next step"), "<div class=aq-note>%s</div>" % escape(translate("Your study starts with a deck.")))
    body = "<div class=aq-deck>%s</div>" % escape(deck["name"])
    body += counts(deck, translate)
    body += "<div class='aq-note %s'>%s</div>" % (STATUS_CLASS.get(deck["status"], ""), escape(deck["label"]))
    if deck["status"] == "review_now":
        body += "<button class=aq-button onclick=\"pycmd('ankiquest:study:%d');return false\">%s</button>" % (
            int(deck["id"]), escape(translate("Study %d due") % deck["ready"]),
        )
    return card(translate("Your next step"), body)


def goal_line(text, values):
    value, target = values
    return "<div class=aq-goal><div class=aq-row><span>%s</span><span>%d / %d</span></div>%s</div>" % (
        escape(text), max(0, int(value or 0)), max(0, int(target or 0)), meter(value, target),
    )


def goal_card(today, translate):
    goal = today.get("goal")
    if not goal:
        return ""
    body = ""
    if goal.get("minimum_meter"):
        body += goal_line(goal["minimum"], goal["minimum_meter"])
    else:
        body += "<div class=aq-note>%s</div>" % escape(goal["minimum"])
    if goal.get("daily"):
        body += goal_line(goal["daily"], goal["daily_meter"])
    if goal.get("backlog"):
        body += "<div class=aq-note>%s</div>" % escape(goal["backlog"])
    if goal.get("action"):
        body += "<button class=aq-goal-edit onclick=\"pycmd('ankiquest:goal');return false\">%s</button>" % escape(goal["action"])
    return card(translate("Today’s goal"), body)


def quest_line(quest, translate):
    progress, target = int(quest.get("progress") or 0), int(quest.get("target") or 0)
    done = quest.get("done")
    status = translate("Complete · %d XP") % int(quest.get("reward") or 0) if done else "%d / %d" % (progress, target)
    return (
        "<div class=aq-quest><div class=aq-quest-icon>%s</div><div class=aq-quest-body>"
        "<div class=aq-row><span>%s</span><span>%s</span></div>%s</div></div>"
    ) % ("✅" if done else "⚡", escape(str(quest.get("title") or "")), escape(status), meter(progress, target))


def quests_card(profile, today, translate):
    quests = [quest for quest in (profile or {}).get("quests") or [] if isinstance(quest, dict)]
    if not quests:
        return ""
    body = ""
    if all(quest.get("done") for quest in quests) and today.get("winner"):
        body += '<img class=aq-winner src="%s" alt="">' % escape(today["winner"])
    body += "".join(quest_line(quest, translate) for quest in quests)
    return card(translate("Daily quests"), body)


def period_picker(chosen, translate):
    options = "".join(
        "<option value=%s%s>%s</option>" % (key, " selected" if key == chosen else "", escape(translate(text)))
        for key, text in PERIOD_LABELS
    )
    return (
        "<select aria-label='%s' onchange=\"pycmd('ankiquest:period:'+this.value)\">%s</select>"
        % (escape(translate("Which leaderboard?"), True), options)
    )


def avatar(name):
    colour = AVATAR_COLOURS[sum(map(ord, name)) % len(AVATAR_COLOURS)]
    initial = name[:1].upper() or "?"
    return "<span class=aq-avatar style='background:%s'>%s</span>" % (colour, escape(initial))


def board_row(place, entry, mine):
    name = str(entry.get("display") or entry.get("user") or "")
    return (
        "<div class='aq-player%s'><span class=aq-place>%s</span>%s<span class=aq-name>%s</span>"
        "<span class=aq-xp>%s XP</span></div>"
    ) % (
        " aq-me" if mine else "",
        MEDALS.get(place, "#%d" % place),
        avatar(name),
        escape(name),
        "{:,}".format(int(entry.get("xp") or 0)),
    )


def board_card(today, translate):
    standing = today.get("board")
    if standing is None:
        return ""
    rows = "".join(board_row(*row) for row in standing["rows"])
    if not rows:
        rows = "<div class=aq-empty-note>%s</div>" % escape(translate("Nobody else plays yet."))
    title = "<div class=aq-board-head><h3>%s</h3>%s</div>" % (escape(translate("Leaderboard")), period_picker(standing["period"], translate))
    return "<div class=aq-card>%s%s</div>" % (title, rows)


def session_line(today, translate):
    if not today.get("session"):
        return ""
    return "<div class=aq-session>%s</div>" % pill("%s: %s" % (translate("Your last study session"), today["session"]), "ghost")


def panel(profile, place, unread, translate, today):
    cards = next_deck(today, translate) + goal_card(today, translate) + quests_card(profile, today, translate) + board_card(today, translate)
    return (
        "%s<div id=ankiquest class=aq-today>%s<div class=aq-grid>%s</div>%s<div class=aq-links>%s</div></div>"
        % (STYLE, head(today, profile, place, translate), cards, session_line(today, translate), links(unread, translate))
    )
