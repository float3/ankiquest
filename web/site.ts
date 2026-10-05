import "./i18n";
import "./aki";

/* Shared navigation, presentation components, and the browser access gate. */
(() => {
  "use strict";
  const {t:aqText,html:aqHtml} = AnkiQuestI18n;

  const embedded = new URLSearchParams(location.search).get("embed") === "1";
  document.documentElement.classList.toggle("embedded", embedded);
  const entities: Record<string, string> = {"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"};
  const escape = (value: unknown) => String(value ?? "").replace(/[&<>"']/g, char => entities[char]);
  const href = (path: string, hash = "") => `${path}${embedded ? (path.includes("?") ? "&" : "?") + "embed=1" : ""}${hash}`;
  const avatar = (user: string, display = user) => {
    if (window.AnkiQuestAvatars) return AnkiQuestAvatars!.markup(user, display);
    let hash = 0;
    for (const char of String(user)) hash = (hash * 31 + char.charCodeAt(0)) | 0;
    const initials = String(display || user).trim().split(/\s+/).map(part => Array.from(part)[0] || "").slice(0, 2).join("").toUpperCase();
    return aqHtml`<span class="avatar" style="border-color:hsl(${Math.abs(hash) % 360} 55% 55%)" aria-hidden="true">${escape(initials)}</span>`;
  };
  const kpi = (label: string, value: unknown, detail: unknown, tone = "") => aqHtml`<div class="kpi ${escape(tone)}"><span class="kpi-label">${escape(label)}</span><strong>${escape(value)}</strong><span>${escape(detail)}</span></div>`;
  let statusPromise: Promise<Access | null> | undefined, lastIdentity: string | null | undefined, statusEpoch = 0;
  const memberUser = (access: Access | null | undefined) => typeof access?.member?.user === "string" ? access.member.user : null;
  function publishIdentity(access: Access | null) {
    const user = memberUser(access);
    if (lastIdentity !== user) {
      lastIdentity = user;
      dispatchEvent(new CustomEvent("ankiquest:identity", {detail: {user}}));
    }
    if (user) setProfile(user);
  }
  async function status(fresh = false, notify = true): Promise<Access | null> {
    if (fresh) { statusPromise = undefined; statusEpoch++; }
    if (!statusPromise) statusPromise = fetch("/auth/status", {cache: "no-store", credentials: "same-origin"})
      .then((response): Promise<Access> | null => response.ok ? response.json() : null).catch(() => null);
    const epoch = statusEpoch, pending = statusPromise;
    const access = await pending;
    if (epoch !== statusEpoch) return status(false, notify);
    if (access) {
      if (notify) publishIdentity(access);
    } else statusPromise = undefined;
    return access;
  }
  async function member(user?: string) {
    const identity = memberUser(await status());
    return identity && (!user || identity === user) ? {user: identity} : null;
  }
  const ownerHeaders = (session: OwnerSession | null | undefined, body?: unknown): Record<string, string> => ({
    ...(session?.token ? {Authorization: "Bearer " + session.token} : {}),
    ...(body === undefined ? {} : {"Content-Type": "application/json", "X-Ankiquest-CSRF": "1"}),
  });
  // Sign-in and logout both mutate the same cookie. Keep their responses in order.
  let connectionEpoch = 0, connectionTail: Promise<unknown> = Promise.resolve();
  const serializeConnection = <T>(operation: () => Promise<T>) => {
    const result = connectionTail.then(operation, operation);
    connectionTail = result.catch(() => {});
    return result;
  };
  const nativeAccount = () => {
    const value = window.ankiquestSession;
    return typeof value?.user === "string" && value.user && typeof value.token === "string" && value.token.trim()
      ? {user: value.user, token: value.token.trim()} : null;
  };
  const nativeKey = (value: OwnerSession | null) => value ? JSON.stringify([value.user, value.token]) : "";
  // Only thrown after a request, which may finish before translations load.
  const changedConnection = async () => {
    await AnkiQuestI18n.ready;
    return new DOMException(aqText("The account changed while connecting."), "AbortError");
  };
  const sessionRequest = (path: string, token?: string) => fetch(path, {method: "POST", credentials: "same-origin", headers: {
    ...(token ? {Authorization: "Bearer " + token} : {}), "X-Ankiquest-CSRF": "1",
  }});
  const invalidateStatus = () => { statusPromise = undefined; statusEpoch++; };
  async function logoutSession() {
    const response = await sessionRequest("/auth/logout");
    if (!response.ok && response.status !== 404) {
      await AnkiQuestI18n.ready;
      throw new Error(aqText("Could not disconnect. Check your connection and try again."));
    }
    invalidateStatus();
  }
  async function reconcileNativeAccount(epoch: number) {
    // An already-sent response can install its cookie even after the caller changes
    // account. Restore the latest host identity before allowing another mutation.
    while (epoch === connectionEpoch) {
      const native = nativeAccount(), key = nativeKey(native);
      let accepted = false;
      if (native) {
        const owner = await fetch("/api/community/reminders/" + encodeURIComponent(native.user), {cache: "no-store", headers: {Authorization: "Bearer " + native.token}});
        if (epoch !== connectionEpoch) return;
        if (nativeKey(nativeAccount()) !== key) continue;
        if (owner.ok) {
          const response = await sessionRequest("/auth/session", native.token);
          accepted = response.ok;
        }
      }
      if (epoch !== connectionEpoch) return;
      if (!accepted) await logoutSession();
      if (nativeKey(nativeAccount()) !== key) continue;
      await status(true);
      if (nativeKey(nativeAccount()) === key) return;
    }
  }
  // A token is kept only for older servers which do not issue an owner session.
  async function connectMember(user: string, token: string, current: () => boolean = () => true): Promise<OwnerSession> {
    const started = connectionEpoch;
    const owner = await fetch("/api/community/reminders/" + encodeURIComponent(user), {cache: "no-store", headers: {Authorization: "Bearer " + token}});
    if (started !== connectionEpoch || !current()) throw await changedConnection();
    if (!owner.ok) {
      await AnkiQuestI18n.ready;
      throw new Error(aqText("This token was not accepted for the selected player. Check the player and token."));
    }
    const epoch = ++connectionEpoch;
    const active = () => epoch === connectionEpoch && current();
    return serializeConnection(async (): Promise<OwnerSession> => {
      if (!active()) {
        if (epoch === connectionEpoch) await reconcileNativeAccount(epoch);
        throw await changedConnection();
      }
      const response = await sessionRequest("/auth/session", token);
      if (!active()) {
        if (response.status !== 404 && epoch === connectionEpoch) await reconcileNativeAccount(epoch);
        throw await changedConnection();
      }
      if (response.status === 404) return {user, token};
      if (!response.ok) {
        await AnkiQuestI18n.ready;
        throw new Error(aqText("Could not connect your account. Check the token and try again."));
      }
      const access = await status(true, false), identity = memberUser(access);
      if (!active()) {
        if (epoch === connectionEpoch) await reconcileNativeAccount(epoch);
        throw await changedConnection();
      }
      if (identity && identity !== user) {
        try { await logoutSession(); }
        finally { dispatchEvent(new Event("ankiquest:locked")); }
        await AnkiQuestI18n.ready;
        throw new Error(aqText("This token belongs to a different player. Reconnect with your own player and token."));
      }
      publishIdentity(access);
      return identity ? {user: identity} : {user, token};
    });
  }
  function disconnectMember() {
    connectionEpoch++;
    return serializeConnection(async () => {
      await logoutSession();
      lastIdentity = undefined;
      try { localStorage.removeItem("ankiquestPlayer"); } catch (_) {}
      dispatchEvent(new Event("ankiquest:locked"));
      return refreshAccess(true);
    });
  }
  const loginURL = () => "/login?next=" + encodeURIComponent(location.pathname + location.search + location.hash);
  async function refreshAccess(fresh = false) {
    const access = await status(fresh);
    document.querySelectorAll<HTMLElement>(".site-lock").forEach(button => button.hidden = !access?.private_site && !memberUser(access));
    document.querySelectorAll<HTMLAnchorElement>(".site-signin").forEach(link => {
      link.hidden = embedded || !access?.registration || !!memberUser(access);
      link.href = loginURL();
    });
    if (memberUser(access)) setProfile(memberUser(access)!);
    if (access?.private_site && !access.authenticated) {
      dispatchEvent(new Event("ankiquest:locked"));
      location.replace(loginURL());
    }
    return access;
  }
  async function checkAccess(response: Response, options: RequestInit = {}) {
    if (response.status === 401 && !new Headers(options.headers).has("Authorization")) {
      const access = await status(true);
      if (access?.private_site && !access.authenticated) {
        dispatchEvent(new Event("ankiquest:locked"));
        location.replace(loginURL());
      }
    }
  }
  async function readJSON<T = unknown>(url: string, options: RequestInit = {}): Promise<T> {
    const response = await fetch(url, {cache: "no-store", ...options});
    await checkAccess(response, options);
    if (!response.ok) throw new Error(`The server could not load this page (${response.status}).`);
    return response.json();
  }
  function setProfile(user: string, current = false) {
    const target = lastIdentity || user;
    document.querySelectorAll<HTMLAnchorElement>("[data-profile-link]").forEach(link => {
      link.hidden = !target;
      link.textContent = lastIdentity ? aqText("My profile") : aqText("Profile");
      link.href = href("/week", "#" + encodeURIComponent(target));
      if (current && target === user) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    });
  }
  async function mountHeader() {
    const header = document.querySelector<HTMLElement>("[data-site-header]");
    if (!header) return;
    const actions = header.querySelector("[data-site-actions]");
    const active = header.dataset.siteSection || "leaderboard";
    await AnkiQuestI18n.ready;
    const links: [string, string, string][] = [["today", "/today", aqText("Today")], ["community", "/community", aqText("Community")], ["settings", "/settings", aqText("Settings")], ["leaderboard", "/week", aqText("Leaderboard")], ["records", "/records", aqText("Records")]];
    header.innerHTML = aqHtml`<a class="brand" href="${href("/")}" aria-label="AnkiQuest home"><img src="/aki/face.png" alt="">ankiquest</a><nav class="site-nav" aria-label="Main navigation">${links.map(([key, path, label]) => aqHtml`<a href="${href(path)}"${key === active ? ' aria-current="page"' : ""}>${label}</a>`).join("")}</nav><div class="top-actions"><a data-profile-link hidden>Profile</a><a class="button-link primary site-signin" href="/login" hidden>Sign in</a><button type="button" class="site-lock" hidden>Lock site</button></div><p class="site-status error" role="status" hidden></p>`;
    if (embedded) header.insertAdjacentHTML("afterend", aqHtml`<nav class="tabs embedded-nav" aria-label="Main navigation">${links.map(([key, path, label]) => aqHtml`<a href="${href(path)}"${key === active ? ' aria-current="page"' : ""}>${label}</a>`).join("")}</nav>`);
    const controls = header.querySelector(".top-actions")!;
    if (actions) while (actions.firstChild) controls.insertBefore(actions.firstChild, controls.querySelector(".site-lock"));
    try { setProfile(localStorage.getItem("ankiquestPlayer") || ""); } catch (_) {}
    const lock = header.querySelector<HTMLButtonElement>(".site-lock")!, message = header.querySelector<HTMLElement>(".site-status")!;
    lock.addEventListener("click", async () => {
      lock.disabled = true;
      message.hidden = true;
      try {
        const access = await disconnectMember();
        if (!access?.private_site) location.replace(loginURL());
      } catch (error) {
        message.textContent = (error as Error).message;
        message.hidden = false;
        lock.disabled = false;
      }
    });
    await refreshAccess();
  }
  window.AnkiQuestSite = {embedded, escape, href, avatar, kpi, setProfile, checkAccess, readJSON, status, member, ownerHeaders, connectMember, disconnectMember};
  document.addEventListener("DOMContentLoaded", mountHeader, {once: true});
  addEventListener("pageshow", event => { if (event.persisted) refreshAccess(true); });
  document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") refreshAccess(true); });
  addEventListener("online", () => refreshAccess(true));
})();
