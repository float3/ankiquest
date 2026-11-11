"""The review heatmap under the deck list, as AnkiQuest-Android shows it under its decks.

Python decides each day's colour, text and search (heatmap.py); the page's script only lays
out the chosen year and shows the clicked day:

    heatmap.load ─▶ with_server_days ─▶ cells() ─▶ JSON ─▶ <script> draws the year
                                                             │ click "View cards"
    browser ◀─ search_for_offset ◀─ on_js_message ◀─ pycmd ◀─┘

Works offline, without an account. With one, AnkiQuest's verdicts and frozen days colour
the days it judged, as on Android.
"""

import json
from datetime import date
from html import escape

from . import heatmap
from .language import _plural as _plural_text

CONFIG_KEY = "review_heatmap"
EXPANDED_KEY = "ankiquestHeatmapExpanded"
MESSAGE = "ankiquest:heatmap:"
CARDS = MESSAGE + "cards:"
EXPANDED = MESSAGE + "expanded:"

# The companion's picture in the heading, when one is chosen.
ART = {"aki": "aki_streak", "ankilope": "ankilope_study"}

# Colours from AnkiQuest-Android's review_heatmap.xml, light then night.
STYLE = """<style>
#aq-heatmap{--hm-empty:#e7ece9;--hm-l1:#bae1c4;--hm-l2:#77c58f;--hm-l3:#369d5b;--hm-l4:#136c37;
--hm-f1:#d4d9df;--hm-f2:#abb3be;--hm-f3:#7d8998;--hm-f4:#546171;--hm-kept:#2f80c9;--hm-frozen:#bfdcf5;
--hm-short:#e9d8a6;--hm-line:#e5e5e5;--hm-bg:#fff;--hm-text:#3c3c3c;--hm-muted:#777;--hm-select:#1cb0f6;
max-width:680px;margin:1.5em auto 0;text-align:left;color:var(--hm-text);background:var(--hm-bg);
border:2px solid var(--hm-line);border-bottom-width:5px;border-radius:18px;padding:14px 16px;
font:600 13px/1.35 "Nunito","Varela Round","Segoe UI Variable Display","Segoe UI",system-ui,sans-serif}
.nightMode #aq-heatmap,.night-mode #aq-heatmap,.night_mode #aq-heatmap{--hm-empty:#323d38;--hm-l1:#245e3d;
--hm-l2:#398654;--hm-l3:#60b67a;--hm-l4:#9adcaa;--hm-f1:#424c59;--hm-f2:#627080;--hm-f3:#8997a7;--hm-f4:#bac4cf;
--hm-kept:#7db8ec;--hm-frozen:#1f4a70;--hm-short:#5e5132;--hm-line:#1e1f22;--hm-bg:#2b2d31;--hm-text:#f2f3f5;
--hm-muted:#b5bac1}
#aq-heatmap button{font:inherit;color:inherit;cursor:pointer}
#aq-heatmap button:disabled{opacity:.35;cursor:default}
#aq-heatmap .hm-head{display:flex;align-items:center;gap:10px}
#aq-heatmap .hm-head img{width:48px;height:48px;object-fit:contain}
#aq-heatmap .hm-title{flex:1;font-size:15px;font-weight:800}
#aq-heatmap .hm-sub,#aq-heatmap .hm-note{color:var(--hm-muted);font-weight:600;font-size:12px}
#aq-heatmap .hm-toggle{background:none;border:0;font-size:18px;padding:4px 8px}
#aq-heatmap .hm-stats{display:flex;gap:8px;margin:12px 0 6px;text-align:center}
#aq-heatmap .hm-stats div{flex:1;background:var(--hm-empty);border-radius:12px;padding:6px 4px;font-size:11px}
#aq-heatmap .hm-stats b{display:block;font-size:15px}
#aq-heatmap .hm-nav{display:flex;align-items:center;justify-content:center;gap:12px;margin:8px 0}
#aq-heatmap .hm-nav button{background:none;border:0;font-size:18px;padding:0 8px}
#aq-heatmap .hm-cal{display:flex;gap:4px}
#aq-heatmap .hm-wd{display:grid;grid-template-rows:repeat(7,11px);row-gap:3px;margin-top:17px;font-size:9px;line-height:11px}
#aq-heatmap .hm-scroll{overflow-x:auto;padding-bottom:4px}
#aq-heatmap .hm-months{position:relative;height:14px;font-size:9px;color:var(--hm-muted)}
#aq-heatmap .hm-months span{position:absolute;top:0}
#aq-heatmap .hm-grid{display:grid;grid-auto-flow:column;grid-template-rows:repeat(7,11px);grid-auto-columns:11px;gap:3px;padding:3px}
#aq-heatmap .hm-grid button{width:11px;height:11px;padding:0;border:0;border-radius:3px;background:var(--hm-empty)}
#aq-heatmap .hm-grid .l1{background:var(--hm-l1)}#aq-heatmap .hm-grid .l2{background:var(--hm-l2)}
#aq-heatmap .hm-grid .l3{background:var(--hm-l3)}#aq-heatmap .hm-grid .l4{background:var(--hm-l4)}
#aq-heatmap .hm-grid .f1{background:var(--hm-f1)}#aq-heatmap .hm-grid .f2{background:var(--hm-f2)}
#aq-heatmap .hm-grid .f3{background:var(--hm-f3)}#aq-heatmap .hm-grid .f4{background:var(--hm-f4)}
#aq-heatmap .hm-grid .s{background:var(--hm-short)}#aq-heatmap .hm-grid .z{background:var(--hm-frozen)}
#aq-heatmap .hm-grid .far{opacity:.27}
#aq-heatmap .hm-grid .k{box-shadow:0 0 0 2px var(--hm-kept)}
#aq-heatmap .hm-grid .today{box-shadow:0 0 0 2px var(--hm-text)}
#aq-heatmap .hm-grid .sel{box-shadow:0 0 0 2px var(--hm-select)}
#aq-heatmap .hm-day{display:flex;align-items:center;gap:8px;flex-wrap:wrap;margin:8px 0}
#aq-heatmap .hm-day span{flex:1}
#aq-heatmap .hm-view{background:var(--hm-select);color:#fff;border:0;border-radius:999px;padding:4px 12px;font-weight:800}
</style>"""

# Lays out one year; every colour, text and search was decided in Python.
SCRIPT = """<script>(function(){
var D=%s,L=D.labels,root=document.getElementById("aq-heatmap");
function q(name){return root.querySelector(".hm-"+name);}
var grid=q("grid"),months=q("months"),scroll=q("scroll"),view=q("view"),body=q("body");
var year=+D.today.slice(0,4),selected=D.today,firstDay=1;
// Monday unless the language's calendar starts elsewhere (1 Monday … 7 Sunday).
try{var locale=new Intl.Locale(D.lang),week=locale.getWeekInfo?locale.getWeekInfo():locale.weekInfo;
if(week&&week.firstDay){firstDay=week.firstDay%%7;}}catch(e){}
function format(options){try{return new Intl.DateTimeFormat(D.lang,options);}catch(e){return new Intl.DateTimeFormat("en",options);}}
var full=format({dateStyle:"full",timeZone:"UTC"}),month=format({month:"short",timeZone:"UTC"}),
weekday=format({weekday:"short",timeZone:"UTC"});
function iso(day){return day.toISOString().slice(0,10);}
function parse(text){return new Date(text+"T00:00:00Z");}
// [class, text, has cards]; days without data get the text for their kind.
function info(day){
  if(D.days[day]){return D.days[day];}
  if(day>D.end){return ["",L.beyond,0];}
  return ["",day>D.today?L.due:L.none,0];
}
function show(){
  var cell=info(selected);
  q("date").textContent=full.format(parse(selected));
  q("count").textContent=cell[1];
  view.disabled=!cell[2];
  view.textContent=selected>D.today?L.viewDue:L.viewReviewed;
  grid.querySelectorAll(".sel").forEach(function(b){b.classList.remove("sel");});
  var button=grid.querySelector("[data-day='"+selected+"']");
  if(button){button.classList.add("sel");}
}
function draw(){
  grid.textContent="";months.textContent="";
  var day=new Date(Date.UTC(year,0,1)),blank=(day.getUTCDay()-firstDay+7)%%7;
  for(var i=0;i<blank;i++){grid.appendChild(document.createElement("i"));}
  for(var n=blank;day.getUTCFullYear()===year;n++,day.setUTCDate(day.getUTCDate()+1)){
    var text=iso(day),cell=info(text),button=document.createElement("button");
    button.className=(cell[0]||"l0")+(text>D.end?" far":"")+(text===D.today?" today":"");
    button.dataset.day=text;
    button.title=full.format(day)+". "+cell[1];
    button.setAttribute("aria-label",button.title);
    grid.appendChild(button);
    if(day.getUTCDate()===1){
      var label=document.createElement("span");
      label.textContent=month.format(day);
      label.style.left=(Math.floor(n/7)*14+3)+"px";
      months.appendChild(label);
    }
  }
  q("year").textContent=year;
  q("prev").disabled=year<=D.firstYear;
  q("next").disabled=year>=D.lastYear;
  show();
  var today=grid.querySelector(".today");
  scroll.scrollLeft=today?Math.max(0,today.offsetLeft-scroll.clientWidth*2/3):0;
}
function step(delta){
  year+=delta;
  selected=year===+D.today.slice(0,4)?D.today:year+"-01-01";
  draw();
}
var labels=q("wd");
for(var row=0;row<7;row++){
  var label=document.createElement("span");
  label.textContent=weekday.format(new Date(Date.UTC(2023,0,1+(firstDay+row)%%7)));
  labels.appendChild(label);
}
grid.onclick=function(event){
  var day=event.target.dataset&&event.target.dataset.day;
  if(day){selected=day;show();}
};
q("prev").onclick=function(){step(-1);};
q("next").onclick=function(){step(1);};
view.onclick=function(){pycmd("%s"+selected);};
q("toggle").onclick=function(){
  var open=body.hidden;
  body.hidden=!open;
  this.textContent=open?"▴":"▾";
  this.setAttribute("aria-label",open?L.collapse:L.expand);
  pycmd("%s"+(open?1:0));
};
draw();
})();</script>"""


def _plural(tr, count, one, other):
    return _plural_text(one, other, count, tr) % count


def _reviews(tr, count):
    return _plural(tr, count, "%s review", "%s reviews")


def _days(tr, count):
    return _plural(tr, count, "%s day", "%s days")


def _state_label(state, tr):
    return tr({
        heatmap.MET: "Streak minimum met",
        heatmap.SHORT: "Short of the streak minimum",
        heatmap.FROZEN: "Frozen — streak kept",
        heatmap.SKIPPED: "Skipped day — streak kept",
        heatmap.NONE: "No study counted",
        "protected": "Protected — streak kept",
    }.get(state, "Streak unknown"))


def describe(data, day, tr):
    """What a day says when clicked, as ReviewHeatmapGrid.kt's dayCountDescription."""
    if day > data.forecast_end:
        return tr("Forecast is available for the next 90 days.")
    if day > data.today:
        return _plural(tr, data.scheduled.get(day, 0), "%s review card due (estimate)", "%s review cards due (estimate)")

    reviews = _reviews(tr, data.reviews.get(day, 0))
    verdict = data.verdicts.get(day)
    if verdict is None:
        return reviews + " · " + _state_label("protected", tr) if day in data.protected else reviews
    if verdict.state != heatmap.SHORT:
        return reviews + " · " + _state_label(verdict.state, tr)
    minutes = min(verdict.minutes, verdict.streak_minutes)
    if day == data.today:
        return tr("%s · %s of %s minutes for the streak so far") % (reviews, minutes, verdict.streak_minutes)
    return tr("%s · %s of %s minutes, short of the streak minimum") % (reviews, minutes, verdict.streak_minutes)


def colour(data, day):
    """A day's colour class, as ReviewHeatmapGrid.kt draws it. AnkiQuest's verdict colours the
    days it judged: only a day that met the streak minimum looks studied, and a frozen day
    looks frozen even with reviews. "k" outlines a day AnkiQuest kept the streak through."""
    future = day > data.today
    verdict = None if future else data.verdicts.get(day)
    state = verdict.state if verdict else None
    if state == heatmap.SHORT:
        found = "s"
    elif state in heatmap.KEEPS_STREAK:
        found = "z"
    elif state == heatmap.NONE:
        found = "l0"
    elif future:
        found = "f%d" % heatmap.intensity(data.scheduled.get(day, 0), data.daily_average)
    else:
        found = "l%d" % heatmap.intensity(data.reviews.get(day, 0), data.daily_average)
    return found + " k" if day in data.protected else found


def cells(data, tr):
    """{ISO date: [colour, text, has cards]} for every day with something to show."""
    found = {}
    for day in sorted(set(data.reviews) | set(data.scheduled) | set(data.verdicts) | data.protected):
        has_cards = 1 if heatmap.search_for_day(data, day) else 0
        found[day.isoformat()] = [colour(data, day), describe(data, day, tr), has_cards]
    return found


def _stat(value, label):
    return "<div><b>%s</b>%s</div>" % (escape(value), escape(label))


def html(data, tr, language="en", expanded=True, art=None):
    """The heatmap block for the deck list."""
    first_year = min((data.first_review_date or data.today).year, data.today.year)
    page = {
        "today": data.today.isoformat(),
        "end": data.forecast_end.isoformat(),
        "firstYear": first_year,
        "lastYear": data.forecast_end.year,
        "lang": language.replace("_", "-") or "en",
        "days": cells(data, tr),
        "labels": {
            "none": _reviews(tr, 0),
            "due": tr("%s review cards due (estimate)") % 0,
            "beyond": tr("Forecast is available for the next 90 days."),
            "viewReviewed": tr("View reviewed cards"),
            "viewDue": tr("View due cards"),
            "expand": tr("Expand review heatmap"),
            "collapse": tr("Collapse review heatmap"),
        },
    }
    # Keeps "</script>" in a label from ending the script.
    script = SCRIPT % (json.dumps(page, ensure_ascii=False).replace("</", "<\\/"), CARDS, EXPANDED)

    picture = "<img src='%s' alt=''>" % escape(art, quote=True) if art else ""
    toggle = tr("Collapse review heatmap") if expanded else tr("Expand review heatmap")
    empty = "" if data.reviews else "<p class=hm-note>%s</p>" % escape(tr("Your reviews will appear here after you study."))
    return (
        STYLE
        + "<div id=aq-heatmap>"
        + "<div class=hm-head>%s<div class=hm-title>%s<div class=hm-sub>%s</div></div>" % (
            picture, escape(tr("Review heatmap")), escape(tr("Your collection, one day at a time")))
        + "<button class=hm-toggle aria-label='%s'>%s</button></div>" % (escape(toggle, quote=True), "▴" if expanded else "▾")
        + "<div class=hm-body%s>" % ("" if expanded else " hidden")
        + empty
        + "<div class=hm-stats>%s%s%s%s</div>" % (
            _stat(_days(tr, data.current_streak), tr("Current streak")),
            _stat(_days(tr, data.longest_streak), tr("Longest streak")),
            _stat(str(data.days_studied), tr("Days studied")),
            _stat("%.1f" % data.daily_average, tr("Reviews / study day")))
        + "<div class=hm-note>%s</div>" % escape(tr("Reviews in available history: %s") % data.total_reviews)
        + "<div class=hm-nav><button class=hm-prev aria-label='%s'>‹</button><b class=hm-year></b>"
        "<button class=hm-next aria-label='%s'>›</button></div>" % (
            escape(tr("Previous year"), quote=True), escape(tr("Next year"), quote=True))
        + "<div class=hm-cal><div class=hm-wd></div><div class=hm-scroll><div class=hm-months></div><div class=hm-grid></div></div></div>"
        + "<div class=hm-day><b class=hm-date></b><span class=hm-count></span><button class=hm-view></button></div>"
        + "<div class=hm-note>%s</div>" % escape(tr("Click a day for details. Gray squares show estimated review cards due over the next 90 days."))
        + "<div class=hm-note>%s</div>" % escape(tr("Fewer reviews → More reviews · Gray = forecast · Blue = streak kept by AnkiQuest · Sand = under the streak minimum"))
        + "</div>"
        + script
        + "</div>"
    )


def failed(tr):
    return STYLE + "<div id=aq-heatmap><p class=hm-note>%s</p></div>" % escape(tr("Review history could not be loaded."))


# Anki glue below: loads the collection's heatmap, keeps it while the collection is unchanged,
# and answers the page's buttons.

local = {"addon": None, "key": None, "data": None}


def _collection_heatmap(col):
    """The collection's heatmap, read again only once a review or the day has changed it."""
    last_review = col.db.scalar("select max(id) from revlog")
    key = (id(col), col.mod, col.sched.day_cutoff, last_review)
    if local["key"] != key:
        local["data"], local["key"] = heatmap.load(col), key
    return local["data"]


def _art(module):
    api = module.client()
    companion = module.current_companion()
    if not api.configured or companion not in ART:
        return None
    from aqt import mw

    return "/_addons/%s/art/%s.png" % (mw.addonManager.addonFromModule(module.__name__), ART[companion])


def on_deck_browser(deck_browser, content):
    from anki.lang import current_lang
    from aqt import mw

    module = local["addon"]
    if not module.config().get(CONFIG_KEY, True) or mw.col is None:
        return
    try:
        data = _collection_heatmap(mw.col)
        profile = module.state["profile"]
        if profile is not None and module.client().configured:
            data = heatmap.with_server_days(data, heatmap.server_days(profile))
        expanded = mw.pm.profile.get(EXPANDED_KEY, True)
        content.stats += html(data, module.tr, current_lang, expanded, _art(module))
    except Exception as e:
        print("ankiquest heatmap:", e)
        content.stats += failed(module.tr)


def open_cards(text):
    """Opens the browser on a day's cards, numbering the day from today as it is now."""
    from aqt import dialogs, mw

    try:
        day = date.fromisoformat(text)
    except ValueError:
        return
    today = heatmap.study_date(mw.col.sched.day_cutoff)
    search = heatmap.search_for_offset((day - today).days)
    if search is None:
        return
    dialogs.open("Browser", mw).search_for(search)


def on_js_message(handled, message, context):
    from aqt import mw

    if context is not mw.deckBrowser or not message.startswith(MESSAGE):
        return handled
    if message.startswith(CARDS):
        open_cards(message[len(CARDS):])
    elif message.startswith(EXPANDED):
        mw.pm.profile[EXPANDED_KEY] = message.endswith("1")
    return (True, None)


def install(module):
    """Hooks the heatmap under the deck list; `module` is the add-on package."""
    from aqt import gui_hooks

    local["addon"] = module
    gui_hooks.deck_browser_will_render_content.append(on_deck_browser)
    gui_hooks.webview_did_receive_js_message.append(on_js_message)
