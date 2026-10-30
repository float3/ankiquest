export {};

/** `/api/dev/<user>/stats` (`Stats` in src/dev.rs). */
interface Stats {
  players: {total: number; configured: number; accounts: number; synced: number};
  active: {day: number; week: number; month: number};
  days: {day: number; reviews: number; players: number; signups: number}[];
  groups: {count: number; members: number; sizes: number[]; friendships: number};
  retention: {eligible: number; retained: number};
  generated_at: number;
}

interface GroupSummary {id: number; name: string; members: number}

interface Announcement {
  id: number;
  sender: string;
  title: string;
  body: string;
  groups: {id: number; name: string | null}[];
  recipients: number;
  created_at: number;
}

interface LogEntry {at: number; user: string | null; message: string}

const DAY_MS = 86_400_000;
const esc = AnkiQuestSite.escape;
const app = document.getElementById("app")!;
const num = (n: number) => n.toLocaleString("en");
const date = (day: number) => new Date(day * DAY_MS).toISOString().slice(0, 10);
const time = (ms: number) => new Date(ms).toISOString().slice(0, 16).replace("T", " ") + " UTC";
const median = (values: number[]) => {
  if (!values.length) return 0;
  const sorted = [...values].sort((a, b) => a - b), middle = sorted.length >> 1;
  return sorted.length % 2 ? sorted[middle]! : (sorted[middle - 1]! + sorted[middle]!) / 2;
};

let user = "";
const api = (path: string) => `/api/dev/${encodeURIComponent(user)}/${path}`;

async function load<T>(path: string): Promise<T> {
  const response = await fetch(api(path), {cache: "no-store", credentials: "same-origin"});
  if (!response.ok) throw new Error(String(response.status));
  return response.json();
}

function statsView(stats: Stats) {
  const {players, active, groups, retention} = stats;
  const share = retention.eligible ? Math.round(100 * retention.retained / retention.eligible) + "%" : "–";
  return `<div class="kpis">
    ${AnkiQuestSite.kpi("Players", num(players.total), `${num(players.configured)} configured · ${num(players.accounts)} accounts · ${num(players.synced)} sync only`)}
    ${AnkiQuestSite.kpi("Active", num(active.day), `24 h · ${num(active.week)} in 7 d · ${num(active.month)} in 30 d`)}
    ${AnkiQuestSite.kpi("Groups", num(groups.count), `${num(groups.members)} members · largest ${num(groups.sizes[0] ?? 0)} · median ${num(median(groups.sizes))} · ${num(groups.friendships)} friendships`)}
    ${AnkiQuestSite.kpi("Retention", share, `${num(retention.retained)} of ${num(retention.eligible)} accounts older than 7 d reviewed in the last 7 d`)}
  </div>`;
}

function daysView(stats: Stats) {
  const top = Math.max(1, ...stats.days.map(day => day.reviews));
  const rows = [...stats.days].reverse().map(day => `<tr><td>${date(day.day)}</td><td class="n">${num(day.reviews)}</td><td><span class="bar" style="width:${Math.round(120 * day.reviews / top)}px"></span></td><td class="n">${num(day.players)}</td><td class="n">${num(day.signups)}</td></tr>`).join("");
  return `<section class="card"><div class="card-head"><h2>Last ${stats.days.length} days</h2><small>UTC</small></div>
    <div class="table-wrap"><table id="dev-days"><thead><tr><th>Day</th><th class="n">Reviews</th><th></th><th class="n">Players</th><th class="n">Sign-ups</th></tr></thead><tbody>${rows}</tbody></table></div>
  </section>`;
}

function announcementView(announcement: Announcement) {
  const audience = announcement.groups.length
    ? announcement.groups.map(group => group.name ?? `#${group.id}`).join(", ")
    : "everyone";
  return `<li><strong>${esc(announcement.title)}</strong> ${esc(announcement.body)}<small>${time(announcement.created_at)} · ${esc(announcement.sender)} → ${esc(audience)} · ${num(announcement.recipients)} players</small></li>`;
}

function errorView(entry: LogEntry) {
  return `<li>${esc(entry.message)}<small>${time(entry.at)}${entry.user ? " · " + esc(entry.user) : ""}</small></li>`;
}

function composer(groups: GroupSummary[]) {
  const options = groups.map(group => `<label><input type="checkbox" name="group" value="${group.id}"> ${esc(group.name)} <small>(${num(group.members)})</small></label>`).join("");
  return `<section class="card"><div class="card-head"><h2>Announcement</h2></div>
    <form id="dev-announce">
      <input type="text" name="title" placeholder="Title" maxlength="80" required aria-label="Title">
      <textarea name="body" placeholder="Message" maxlength="200" required aria-label="Message"></textarea>
      <fieldset><legend>To</legend>
        <label><input type="radio" name="to" value="all" checked> Everyone</label>
        <label><input type="radio" name="to" value="groups"${groups.length ? "" : " disabled"}> Groups</label>
        <div class="dev-groups" hidden>${options}</div>
      </fieldset>
      <div><button type="submit" class="primary">Send</button></div>
      <p class="status" id="dev-status" role="status"></p>
    </form>
  </section>`;
}

async function refreshAnnouncements() {
  const list = document.getElementById("dev-announcements")!;
  const announcements = await load<Announcement[]>("announcements");
  list.innerHTML = announcements.length ? announcements.map(announcementView).join("") : "<li>None yet.</li>";
}

function wire(form: HTMLFormElement) {
  const picker = form.querySelector<HTMLElement>(".dev-groups")!;
  const status = form.querySelector<HTMLElement>("#dev-status")!;
  const button = form.querySelector<HTMLButtonElement>("button[type=submit]")!;
  form.addEventListener("change", () => {
    picker.hidden = new FormData(form).get("to") !== "groups";
  });
  form.addEventListener("submit", async event => {
    event.preventDefault();
    const data = new FormData(form);
    const title = String(data.get("title") ?? "").trim(), body = String(data.get("body") ?? "").replace(/\s+/g, " ").trim();
    const groups = data.get("to") === "groups" ? data.getAll("group").map(Number) : null;
    if (groups && !groups.length) {
      status.textContent = "Pick at least one group.";
      status.className = "status error";
      return;
    }
    const audience = groups ? `${groups.length} group${groups.length === 1 ? "" : "s"}` : "everyone";
    if (!confirm(`Send “${title}” to ${audience}?`)) return;
    const request = groups ? {title, body, groups} : {title, body};
    button.disabled = true;
    status.className = "status";
    status.textContent = "Sending…";
    try {
      const response = await fetch(api("announcements"), {
        method: "POST",
        credentials: "same-origin",
        headers: AnkiQuestSite.ownerHeaders({user}, request),
        body: JSON.stringify(request),
      });
      if (!response.ok) throw new Error(response.status === 409 ? "Nobody is in those groups." : `Not sent (${response.status}).`);
      const sent = await response.json() as {recipients: number};
      status.textContent = `Sent to ${num(sent.recipients)} players.`;
      form.reset();
      picker.hidden = true;
      await refreshAnnouncements();
    } catch (error) {
      status.textContent = (error as Error).message;
      status.className = "status error";
    } finally {
      button.disabled = false;
    }
  });
}

async function start() {
  const access = await AnkiQuestSite.status();
  const member = typeof access?.member?.user === "string" ? access.member.user : "";
  if (!member || !access?.dev) {
    app.innerHTML = member ? "<p>This account is not a dev.</p>" : `<p><a href="/login?next=%2Fdev">Sign in</a> with a dev account.</p>`;
    app.removeAttribute("aria-busy");
    return;
  }
  user = member;
  try {
    const [stats, groups, errors] = await Promise.all([load<Stats>("stats"), load<GroupSummary[]>("groups"), load<LogEntry[]>("errors")]);
    app.innerHTML = `<h1>Dev</h1>${statsView(stats)}<div class="grid">${composer(groups)}
      <section class="card"><div class="card-head"><h2>Sent</h2></div><ul class="dev-list" id="dev-announcements"></ul></section></div>${daysView(stats)}
      <section class="card"><div class="card-head"><h2>Errors</h2><small>last 7 days</small></div><ul class="dev-list" id="dev-errors">${errors.length ? errors.map(errorView).join("") : "<li>None.</li>"}</ul></section>`;
    wire(document.getElementById("dev-announce") as HTMLFormElement);
    await refreshAnnouncements();
  } catch (error) {
    app.innerHTML = `<p class="error">Could not load (${esc((error as Error).message)}).</p>`;
  }
  app.removeAttribute("aria-busy");
}

document.addEventListener("DOMContentLoaded", start, {once: true});
