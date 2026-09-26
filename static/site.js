/* Shared navigation, presentation components, and the browser access gate. */
(() => {
  "use strict";
  const {t:aqText,html:aqHtml} = AnkiQuestI18n;

  const embedded = new URLSearchParams(location.search).get("embed") === "1";
  document.documentElement.classList.toggle("embedded", embedded);
  const escape = value => String(value ?? "").replace(/[&<>"']/g, char => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[char]));
  const href = (path, hash = "") => `${path}${embedded ? (path.includes("?") ? "&" : "?") + "embed=1" : ""}${hash}`;
  const avatar = (user, display = user) => {
    if (window.AnkiQuestAvatars) return AnkiQuestAvatars.markup(user, display);
    let hash = 0;
    for (const char of String(user)) hash = (hash * 31 + char.charCodeAt(0)) | 0;
    const initials = String(display || user).trim().split(/\s+/).map(part => Array.from(part)[0] || "").slice(0, 2).join("").toUpperCase();
    return aqHtml`<span class="avatar" style="border-color:hsl(${Math.abs(hash) % 360} 55% 55%)" aria-hidden="true">${escape(initials)}</span>`;
  };
  const kpi = (label, value, detail, tone = "") => aqHtml`<div class="kpi ${escape(tone)}"><span class="kpi-label">${escape(label)}</span><strong>${escape(value)}</strong><span>${escape(detail)}</span></div>`;
  let statusPromise, lastIdentity, statusEpoch = 0;
  const memberUser = access => typeof access?.member?.user === "string" ? access.member.user : null;
  function publishIdentity(access) {
    const user = memberUser(access);
    if (lastIdentity !== user) {
      lastIdentity = user;
      dispatchEvent(new CustomEvent("ankiquest:identity", {detail: {user}}));
    }
    if (user) setProfile(user);
  }
  async function status(fresh = false, notify = true) {
    if (fresh) { statusPromise = undefined; statusEpoch++; }
    if (!statusPromise) statusPromise = fetch("/auth/status", {cache: "no-store", credentials: "same-origin"})
      .then(response => response.ok ? response.json() : null).catch(() => null);
    const epoch = statusEpoch, pending = statusPromise;
    const access = await pending;
    if (epoch !== statusEpoch) return status(false, notify);
    if (access) {
      if (notify) publishIdentity(access);
    } else statusPromise = undefined;
    return access;
  }
  async function member(user) {
    const identity = memberUser(await status());
    return identity && (!user || identity === user) ? {user: identity} : null;
  }
  const ownerHeaders = (session, body) => ({
    ...(session?.token ? {Authorization: "Bearer " + session.token} : {}),
    ...(body === undefined ? {} : {"Content-Type": "application/json", "X-Ankiquest-CSRF": "1"}),
  });
  // Sign-in and logout both mutate the same cookie. Keep their responses in order.
  let connectionEpoch = 0, connectionTail = Promise.resolve();
  const serializeConnection = operation => {
    const result = connectionTail.then(operation, operation);
    connectionTail = result.catch(() => {});
    return result;
  };
  const nativeAccount = () => {
    const value = window.ankiquestSession;
    return typeof value?.user === "string" && value.user && typeof value.token === "string" && value.token.trim()
      ? {user: value.user, token: value.token.trim()} : null;
  };
  const nativeKey = value => value ? JSON.stringify([value.user, value.token]) : "";
  const changedConnection = () => new DOMException(aqText("The account changed while connecting."), "AbortError");
  const sessionRequest = (path, token) => fetch(path, {method: "POST", credentials: "same-origin", headers: {
    ...(token ? {Authorization: "Bearer " + token} : {}), "X-Ankiquest-CSRF": "1",
  }});
  const invalidateStatus = () => { statusPromise = undefined; statusEpoch++; };
  async function logoutSession() {
    const response = await sessionRequest("/auth/logout");
    if (!response.ok && response.status !== 404) throw new Error(aqText("Could not disconnect. Check your connection and try again."));
    invalidateStatus();
  }
  async function reconcileNativeAccount(epoch) {
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
  async function connectMember(user, token, current = () => true) {
    const started = connectionEpoch;
    const owner = await fetch("/api/community/reminders/" + encodeURIComponent(user), {cache: "no-store", headers: {Authorization: "Bearer " + token}});
    if (started !== connectionEpoch || !current()) throw changedConnection();
    if (!owner.ok) throw new Error(aqText("This token was not accepted for the selected player. Check the player and token."));
    const epoch = ++connectionEpoch;
    const active = () => epoch === connectionEpoch && current();
    return serializeConnection(async () => {
      if (!active()) {
        if (epoch === connectionEpoch) await reconcileNativeAccount(epoch);
        throw changedConnection();
      }
      const response = await sessionRequest("/auth/session", token);
      if (!active()) {
        if (response.status !== 404 && epoch === connectionEpoch) await reconcileNativeAccount(epoch);
        throw changedConnection();
      }
      if (response.status === 404) return {user, token};
      if (!response.ok) throw new Error(aqText("Could not connect your account. Check the token and try again."));
      const access = await status(true, false), identity = memberUser(access);
      if (!active()) {
        if (epoch === connectionEpoch) await reconcileNativeAccount(epoch);
        throw changedConnection();
      }
      if (identity && identity !== user) {
        try { await logoutSession(); }
        finally { dispatchEvent(new Event("ankiquest:locked")); }
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
    document.querySelectorAll(".site-lock").forEach(button => button.hidden = !access?.private_site && !memberUser(access));
    if (memberUser(access)) setProfile(memberUser(access));
    if (access?.private_site && !access.authenticated) {
      dispatchEvent(new Event("ankiquest:locked"));
      location.replace(loginURL());
    }
    return access;
  }
  async function checkAccess(response, options = {}) {
    if (response.status === 401 && !new Headers(options.headers).has("Authorization")) {
      const access = await status(true);
      if (access?.private_site && !access.authenticated) {
        dispatchEvent(new Event("ankiquest:locked"));
        location.replace(loginURL());
      }
    }
  }
  async function readJSON(url, options = {}) {
    const response = await fetch(url, {cache: "no-store", ...options});
    await checkAccess(response, options);
    if (!response.ok) throw new Error(`The server could not load this page (${response.status}).`);
    return response.json();
  }
  function setProfile(user, current = false) {
    const target = lastIdentity || user;
    document.querySelectorAll("[data-profile-link]").forEach(link => {
      link.hidden = !target;
      link.textContent = lastIdentity ? aqText("My profile") : aqText("Profile");
      link.href = href("/week", "#" + encodeURIComponent(target));
      if (current && target === user) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    });
  }
  async function mountHeader() {
    const header = document.querySelector("[data-site-header]");
    if (!header) return;
    const actions = header.querySelector("[data-site-actions]");
    const active = header.dataset.siteSection || "leaderboard";
    const links = [["leaderboard", "/week", aqText("Leaderboard")], ["records", "/records", aqText("Records")], ["community", "/community", aqText("Community")]];
    header.innerHTML = aqHtml`<a class="brand" href="${href("/")}" aria-label="AnkiQuest home"><img src="/icon.svg" alt="">ankiquest</a><nav class="site-nav" aria-label="Main navigation">${links.map(([key, path, label]) => aqHtml`<a href="${href(path)}"${key === active ? ' aria-current="page"' : ""}>${label}</a>`).join("")}</nav><div class="top-actions"><a data-profile-link hidden>Profile</a><button type="button" class="site-lock" hidden>Lock site</button></div><p class="site-status error" role="status" hidden></p>`;
    if (embedded) header.insertAdjacentHTML("afterend", aqHtml`<nav class="tabs embedded-nav" aria-label="Main navigation">${links.map(([key, path, label]) => aqHtml`<a href="${href(path)}"${key === active ? ' aria-current="page"' : ""}>${label}</a>`).join("")}</nav>`);
    const controls = header.querySelector(".top-actions");
    if (actions) while (actions.firstChild) controls.insertBefore(actions.firstChild, controls.querySelector(".site-lock"));
    try { setProfile(localStorage.getItem("ankiquestPlayer") || ""); } catch (_) {}
    const lock = header.querySelector(".site-lock"), message = header.querySelector(".site-status");
    lock.addEventListener("click", async () => {
      lock.disabled = true;
      message.hidden = true;
      try {
        const access = await disconnectMember();
        if (!access?.private_site) location.replace(loginURL());
      } catch (error) {
        message.textContent = error.message;
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
