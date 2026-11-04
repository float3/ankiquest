import {day, escape as esc, lexical, nativeAccount, number as num} from "../common";
import {request, send} from "../http";

const {t:aqText,html:aqHtml} = AnkiQuestI18n;

type Period = "hour" | "day" | "week" | "month" | "year" | "all";
type RecordWindow = "hour" | "day" | "week" | "month" | "year";
type RecordKind = RecordWindow | "streak" | "days";
type WinMetric = "day" | "week" | "month";
type WinScope = "shared" | "lifetime";

interface Person { user: string; display: string; }

/** A row of `/api/leaderboard` (`Standing` in src/main.rs). */
interface Standing extends Person {
  level: number;
  xp_total: number;
  week_xp: number;
  /** XP in the requested period. */
  xp: number;
  period: string;
  periods: Record<Period, number>;
  streak: number;
  streak_state: string;
  day_ends_at: number;
  today_reviews: number;
  today_minutes: number;
  /** Review minutes today needs for the streak; 0 or missing when any review counts. */
  streak_minutes?: number;
  /** XP still counting toward the week that just reset, while this player's day began before it. */
  closing_xp?: number;
}

interface RecordHolder extends Person { value: number; detail: number; at: number; until: number; }
interface RecordBoard { window: string; unit: string; holders: RecordHolder[]; }
interface WeekInfo { ends_at: number; timezone: string; }

interface PersonalBest { xp: number; reviews: number; at: number; until: number; }
interface Quest { title: string; progress: number; target: number; done: boolean; reward: number; }
interface Achievement { id: string; title: string; description: string; reward: number; progress: number; target: number; unlocked: string | null; }
interface HeatCell { date: string; reviews: number; xp: number; frozen: boolean; }

/** `/api/profile/{user}` (`Profile` in src/game.rs). */
interface Profile extends Person {
  level: number;
  xp_total: number;
  xp_into_level: number;
  xp_for_next: number;
  week_xp: number;
  periods: Record<Period, number>;
  records: Record<RecordWindow, PersonalBest>;
  streak: number;
  streak_state: string;
  freezes: number;
  stored_freezes: number;
  freezes_enabled: boolean;
  freeze_earned_today: boolean;
  at_risk: boolean;
  day_ends_at: number;
  local_hour: number;
  today: {reviews: number; minutes: number; streak_minutes?: number; xp: number; max_combo: number; current_combo: number; new_cards: number};
  lifetime: {reviews: number; hours: number; days_active: number; best_streak: number; best_streak_at: number; first_day_at: number; best_day: number; best_combo: number; quests: number; conquered?: number};
  quests: Quest[];
  achievements: Achievement[];
  heatmap: HeatCell[];
  conquests: Conquest[];
  last_review_id: number;
  /** Present while the player's current day still counts toward the week that just reset. */
  closing_week?: ClosingWeek;
}

/** The closing week, still counting the player's current day (`ClosingWeek` in src/game.rs). */
interface ClosingWeek { xp: number; ends_at: number; }

/** A hard card learned at last (`Conquest` in src/game.rs). */
interface Conquest { at: number; lapses: number; answers: number; since: number; leech: boolean; }

interface WinnerTotals extends Person { history_start: string | null; day_wins: number; week_wins: number; month_wins: number; }
interface WinnerScope {
  players: WinnerTotals[];
  start_date?: string | null;
  player_count?: number;
  periods?: Record<WinMetric, number>;
}
/** `/api/winners` (`winner_history` in src/competition.rs). */
interface WinnerHistory {
  meta?: {start_date: string | null; start_source: string; time_zone: string | null; rollover_hour: number | null};
  shared: WinnerScope;
  lifetime: WinnerScope;
  waiting_players?: Person[];
}

const app = document.getElementById("app")!;
document.querySelector<HTMLElement>("[data-site-header]")!.dataset.siteSection = location.pathname.replace(/\/+$/, "") === "/records" ? "records" : "leaderboard";
document.querySelector<HTMLAnchorElement>("[data-community-link]")!.href = AnkiQuestSite.href("/community");
document.querySelector(".skip")!.addEventListener("click", event => { event.preventDefault(); app.focus(); });
const pct = (a: number, b: number) => b ? Math.min(100, Math.round(a / b * 100)) : 0;
const entries = <K extends string, V>(object: Record<K, V>) => Object.entries(object) as [K, V][];

let weekEnds = 0;
let renderId = 0, privatePageHidden = false;
addEventListener("ankiquest:locked",()=>{delete window.ankiquestSession;renderId++;app.innerHTML="";});
addEventListener("pagehide",()=>{privatePageHidden=true;renderId++;});
addEventListener("pageshow",()=>{privatePageHidden=false;});
const winnerState: {data: WinnerHistory | null; scope: WinScope; metric: WinMetric; error: boolean; request: number} = { data: null, scope: "shared", metric: "day", error: false, request: 0 };
const settingsOpen = () => AnkiQuestAvatars!.isOpen();
// Translated once the catalogs have loaded; render() waits for them.
let PERIODS: Record<Period, string>, WINDOWS: Record<RecordWindow, string>, RECORDS: Record<RecordKind, string>;
let labelsReady = false;
const labels = AnkiQuestI18n.ready.then(() => {
  PERIODS = { hour: aqText("This hour"), day: aqText("Today"), week: aqText("This week"), month: aqText("This month"), year: aqText("This year"), all: aqText("All time") };

  WINDOWS = { hour: aqText("Best hour"), day: aqText("Best 24 hours"), week: aqText("Best 7 days"), month: aqText("Best 30 days"), year: aqText("Best 365 days") };
  RECORDS = { ...WINDOWS, streak: aqText("Longest streak"), days: aqText("Days studied") };
  labelsReady = true;
});
const SINCE: Partial<Record<RecordKind, string>> = { streak: "ended", days: "since" };

const isPeriod = (name: string): name is Period => name in PERIODS;
function currentView(): Period | "records" {
  const name = location.pathname.replace(/\/+$/, "").split("/").pop()!;
  if (name === "records") return "records";
  return isPeriod(name) ? name : "week";
}

function currentPeriod() {
  const view = currentView();
  return view === "records" ? "week" : view;
}

function tabs(active: Period) {
  const links = entries(PERIODS).map(([name, label]) =>
    aqHtml`<a ${name === active ? 'aria-current="page"' : ""} href="${AnkiQuestSite.href("/" + name, location.hash)}">${label}</a>`);
  return aqHtml`<nav class="tabs" aria-label="Leaderboard period">${links.join("")}</nav>`;
}

/** Which players a board shows: a member's friends and groups, everyone public, or one group. */
type ScopeChoice = [value: string, label: string];
interface SocialOverview { groups: {id: number; name: string}[]; friends: unknown[] }
let scopeCache: {user: string; choices: Promise<ScopeChoice[]>} | null = null;
addEventListener("ankiquest:identity", () => { scopeCache = null; });
addEventListener("ankiquest-auth", () => { scopeCache = null; });

async function viewer(): Promise<OwnerSession | null> {
  return nativeAccount() ?? AnkiQuestSite.member();
}

function scopeChoices(session: OwnerSession): Promise<ScopeChoice[]> {
  if (scopeCache?.user !== session.user) {
    const social = request<SocialOverview>("/api/social/" + encodeURIComponent(session.user), {session});
    const choices = social.then((social): ScopeChoice[] => social.groups.length || social.friends.length
      ? [["circle", aqText("Friends and groups")], ["global", aqText("Everyone")], ...social.groups.map((group): ScopeChoice => ["group:" + group.id, group.name])]
      : []).catch(() => []);
    scopeCache = {user: session.user, choices};
  }
  return scopeCache.choices;
}

function storedScope() {
  try { return localStorage.getItem("ankiquestScope") || ""; } catch (e) { return ""; }
}

function scopePicker(choices: ScopeChoice[], scope: string) {
  if (!choices.length) return "";
  const selected = scope || "circle";
  const options = choices.map(([value, label]) => `<option value="${esc(value)}" ${value === selected ? "selected" : ""}>${esc(label)}</option>`).join("");
  return aqHtml`<label class="scope-picker"><span>Show</span><select id="board-scope">${options}</select></label>`;
}

document.addEventListener("change", event => {
  const target = event.target as HTMLSelectElement;
  if (target.id !== "board-scope") return;
  try { localStorage.setItem("ankiquestScope", target.value); } catch (e) {}
  render();
});

const ADDON_RELEASES = "https://github.com/float3/ankiquest/releases/latest";
const ANDROID_RELEASES = "https://github.com/float3/AnkiQuest-Android/releases/latest";

/** What AnkiQuest is and how to start, for visitors without an account on a server that takes sign-ups. */
function welcome() {
  return aqHtml`<section class="card welcome" aria-labelledby="welcome-title">
    <div class="eyebrow">New here?</div>
    <h2 id="welcome-title">Turn your Anki reviews into a friendly game.</h2>
    <p class="section-intro">AnkiQuest gives every review XP, keeps your streak, sets three small quests a day and lets you study alongside friends. You keep using Anki exactly as before.</p>
    <ol class="welcome-steps">
      <li><strong>Create an account</strong><span>A username and a password. Your profile stays private unless you make it public.</span></li>
      <li><strong>Connect Anki</strong><span>Install the add-on for Anki on your computer, or AnkiQuest for Android, and sign in there.</span></li>
      <li><strong>Study as usual</strong><span>Reviews sync on their own. Add friends or start a group with an invite link.</span></li>
    </ol>
    <div class="setting-actions"><a class="button-link primary" href="/login?signup=1">Create an account</a><a class="button-link" href="${ADDON_RELEASES}" rel="noopener">Get the Anki add-on</a><a class="button-link" href="${ANDROID_RELEASES}" rel="noopener">Get the Android app</a></div>
    <p class="settings-hint">Free and open source. Only the timing of your reviews reaches the server, never the content of your cards. <a href="/privacy">Privacy</a></p>
  </section>`;
}

function pageHero(title: string, description: string, note = "") {
  return aqHtml`<div class="page-hero"><div><div class="eyebrow">A little progress, every day</div><h1>${esc(title)}</h1><p>${esc(description)}</p></div>${note ? aqHtml`<div class="history-note">${note}</div>` : ""}</div>`;
}

function leaderboardKpis(rows: Standing[], period: Period) {
  const total = rows.reduce((sum, row) => sum + row.xp, 0);
  const reviews = rows.reduce((sum, row) => sum + row.today_reviews, 0);
  const bestStreak = Math.max(0, ...rows.map(row => row.streak));
  return aqHtml`<div class="kpis leaderboard-kpis">` +
    AnkiQuestSite.kpi(aqText("Community XP"), num(total), PERIODS[period], "gold") +
    AnkiQuestSite.kpi(aqText("Reviews today"), num(reviews), aqText("Every card is a little progress"), "blue") +
    AnkiQuestSite.kpi(aqText("Study buddies"), num(rows.length), aqText("Growing together")) +
    AnkiQuestSite.kpi(aqText("Longest active streak"), num(bestStreak) + " days", aqText("One study day at a time"), "green") + aqHtml`</div>`;
}

function span(window: string, at: number, until: number) {
  const hours = window === "hour" || window === "day";
  const format = new Intl.DateTimeFormat(AnkiQuestI18n.language, hours ? { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" } : { year: "numeric", month: "short", day: "numeric" });
  const end = Number(until) > Number(at) ? until : at;
  return typeof format.formatRange === "function" ? format.formatRange(new Date(at), new Date(end)) : format.format(new Date(at));
}

function records(rows: RecordBoard[], me: string) {
  const held = Object.fromEntries(rows.map(row => [row.window, row]));
  if (!rows.some(row => (row.holders ?? []).length)) return aqHtml`<p class="empty">Nobody has earned any XP yet.</p>`;
  return aqHtml`<div class="records">${entries(RECORDS).map(([window, label]) => {
    const row = held[window];
    const [best, ...rest] = row?.holders ?? [];
    if (!best) return "";
    const unit = row.unit === "days" ? aqText("days") : "XP";
    const when = SINCE[window] ? new Date(best.at).toLocaleDateString(AnkiQuestI18n.language, { year: "numeric", month: "short", day: "numeric" }) : span(window, best.at, best.until);
    const entry = (holder: RecordHolder, place: string | number, lead: boolean) => aqHtml`
      <div class="entry ${lead ? "lead" : "chase"}${holder.user === me ? " mine" : ""}">
        <span class="place">${place}</span>
        <a href="${AnkiQuestSite.href("/week", "#" + encodeURIComponent(holder.user))}">${esc(holder.display)}</a>
        <span class="value">${num(holder.value)}${lead ? aqHtml`<small>${unit}</small>` : ""}</span>
        <span class="behind">${lead ? "" : `−${num(best.value - holder.value)}`}</span>
        <div class="bar${lead ? " gold" : ""}"><i style="width:${pct(holder.value, best.value)}%"></i></div>
      </div>`;
    return aqHtml`<article class="card record">
      <h3>${label}<em>${SINCE[window] ? `${SINCE[window]} ${when}` : when}</em></h3>
      ${entry(best, "🏆", true)}
      ${best.detail ? aqHtml`<div class="detail">${num(best.detail)} reviews</div>` : ""}
      ${rest.map((holder, index) => entry(holder, index + 2, false)).join("")}
    </article>`;
  }).join("")}</div>`;
}

function bests(player: Profile) {
  const windows = entries(WINDOWS).map(([window, label]) => {
    const record = player.records?.[window] ?? { xp: 0, reviews: 0 };
    return aqHtml`<div class="stat"><b>${num(record.xp)}</b><span>${label.toLowerCase()} · ${num(record.reviews)} reviews</span></div>`;
  });
  windows.push(aqHtml`<div class="stat"><b>${player.lifetime.best_streak}</b><span>longest streak</span></div>`);
  windows.push(aqHtml`<div class="stat"><b>${num(player.lifetime.days_active)}</b><span>days studied</span></div>`);
  return aqHtml`<div class="stats">${windows.join("")}</div>`;
}

function boardTitle(period: Period) {
  if (period !== "week") return PERIODS[period];
  const left = weekEnds - Date.now();
  if (left <= 0) return aqText("This week");
  const h = Math.floor(left / 3600000), d = Math.floor(h / 24);
  const when = new Date(weekEnds).toLocaleString(AnkiQuestI18n.language, { weekday: "short", hour: "2-digit", minute: "2-digit" });
  return aqHtml`This week · resets in ${d ? `${d}d ${h % 24}h` : h ? `${h}h` : "<1h"}<span class="sub"> · ${when}</span>`;
}

/** The week that reset on the board but still counts this player's day, until that day ends. */
function closingWeek(closing: ClosingWeek | undefined) {
  if (!closing || !(closing.ends_at > Date.now())) return "";
  const at = new Date(closing.ends_at).toLocaleTimeString(AnkiQuestI18n.language, { hour: "2-digit", minute: "2-digit" });
  return aqHtml`<section class="notice closing-week wide" role="status">${esc(aqText`Last week still counting: ${num(closing.xp)} XP · closes at ${at}`)}</section>`;
}

function streakIcon(state: string) {
  const flame = 'M12 2C13 6 18 8 18 13C18 17 15 20 11 20C7 20 4 17 4 13C4 10 6 7 8 5C8 8 9 9 10 10C12 8 13 5 12 2Z';
  const waiting = state === "pending" || state === "unknown";
  const badge = waiting ? '<g class="streak-clock"><circle cx="18" cy="18" r="5" fill="var(--card)" stroke="currentColor" stroke-width="1.6"/><path d="M18 15v3h2" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></g>' : state === "protected" ? '<path class="streak-snowflake" d="M11 9v8M7.5 11l7 4M7.5 15l7-4" fill="none" stroke="var(--card)" stroke-width="1.5" stroke-linecap="round"/>' : '';
  return `<svg class="streak-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="${flame}" fill="${waiting ? 'none' : 'currentColor'}" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>${badge}</svg>`;
}
function streakBadge(row: Standing) {
  if (!(row.streak > 0)) return "";
  // A cached result from an ended Anki day cannot describe today's activity.
  const expired = row.day_ends_at && row.day_ends_at <= Date.now();
  const known = typeof row.today_reviews === "number";
  const needed = row.streak_minutes ?? 0;
  const counted = needed > 0 ? row.streak_state === "studied" : row.today_reviews > 0;
  const state = expired || !known ? "unknown" : counted ? "studied" : row.streak_state === "protected" ? "protected" : "pending";
  const progress = aqText`${num(Math.min(row.today_minutes, needed))} of ${num(needed)} minutes today`;
  const status = state === "studied" ? aqText("Studied today") : needed > 0 && state !== "unknown" ? progress : state === "protected" ? aqText("Streak protected · Not studied yet") : state === "pending" ? aqText("Not studied yet") : aqText("Refresh to see today's study status");
  const label = aqText`${num(row.streak)} day streak · ${status}`;
  return aqHtml`<span class="streak" data-streak-state="${state}" role="img" tabindex="0" aria-label="${esc(label)}" title="${esc(label)}">${streakIcon(state)}${num(row.streak)}</span>`;
}
function board(rows: Standing[], me: string) {
  if (!rows.length) return aqHtml`<p class="empty">Nobody has uploaded reviews yet.</p>`;
  if (rows.every(row => row.xp === 0)) return aqHtml`<p class="empty">No XP in this period yet.</p>`;
  const lead = Math.max(1, rows[0].xp);
  return aqHtml`<ol class="board">${rows.map((r, i) => aqHtml`
    <li class="${[r.user === me && "me", i === 0 && r.xp > 0 && "lead"].filter(Boolean).join(" ")}">
      <span class="rank">${["👑", "🥈", "🥉"][i] ?? i + 1}</span>
      <div class="player-cell">${AnkiQuestSite.avatar(r.user, r.display)}<div class="player-progress">
        <div class="line">
          <a class="name" href="#${encodeURIComponent(r.user)}">${esc(r.display)}</a>
          ${streakBadge(r)}
          <span class="lvl">Lv ${r.level}</span>
        </div>
        <div class="bar"><i style="width:${pct(r.xp, lead)}%"></i></div>
      </div>
      </div><div class="xp"><b>${num(r.xp)}</b><span>${num(r.today_reviews)} today</span>${r.closing_xp != null && r.day_ends_at > Date.now() ? aqHtml`<span class="closing-xp">${esc(aqText`${num(r.closing_xp)} XP still counting for last week`)}</span>` : ""}</div>
    </li>`).join("")}
  </ol><p class="streak-legend">${[["studied",aqText("Studied today")],["pending",aqText("Not studied yet")],["protected",aqText("Streak protected")]].map(([state,label]) => aqHtml`<span data-streak-state="${state}">${streakIcon(state)}${label}</span>`).join("")}</p>`;
}

const winCount = (value: unknown) => Number.isFinite(Number(value)) ? Math.max(0, Number(value)) : 0;
function winnerContent() {
  const data = winnerState.data, shared = winnerState.scope === "shared", metric = winnerState.metric;
  const heading = aqHtml`<div class="win-heading"><h2 id="winner-history-title">Winner history</h2><a id="winner-calendar-link" href="${AnkiQuestSite.href("/community?period=" + metric, "#calendar")}">Open winner calendar →</a></div>`;
  const controls = aqHtml`<div class="win-controls"><div class="win-scopes" role="group" aria-label="Winner history scope"><button type="button" data-win-scope="shared" aria-pressed="${shared}">Shared history</button><button type="button" data-win-scope="lifetime" aria-pressed="${!shared}">Lifetime</button></div><label class="win-sort" for="win-metric">Rank by <select id="win-metric">${[["day",aqText("Days won")],["week",aqText("Weeks won")],["month",aqText("Months won")]].map(([key,label])=>aqHtml`<option value="${key}"${metric===key?" selected":""}>${label}</option>`).join("")}</select></label></div>`;
  const failure = winnerState.error ? aqHtml`<p role="status">Winner history could not ${data ? aqText("refresh; showing the last loaded results") : "load"}. <button type="button" data-win-retry>Try again</button></p>` : "";
  if (!data) return heading + controls + failure + (winnerState.error ? "" : aqHtml`<p role="status">Loading winner history…</p>`);
  const scope = shared ? data.shared : data.lifetime;
  const ready = !shared || winCount(data.shared?.player_count) >= 2;
  const key = `${metric}_wins` as const;
  const players = (scope?.players || []).filter(player=>player.history_start).sort((a,b) => winCount(b[key])-winCount(a[key]) || lexical(String(a.display),String(b.display)) || lexical(String(a.user),String(b.user)));
  const waiting = (data.waiting_players || []).slice().sort((a,b) => lexical(String(a.display),String(b.display)) || lexical(String(a.user),String(b.user)));
  const lifetimeStart = data.meta?.start_source === "configured"
    ? `since the configured server start${data.meta.start_date ? ` on ${esc(day(data.meta.start_date))}` : ""}`
    : `across available retained history${data.meta?.start_date ? ` from ${esc(day(data.meta.start_date))}` : ""} (which may predate this server)`;
  const note = shared
    ? ready ? `Same window for all ${num(winCount(data.shared.player_count))} players with recorded history, from ${esc(day(scope.start_date))}. The window starts with the newest player’s first recorded study date. Only full weeks and months within that window count.`
      : aqText("Shared history needs at least two players with recorded study history. Earlier solo wins remain in Lifetime.")
    : `Raw win totals ${lifetimeStart}, including earlier solo wins and partial periods. Players may have different amounts of recorded history.`;
  let rank = 0, previous: number | null = null;
  const playerRows = players.map((player,index) => {
    const score = winCount(player[key]);
    if (previous !== score) rank = index + 1;
    previous = score;
    return aqHtml`<tr><td>${ready && score > 0 ? rank : "—"}</td><td><a href="#${esc(encodeURIComponent(player.user))}">${esc(player.display)}</a><small>${ready ? `History from ${esc(day(player.history_start))}` : aqText("Waiting for a second player")}</small></td>${(["day","week","month"] as const).map(kind=>aqHtml`<td class="${kind===metric?"win-selected":""}">${ready ? num(winCount(player[`${kind}_wins`])) : "—"}</td>`).join("")}</tr>`;
  }).join("");
  const waitingRows = waiting.map(player=>aqHtml`<tr class="win-waiting"><td>—</td><td><a href="#${esc(encodeURIComponent(player.user))}">${esc(player.display)}</a><small>Waiting for study history</small></td><td>—</td><td>—</td><td>—</td></tr>`).join("");
  const table = players.length || waiting.length ? aqHtml`<table id="winner-history-table" class="win-table"><caption class="win-sr">${shared?aqText("Shared history"):aqText("Lifetime")} wins, ranked by ${aqText({day:"days",week:"weeks",month:"months"}[metric])} won</caption><thead><tr><th scope="col"><span class="win-sr">Rank</span>#</th><th scope="col">Player</th>${[["day",aqText("Days")],["week",aqText("Weeks")],["month",aqText("Months")]].map(([kind,label])=>aqHtml`<th scope="col"${kind===metric?' aria-sort="descending" class="win-selected"':""}>${label}<span class="win-sr"> won</span></th>`).join("")}</tr></thead><tbody>${playerRows}${waitingRows}</tbody></table>` : aqHtml`<p class="empty">The first recorded study sessions will start your winner history.</p>`;
  const cutoff = data.meta?.time_zone ? aqText` Cutoff: ${esc(String(data.meta.rollover_hour ?? 0).padStart(2,"0"))}:00 ${esc(data.meta.time_zone)}.` : "";
  const periods = shared && ready && scope.periods ? aqHtml`<p>${num(winCount(scope.periods.day))} finalized days · ${num(winCount(scope.periods.week))} full weeks · ${num(winCount(scope.periods.month))} full months in this window.</p>` : "";
  return heading + controls + failure + aqHtml`<p id="winner-window">${note}</p>` + periods + table + aqHtml`<p>Finalized results only. Tied winners each earn a win; equal win totals share a rank.${cutoff}</p>`;
}
function winnerSection() { return aqHtml`<section id="winner-history" class="card winner-history wide" aria-labelledby="winner-history-title">${winnerContent()}</section>`; }
function refreshWinnerContent() {
  const section = document.getElementById("winner-history");
  if (section) section.innerHTML = winnerContent();
}
async function loadWinners() {
  const id = ++winnerState.request;
  try {
    const data = await request<WinnerHistory>("/api/winners");
    if (!data.shared || !Array.isArray(data.shared.players) || !Array.isArray(data.lifetime?.players)) throw new Error(aqText("Incomplete winner history"));
    if (id !== winnerState.request) return;
    winnerState.data = data;
    winnerState.error = false;
  } catch (error) {
    if (id !== winnerState.request) return;
    winnerState.error = true;
  }
  refreshWinnerContent();
}

app.addEventListener("click", event => {
  const button = (event.target as Element).closest("button");
  const scope = button?.dataset.winScope;
  if (scope === "shared" || scope === "lifetime") {
    winnerState.scope = scope;
    refreshWinnerContent();
    app.querySelector<HTMLElement>(`[data-win-scope="${winnerState.scope}"]`)?.focus();
  }
  if (button?.hasAttribute("data-win-retry")) { winnerState.error = false; refreshWinnerContent(); loadWinners(); }
});
app.addEventListener("change", event => {
  const target = event.target as HTMLSelectElement;
  if (target.id === "win-metric" && (target.value === "day" || target.value === "week" || target.value === "month")) {
    winnerState.metric = target.value;
    refreshWinnerContent();
    document.getElementById("win-metric")?.focus();
  }
});

function heat(cells: HeatCell[]) {
  if (!cells.length) return aqHtml`<span class="sub">Your study history will appear here.</span>`;
  const max = Math.max(1, ...cells.map(c => c.reviews));
  const first = new Date(cells[0].date + "T00:00:00Z");
  const pad = (first.getUTCDay() + 6) % 7;
  const level = (n: number) => n === 0 ? 0 : Math.min(4, 1 + Math.floor(n / max * 3.999));
  return aqHtml`<i class='pad'></i>`.repeat(pad) + cells.map(c =>
    aqHtml`<i class="${c.frozen ? "frozen" : "l" + level(c.reviews)}" title="${c.date}: ${c.frozen ? aqText("streak freeze") : num(c.reviews) + " reviews, " + num(c.xp) + " XP"}"></i>`).join("");
}

const freezeCount = (value: unknown) => Math.max(0, Math.min(3, Math.floor(Number(value) || 0)));

function freezeSlots(count: number, enabled: boolean) {
  return aqHtml`<div class="freeze-display${enabled ? "" : " off"}" role="img" aria-label="${count} of 3 streak freezes ${enabled ? "ready" : aqText("saved; protection off")}">
    ${Array.from({ length: 3 }, (_, i) => aqHtml`<span class="freeze-slot${i < count ? " filled" : ""}">
      <svg viewBox="0 0 40 48" aria-hidden="true" focusable="false">
        <path class="shell" d="M20 2 33 11 36 30 26 44H14L4 30 7 11Z"/>
        <path class="facet facet-left" d="M20 2 16 17 14 44 4 30 7 11Z"/>
        <path class="facet facet-top" d="M20 2 33 11 25 17 16 17 7 11Z"/>
        <path class="facet facet-right" d="m25 17 8-6 3 19-10 14-2-12Z"/>
        <path class="flame" d="M21 13c2 7-6 9-3 14 2-1 4-4 4-6 5 4 7 7 5 12-2 5-10 7-14 2-5-6 3-11 4-15 1 2 1 3 1 4 3-3 2-7 3-11Z"/>
        <path class="glint" d="m10 12 6-5m-6 9-1 6m18 15-3 4"/>
      </svg>
    </span>`).join("")}
  </div>`;
}

function streakFreezes(player: Profile, owner: boolean) {
  const count = freezeCount(player.stored_freezes ?? player.freezes), enabled = player.freezes_enabled === true;
  const state = !enabled ? "off" : count === 3 ? "full" : count ? "ready" : "empty";
  const badge = !enabled ? aqText("Off") : count === 3 ? aqText("Full") : aqText("On");
  const title = enabled ? `${count} of 3 freezes ready` : count ? `${count} of 3 freezes saved` : aqText("A little backup for your streak");
  const detail = !enabled
    ? count ? aqText("Protection is off. Your saved freezes are kept.") : aqText("Opt in, then finish all three daily quests to earn a freeze.")
    : count === 3 ? player.freeze_earned_today ? aqText("You earned a freeze today. You're fully stocked.") : aqText("Fully stocked. Each freeze can protect one missed Anki day.")
    : player.freeze_earned_today ? aqText("You earned a freeze today. Earn another with tomorrow's quests.")
    : player.quests.length > 0 && player.quests.every(quest => quest.done) ? aqText("Today's quests are already complete. Earn a freeze with tomorrow's quests.")
    : aqText("Finish all three daily quests to earn one freeze. At most one per day.");
  return aqHtml`<div class="freeze-panel" data-state="${state}">
    <div class="freeze-heading"><h3>Streak freezes <span class="freeze-badge">${badge}</span></h3>
      ${owner ? aqHtml`<a id="manage-freezes" class="button-link freeze-button" href="${settingsLink("#streak-protection")}">Manage streak freezes</a>` : ""}</div>
    <div class="freeze-main">${freezeSlots(count, enabled)}<div><strong>${title}</strong><p>${detail}</p></div></div>
  </div>`;
}

function conquests(p: Profile) {
  const items = p.conquests;
  if (!items.length) return "";
  const total = p.lifetime.conquered ?? items.length;
  return aqHtml`<section class="card wide" id="conquests"><div class="card-head"><div><h2>Hard-won cards</h2><p>${total === 1 ? aqText("One card that kept slipping away is learned at last.") : aqText`${num(total)} cards that kept slipping away are learned at last.`}</p></div><span class="symbol" aria-hidden="true">🏆</span></div><div class="ach">${items.map(c => aqHtml`
  <a class="got conquest" href="/conquered/${encodeURIComponent(p.user)}/${c.at}"><b>${c.leech ? aqText("Leech tamed") : aqText("Card conquered")}</b>${aqText`Forgotten ${c.lapses} times`}
    <div class="sub">${esc(new Date(c.at).toLocaleDateString(AnkiQuestI18n.language, {year: "numeric", month: "short", day: "numeric"}))} · ${aqText("Share")} →</div>
  </a>`).join("")}</div></section>`;
}

/** A link to a card on /settings, where an owner changes what the leaderboard shows. */
const settingsLink = (card = "") => AnkiQuestSite.href("/settings", card);

function receivingNotificationsEntry() {
  return aqHtml`<section class="card deck-sharing"><h2>Deck completion notifications</h2>
    <p class="sub">Choose whose deck completion alerts you receive, even if you have not studied yet.</p>
    <div class="actions"><a class="button-link" id="manage-received-notifications" href="${settingsLink()}">Notifications I receive</a></div>
  </section>`;
}

function view(p: Profile, rows: Standing[], canEditAvatar = false) {
  const unlocked = p.achievements.filter(a => a.unlocked).length;
  const achievements = [...p.achievements].sort((a, b) => Number(!!b.unlocked) - Number(!!a.unlocked) || pct(b.progress, b.target) - pct(a.progress, a.target));
  return aqHtml`
${pageHero(p.display, aqText("A study journey, one session at a time. Quests, milestones, and the progress that adds up."), aqHtml`<a href="` + AnkiQuestSite.href("/" + currentPeriod()) + aqHtml`">` + aqText("← Back to leaderboard") + aqHtml`</a>`)}
<div class="embedded-profile-title"><strong>${esc(p.display)}</strong><a href="${AnkiQuestSite.href("/" + currentPeriod())}">← Leaderboard</a></div>
<div class="profile-layout">
${AnkiQuestAki.profile(p)}
<section class="card profile-hero wide" aria-label="Level progress">
  <div class="level"><div><small>Level</small>${p.level}</div></div>
  <div>
    <h2>Keep the progress going.</h2>
    <div class="sub">${num(p.xp_total)} XP earned so far</div>
    <div class="name avatar-name">${AnkiQuestSite.avatar(p.user,p.display)}<span>${esc(p.display)}</span></div>
    ${canEditAvatar ? aqHtml`<div><button class="avatar-edit-button" type="button" id="manage-avatar">Profile picture</button></div>` : ''}
    <div class="bar gold"><i style="width:${pct(p.xp_into_level, p.xp_for_next)}%"></i></div>
    <div class="sub">${num(p.xp_into_level)} / ${num(p.xp_for_next)} XP to level ${p.level + 1} · ${num(p.xp_total)} total</div>
  </div>
</section>
${p.at_risk ? (p.today.streak_minutes ?? 0) > 0 ? aqHtml`<section class="notice risk wide">Your ${p.streak} day streak is waiting: ${Math.min(p.today.minutes, p.today.streak_minutes!)} of ${p.today.streak_minutes!} minutes today.</section>` : aqHtml`<section class="notice risk wide">Your ${p.streak} day streak is waiting for today's reviews.</section>` : ""}
${closingWeek(p.closing_week)}
<div class="kpis wide">
  ${AnkiQuestSite.kpi(aqText("Day streak"), num(p.streak), (p.today.streak_minutes ?? 0) > 0 ? aqText`${num(Math.min(p.today.minutes, p.today.streak_minutes!))} of ${num(p.today.streak_minutes!)} minutes today` : aqText("Keep showing up"), "gold")}
  ${AnkiQuestSite.kpi(aqText("Streak freezes"), freezeCount(p.stored_freezes ?? p.freezes) + "/3", p.freezes_enabled ? aqText("Ready to protect your progress") : aqText("Protection paused"), "blue")}
  ${AnkiQuestSite.kpi(aqText("Reviews today"), num(p.today.reviews), aqText("One card at a time"))}
  ${AnkiQuestSite.kpi(aqText("XP today"), num(p.today.xp), aqText("Today’s progress"), "green")}
</div>
<section class="card"><h2 class="section-title">Daily quests</h2>${p.quests.map(q => aqHtml`
  <div class="quest ${q.done ? "done" : ""}">
    <div class="row"><span class="title">${q.done ? "✓ " : ""}${esc(q.title)}</span><span class="sub">${num(q.progress)} / ${num(q.target)} · +${q.reward} XP</span></div>
    <div class="bar ${q.done ? "ok" : ""}"><i style="width:${pct(q.progress, q.target)}%"></i></div>
  </div>`).join("")}
  <div class="sub">Finish all three for a +100 XP bonus.</div>
  ${streakFreezes(p, canEditAvatar)}
</section>
${canEditAvatar ? aqHtml`<section class="card deck-sharing"><div class="card-head"><div><h2>A little encouragement</h2><p>Share your progress with the people who keep you going.</p></div><span class="symbol" aria-hidden="true">↗</span></div>
  <h3>Deck completion notifications</h3><p class="sub">Choose who hears when you finish a deck, or whose completion alerts you receive. Sharing your own decks is off by default.</p>
  <div class="actions"><a class="button-link" id="manage-decks" href="${settingsLink("#deck-sharing")}">Manage deck notifications</a><a class="button-link" id="manage-received-notifications" href="${settingsLink("#deck-alerts")}">Notifications I receive</a></div>
  <p class="settings-hint">Visit <a href="${AnkiQuestSite.href("/community")}">Community</a> for friendly challenges, shared milestones, and reminder preferences.</p>
</section>` : ""}
<section class="card wide"><div class="card-head"><div><h2>Small steps, steady progress</h2><p>Your last 26 weeks of studying.</p></div></div><div class="heat" role="img" aria-label="Study activity over the last 26 weeks; each square represents one day">${heat(p.heatmap)}</div></section>
<section class="card wide"><div class="card-head"><div><h2>${boardTitle(currentPeriod())}</h2><p>Your place in the community.</p></div></div>${tabs(currentPeriod())}${board(rows, p.user)}</section>
${winnerSection()}
<section class="card wide"><h2 class="section-title">Personal bests</h2>${bests(p)}</section>
<section class="card wide"><h2 class="section-title">A lifetime of learning</h2><div class="stats">
  <div class="stat"><b>${num(p.lifetime.reviews)}</b><span>reviews</span></div>
  <div class="stat"><b>${num(p.lifetime.hours)}</b><span>hours</span></div>
  <div class="stat"><b>${p.lifetime.best_streak}</b><span>best streak</span></div>
  <div class="stat"><b>${num(p.lifetime.best_combo)}</b><span>best combo</span></div>
</div></section>
<section class="card wide"><div class="card-head"><div><h2>Milestones along the way</h2><p>${unlocked} of ${p.achievements.length} achievements unlocked.</p></div><span class="symbol" aria-hidden="true">✦</span></div><div class="ach">${achievements.map(a => aqHtml`
  <div class="${a.unlocked ? "got" : "locked"}"><b>${esc(a.title)}</b>${esc(a.description)}
    ${a.unlocked ? aqHtml`<div class="sub">${a.unlocked} · +${a.reward} XP</div>` : aqHtml`<div class="bar"><i style="width:${pct(a.progress, a.target)}%"></i></div>`}
  </div>`).join("")}</div>
</section>
${conquests(p)}
</div>`;
}

async function render() {
  if (!labelsReady) await labels;
  if (settingsOpen() || privatePageHidden) return;
  const id = ++renderId;
  const canRender = () => id === renderId && !settingsOpen();
  const user = decodeURIComponent(location.hash.slice(1));
  try {
    let me = "";
    try { me = localStorage.getItem("ankiquestPlayer") || ""; } catch (e) {}
    const session = await viewer();
    const newcomer = !session && !AnkiQuestSite.embedded && !!(await AnkiQuestSite.status())?.registration;
    const choices = session ? await scopeChoices(session) : [];
    const stored = storedScope(), scope = choices.some(([value]) => value === stored) ? stored : "";
    const auth: RequestInit = session?.token ? {headers: AnkiQuestSite.ownerHeaders(session)} : {};
    const scoped = (url: string) => scope ? url + (url.includes("?") ? "&" : "?") + "scope=" + encodeURIComponent(scope) : url;
    if (!canRender()) return;
    if (currentView() === "records") {
      const held = await request<RecordBoard[]>(scoped("/api/records"), auth);
      if (!canRender()) return;
      document.title = aqText("Records · AnkiQuest");
      AnkiQuestSite.setProfile(me);
      app.innerHTML = pageHero(aqText("Records worth chasing."), aqText("Personal bests become shared milestones. Celebrate the sessions, streaks, and study days that raised the bar."), aqHtml`<strong>All-time achievements</strong>A new record starts with one session.`) + scopePicker(choices, scope) + records(held, me) + receivingNotificationsEntry();
      return;
    }
    const period = currentPeriod();
    loadWinners();
    const [rows, week] = await Promise.all([
      request<Standing[]>(scoped("/api/leaderboard?period=" + period), auth),
      request<WeekInfo>("/api/week").catch(() => null),
    ]);
    weekEnds = week?.ends_at ?? 0;
    if (user) {
      const res = await send("/api/profile/" + encodeURIComponent(user), auth);
      if (res.ok) {
        const p: Profile = await res.json();
        if (!canRender()) return;
        document.title = `${p.display} · AnkiQuest`;
        AnkiQuestSite.setProfile(p.user, true);
        try { localStorage.setItem("ankiquestPlayer", p.user); } catch (e) {}
        const access = await AnkiQuestSite.status();
        if (!canRender()) return;
        const canEditAvatar = access?.member?.user === p.user || nativeAccount()?.user === p.user;
        app.innerHTML = view(p, rows, canEditAvatar);
        if (canEditAvatar) document.getElementById("manage-avatar")!.addEventListener("click", () => AnkiQuestAvatars!.open({user:p.user,display:p.display}));
        return;
      }
    }
    if (!canRender()) return;
    document.title = aqText("Leaderboard · AnkiQuest");
    AnkiQuestSite.setProfile(me);
    app.innerHTML = pageHero(aqText("Every session counts."), aqText("A little friendly competition, a little more motivation. See how your study community is growing together."), aqHtml`<strong>Made for steady progress</strong>Standings update as members sync.`) + (newcomer ? welcome() : "") + leaderboardKpis(rows, period) + tabs(period) + scopePicker(choices, scope) + aqHtml`<section class="card"><div class="card-head"><div><h2>${boardTitle(period)}</h2><p>XP earned together, one study session at a time.</p></div><span class="chip blue">${num(rows.length)} players</span></div>${board(rows, me)}</section>` + winnerSection() + receivingNotificationsEntry();
  } catch (e) {
    if (!canRender()) return;
    app.innerHTML = aqHtml`<div class="notice error" role="alert">Could not reach the server. <button type="button" id="retry-load">Try again</button></div>`;
    document.getElementById("retry-load")!.addEventListener("click", render);
  } finally {
    if (canRender()) app.setAttribute("aria-busy", "false");
  }
}

addEventListener("hashchange", render);
document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && render());
setInterval(render, 30000);
render();
