/* Private Today, study history, session recap and account settings. */
(() => {
  "use strict";
  interface Quest { title: string; progress: number; target: number; done: boolean; reward: number }
  interface Profile { user: string; display: string; level: number; streak: number; today: {reviews: number; xp: number}; quests: Quest[] }
  interface StudyDay { date: string; reviews: number; time_ms: number; xp: number; new_cards: number; streak: number; frozen: boolean }
  interface StudySession { started_at: number; ended_at: number; reviews: number; time_ms: number; new_cards: number }
  interface StudyHistory { year: number; years: number[]; days: StudyDay[]; last_review_at: number | null; last_received_at: number | null; latest_session: StudySession | null }
  interface Challenges { challenges: {id: number; title: string; status: string; members: {user: string; status: string}[]}[] }
  interface Activity { unread_count: number }
  interface Person { user: string; display: string }
  type ReminderKey = "gentle_daily" | "urgent_streak" | "freeze_used" | "freeze_refill" | "milestone" | "weekly_closing" | "weekly_recap";
  type Reminders = Record<ReminderKey, boolean> & {reminder_hour: number; quiet_start: number; quiet_end: number; daily_limit: number};
  interface Settings {
    companion?: {companion: string} | null;
    reminders?: Reminders | null;
    freezes?: {enabled: boolean; freezes: number; capacity: number} | null;
    nudges?: {receiving: boolean; automatic_receiving: boolean; friends: (Person & {muted_by_me: boolean})[]} | null;
    subscriptions?: {enabled: boolean; senders: Person[]; unsubscribed_senders: string[]; sharing_senders: string[]} | null;
    decks?: {decks: {id: string; name: string; enabled: boolean; recipients: string[]}[]; recipients: Person[]; celebrations: boolean} | null;
  }
  interface Group { id: number; name: string; owner: boolean; invite: string | null; members: Person[] }
  interface Social { groups: Group[]; friends: Person[]; incoming: Person[]; outgoing: Person[]; public: boolean }
  interface Invite { name: string; members: number }
  type SettingName = keyof Settings;
  interface State {
    session: OwnerSession | null; epoch: number; profile: Profile | null; study: StudyHistory | null; challenges: Challenges | null; activity: Activity | null;
    settings: Settings; errors: Record<string, string>; year: number | null; selected: string | null; native: boolean;
    social: Social | null; invite: Invite | null; account: {created_at: number} | null;
  }

  const {t:tr, html} = AnkiQuestI18n;
  const site = AnkiQuestSite;
  const $ = <E extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as E;
  const esc = site.escape;
  const number = (value: unknown) => Number(value || 0).toLocaleString(AnkiQuestI18n.language);
  const when = (value: unknown) => new Date(Number(value)).toLocaleString(AnkiQuestI18n.language, {dateStyle:"medium", timeStyle:"short"});
  const date = (value: string) => new Intl.DateTimeFormat(AnkiQuestI18n.language, {dateStyle:"full", timeZone:"UTC"}).format(new Date(value + "T12:00:00Z"));
  const path = location.pathname.replace(/\/+$/, "");
  const page = path === "/history" ? "history" : path === "/settings" ? "settings" : path === "/friends" ? "friends" : path.startsWith("/join/") ? "join" : "today";
  const inviteCode = page === "join" ? decodeURIComponent(path.slice("/join/".length)) : "";
  let reminders: [ReminderKey, string][] = [];
  const state: State = {session:null, epoch:0, profile:null, study:null, challenges:null, activity:null, settings:{}, errors:{}, year:null, selected:null, native:false, social:null, invite:null, account:null};
  document.querySelector<HTMLElement>("[data-site-header]")!.dataset.siteSection = page === "settings" ? "settings" : "today";
  // Text waits for translations; restore() does too, so reminders are set before any render.
  void AnkiQuestI18n.ready.then(() => {
    const titles = {today:tr("Today"), history:tr("Study history"), settings:tr("Your settings"), friends:tr("Friends and groups"), join:tr("Join a group")};
    const intros = {
      today:tr("Your progress, one day at a time."),
      history:tr("Every day you showed up adds to your story."),
      settings:tr("Choose how AnkiQuest encourages you and what you share."),
      friends:tr("Study with the people you choose. Only they see your progress unless you make your profile public."),
      join:tr("You were invited to study together."),
    };
    reminders = [
      ["gentle_daily",tr("Gentle daily reminder")], ["urgent_streak",tr("Unprotected streak warning")],
      ["freeze_used",tr("Freeze used")], ["freeze_refill",tr("Freeze refill reminder")],
      ["milestone",tr("Streak milestones")], ["weekly_closing",tr("Weekly competition closing")],
      ["weekly_recap",tr("Weekly recap")],
    ];
    $("personal-title").textContent = titles[page];
    $("personal-intro").textContent = intros[page];
    document.title = titles[page] + " · AnkiQuest";
  });
  document.querySelectorAll<HTMLAnchorElement>("[data-personal-tab]").forEach(tab => {
    tab.href = site.href("/" + tab.dataset.personalTab);
    if (tab.dataset.personalTab === page) tab.setAttribute("aria-current", "page");
  });
  $("personal-refresh").addEventListener("click", () => load());
  document.querySelector(".skip")!.addEventListener("click", event => { event.preventDefault(); $("personal-app").focus(); });

  function nativeAccount() {
    const candidate = window.ankiquestSession;
    return typeof candidate?.user === "string" && candidate.user && typeof candidate.token === "string" && candidate.token.trim()
      ? {user:candidate.user, token:candidate.token.trim()} : null;
  }
  function showGate(message = "") {
    state.epoch++;
    state.session = null;
    state.profile = state.study = state.challenges = state.activity = null;
    state.settings = {};
    state.social = state.invite = null;
    $("personal-gate").hidden = false;
    void site.status().then(access => { if (access?.registration) $("personal-login").hidden = false; });
    ($<HTMLFormElement>("personal-connect").elements.namedItem("token") as HTMLInputElement).value = "";
    $("personal-connect").querySelectorAll<HTMLInputElement | HTMLButtonElement>("input,button").forEach(control => control.disabled = false);
    $("personal-content").hidden = true;
    $("personal-refresh").hidden = true;
    $("personal-error").hidden = true;
    $("connect-status").textContent = message;
    $("personal-app").setAttribute("aria-busy", "false");
  }
  function useAccount(session: OwnerSession | null, native = false) {
    if (!session?.user) return showGate();
    if (state.session?.user === session.user && state.session?.token === session.token) return;
    state.session = session;
    state.native = native;
    state.year = null;
    state.selected = null;
    site.setProfile(session.user, true);
    $("personal-gate").hidden = true;
    $("personal-content").hidden = false;
    $("personal-refresh").hidden = false;
    load();
  }
  async function restore() {
    await AnkiQuestI18n.ready;
    const native = nativeAccount();
    if (native) return useAccount(native, true);
    if (state.native) return showGate(tr("Your account changed. Reconnect to continue."));
    const member = await site.member();
    if (member) useAccount(member);
    else showGate();
  }
  async function request<T = unknown>(route: string, body?: unknown, session = state.session): Promise<T | null> {
    if (!session) throw Error(tr("Connect your account first."));
    const options: RequestInit = {cache:"no-store", credentials:"same-origin", method:body === undefined ? "GET" : "POST", headers:site.ownerHeaders(session,body)};
    if (body !== undefined) options.body = JSON.stringify(body);
    const response = await fetch(route, options);
    await site.checkAccess(response, options);
    if (!response.ok) {
      const error: Error & {status?: number} = new Error(response.status === 401 || response.status === 403
        ? tr("Your account could not be verified. Reconnect with your own token.")
        : tr("This section could not load. Check your connection and try again."));
      error.status = response.status;
      if (state.session === session && (response.status === 401 || response.status === 403)) showGate(error.message);
      throw error;
    }
    return response.status === 204 ? null : response.json();
  }
  const owner = <T = unknown>(route: string, body?: unknown, session = state.session) => request<T>(route + "/" + encodeURIComponent(session!.user), body, session);
  function error(message: string) {
    $("personal-error").hidden = false;
    $("personal-error").textContent = message;
    $<HTMLButtonElement>("personal-refresh").disabled = false;
    $("personal-app").setAttribute("aria-busy", "false");
  }
  async function load() {
    const session = state.session;
    if (!session) return;
    const epoch = ++state.epoch;
    $("personal-error").hidden = true;
    $("personal-app").setAttribute("aria-busy", "true");
    $<HTMLButtonElement>("personal-refresh").disabled = true;
    $("personal-content").innerHTML = html`<p class="loading-copy" role="status">${tr("Loading your progress…")}</p>`;
    try {
      if (page === "friends") {
        const social = await owner<Social>("/api/social",undefined,session);
        if (state.session !== session || state.epoch !== epoch) return;
        state.social = social;
        renderFriends();
      } else if (page === "join") {
        const [invite,social] = await Promise.allSettled([request<Invite>(`/api/invites/${encodeURIComponent(inviteCode)}`,undefined,session),owner<Social>("/api/social",undefined,session)]);
        if (state.session !== session || state.epoch !== epoch) return;
        if (social.status === "rejected") throw social.reason;
        state.social = social.value;
        state.invite = invite.status === "fulfilled" ? invite.value : null;
        renderJoin();
      } else if (page === "settings") {
        const names: SettingName[] = ["companion","reminders","freezes","nudges","subscriptions","decks"];
        const calls = [owner("/api/companion",undefined,session),owner("/api/community/reminders",undefined,session),owner("/api/streak-freezes",undefined,session),owner("/api/friend-nudges",undefined,session),owner("/api/deck-subscriptions",undefined,session),owner("/api/decks",undefined,session)];
        const [results, account] = await Promise.all([Promise.allSettled(calls), owner<{created_at: number}>("/api/accounts",undefined,session).catch(() => null)]);
        if (state.session !== session || state.epoch !== epoch) return;
        state.account = account;
        state.errors = {};
        results.forEach((result,index) => {
          const settings = state.settings as Record<SettingName, unknown>;
          if (result.status === "fulfilled") settings[names[index]] = result.value;
          else { settings[names[index]] = null; state.errors[names[index]] = (result.reason as Error).message; }
        });
        renderSettings();
      } else {
        const year = state.year == null ? "" : `?year=${state.year}`;
        const [profile,study,challenges,activity] = await Promise.allSettled([
          owner<Profile>("/api/profile",undefined,session),
          request<StudyHistory>(`/api/study/${encodeURIComponent(session.user)}${year}`,undefined,session),
          owner<Challenges>("/api/community/challenges",undefined,session),
          request<Activity>(`/api/activity/${encodeURIComponent(session.user)}?days=30&limit=20`,undefined,session),
        ]);
        if (state.session !== session || state.epoch !== epoch) return;
        if (study.status === "rejected") throw study.reason;
        state.profile = profile.status === "fulfilled" ? profile.value : null;
        state.study = study.value;
        state.challenges = challenges.status === "fulfilled" ? challenges.value : null;
        state.activity = activity.status === "fulfilled" ? activity.value : null;
        state.year = study.value!.year;
        if (page === "today") renderToday(); else renderHistory();
      }
    } catch (cause) {
      if (state.session === session && state.epoch === epoch) error((cause as Error).message);
    } finally {
      if (state.session === session && state.epoch === epoch) {
        $<HTMLButtonElement>("personal-refresh").disabled = false;
        $("personal-app").setAttribute("aria-busy", "false");
      }
    }
  }
  function link(route: string,label: string,primary=false) {
    const [path,fragment] = route.split("#");
    return html`<a class="button-link${primary ? " primary" : ""}" href="${esc(site.href(path, fragment ? "#"+fragment : ""))}">${esc(label)}</a>`;
  }
  function freshness() {
    const sync = state.study?.last_received_at, review = state.study?.last_review_at;
    if (sync) return html`<span class="personal-stamp">${tr("Study data updated: ")}${esc(when(sync))}</span>`;
    if (review) return html`<span class="personal-stamp">${tr("Latest uploaded review: ")}${esc(when(review))}</span>`;
    return html`<span class="personal-stamp">${tr("No reviews uploaded yet.")}</span>`;
  }
  function recap() {
    const session = state.study?.latest_session;
    if (!session) return html`<article class="card"><h2>${tr("Latest synced session")}</h2><p class="section-intro">${tr("Your next study session will appear after its reviews are uploaded.")}</p></article>`;
    const time = Number(session.time_ms) < 60000 ? tr("under a minute") : tr`${Math.round(Number(session.time_ms)/60000)} minutes`;
    return html`<article class="card"><div class="card-head"><div><h2>${tr("Latest synced session")}</h2><p>${esc(when(session.ended_at))}</p></div><span class="chip green">${tr("Nice work")}</span></div><div class="personal-summary"><div><strong>${number(session.reviews)}</strong><span>${tr("reviews")}</span></div><div><strong>${esc(time)}</strong><span>${tr("study time")}</span></div><div><strong>${number(session.new_cards)}</strong><span>${tr("new cards")}</span></div></div><p class="settings-hint">${tr("This recap groups reviews separated by less than five minutes on the same Anki day. It appears after those reviews reach AnkiQuest.")}</p></article>`;
  }
  function quest(quest: Quest) {
    return html`<div class="personal-quest"><div class="toolbar"><strong>${esc(quest.title)}</strong><span class="chip ${quest.done ? "green" : ""}">${quest.done ? tr("Complete") : tr("In progress")}</span></div><progress max="${Math.max(1,Number(quest.target)||1)}" value="${Math.max(0,Number(quest.progress)||0)}"></progress><small>${number(quest.progress)} / ${number(quest.target)} · +${number(quest.reward)} XP</small></div>`;
  }
  function activeGoals() {
    const all = state.challenges?.challenges || [];
    const visible = all.filter(goal => !["complete","cancelled","declined","expired"].includes(goal.status)).slice(0,3);
    const invited = visible.filter(goal => goal.members?.some(member => member.user === state.session!.user && member.status === "invited"));
    const unread = Number(state.activity?.unread_count || 0);
    return html`<article class="card"><div class="card-head"><div><h2>${tr("Study with friends")}</h2><p>${invited.length===1 ? tr("One invitation waiting for you") : invited.length ? tr`${invited.length} invitations waiting for you` : tr("A little encouragement goes a long way.")}</p></div><span class="symbol" aria-hidden="true">✦</span></div>${visible.length ? html`<div class="stack">${visible.map(goal => html`<div class="list-row"><span class="symbol" aria-hidden="true">★</span><div class="row-text"><strong>${esc(goal.title)}</strong><small>${goal.members?.some(member => member.user === state.session!.user && member.status === "invited") ? tr("Invitation waiting") : tr("Friend goal in progress")}</small></div><a class="button-link" href="${esc(site.href("/community","#challenge-"+encodeURIComponent(goal.id)))}">${tr("Open")}</a></div>`).join("")}</div>` : html`<p class="section-intro">${tr("Start a small goal together or cheer on a friend.")}</p>`}<div class="personal-actions">${link("/community#challenges",tr("Friends"),true)}${link("/community#activity",unread ? tr`${unread} new updates` : tr("Activity"))}</div></article>`;
  }
  function renderToday() {
    const profile = state.profile, today: Partial<Profile["today"]> = profile?.today || {}, quests = profile?.quests || [];
    const reviewed = Number(today.reviews || 0);
    const lead = reviewed ? tr("Every review moved you forward today.") : tr("A small start still counts. Your cards are waiting in Anki.");
    $("personal-content").innerHTML = html`<section class="personal-lead"><div class="eyebrow">${tr("Your Anki day")}</div><h2>${profile ? tr`Hello, ${esc(profile.display)}.` : tr("A fresh page for today.")}</h2><p>${lead}</p><div class="personal-actions">${link("/community#challenges",tr("See friend goals"),true)}${link("/history",tr("View study history"))}<a class="button-link" href="https://ankiweb.net/decks" target="_blank" rel="noopener noreferrer">${tr("Study in AnkiWeb ↗")}</a></div>${freshness()}</section><div class="kpis">${site.kpi(tr("Reviews today"),number(today.reviews),tr("Synced from Anki"),"blue")}${site.kpi(tr("Today's XP"),number(today.xp),tr("Every card counts"),"gold")}${site.kpi(tr("Current streak"),number(profile?.streak || 0),tr("Anki study days"),"green")}${site.kpi(tr("Level"),number(profile?.level || 1),tr("Keep moving forward"))}</div><div class="grid"><article class="card"><div class="card-head"><div><h2>${tr("Today's quests")}</h2><p>${tr("Three small wins to aim for.")}</p></div></div>${quests.length ? html`<div class="stack">${quests.map(quest).join("")}</div>` : html`<p class="personal-empty">${tr("Upload some reviews to see your quests.")}</p>`}</article>${activeGoals()}</div>${recap()}`;
  }
  function calendarMonth(year: number, month: number, map: Map<string, StudyDay>, max: number) {
    const first = new Date(Date.UTC(year,month,1)), count = new Date(Date.UTC(year,month+1,0)).getUTCDate();
    const pad = (first.getUTCDay()+6)%7;
    const title = new Intl.DateTimeFormat(AnkiQuestI18n.language,{month:"long",timeZone:"UTC"}).format(first);
    const monthDays = [...map.values()].filter(day=>Number(day.date.slice(5,7))===month+1);
    const monthReviews = monthDays.reduce((sum,day)=>sum+Number(day.reviews||0),0);
    const selectedMonth = state.selected?.startsWith(`${year}-${String(month+1).padStart(2,"0")}-`);
    const weekdays = [1,2,3,4,5,6,0].map(day => new Intl.DateTimeFormat(AnkiQuestI18n.language,{weekday:"narrow",timeZone:"UTC"}).format(new Date(Date.UTC(2024,0,7+day))));
    const cells = Array.from({length:pad},()=>html`<span aria-hidden="true"></span>`);
    for(let day=1;day<=count;day++) {
      const key = `${year}-${String(month+1).padStart(2,"0")}-${String(day).padStart(2,"0")}`;
      const item = map.get(key), reviews = Number(item?.reviews||0);
      const level = reviews ? Math.min(4,1+Math.floor(reviews/Math.max(1,max)*3.99)) : 0;
      const future = new Date(key+"T12:00:00Z").getTime() > Date.now()+12*3600000;
      cells.push(future && !item ? html`<span aria-hidden="true"></span>` : html`<button type="button" data-day="${key}" data-level="${level}" data-frozen="${!!item?.frozen}" aria-pressed="${state.selected===key}" aria-label="${esc(date(key))}: ${number(reviews)} ${tr("reviews")}${item?.frozen ? tr(", streak protected") : ""}">${day}</button>`);
    }
    return html`<details class="card personal-month" ${selectedMonth?"open":""}><summary>${esc(title)}<small>${number(monthReviews)} ${tr("reviews")}</small></summary><div class="personal-month-body"><div class="personal-weekdays" aria-hidden="true">${weekdays.map(label=>html`<span>${esc(label)}</span>`).join("")}</div><div class="personal-calendar">${cells.join("")}</div></div></details>`;
  }
  function dayDetail() {
    if (!state.selected) return html`<article class="card" id="day-detail"><h2>${tr("Choose a day")}</h2><p class="section-intro">${tr("Select a square to see what happened that day.")}</p></article>`;
    const day = state.study!.days.find(item => item.date === state.selected) || {date:state.selected,reviews:0,time_ms:0,xp:0,new_cards:0,streak:0,frozen:false};
    return html`<article class="card" id="day-detail" tabindex="-1"><div class="card-head"><div><div class="eyebrow">${tr("Your Anki day")}</div><h2>${esc(date(day.date))}</h2></div>${day.frozen ? html`<span class="chip blue">${tr("Streak protected")}</span>` : ""}</div><div class="personal-summary"><div><strong>${number(day.reviews)}</strong><span>${tr("reviews")}</span></div><div><strong>${number(Math.round(Number(day.time_ms)/60000))}</strong><span>${tr("minutes studied")}</span></div><div><strong>${number(day.xp)}</strong><span>${tr("XP earned")}</span></div><div><strong>${number(day.new_cards)}</strong><span>${tr("new cards")}</span></div><div><strong>${number(day.streak)}</strong><span>${tr("day streak")}</span></div></div><p class="settings-hint">${day.reviews ? tr("These totals reflect reviews uploaded to AnkiQuest.") : tr("No reviews recorded for this day.")}</p></article>`;
  }
  function renderHistory() {
    const days = state.study!.days || [], year = state.study!.year, map = new Map(days.map(day=>[day.date,day]));
    const max = Math.max(1,...days.map(day=>Number(day.reviews)||0));
    const years = [...new Set([year,...(state.study!.years||[])])].sort((a,b)=>b-a);
    if (!state.selected || !map.has(state.selected)) state.selected = [...days].reverse().find(day=>day.reviews>0)?.date || days.at(-1)?.date || null;
    const reviews = days.reduce((sum,day)=>sum+Number(day.reviews),0), studied = days.filter(day=>day.reviews>0).length, xp=days.reduce((sum,day)=>sum+Number(day.xp),0);
    $("personal-content").innerHTML = html`<div class="personal-year"><label for="history-year">${tr("Study year")}</label><select id="history-year">${years.map(item=>html`<option value="${item}" ${item===year?"selected":""}>${item}</option>`).join("")}</select>${freshness()}</div><div class="kpis">${site.kpi(tr("Reviews"),number(reviews),tr("This year"),"blue")}${site.kpi(tr("Study days"),number(studied),tr("This year"),"green")}${site.kpi(tr("XP earned"),number(xp),tr("This year"),"gold")}</div>${dayDetail()}<div class="personal-months">${Array.from({length:12},(_,month)=>calendarMonth(year,month,map,max)).join("")}</div><p class="settings-hint">${tr("A day follows your Anki cutoff. These are uploaded reviews, so another device may have newer activity until it syncs.")}</p>`;
  }
  function personRow(person: Person, actions: string) {
    return html`<div class="list-row">${site.avatar(person.user,person.display)}<div class="row-text"><strong>${esc(person.display)}</strong><small>${esc(person.user)}</small></div>${actions}</div>`;
  }
  const action = (label: string, body: Record<string, unknown>, extra = "") => html`<button type="button" data-social="${esc(JSON.stringify(body))}" ${extra}>${esc(label)}</button>`;
  const inviteLink = (code: string) => location.origin + site.href("/join/" + encodeURIComponent(code));
  const members = (count: number) => count === 1 ? tr("1 member") : tr`${count} members`;
  function renderFriends() {
    const social = state.social!, me = state.session!.user;
    const visibility = html`<article class="card"><div class="card-head"><div><h2>${tr("Your profile")}</h2><p>${social.public ? tr("Anyone can see your profile.") : tr("Only your friends and groups can see your profile.")}</p></div></div>${check("",tr("Public profile"),social.public,tr("Show my profile and standings on the global leaderboard."),"data-social-public")}</article>`;
    const requests = social.incoming.length ? html`<h3>${tr("Friend requests")}</h3><div class="stack">${social.incoming.map(person=>personRow(person,action(tr("Accept"),{action:"befriend",user:person.user},'class="primary"')+action(tr("Decline"),{action:"unfriend",user:person.user}))).join("")}</div>` : "";
    const waiting = social.outgoing.length ? html`<h3>${tr("Waiting for an answer")}</h3><div class="stack">${social.outgoing.map(person=>personRow(person,action(tr("Cancel request"),{action:"unfriend",user:person.user}))).join("")}</div>` : "";
    const friendList = social.friends.length
      ? html`<div class="stack">${social.friends.map(person=>personRow(person,action(tr("Remove"),{action:"unfriend",user:person.user},`data-confirm="${esc(tr`Remove ${person.display} from your friends?`)}"`))).join("")}</div>`
      : html`<p class="settings-hint">${tr("No friends yet. Send a request with their username.")}</p>`;
    const friendsCard = html`<article class="card"><div class="card-head"><div><h2>${tr("Friends")}</h2><p>${tr("Friends see each other's progress and can share decks, goals and nudges.")}</p></div></div><form id="friend-form" class="social-form"><label class="field"><span>${tr("Username")}</span><input name="user" required maxlength="32" autocomplete="off" spellcheck="false"></label><button class="primary" type="submit">${tr("Send friend request")}</button></form>${requests}${waiting}<h3>${tr("Your friends")}</h3>${friendList}</article>`;
    const invite = (item: Group) => item.owner && item.invite
      ? html`<div class="social-invite"><label class="field"><span>${tr("Invite link")}</span><input readonly value="${esc(inviteLink(item.invite))}" data-invite-link></label><div class="setting-actions"><button type="button" data-copy-invite>${tr("Copy link")}</button>${action(tr("New link"),{action:"new_invite",group:item.id},`data-confirm="${esc(tr("The old link will stop working. Make a new one?"))}"`)}</div><p class="settings-hint">${tr("Anyone with this link can join. Share it only with people you want in the group.")}</p></div>`
      : "";
    const member = (item: Group, person: Person) => personRow(person,item.owner && person.user !== me ? action(tr("Remove"),{action:"remove",group:item.id,member:person.user},`data-confirm="${esc(tr`Remove ${person.display} from ${item.name}?`)}"`) : "");
    const group = (item: Group) => html`<details class="card social-group" data-group="${item.id}" open><summary><strong>${esc(item.name)}</strong><small>${esc(members(item.members.length))}</small></summary>${invite(item)}<div class="stack">${item.members.map(person=>member(item,person)).join("")}</div><div class="setting-actions">${action(tr("Leave group"),{action:"leave",group:item.id},`data-confirm="${esc(tr`Leave ${item.name}?`)}"`)}</div></details>`;
    const groupsCard = html`<article class="card"><div class="card-head"><div><h2>${tr("Groups")}</h2><p>${tr("Everyone in a group sees each other on the group's leaderboard.")}</p></div></div><form id="group-form" class="social-form"><label class="field"><span>${tr("Group name")}</span><input name="name" required maxlength="40" autocomplete="off"></label><button class="primary" type="submit">${tr("Create group")}</button></form></article>${social.groups.map(group).join("")}`;
    $("personal-content").innerHTML = html`<div class="personal-settings">${visibility}${friendsCard}${groupsCard}</div><p id="social-status" class="status" role="status"></p>`;
  }
  function renderJoin() {
    const invite = state.invite;
    $("personal-content").innerHTML = invite
      ? html`<article class="card"><div class="card-head"><div><div class="eyebrow">${tr("Group invitation")}</div><h2>${esc(invite.name)}</h2><p>${esc(members(invite.members))}</p></div></div><p class="section-intro">${tr("Members see each other's progress on the group's leaderboard and can share decks, goals and nudges.")}</p><div class="setting-actions"><button class="primary" type="button" id="join-group">${tr("Join group")}</button>${link("/friends",tr("Not now"))}</div><p id="social-status" class="status" role="status"></p></article>`
      : html`<article class="card"><h2>${tr("This invite link is not valid any more.")}</h2><p class="section-intro">${tr("Ask whoever sent it for a new link.")}</p><div class="setting-actions">${link("/friends",tr("Friends and groups"),true)}</div></article>`;
  }
  function socialError(status: number | undefined, body: Record<string, unknown>) {
    if (status === 404) return body.action === "join" ? tr("This invite link is not valid any more.") : tr("There is nobody with that username.");
    if (status === 409) return body.action === "befriend" ? tr("Wait for some friend requests to be answered first.") : tr("That group, or your list of groups, is full.");
    if (status === 400) return tr("Group names are 1 to 40 characters.");
    return tr("That could not be saved. Please try again.");
  }
  async function social(body: Record<string, unknown>, controls: (HTMLInputElement | HTMLButtonElement)[]) {
    const session = state.session;
    if (!session) return false;
    const status = document.getElementById("social-status");
    controls.forEach(control => control.disabled = true);
    if (status) { status.className = "status"; status.textContent = tr("Saving…"); }
    try {
      const saved = await owner<Social>("/api/social",body,session);
      if (state.session !== session) return false;
      state.social = saved;
      if (page === "friends") { renderFriends(); $("social-status").textContent = tr("Saved."); }
      return true;
    } catch (cause) {
      if (state.session === session) {
        controls.forEach(control => control.disabled = false);
        const notice = document.getElementById("social-status");
        if (notice) { notice.className = "status error"; notice.textContent = socialError((cause as {status?: number}).status, body); }
      }
      return false;
    }
  }
  function check(name: string,title: string,checked: boolean,description="",extra="") {
    return html`<label class="check"><input type="checkbox" ${name?`name="${esc(name)}"`:""} ${extra} ${checked?"checked":""}><span><strong>${esc(title)}</strong>${description?html`<small>${esc(description)}</small>`:""}</span></label>`;
  }
  const hourOptions = (selected: number) => Array.from({length:24},(_,hour)=>html`<option value="${hour}" ${hour===Number(selected)?"selected":""}>${String(hour).padStart(2,"0")}:00</option>`).join("");
  function settingError(name: SettingName) { return html`<p class="status error" role="alert">${esc(state.errors[name] || tr("This setting could not load. Refresh to try again."))}</p>`; }
  function renderSettings(card: SettingName | null = null) {
    const {companion,reminders:prefs,freezes,nudges,subscriptions,decks} = state.settings;
    const companionCard = html`<article class="card"><div class="card-head"><div><h2>${tr("Your study companion")}</h2><p>${tr("Choose who cheers you on. This choice follows your account across devices.")}</p></div></div>${companion?html`<form id="companion-form"><div class="companion-options" role="radiogroup" aria-label="${tr("Your study companion")}"><label class="companion-option"><input type="radio" name="companion" value="aki" ${companion.companion==="aki"?"checked":""}><img src="/aki/face.png" alt=""><strong>Aki</strong></label><label class="companion-option"><input type="radio" name="companion" value="ankilope" ${companion.companion==="ankilope"?"checked":""}><img src="/ankilope/face.png" alt=""><strong>Ankilope</strong></label><label class="companion-option companion-none"><input type="radio" name="companion" value="none" ${companion.companion==="none"?"checked":""}><span aria-hidden="true">○</span><strong>${tr("No companion")}</strong></label></div><p class="settings-hint">${tr("Your companion only changes the artwork and encouragement you see.")}</p><div class="setting-actions"><button class="primary" type="submit">${tr("Save companion")}</button></div><p class="status" role="status"></p></form>`:settingError("companion")}</article>`;
    const reminderCard = html`<article class="card"><div class="card-head"><div><h2>${tr("Reminders")}</h2><p>${tr("Choose when AnkiQuest checks in.")}</p></div></div>${prefs ? html`<form id="reminders-form"><div class="stack">${reminders.map(([key,label])=>check(key,label,prefs[key])).join("")}</div><div class="time-grid"><label class="field"><span>${tr("Daily reminder")}</span><select name="reminder_hour">${hourOptions(prefs.reminder_hour)}</select></label><label class="field"><span>${tr("Quiet from")}</span><select name="quiet_start">${hourOptions(prefs.quiet_start)}</select></label><label class="field"><span>${tr("Quiet until")}</span><select name="quiet_end">${hourOptions(prefs.quiet_end)}</select></label><label class="field"><span>${tr("Daily limit")}</span><select name="daily_limit">${[1,2,3,4,5].map(n=>html`<option value="${n}" ${n===prefs.daily_limit?"selected":""}>${n}</option>`).join("")}</select></label></div><p class="settings-hint">${tr("Reminder times use your AnkiQuest timezone. Equal quiet-hour times disable quiet hours.")}</p><div class="setting-actions"><button class="primary" type="submit">${tr("Save reminders")}</button></div><p class="status" role="status"></p></form>` : settingError("reminders")}</article>`;
    const nudgeCard = html`<article class="card"><div class="card-head"><div><h2>${tr("Encouragement")}</h2><p>${tr("Friendly nudges and progress celebrations.")}</p></div></div>${nudges ? html`${check("",tr("Allow friends to nudge me"),nudges.receiving,"",'data-nudge-field="receiving"')}${check("",tr("Automatic progress nudges"),nudges.automatic_receiving,"",'data-nudge-field="automatic"')}<fieldset><legend>${tr("Friends who can nudge me")}</legend>${nudges.friends.length ? nudges.friends.map(friend=>check("",friend.display,!friend.muted_by_me,"",`data-nudge-sender="${esc(friend.user)}"`)).join("") : html`<p class="settings-hint">${tr("No friends to choose yet.")}</p>`}</fieldset><p id="nudge-status" class="status" role="status"></p>` : settingError("nudges")}</article>`;
    const freezeCard = html`<article class="card"><div class="card-head"><div><h2>${tr("Streak protection")}</h2><p>${freezes?tr`${freezes.freezes} of ${freezes.capacity} freezes ready`:tr("Keep a little backup for busy days.")}</p></div></div>${freezes?html`${check("",tr("Use saved freezes automatically"),freezes.enabled,"",'id="freeze-toggle"')}<p class="settings-hint">${tr("Your saved freezes stay with you when protection is off.")}</p><p id="freeze-status" class="status" role="status"></p>`:settingError("freezes")}</article>`;
    const subscribed = new Set(subscriptions?.unsubscribed_senders || []), shared = new Set([...(subscriptions?.sharing_senders||[]),...subscribed]);
    const subscriptionCard = html`<article class="card"><div class="card-head"><div><h2>${tr("Deck completion alerts I receive")}</h2><p>${tr("Choose the people whose completed decks reach you.")}</p></div></div>${subscriptions?html`<form id="subscriptions-form">${check("enabled",tr("Receive deck completion alerts"),subscriptions.enabled)}<fieldset><legend>${tr("Unsubscribe from specific people")}</legend>${subscriptions.senders.filter(person=>shared.has(person.user)).map(person=>check("",person.display,subscribed.has(person.user),"",`data-unsubscribe="${esc(person.user)}"`)).join("") || html`<p class="settings-hint">${tr("No one is sharing decks with you yet.")}</p>`}</fieldset><div class="setting-actions"><button class="primary" type="submit">${tr("Save alert preferences")}</button></div><p class="status" role="status"></p></form>`:settingError("subscriptions")}</article>`;
    const deckCard = html`<article class="card"><div class="card-head"><div><h2>${tr("What I share")}</h2><p>${tr("Choose which finished decks friends can hear about.")}</p></div></div>${decks?html`<form id="decks-form"><details><summary>${tr("Manage deck sharing")}</summary>${decks.decks.length?decks.decks.map((deck,index)=>html`<fieldset data-deck-index="${index}"><legend>${esc(deck.name)}</legend>${check("",tr("Share completion"),deck.enabled,"",'data-deck-enabled')}<div class="stack">${decks.recipients.map(person=>check("",person.display,deck.recipients.includes(person.user),"",`data-deck-recipient="${esc(person.user)}"`)).join("") || html`<p class="settings-hint">${tr("No friends are available yet.")}</p>`}</div></fieldset>`).join(""):html`<p class="settings-hint">${tr("Upload reviews from Anki to see your decks here.")}</p>`}</details>${check("",tr("Celebrate milestones"),decks.celebrations!==false,"",'id="celebrations-toggle"')}<div class="setting-actions"><button class="primary" type="submit">${tr("Save deck sharing")}</button></div><p class="status" role="status"></p></form>`:settingError("decks")}</article>`;
    const markup = html`<section class="personal-lead"><div class="eyebrow">${tr("Your account")}</div><h2>${esc(state.session!.user)}</h2><p>${tr("All changes here apply to your AnkiQuest account across devices.")}</p></section><div class="personal-settings">${companionCard}${reminderCard}${nudgeCard}${freezeCard}${subscriptionCard}${deckCard}<article class="card"><h2>${tr("Privacy and account")}</h2><p class="section-intro">${tr("Your member token controls personal changes. The shared website password only lets members view the community.")}</p><div class="setting-actions"><a class="button-link" href="${esc(site.href("/week","#"+encodeURIComponent(state.session!.user)))}">${tr("Edit profile picture")}</a><button type="button" id="personal-disconnect">${tr("Disconnect account")}</button></div><p class="status" role="status"></p></article>${privacyCard()}</div>`;
    const selector = card && {companion:"#companion-form",reminders:"#reminders-form",nudges:"#nudge-status",freezes:"#freeze-toggle",subscriptions:"#subscriptions-form",decks:"#decks-form"}[card];
    const current = selector && $("personal-content").querySelector(selector)?.closest(".card");
    if (!current) {$("personal-content").innerHTML = markup;return;}
    const fresh = document.createElement("div");fresh.innerHTML = markup;
    const replacement = fresh.querySelector(selector)!.closest(".card")!;
    if (card === "decks") replacement.querySelector("details")!.open = current.querySelector("details")?.open || false;
    current.replaceWith(replacement);
  }
  function privacyCard() {
    const remove = state.account ? html`<form id="delete-account-form" class="danger-zone"><h3>${tr("Delete your account")}</h3><p class="settings-hint">${tr("This removes your account, your study data, your settings and everything you shared, straight away. It cannot be undone.")}</p><label class="field"><span>${tr("Password")}</span><input name="password" type="password" required autocomplete="current-password" maxlength="256"></label><div class="setting-actions"><button type="submit" class="danger">${tr("Delete my account")}</button></div><p class="status" role="status"></p></form>` : "";
    return html`<article class="card"><h2>${tr("Your data")}</h2><p class="section-intro">${tr("Download everything AnkiQuest stores about you, as one file.")}</p><div class="setting-actions"><button type="button" id="personal-export">${tr("Download my data")}</button><a class="button-link" href="${esc(site.href("/privacy"))}">${tr("Privacy notice")}</a></div><p id="export-status" class="status" role="status"></p>${remove}</article>`;
  }
  async function downloadData(button: HTMLButtonElement) {
    const session = state.session;
    if (!session) return;
    button.disabled = true;
    const status = $("export-status");
    status.className = "status"; status.textContent = tr("Preparing your download…");
    try {
      const response = await fetch(`/api/export/${encodeURIComponent(session.user)}`, {cache:"no-store", credentials:"same-origin", headers:site.ownerHeaders(session)});
      if (!response.ok) throw Error(tr("The download could not be prepared. Please try again."));
      const url = URL.createObjectURL(await response.blob()), link = document.createElement("a");
      link.href = url; link.download = `ankiquest-${session.user}.json`; document.body.append(link); link.click(); link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 10000);
      status.textContent = tr("Your download has started.");
    } catch (cause) {
      status.className = "status error"; status.textContent = (cause as Error).message;
    } finally { button.disabled = false; }
  }
  async function deleteAccount(form: HTMLFormElement) {
    const session = state.session, button = form.querySelector<HTMLButtonElement>("button[type=submit]")!, status = form.querySelector<HTMLElement>(".status")!;
    if (!session) return;
    if (!button.dataset.armed) {
      button.dataset.armed = "1"; button.textContent = tr("Tap again to delete everything");
      status.className = "status error"; status.textContent = tr("This cannot be undone.");
      return;
    }
    const password = (form.elements.namedItem("password") as HTMLInputElement).value, body = {password};
    form.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input,button").forEach(control => control.disabled = true);
    status.className = "status"; status.textContent = tr("Deleting…");
    try {
      const response = await fetch(`/api/accounts/${encodeURIComponent(session.user)}`, {method:"DELETE", cache:"no-store", credentials:"same-origin", headers:site.ownerHeaders(session, body), body:JSON.stringify(body)});
      if (response.status === 204) {
        await site.disconnectMember().catch(() => {});
        showGate(tr("Your account and its data were deleted."));
        return;
      }
      throw Error(response.status === 403 ? tr("That password is not right.") : response.status === 429 ? tr("Too many attempts. Try again in one minute.") : tr("Your account could not be deleted. Please try again."));
    } catch (cause) {
      if (state.session !== session) return;
      form.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input,button").forEach(control => control.disabled = false);
      delete button.dataset.armed; button.textContent = tr("Delete my account");
      status.className = "status error"; status.textContent = (cause as Error).message;
    }
  }
  async function submitForm<K extends SettingName>(form: HTMLFormElement, route: string, body: unknown, key: K) {
    const session = state.session, controls = [...form.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>("input,select,button")], status = form.querySelector<HTMLElement>(".status")!;
    controls.forEach(control=>control.disabled=true);status.className="status";status.textContent=tr("Saving…");
    try {
      const saved = await owner<Settings[K]>(route,body,session);
      if (state.session!==session) return;
      state.settings[key]=saved;
      if (key === "companion") AnkiQuestAki.setChoice((saved as {companion: string}).companion);
      renderSettings(key);
      const notice = $("personal-content").querySelector(`#${form.id} .status`);
      if(notice) notice.textContent=tr("Saved across your devices.");
    } catch(cause) {
      if (state.session===session){status.className="status error";status.textContent=(cause as Error).message;controls.forEach(control=>control.disabled=false);}
    }
  }
  document.addEventListener("submit", async event => {
    const form = event.target as HTMLFormElement;
    if (form.id === "personal-connect") {
      event.preventDefault();const controls=[...form.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input,button")];controls.forEach(c=>c.disabled=true);
      const user=(form.elements.namedItem("user") as HTMLInputElement).value.trim(), token=(form.elements.namedItem("token") as HTMLInputElement).value.trim();$("connect-status").textContent=tr("Connecting…");
      try { const session=await site.connectMember(user,token,()=>form.isConnected);(form.elements.namedItem("token") as HTMLInputElement).value="";useAccount(session); }
      catch(cause){(form.elements.namedItem("token") as HTMLInputElement).value="";$("connect-status").textContent=(cause as Error).message;controls.forEach(c=>c.disabled=false);}
      return;
    }
    if (state.session && page==="friends" && (form.id === "friend-form" || form.id === "group-form")) {
      event.preventDefault();
      const controls=[...form.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input,button")];
      const body=form.id === "friend-form" ? {action:"befriend",user:(form.elements.namedItem("user") as HTMLInputElement).value.trim().toLowerCase()} : {action:"create_group",name:(form.elements.namedItem("name") as HTMLInputElement).value.trim()};
      social(body,controls);
      return;
    }
    if (!state.session || page!=="settings") return;
    if (form.id === "delete-account-form") { event.preventDefault(); deleteAccount(form); return; }
    if (form.id === "companion-form") {
      event.preventDefault();submitForm(form,"/api/companion",{companion:(form.elements.namedItem("companion") as RadioNodeList).value},"companion");
    }
    if (form.id === "reminders-form") {
      event.preventDefault();const data=new FormData(form),body: Record<string, boolean | number>={};
      for(const [key] of reminders)body[key]=data.has(key);
      for(const key of ["reminder_hour","quiet_start","quiet_end","daily_limit"])body[key]=Number(data.get(key));
      submitForm(form,"/api/community/reminders",body,"reminders");
    }
    if (form.id === "subscriptions-form") {
      event.preventDefault();submitForm(form,"/api/deck-subscriptions",{enabled:(form.elements.namedItem("enabled") as HTMLInputElement).checked,unsubscribed_senders:[...form.querySelectorAll<HTMLElement>("[data-unsubscribe]:checked")].map(item=>item.dataset.unsubscribe)},"subscriptions");
    }
    if (form.id === "decks-form") {
      event.preventDefault();const current=state.settings.decks!;
      const decks=[...form.querySelectorAll<HTMLElement>("[data-deck-index]")].map(field=>({id:current.decks[Number(field.dataset.deckIndex)].id,enabled:field.querySelector<HTMLInputElement>("[data-deck-enabled]")!.checked,recipients:[...field.querySelectorAll<HTMLElement>("[data-deck-recipient]:checked")].map(input=>input.dataset.deckRecipient)}));
      if (decks.some(deck=>deck.enabled&&!deck.recipients.length)) {form.querySelector(".status")!.textContent=tr("Choose at least one friend for each shared deck.");return;}
      submitForm(form,"/api/decks",{decks,celebrations:form.querySelector<HTMLInputElement>("#celebrations-toggle")!.checked},"decks");
    }
  });
  document.addEventListener("change", async event => {
    const target=event.target as HTMLInputElement;
    if (target.id==="history-year") {state.year=Number(target.value);state.selected=null;load();return;}
    if (target.hasAttribute("data-social-public") && state.session && page==="friends") {social({action:"visibility",public:target.checked},[target]);return;}
    if (!state.session || page!=="settings") return;
    const session=state.session;
    if (target.id==="freeze-toggle") {
      target.disabled=true;const status=$("freeze-status");status.textContent=tr("Saving…");
      try {const saved=await owner<Settings["freezes"]>("/api/streak-freezes",{enabled:target.checked},session);if(state.session!==session)return;state.settings.freezes=saved;renderSettings("freezes");$("freeze-status").textContent=tr("Saved across your devices.");}
      catch(cause){if(state.session===session){target.checked=!target.checked;target.disabled=false;status.className="status error";status.textContent=(cause as Error).message;}}
    }
    if (target.dataset.nudgeField || target.dataset.nudgeSender) {
      target.disabled=true;const status=$("nudge-status");status.textContent=tr("Saving…");
      const route=target.dataset.nudgeSender ? `/api/friend-nudges/${encodeURIComponent(session.user)}/senders/${encodeURIComponent(target.dataset.nudgeSender)}` : `/api/friend-nudges/${encodeURIComponent(session.user)}/${target.dataset.nudgeField}`;
      try {await request(route,{enabled:target.checked},session);if(state.session!==session)return;state.settings.nudges=await owner<Settings["nudges"]>("/api/friend-nudges",undefined,session);if(state.session!==session)return;renderSettings("nudges");$("nudge-status").textContent=tr("Saved across your devices.");}
      catch(cause){if(state.session===session){target.checked=!target.checked;target.disabled=false;status.className="status error";status.textContent=(cause as Error).message;}}
    }
  });
  document.addEventListener("click", async event => {
    const day=(event.target as Element).closest<HTMLElement>("[data-day]");
    const target=event.target as Element, socialButton=target.closest<HTMLButtonElement>("[data-social]");
    if (socialButton && state.session) {
      if (socialButton.dataset.confirm && !socialButton.dataset.armed) {
        socialButton.dataset.armed = "1";
        socialButton.textContent = tr("Tap again to confirm");
        $("social-status").className = "status";
        $("social-status").textContent = socialButton.dataset.confirm;
        return;
      }
      social(JSON.parse(socialButton.dataset.social!),[socialButton]);
    }
    if (target.closest("[data-copy-invite]")) {
      const input=target.closest(".social-invite")!.querySelector<HTMLInputElement>("[data-invite-link]")!;
      try { await navigator.clipboard.writeText(input.value); $("social-status").textContent=tr("Invite link copied."); }
      catch { input.select(); }
    }
    if (target.id==="join-group" && state.session) {
      if (await social({action:"join",invite:inviteCode},[target as HTMLButtonElement])) location.assign(site.href("/friends"));
    }
    if(day && page==="history") {state.selected=day.dataset.day!;document.querySelectorAll("[data-day]").forEach(button=>button.setAttribute("aria-pressed",String(button===day)));$("day-detail").outerHTML=dayDetail();$("day-detail").focus();}
    if(target.id==="personal-export") downloadData(target as HTMLButtonElement);
    if((event.target as Element).id==="personal-disconnect") {
      const button=event.target as HTMLButtonElement;button.disabled=true;
      try {await site.disconnectMember();showGate(tr("Account disconnected."));}
      catch(cause){button.disabled=false;button.closest(".card")!.querySelector(".status")!.textContent=(cause as Error).message;}
    }
  });
  addEventListener("ankiquest:locked",()=>void AnkiQuestI18n.ready.then(()=>showGate(tr("Account disconnected."))));
  addEventListener("ankiquest-auth",()=>restore());
  addEventListener("ankiquest:identity",()=>{if(!state.native)restore();});
  document.addEventListener("DOMContentLoaded",restore,{once:true});
})();
