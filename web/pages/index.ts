export {};

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
  /** XP in the requested period; missing from older servers. */
  xp?: number;
  period: string;
  periods: Record<Period, number>;
  streak: number;
  streak_state: string;
  day_ends_at: number;
  today_reviews: number;
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
  today: {reviews: number; minutes: number; xp: number; max_combo: number; current_combo: number; new_cards: number};
  lifetime: {reviews: number; hours: number; days_active: number; best_streak: number; best_streak_at: number; first_day_at: number; best_day: number; best_combo: number; quests: number};
  quests: Quest[];
  achievements: Achievement[];
  heatmap: HeatCell[];
  last_review_id: number;
}

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

interface DeckSetting { id: string; name: string; enabled: boolean; recipients: string[]; }
/** `/api/decks/{user}` (`decks::Settings`). */
interface DeckSettings { decks: DeckSetting[]; recipients: Person[]; nudges: boolean; celebrations: boolean; }
interface DeckUpdate { decks: {id: string; enabled: boolean; recipients: string[]}[]; nudges: boolean; celebrations: boolean; }

/** `/api/deck-subscriptions/{user}` (`IncomingPreferences` in src/main.rs). */
interface NotificationSettings {
  enabled: boolean;
  muted_senders: string[];
  unsubscribed_senders: string[];
  sharing_senders: string[];
  senders: Person[];
}
interface NotificationUpdate { enabled: boolean; unsubscribed_senders: string[]; }

/** `/api/streak-freezes/{user}` (`FreezeSettings` in src/main.rs). */
interface FreezeSettings { enabled: boolean; freezes: number; capacity: number; }

const app = document.getElementById("app")!;
document.querySelector<HTMLElement>("[data-site-header]")!.dataset.siteSection = location.pathname.replace(/\/+$/, "") === "/records" ? "records" : "leaderboard";
document.querySelector<HTMLAnchorElement>("[data-community-link]")!.href = AnkiQuestSite.href("/community");
document.querySelector(".skip")!.addEventListener("click", event => { event.preventDefault(); app.focus(); });
const escapes: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" };
const esc = (s: unknown) => String(s).replace(/[&<>"]/g, c => escapes[c]);
const num = (n: number) => n.toLocaleString(AnkiQuestI18n.language);
const pct = (a: number, b: number) => b ? Math.min(100, Math.round(a / b * 100)) : 0;
const dialogElement = (id: string) => document.getElementById(id) as HTMLDialogElement;
const entries = <K extends string, V>(object: Record<K, V>) => Object.entries(object) as [K, V][];

let weekEnds = 0;
let renderId = 0, privatePageHidden = false, knownMember: string | null | undefined;
function closePersonalSettings() {
  renderId++;
  for(const id of ["deck-sharing","streak-freezes","notification-preferences"]) {
    const dialog=dialogElement(id);if(dialog.open)dialog.close();
  }
}
addEventListener("ankiquest:locked",()=>{settingsTokens.clear();delete window.ankiquestSession;closePersonalSettings();app.innerHTML="";});
addEventListener("ankiquest:identity",event=>{const user=event.detail.user;if(knownMember&&knownMember!==user){settingsTokens.clear();closePersonalSettings();}knownMember=user;});
addEventListener("pagehide",()=>{privatePageHidden=true;settingsTokens.clear();closePersonalSettings();});
addEventListener("pageshow",()=>{privatePageHidden=false;});
const winnerState: {data: WinnerHistory | null; scope: WinScope; metric: WinMetric; error: boolean; request: number} = { data: null, scope: "shared", metric: "day", error: false, request: 0 };
const settingsOpen = () => AnkiQuestAvatars!.isOpen() || dialogElement("deck-sharing").open || dialogElement("streak-freezes").open || dialogElement("notification-preferences").open;
// Page memory only: share credentials between settings dialogs, never between players.
const settingsTokens = new Map<string, string>();
let nativeSettingsCleared = false, previousNativeSettings = nativeSettingsIdentity();
addEventListener("ankiquest-auth", () => {
  const identity = nativeSettingsIdentity();
  nativeSettingsCleared = !identity;
  if (!identity || identity !== previousNativeSettings) {
    settingsTokens.clear();
    const recipientDialog = document.getElementById("notification-preferences") as HTMLDialogElement | null;
    if (recipientDialog?.open) recipientDialog.close();
  }
  previousNativeSettings = identity;
});
function nativeSettingsToken(user: string) {
  const session = window.ankiquestSession;
  return session?.user === user && typeof session.token === "string" && session.token.trim()
    ? session.token.trim() : "";
}
function nativeSettingsIdentity() {
  const session = window.ankiquestSession;
  return typeof session?.user === "string" && session.user && typeof session.token === "string" && session.token.trim()
    ? JSON.stringify([session.user, session.token.trim()]) : "";
}
function settingsToken(user: string) {
  return nativeSettingsToken(user) || settingsTokens.get(user) || "";
}
function forgetSettingsToken(user: string, failedToken: string) {
  if (settingsTokens.get(user) === failedToken) settingsTokens.delete(user);
  if (nativeSettingsToken(user) === failedToken) delete window.ankiquestSession;
}

type Say = (message: string, error?: boolean) => void;
interface SettingsAuth<T> {
  user: string;
  form: HTMLFormElement;
  input: HTMLInputElement;
  controller: AbortController;
  load(token: string, session: OwnerSession | null): Promise<T>;
  clear(): void;
  show(data: T): void;
  say: Say;
  useSession(session: OwnerSession): void;
}

function connectSettingsAuth<T>({ user, form, input, controller, load, clear, show, say, useSession }: SettingsAuth<T>) {
  let busy = false, unlocked = false, generation = 0, queued = false;
  let activeToken = "", fromNative = false, observedNative = nativeSettingsIdentity(), cookieSession: OwnerSession | null = null;
  const content = form.parentElement!;
  const button = form.querySelector("button")!;
  const reset = (message = "") => {
    if (controller.signal.aborted) return;
    unlocked = false;
    activeToken = "";
    fromNative = false;
    cookieSession = null;
    observedNative = nativeSettingsIdentity();
    clear();
    input.value = "";
    input.required = true;
    form.hidden = false;
    content.replaceChildren(form);
    say(message, Boolean(message));
    input.focus();
  };
  const unlock = async (token: string, session: OwnerSession | null = null) => {
    if (busy || unlocked || controller.signal.aborted) return;
    if (!token && !session) { input.focus(); return; }
    const attempt = generation;
    const startedNative = Boolean(token) && nativeSettingsToken(user) === token;
    activeToken = token;
    fromNative = startedNative;
    busy = true;
    input.value = "";
    form.hidden = true;
    button.disabled = true;
    say(aqText("Loading settings…"));
    try {
      const data = await load(token, session);
      if (!controller.signal.aborted && attempt === generation) {
        if (startedNative && nativeSettingsToken(user) !== token) { queued = true; return; }
        // Native credentials remain owned by the host, so clearing its session
        // cannot leave a second copy available for a later dialog.
        if (startedNative || session || nativeSettingsToken(user) === token) {
          settingsTokens.delete(user);
        } else {
          const connected = await AnkiQuestSite.connectMember(user, token, () => !controller.signal.aborted && attempt === generation);
          if (controller.signal.aborted || attempt !== generation) return;
          nativeSettingsCleared = false;
          useSession(connected);
          if (connected.token) settingsTokens.set(user, connected.token);
          else { settingsTokens.delete(user); cookieSession = connected; }
        }
        show(data);
        unlocked = true;
        say("");
      }
    } catch (caught) {
      const error = caught as Error;
      if (!controller.signal.aborted && attempt === generation && error.name !== "AbortError") {
        form.hidden = false;
        input.required = !settingsToken(user) && !cookieSession;
        say(error.message, true);
        (input.required ? input : button).focus();
      }
    } finally {
      busy = false;
      button.disabled = false;
      if (queued || observedNative !== nativeSettingsIdentity()) {
        queued = false;
        autoUnlock();
      }
    }
  };
  form.addEventListener("submit", event => {
    event.preventDefault();
    const token = input.value.trim() || settingsToken(user);
    unlock(token, token ? null : cookieSession);
  }, { signal: controller.signal });
  const autoUnlock = () => {
    if (controller.signal.aborted) return;
    // An explicit host clear also revokes a cookie session loaded before the first host event.
    if (nativeSettingsCleared && (cookieSession || activeToken)) {
      generation++;
      reset();
      return;
    }
    const native = nativeSettingsToken(user), identity = nativeSettingsIdentity();
    if (identity !== observedNative) {
      observedNative = identity;
      if (activeToken && native === activeToken) {
        fromNative = true;
        settingsTokens.delete(user);
      } else if (activeToken || fromNative || native) {
        generation++;
        settingsTokens.delete(user);
        reset();
      }
    }
    const token = settingsToken(user);
    if (!token || unlocked) return;
    if (busy) queued = true;
    else unlock(token);
  };
  // The native WebView can supply its saved account after the dialog has opened.
  addEventListener("ankiquest-auth", autoUnlock, { signal: controller.signal });
  autoUnlock();
  AnkiQuestSite.member(user).then(session => {
    if (controller.signal.aborted || nativeSettingsCleared || !session || busy || unlocked || input.value.trim() || settingsToken(user) ||
        (nativeSettingsIdentity() && !nativeSettingsToken(user))) return;
    cookieSession = session;
    unlock("", session);
  });
  return reset;
}
let notificationDialogSession = 0;
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
  const native = window.ankiquestSession;
  if (typeof native?.user === "string" && native.user && typeof native.token === "string" && native.token.trim()) return {user: native.user, token: native.token.trim()};
  return AnkiQuestSite.member();
}

function scopeChoices(session: OwnerSession): Promise<ScopeChoice[]> {
  if (scopeCache?.user !== session.user) {
    const request = AnkiQuestSite.readJSON<SocialOverview>("/api/social/" + encodeURIComponent(session.user), {headers: AnkiQuestSite.ownerHeaders(session)});
    const choices = request.then((social): ScopeChoice[] => social.groups.length || social.friends.length
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
  const total = rows.reduce((sum, row) => sum + (row.xp ?? row.week_xp ?? 0), 0);
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
  const state = expired || !known ? "unknown" : row.today_reviews > 0 ? "studied" : row.streak_state === "protected" ? "protected" : "pending";
  const status = state === "studied" ? aqText("Studied today") : state === "protected" ? aqText("Streak protected · Not studied yet") : state === "pending" ? aqText("Not studied yet") : aqText("Refresh to see today's study status");
  const label = aqText`${num(row.streak)} day streak · ${status}`;
  return aqHtml`<span class="streak" data-streak-state="${state}" role="img" tabindex="0" aria-label="${esc(label)}" title="${esc(label)}">${streakIcon(state)}${num(row.streak)}</span>`;
}
function board(rows: Standing[], me: string) {
  if (!rows.length) return aqHtml`<p class="empty">Nobody has uploaded reviews yet.</p>`;
  if (rows.every(row => (row.xp ?? row.week_xp) === 0)) return aqHtml`<p class="empty">No XP in this period yet.</p>`;
  const xpOf = (row: Standing) => row.xp ?? row.week_xp;
  const lead = Math.max(1, xpOf(rows[0]));
  return aqHtml`<ol class="board">${rows.map((r, i) => aqHtml`
    <li class="${[r.user === me && "me", i === 0 && xpOf(r) > 0 && "lead"].filter(Boolean).join(" ")}">
      <span class="rank">${["👑", "🥈", "🥉"][i] ?? i + 1}</span>
      <div class="player-cell">${AnkiQuestSite.avatar(r.user, r.display)}<div class="player-progress">
        <div class="line">
          <a class="name" href="#${encodeURIComponent(r.user)}">${esc(r.display)}</a>
          ${streakBadge(r)}
          <span class="lvl">Lv ${r.level}</span>
        </div>
        <div class="bar"><i style="width:${pct(xpOf(r), lead)}%"></i></div>
      </div>
      </div><div class="xp"><b>${num(xpOf(r))}</b><span>${num(r.today_reviews)} today</span></div>
    </li>`).join("")}
  </ol><p class="streak-legend">${[["studied",aqText("Studied today")],["pending",aqText("Not studied yet")],["protected",aqText("Streak protected")]].map(([state,label]) => aqHtml`<span data-streak-state="${state}">${streakIcon(state)}${label}</span>`).join("")}</p>`;
}

const winCount = (value: unknown) => Number.isFinite(Number(value)) ? Math.max(0, Number(value)) : 0;
function winDate(value: string | null | undefined) {
  if (!value) return "—";
  const parsed = new Date(String(value) + "T00:00:00Z");
  return Number.isNaN(parsed.getTime()) ? String(value) : parsed.toLocaleDateString(AnkiQuestI18n.language, { year: "numeric", month: "short", day: "numeric", timeZone: "UTC" });
}
function winnerContent() {
  const data = winnerState.data, shared = winnerState.scope === "shared", metric = winnerState.metric;
  const heading = aqHtml`<div class="win-heading"><h2 id="winner-history-title">Winner history</h2><a id="winner-calendar-link" href="${AnkiQuestSite.href("/community?period=" + metric, "#calendar")}">Open winner calendar →</a></div>`;
  const controls = aqHtml`<div class="win-controls"><div class="win-scopes" role="group" aria-label="Winner history scope"><button type="button" data-win-scope="shared" aria-pressed="${shared}">Shared history</button><button type="button" data-win-scope="lifetime" aria-pressed="${!shared}">Lifetime</button></div><label class="win-sort" for="win-metric">Rank by <select id="win-metric">${[["day",aqText("Days won")],["week",aqText("Weeks won")],["month",aqText("Months won")]].map(([key,label])=>aqHtml`<option value="${key}"${metric===key?" selected":""}>${label}</option>`).join("")}</select></label></div>`;
  const failure = winnerState.error ? aqHtml`<p role="status">Winner history could not ${data ? aqText("refresh; showing the last loaded results") : "load"}. <button type="button" data-win-retry>Try again</button></p>` : "";
  if (!data) return heading + controls + failure + (winnerState.error ? "" : aqHtml`<p role="status">Loading winner history…</p>`);
  const scope = shared ? data.shared : data.lifetime;
  const ready = !shared || winCount(data.shared?.player_count) >= 2;
  const key = `${metric}_wins` as const;
  const lexical = (a: string, b: string) => a < b ? -1 : a > b ? 1 : 0;
  const players = (scope?.players || []).filter(player=>player.history_start).sort((a,b) => winCount(b[key])-winCount(a[key]) || lexical(String(a.display),String(b.display)) || lexical(String(a.user),String(b.user)));
  const waiting = (data.waiting_players || []).slice().sort((a,b) => lexical(String(a.display),String(b.display)) || lexical(String(a.user),String(b.user)));
  const lifetimeStart = data.meta?.start_source === "configured"
    ? `since the configured server start${data.meta.start_date ? ` on ${esc(winDate(data.meta.start_date))}` : ""}`
    : `across available retained history${data.meta?.start_date ? ` from ${esc(winDate(data.meta.start_date))}` : ""} (which may predate this server)`;
  const note = shared
    ? ready ? `Same window for all ${num(winCount(data.shared.player_count))} players with recorded history, from ${esc(winDate(scope.start_date))}. The window starts with the newest player’s first recorded study date. Only full weeks and months within that window count.`
      : aqText("Shared history needs at least two players with recorded study history. Earlier solo wins remain in Lifetime.")
    : `Raw win totals ${lifetimeStart}, including earlier solo wins and partial periods. Players may have different amounts of recorded history.`;
  let rank = 0, previous: number | null = null;
  const playerRows = players.map((player,index) => {
    const score = winCount(player[key]);
    if (previous !== score) rank = index + 1;
    previous = score;
    return aqHtml`<tr><td>${ready && score > 0 ? rank : "—"}</td><td><a href="#${esc(encodeURIComponent(player.user))}">${esc(player.display)}</a><small>${ready ? `History from ${esc(winDate(player.history_start))}` : aqText("Waiting for a second player")}</small></td>${(["day","week","month"] as const).map(kind=>aqHtml`<td class="${kind===metric?"win-selected":""}">${ready ? num(winCount(player[`${kind}_wins`])) : "—"}</td>`).join("")}</tr>`;
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
    const data = await AnkiQuestSite.readJSON<WinnerHistory>("/api/winners");
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

function streakFreezes(player: Profile) {
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
      <button type="button" id="manage-freezes" class="freeze-button">Manage streak freezes</button></div>
    <div class="freeze-main">${freezeSlots(count, enabled)}<div><strong>${title}</strong><p>${detail}</p></div></div>
  </div>`;
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
${p.at_risk ? aqHtml`<section class="notice risk wide">Your ${p.streak} day streak is waiting for today's reviews.</section>` : ""}
<div class="kpis wide">
  ${AnkiQuestSite.kpi(aqText("Day streak"), num(p.streak), aqText("Keep showing up"), "gold")}
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
  ${streakFreezes(p)}
</section>
<section class="card deck-sharing"><div class="card-head"><div><h2>A little encouragement</h2><p>Share your progress with the people who keep you going.</p></div><span class="symbol" aria-hidden="true">↗</span></div>
  <h3>Deck completion notifications</h3><p class="sub">Choose who hears when you finish a deck, or whose completion alerts you receive. Sharing your own decks is off by default.</p>
  <div class="actions"><button type="button" id="manage-decks">Manage deck notifications</button><button type="button" id="manage-received-notifications" data-open-notification-preferences>Notifications I receive</button></div>
  <p class="settings-hint">Visit <a href="${AnkiQuestSite.href("/community")}">Community</a> for friendly challenges, shared milestones, and reminder preferences.</p>
</section>
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
</div>`;
}

function openDeckSharing(user: string, display: string) {
  if (settingsOpen()) return;
  renderId++;
  const dialog = dialogElement("deck-sharing");
  let token = "", session: OwnerSession | null = null;
  let closed = false;
  let resetAuth: (message?: string) => void = () => {};
  const controller = new AbortController();
  dialog.innerHTML = aqHtml`<h3 id="deck-sharing-title">Deck notifications · ${esc(display)}</h3>
    <p class="sub">Notify selected people once per deck each Anki day, after you finish all scheduled cards, including learning cards due later today.</p>
    <div id="deck-sharing-content"><form id="deck-unlock">
      <label for="deck-token">Your AnkiQuest token</label>
      <input id="deck-token" type="password" required autocomplete="off" spellcheck="false" aria-describedby="deck-token-help">
      <p id="deck-token-help" class="sub">Connect with your own token. Your personal connection also works across Community and other settings.</p>
      <button type="submit">Load my decks</button>
    </form></div>
    <p id="deck-status" role="status" aria-live="polite"></p>
    <div class="actions"><button type="button" id="deck-close">Close</button></div>`;
  const content = dialog.querySelector("#deck-sharing-content")!;
  const status = dialog.querySelector("#deck-status")!;
  const say = (message: string, error = false) => {
    status.textContent = message;
    status.className = error ? "error" : "sub";
  };
  const request = async (body?: DeckUpdate): Promise<DeckSettings> => {
    const requestToken = token, requestSession = session;
    const response = await fetch("/api/decks/" + encodeURIComponent(user), {
      method: body ? "POST" : "GET",
      headers: AnkiQuestSite.ownerHeaders(requestToken ? {user, token: requestToken} : requestSession, body),
      ...(body ? { body: JSON.stringify(body) } : {}),
      cache: "no-store", signal: controller.signal,
    });
    if (controller.signal.aborted || requestToken !== token || requestSession !== session) throw new DOMException(aqText("Settings changed."), "AbortError");
    if (!response.ok) {
      if (response.status === 401 || response.status === 403) {
        if (!requestToken) await AnkiQuestSite.checkAccess(response);
        if (controller.signal.aborted || requestToken !== token || requestSession !== session) throw new DOMException(aqText("Settings changed."), "AbortError");
        forgetSettingsToken(user, requestToken);
        const message = body === undefined
          ? aqText("That token was not accepted for this player. Check it and try again.")
          : aqText("That token is no longer accepted. Enter it again to reload your settings.");
        resetAuth(message);
        throw new Error(message);
      }
      if (response.status === 400 || response.status === 404) throw new Error(aqText("The deck or recipient list has changed. Close and reopen this window to refresh it."));
      throw new Error(aqText("Could not save or load deck notifications. Please try again."));
    }
    const data: DeckSettings = await response.json();
    if (controller.signal.aborted || requestToken !== token || requestSession !== session) throw new DOMException(aqText("Settings changed."), "AbortError");
    return data;
  };
  const showDecks = (data: DeckSettings) => {
    data.decks.sort((a, b) => {
      const left = a.name.split("::"), right = b.name.split("::");
      for (let i = 0; i < Math.min(left.length, right.length); i++) {
        const order = left[i].localeCompare(right[i], undefined, { sensitivity: "base" });
        if (order) return order;
      }
      return left.length - right.length;
    });
    const subdecks = (deck: DeckSetting) => data.decks.map((other, i) => other.name.startsWith(deck.name + "::") ? i : -1).filter(i => i >= 0);
    const collapsed = new Set<string>();
    content.innerHTML = aqHtml`<form id="deck-settings">
      ${data.decks.length ? "" : aqHtml`<p>No decks have been uploaded yet. Open an updated AnkiQuest client and sync your reviews, then reopen this window. Nudges work without them.</p>`}
      ${data.decks.map((deck, index) => aqHtml`<fieldset data-deck="${index}" style="margin-left:${(deck.name.split("::").length - 1) * 14}px">
        <legend>${subdecks(deck).length ? aqHtml`<button type="button" class="fold" aria-expanded="true">▾</button> ` : ""}<span title="${esc(deck.name)}">${esc(deck.name.split("::").pop())}</span></legend>
        <label><input type="checkbox" class="deck-enabled" ${deck.enabled ? "checked" : ""}>Notify people when I finish this deck</label>
        <div class="recipients" ${deck.enabled ? "" : "hidden"}>
          <span class="sub">Notify these people:</span>
          ${data.recipients.map((person, i) => aqHtml`<label><input type="checkbox" data-recipient="${i}" ${deck.recipients.includes(person.user) ? "checked" : ""}>${esc(person.display)}${person.display !== person.user ? aqHtml` <span class="sub">(${esc(person.user)})</span>` : ""}</label>`).join("") || aqHtml`<p class="sub">No other players are available yet.</p>`}
        </div>
        ${subdecks(deck).length ? aqHtml`<button type="button" class="apply-subdecks">Apply to ${subdecks(deck).length === 1 ? aqText("its subdeck") : `all ${subdecks(deck).length} subdecks`}</button>` : ""}
      </fieldset>`).join("")}
      <fieldset id="nudge-settings">
        <legend>Nudges</legend>
        <label><input type="checkbox" id="nudge-enabled" ${data.nudges ? "checked" : ""}>Nudge me when a place, a personal best or a level is within reach</label>
        <p class="sub">At most one of each a day, while you are awake and have already studied.</p>
      </fieldset>
      <fieldset id="celebration-settings">
        <legend>Celebrations</legend>
        <label><input type="checkbox" id="celebrations-enabled" ${data.celebrations !== false ? "checked" : ""}>Celebrate achievements, streak milestones and personal bests</label>
      </fieldset>
      ${data.decks.length ? aqHtml`<p class="sub">Only selected people receive your name and the deck name. Changes apply to future completions.</p>` : ""}
      <button type="submit">Save preferences</button>
    </form>`;
    const form = content.querySelector("form")!;
    const fields = form.querySelectorAll<HTMLFieldSetElement>("fieldset[data-deck]");
    const refold = () => fields.forEach((field, i) => {
      field.hidden = [...collapsed].some(name => data.decks[i].name.startsWith(name + "::"));
      const fold = field.querySelector(".fold");
      if (!fold) return;
      const folded = collapsed.has(data.decks[i].name);
      fold.textContent = folded ? "▸" : "▾";
      fold.setAttribute("aria-expanded", folded ? "false" : "true");
    });
    fields.forEach((field, i) => field.querySelector(".fold")?.addEventListener("click", () => {
      const name = data.decks[i].name;
      if (!collapsed.delete(name)) collapsed.add(name);
      refold();
    }));
    form.querySelectorAll<HTMLFieldSetElement>("fieldset[data-deck]").forEach(field => {
      field.querySelector(".deck-enabled")!.addEventListener("change", event => {
        field.querySelector<HTMLElement>(".recipients")!.hidden = !(event.target as HTMLInputElement).checked;
        say(aqText("Unsaved changes."));
      });
      field.querySelectorAll("[data-recipient]").forEach(input => input.addEventListener("change", () => say(aqText("Unsaved changes."))));
      field.querySelector(".apply-subdecks")?.addEventListener("click", () => {
        const enabled = field.querySelector<HTMLInputElement>(".deck-enabled")!.checked;
        const chosen = [...field.querySelectorAll<HTMLInputElement>("[data-recipient]")].map(input => input.checked);
        const fields = form.querySelectorAll<HTMLFieldSetElement>("fieldset[data-deck]");
        for (const i of subdecks(data.decks[Number(field.dataset.deck)])) {
          const target = fields[i];
          target.querySelector<HTMLInputElement>(".deck-enabled")!.checked = enabled;
          target.querySelector<HTMLElement>(".recipients")!.hidden = !enabled;
          target.querySelectorAll<HTMLInputElement>("[data-recipient]").forEach((input, r) => { input.checked = chosen[r]; });
        }
        say(aqText("Copied to subdecks. Save to keep it."));
      });
    });
    form.querySelector("#nudge-enabled")!.addEventListener("change", () => say(aqText("Unsaved changes.")));
    form.querySelector("#celebrations-enabled")!.addEventListener("change", () => say(aqText("Unsaved changes.")));
    form.addEventListener("submit", async event => {
      event.preventDefault();
      const decks = [...form.querySelectorAll("fieldset[data-deck]")].map((field, i) => ({
        id: data.decks[i].id,
        enabled: field.querySelector<HTMLInputElement>(".deck-enabled")!.checked,
        recipients: [...field.querySelectorAll<HTMLInputElement>("[data-recipient]:checked")].map(input => data.recipients[Number(input.dataset.recipient)].user),
      }));
      const invalid = decks.findIndex(deck => deck.enabled && !deck.recipients.length);
      if (invalid >= 0) {
        say(`Select at least one person for ${data.decks[invalid].name}, or turn its notifications off.`, true);
        form.querySelectorAll("fieldset[data-deck]")[invalid].querySelector<HTMLInputElement>(".deck-enabled")!.focus();
        return;
      }
      const controls = [...form.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button")];
      controls.forEach(control => { control.disabled = true; });
      say(aqText("Saving…"));
      try {
        await request({ decks, nudges: form.querySelector<HTMLInputElement>("#nudge-enabled")!.checked, celebrations: form.querySelector<HTMLInputElement>("#celebrations-enabled")!.checked });
        if (!closed && form.isConnected) say(aqText("Preferences saved."));
      } catch (caught) {
        const error = caught as Error;
        if (!closed && form.isConnected && error.name !== "AbortError") say(error.message, true);
      } finally {
        controls.forEach(control => { control.disabled = false; });
      }
    });
  };
  dialog.querySelector("#deck-close")!.addEventListener("click", () => dialog.close());
  dialog.addEventListener("close", () => {
    closed = true;
    token = "";
    session = null;
    controller.abort();
    dialog.innerHTML = "";
    render();
  }, { once: true });
  dialog.showModal();
  resetAuth = connectSettingsAuth({
    user, form: content.querySelector("form")!, input: dialog.querySelector<HTMLInputElement>("#deck-token")!, controller,
    load: (value, identity) => { token = value; session = identity; return request(); },
    clear: () => { token = ""; session = null; },
    useSession: identity => { token = identity.token || ""; session = identity; },
    show: data => { showDecks(data); content.querySelector<HTMLElement>("input, button")?.focus(); }, say,
  });
}

function receivingNotificationsEntry() {
  return aqHtml`<section class="card deck-sharing"><h2>Deck completion notifications</h2>
    <p class="sub">Choose whose deck completion alerts you receive, even if you have not studied yet.</p>
    <div class="notification-actions"><button type="button" id="manage-received-notifications" data-open-notification-preferences>Notifications I receive</button></div>
  </section>`;
}

const senderNames = (value: unknown) => Array.isArray(value) && value.every((sender: unknown) => typeof sender === "string" && sender.length > 0);
function isNotificationSettings(value: unknown): value is NotificationSettings {
  const data = value as Record<string, unknown> | null;
  return !!data && typeof data.enabled === "boolean" && senderNames(data.muted_senders) &&
    senderNames(data.unsubscribed_senders) && senderNames(data.sharing_senders) &&
    Array.isArray(data.senders) && data.senders.every((entry: unknown) => {
      const sender = entry as Record<string, unknown> | null;
      return !!sender && typeof sender.user === "string" && sender.user.length > 0 && typeof sender.display === "string";
    });
}

function openNotificationPreferences(user = "", display = "") {
  if (settingsOpen()) return;
  renderId++;
  const dialog = dialogElement("notification-preferences");
  const session = ++notificationDialogSession;
  const controller = new AbortController();
  const fixedUser = user;
  const opener = document.activeElement;
  let token = "", ownerSession: OwnerSession | null = null, closed = false, busy = false;
  let authController: AbortController | null = null, authUser: string | null = null;
  let resetAuth: (message?: string) => void = () => {}, playerEdited = false;
  let remembered = "";
  if (!fixedUser) {
    try { remembered = localStorage.getItem("ankiquestPlayer") || ""; } catch (e) {}
  }
  const current = () => !closed && notificationDialogSession === session && dialog.open;
  dialog.innerHTML = aqHtml`<h3 id="notification-title">Notifications I receive</h3>
    <p id="notification-intro" class="intro">${fixedUser ? `Deck completion alerts for ${esc(display || user)}.` : aqText("Choose which deck completion alerts reach you.")} These settings apply across your devices.</p>
    <p class="scope-note">Streak reminders, messages and replies are unaffected.</p>
    <div id="notification-content"><form id="notification-unlock" data-notification-unlock>
      ${fixedUser ? "" : aqHtml`<label class="account-label" for="notification-player">Your player name
        <input id="notification-player" name="player" type="text" required autocomplete="off" autocapitalize="none" spellcheck="false" value="${esc(remembered)}" aria-describedby="notification-player-help" data-notification-player>
        <small id="notification-player-help" class="sub">Use the player name from your AnkiQuest settings.</small>
      </label>`}
      <label class="token-label" for="notification-token">Your AnkiQuest token</label>
      <input id="notification-token" type="password" required autocomplete="off" spellcheck="false" aria-describedby="notification-token-help" data-notification-token>
      <p id="notification-token-help" class="token-help">Connect once with your AnkiQuest token. Your connected account is reused across personal settings.</p>
      <button type="submit" class="freeze-button primary" data-notification-load>Load my settings</button>
    </form></div>
    <p id="notification-status" class="status" role="status" aria-live="polite" data-notification-status></p>
    <div class="actions"><button type="button" id="notification-close" class="freeze-button" data-notification-close>Close</button></div>`;
  const content = dialog.querySelector("#notification-content")!;
  const status = dialog.querySelector("#notification-status")!;
  const say = (message: string, error = false) => {
    if (!current()) return;
    status.textContent = message;
    status.className = "status" + (error ? " error" : "");
  };
  const request = async (body?: NotificationUpdate): Promise<NotificationSettings> => {
    const loading = body === undefined;
    const requestUser = user, requestToken = token, requestSession = ownerSession, requestController = authController!;
    const active = () => current() && !requestController.signal.aborted && requestUser === user && requestToken === token && requestSession === ownerSession;
    const response = await fetch("/api/deck-subscriptions/" + encodeURIComponent(requestUser), {
      method: loading ? "GET" : "POST",
      headers: AnkiQuestSite.ownerHeaders(requestToken ? {user:requestUser, token:requestToken} : requestSession, body),
      ...(loading ? {} : { body: JSON.stringify(body) }),
      cache: "no-store", signal: requestController.signal,
    });
    if (!active()) throw new DOMException(aqText("Settings changed."), "AbortError");
    if (!response.ok) {
      if (response.status === 401 || response.status === 403) {
        if (!requestToken) await AnkiQuestSite.checkAccess(response);
        if (!active()) throw new DOMException(aqText("Settings changed."), "AbortError");
        forgetSettingsToken(requestUser, requestToken);
        const message = loading
          ? aqText("That token was not accepted for this player. Check your player name and token, then try again.")
          : aqText("That token is no longer accepted. Enter it again to reload your settings.");
        resetAuth(message);
        throw new Error(message);
      }
      throw new Error(loading ? aqText("Could not load your notification settings. Please try again.")
        : aqText("Could not save your notification settings. Your changes are still here; please try again."));
    }
    const data: unknown = await response.json();
    if (!active()) throw new DOMException(aqText("Settings changed."), "AbortError");
    if (!isNotificationSettings(data)) {
      throw new Error(aqText("The server returned unexpected notification settings. Please try again."));
    }
    return data;
  };
  const showSettings = (data: NotificationSettings) => {
    if (!current()) return;
    const unsubscribed = new Set([...data.unsubscribed_senders, ...data.muted_senders]);
    const relevant = new Set([...data.sharing_senders, ...unsubscribed]);
    const people = new Map(data.senders.filter(person => relevant.has(person.user)).map(person => [person.user, person]));
    // Keep subscriptions and previous mutes editable for former players too.
    for (const sender of unsubscribed) if (!people.has(sender)) people.set(sender, { user: sender, display: sender });
    const senders = [...people.values()].sort((a, b) => a.display.localeCompare(b.display) || a.user.localeCompare(b.user));
    dialog.querySelector("#notification-intro")!.textContent = `Deck completion alerts for ${fixedUser ? display || user : user}. These settings apply across your devices.`;
    content.innerHTML = aqHtml`<form id="notification-settings" data-notification-settings>
      <label class="receive-toggle" for="notification-enabled">
        <input id="notification-enabled" type="checkbox" role="switch" ${data.enabled ? "checked" : ""} data-receiving-enabled>
        <span><strong>Receive deck completion notifications</strong><small>Turn off to stop all deck completion alerts.</small></span>
      </label>
      <fieldset aria-describedby="notification-subscription-help">
        <legend>Unsubscribe from specific people</legend>
        <p id="notification-subscription-help" class="scope-note">A checked name removes you from all of their deck-sharing lists. They cannot add you back. Clear the name and save to subscribe again.</p>
        ${senders.map((person, index) => aqHtml`<label class="mute-person" for="notification-sender-${index}">
          <input id="notification-sender-${index}" type="checkbox" data-unsubscribed-sender="${esc(person.user)}" ${unsubscribed.has(person.user) ? "checked" : ""}>
          <span>Unsubscribe from ${esc(person.display || person.user)}${person.display && person.display !== person.user ? aqHtml`<small>${esc(person.user)}</small>` : ""}</span>
        </label>`).join("") || aqHtml`<p class="sub">No one is sharing decks with you yet. You can still turn all deck completion alerts off.</p>`}
      </fieldset>
      ${data.muted_senders.length ? aqHtml`<p class="scope-note">Your previous mutes are selected above. Saving also removes you from those people’s sharing lists.</p>` : ""}
      <ul class="rules">
        <li>Turning all alerts off cancels pending deck completion alerts without changing your subscriptions.</li>
        <li>Unsubscribing cancels that person’s pending completion alerts and removes matching alerts from your inbox.</li>
        <li>Subscribing again restores your previous deck selections unless the sender has changed or deleted them. Only new completions are sent; old alerts are not replayed.</li>
      </ul>
      <button type="submit" class="freeze-button primary" data-notification-save>Save preferences</button>
    </form>`;
    const form = content.querySelector("form")!;
    const enabled = form.querySelector<HTMLInputElement>("#notification-enabled")!;
    const senderInputs = [...form.querySelectorAll<HTMLInputElement>("[data-unsubscribed-sender]")];
    const saveButton = form.querySelector<HTMLButtonElement>("[data-notification-save]")!;
    const syncControls = () => {
      form.setAttribute("aria-busy", String(busy));
      enabled.disabled = busy;
      saveButton.disabled = busy;
      senderInputs.forEach(input => { input.disabled = busy; });
    };
    enabled.addEventListener("change", () => { syncControls(); say(aqText("Unsaved changes. Save to apply them.")); });
    senderInputs.forEach(input => input.addEventListener("change", () => say(aqText("Unsaved changes. Save to apply them."))));
    form.addEventListener("submit", async event => {
      event.preventDefault();
      if (busy || !current()) return;
      const draft = { enabled: enabled.checked, unsubscribed_senders: senderInputs.filter(input => input.checked).map(input => input.dataset.unsubscribedSender!) };
      busy = true;
      syncControls();
      say(aqText("Saving…"));
      try {
        const saved = await request(draft);
        if (current()) {
          busy = false;
          showSettings(saved);
          say(saved.enabled ? aqText("Preferences saved. Your sender subscriptions have been updated.")
            : aqText("Preferences saved. All deck completion alerts are off. Your sender subscriptions are saved."));
          content.querySelector<HTMLButtonElement>("[data-notification-save]")!.focus();
        }
      } catch (error) {
        if (current()) say((error as Error).message, true);
      } finally {
        busy = false;
        if (current() && form.isConnected) syncControls();
      }
    });
    syncControls();
  };
  const form = content.querySelector("form")!, input = form.querySelector<HTMLInputElement>("#notification-token")!, player = form.querySelector<HTMLInputElement>("#notification-player");
  const bindAccount = (target: string) => {
    if (!current() || !target || target === authUser) return;
    authController?.abort();
    authController = new AbortController();
    const binding = authController;
    controller.signal.addEventListener("abort", () => binding.abort(), {once:true});
    authUser = user = target; token = ""; ownerSession = null;
    if (player) player.value = target;
    resetAuth = connectSettingsAuth({
      user, form, input, controller:authController,
      load: (value, identity) => { token = value; ownerSession = identity; return request(); },
      clear: () => { token = ""; ownerSession = null; },
      useSession: identity => { token = identity.token || ""; ownerSession = identity; },
      show: data => { showSettings(data); content.querySelector<HTMLInputElement>("#notification-enabled")!.focus(); }, say,
    });
  };
  // A leaderboard visitor may choose a player; once connected, the same shared
  // auth helper used by deck/freeze preferences owns all loading and retries.
  form.addEventListener("submit", event => {
    playerEdited = true;
    const target = fixedUser || player!.value.trim();
    if (target === authUser) return;
    event.preventDefault();event.stopImmediatePropagation();
    if (!target) { player!.focus();return; }
    bindAccount(target);
    // A new task avoids the browser's re-entrant form-submission guard.
    setTimeout(() => { if (current()) form.requestSubmit(); }, 0);
  });
  player?.addEventListener("input", () => { playerEdited = true; });
  const finish = () => {
    if (closed) return;
    closed = true;
    token = "";
    ownerSession = null;
    controller.abort();
    dialog.removeEventListener("close", onClose);
    dialog.removeEventListener("cancel", onCancel);
    if (notificationDialogSession !== session) return;
    dialog.innerHTML = "";
    render().then(() => {
      if (notificationDialogSession === session && !settingsOpen() &&
          (document.activeElement === document.body || document.activeElement === opener)) {
        document.getElementById("manage-received-notifications")?.focus();
      }
    });
  };
  const close = () => { dialog.close(); finish(); };
  const onClose = () => { if (!dialog.open) finish(); };
  const onCancel = (event: Event) => { event.preventDefault(); close(); };
  dialog.querySelector("#notification-close")!.addEventListener("click", close);
  dialog.addEventListener("close", onClose);
  dialog.addEventListener("cancel", onCancel);
  dialog.showModal();
  const native = window.ankiquestSession;
  if (fixedUser) bindAccount(fixedUser);
  // A native identity means `native.user` is a non-empty string.
  else if (nativeSettingsIdentity()) bindAccount(native!.user as string);
  else AnkiQuestSite.member().then(identity => {
    if (!current() || playerEdited || nativeSettingsCleared || nativeSettingsIdentity()) return;
    if (identity) bindAccount(identity.user);
    else if (remembered) bindAccount(remembered);
  });
}

function isFreezeSettings(value: unknown): value is FreezeSettings {
  const {enabled, freezes, capacity} = value as Record<string, unknown>;
  return typeof enabled === "boolean" && typeof freezes === "number" && Number.isInteger(freezes) && freezes >= 0 && freezes <= 3 && capacity === 3;
}

function openStreakFreezes(user: string, display: string) {
  if (settingsOpen()) return;
  renderId++;
  const dialog = dialogElement("streak-freezes");
  const controller = new AbortController();
  let token = "", session: OwnerSession | null = null, closed = false, busy = false;
  let resetAuth: (message?: string) => void = () => {};
  dialog.innerHTML = aqHtml`<h3 id="freeze-title">Streak protection</h3>
    <p id="freeze-intro" class="intro">A little backup for ${esc(display)}'s study streak. On by default.</p>
    <ul class="rules">
      <li><strong>Earn your ice.</strong> Start with 0 freezes. Finish all three daily quests while protection is on to earn one per Anki day. Hold up to 3.</li>
      <li><strong>Keep your streak.</strong> While protection is on, one freeze is used automatically for each ended Anki day with no reviews.</li>
      <li><strong>Pause whenever you like.</strong> Turning it off pauses earning and use; saved freezes stay. Quests completed while protection is off do not earn freezes.</li>
    </ul>
    <div id="freeze-content"><form id="freeze-unlock">
      <label class="token-label" for="freeze-token">Your AnkiQuest token</label>
      <input id="freeze-token" type="password" required autocomplete="off" spellcheck="false" aria-describedby="freeze-token-help">
      <p id="freeze-token-help" class="token-help">Connect with your own token. Your personal connection also works across Community and other settings.</p>
      <button type="submit" class="freeze-button primary">Load my settings</button>
    </form></div>
    <p id="freeze-status" class="status" role="status" aria-live="polite"></p>
    <div class="actions"><button type="button" id="freeze-close" class="freeze-button">Close</button></div>`;
  const content = dialog.querySelector("#freeze-content")!;
  const status = dialog.querySelector("#freeze-status")!;
  const say = (message: string, error = false) => {
    status.textContent = message;
    status.className = "status" + (error ? " error" : "");
  };
  const request = async (body?: {enabled: boolean}): Promise<FreezeSettings> => {
    const requestToken = token, requestSession = session;
    const response = await fetch("/api/streak-freezes/" + encodeURIComponent(user), {
      method: body === undefined ? "GET" : "POST",
      headers: AnkiQuestSite.ownerHeaders(requestToken ? {user, token: requestToken} : requestSession, body),
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      cache: "no-store", signal: controller.signal,
    });
    if (controller.signal.aborted || requestToken !== token || requestSession !== session) throw new DOMException(aqText("Settings changed."), "AbortError");
    if (!response.ok) {
      if (response.status === 401 || response.status === 403) {
        if (!requestToken) await AnkiQuestSite.checkAccess(response);
        if (controller.signal.aborted || requestToken !== token || requestSession !== session) throw new DOMException(aqText("Settings changed."), "AbortError");
        forgetSettingsToken(user, requestToken);
        const message = body === undefined
          ? aqText("That token was not accepted for this player. Check it and try again.")
          : aqText("That token is no longer accepted. Enter it again to reload your settings.");
        resetAuth(message);
        throw new Error(message);
      }
      throw new Error(aqText("Could not load or save streak protection. Please try again."));
    }
    const data: unknown = await response.json();
    if (controller.signal.aborted || requestToken !== token || requestSession !== session) throw new DOMException(aqText("Settings changed."), "AbortError");
    if (!isFreezeSettings(data)) {
      throw new Error(aqText("The server returned unexpected freeze settings. Please try again."));
    }
    return data;
  };
  const showSettings = (data: FreezeSettings) => {
    content.innerHTML = aqHtml`<form id="freeze-preferences">
      <div class="freeze-balance">${freezeSlots(data.freezes, data.enabled)}
        <div><strong>${data.freezes} of 3 freezes ${data.enabled ? "ready" : "saved"}</strong><span class="sub">${data.enabled ? data.freezes === 3 ? aqText("Fully stocked") : aqText("Earn more with daily quests") : aqText("Protection is off")}</span></div></div>
      <label class="freeze-toggle" for="freeze-enabled">
        <input id="freeze-enabled" type="checkbox" role="switch" ${data.enabled ? "checked" : ""}>
        <span><strong>Use streak protection</strong><small>Earn and automatically use your freezes while on.</small></span>
      </label>
      <button type="submit" class="freeze-button primary">Save preference</button>
    </form>`;
    const form = content.querySelector("form")!;
    form.querySelector("#freeze-enabled")!.addEventListener("change", () => say(aqText("Unsaved change. Save to apply it.")));
    form.addEventListener("submit", async event => {
      event.preventDefault();
      if (busy || closed) return;
      busy = true;
      const enabled = form.querySelector<HTMLInputElement>("#freeze-enabled")!.checked;
      const controls = [...form.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button")];
      controls.forEach(control => { control.disabled = true; });
      say(aqText("Saving…"));
      try {
        const saved = await request({ enabled });
        if (!closed && form.isConnected) {
          showSettings(saved);
          say(saved.enabled ? aqText("Streak protection is on.") : aqText("Streak protection is off. Your saved freezes are kept."));
          content.querySelector("button")!.focus();
        }
      } catch (caught) {
        const error = caught as Error;
        if (!closed && form.isConnected && error.name !== "AbortError") say(error.message, true);
      } finally {
        busy = false;
        controls.forEach(control => { control.disabled = false; });
      }
    });
  };
  dialog.querySelector("#freeze-close")!.addEventListener("click", () => dialog.close());
  dialog.addEventListener("close", () => {
    closed = true;
    token = "";
    session = null;
    controller.abort();
    dialog.innerHTML = "";
    render().then(() => {
      if (!settingsOpen() && decodeURIComponent(location.hash.slice(1)) === user &&
          (document.activeElement === document.body || document.activeElement?.id === "manage-freezes")) {
        document.getElementById("manage-freezes")?.focus();
      }
    });
  }, { once: true });
  dialog.showModal();
  resetAuth = connectSettingsAuth({
    user, form: content.querySelector("form")!, input: dialog.querySelector<HTMLInputElement>("#freeze-token")!, controller,
    load: (value, identity) => { token = value; session = identity; return request(); },
    clear: () => { token = ""; session = null; },
    useSession: identity => { token = identity.token || ""; session = identity; },
    show: data => { showSettings(data); content.querySelector<HTMLInputElement>("#freeze-enabled")!.focus(); }, say,
  });
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
      const held = await AnkiQuestSite.readJSON<RecordBoard[]>(scoped("/api/records"), auth);
      if (!canRender()) return;
      document.title = aqText("Records · AnkiQuest");
      AnkiQuestSite.setProfile(me);
      app.innerHTML = pageHero(aqText("Records worth chasing."), aqText("Personal bests become shared milestones. Celebrate the sessions, streaks, and study days that raised the bar."), aqHtml`<strong>All-time achievements</strong>A new record starts with one session.`) + scopePicker(choices, scope) + records(held, me) + receivingNotificationsEntry();
      document.getElementById("manage-received-notifications")!.addEventListener("click", () => openNotificationPreferences());
      return;
    }
    const period = currentPeriod();
    loadWinners();
    const [rows, week] = await Promise.all([
      AnkiQuestSite.readJSON<Standing[]>(scoped("/api/leaderboard?period=" + period), auth),
      AnkiQuestSite.readJSON<WeekInfo>("/api/week").catch(() => null),
    ]);
    weekEnds = week?.ends_at ?? 0;
    if (user) {
      const res = await fetch("/api/profile/" + encodeURIComponent(user), {cache: "no-store"});
      await AnkiQuestSite.checkAccess(res);
      if (res.ok) {
        const p: Profile = await res.json();
        if (!canRender()) return;
        document.title = `${p.display} · AnkiQuest`;
        AnkiQuestSite.setProfile(p.user, true);
        try { localStorage.setItem("ankiquestPlayer", p.user); } catch (e) {}
        const access = await AnkiQuestSite.status();
        if (!canRender()) return;
        const native = window.ankiquestSession;
        const canEditAvatar = access?.member?.user === p.user || (native?.user === p.user && typeof native.token === "string" && !!native.token.trim());
        app.innerHTML = view(p, rows, canEditAvatar);
        if (canEditAvatar) document.getElementById("manage-avatar")!.addEventListener("click", () => AnkiQuestAvatars!.open({user:p.user,display:p.display}));
        document.getElementById("manage-decks")!.addEventListener("click", () => openDeckSharing(p.user, p.display));
        document.getElementById("manage-received-notifications")!.addEventListener("click", () => openNotificationPreferences(p.user, p.display));
        document.getElementById("manage-freezes")!.addEventListener("click", () => openStreakFreezes(p.user, p.display));
        return;
      }
    }
    if (!canRender()) return;
    document.title = aqText("Leaderboard · AnkiQuest");
    AnkiQuestSite.setProfile(me);
    app.innerHTML = pageHero(aqText("Every session counts."), aqText("A little friendly competition, a little more motivation. See how your study community is growing together."), aqHtml`<strong>Made for steady progress</strong>Standings update as members sync.`) + (newcomer ? welcome() : "") + leaderboardKpis(rows, period) + tabs(period) + scopePicker(choices, scope) + aqHtml`<section class="card"><div class="card-head"><div><h2>${boardTitle(period)}</h2><p>XP earned together, one study session at a time.</p></div><span class="chip blue">${num(rows.length)} players</span></div>${board(rows, me)}</section>` + winnerSection() + receivingNotificationsEntry();
    document.getElementById("manage-received-notifications")!.addEventListener("click", () => openNotificationPreferences());
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
