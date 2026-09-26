/* Shared profile pictures. Tokens and selected photos live only in the open editor. */
(() => {
  "use strict";
  const {t:aqText,html:aqHtml} = AnkiQuestI18n;

  let revisions = Object.create(null), generation = 0, pending = null, editor = null, closeEditor = null, locked = false;
  const failed = new Set();
  const esc = value => String(value ?? "").replace(/[&<>"']/g, char => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[char]));
  const imageUrl = (user, revision) => `/api/avatar/${encodeURIComponent(user)}?v=${encodeURIComponent(revision)}`;
  const key = (user, revision) => JSON.stringify([user, revision]);

  function markup(user, display = user) {
    const initials = String(display || user).trim().split(/\s+/).map(part => Array.from(part)[0] || "").slice(0, 2).join("").toUpperCase();
    let hash = 0;
    for (const char of String(user)) hash = (hash * 31 + char.charCodeAt(0)) | 0;
    return aqHtml`<span class="avatar" data-avatar-user="${esc(user)}" style="border-color:hsl(${Math.abs(hash) % 360} 55% 55%)" aria-hidden="true"><span class="avatar-initials">${esc(initials)}</span></span>`;
  }

  function hydrate() {
    document.querySelectorAll(".avatar[data-avatar-user]").forEach(element => {
      const user = element.dataset.avatarUser, revision = revisions[user];
      const existing = element.querySelector(".avatar-photo");
      if (!revision || failed.has(key(user, revision))) { delete element.dataset.avatarLoaded; existing?.remove(); return; }
      if (existing?.dataset.revision === revision) return;
      delete element.dataset.avatarLoaded;
      existing?.remove();
      const photo = document.createElement("img");
      photo.className = "avatar-photo";
      photo.alt = "";
      photo.dataset.revision = revision;
      photo.decoding = "async";
      photo.addEventListener("load", () => {
        if (photo.parentElement === element) { photo.dataset.loaded = ""; element.dataset.avatarLoaded = ""; }
      });
      photo.addEventListener("error", () => {
        failed.add(key(user, revision));
        if (photo.parentElement === element) delete element.dataset.avatarLoaded;
        photo.remove();
      }, { once: true });
      photo.src = imageUrl(user, revision);
      element.append(photo);
    });
  }

  async function refresh() {
    if (locked) return;
    if (pending) return pending;
    const epoch = generation;
    pending = (async () => {
      try {
        const response = await fetch("/api/avatars", { cache: "no-store" });
        if (!response.ok) {
          if ([401, 403, 404].includes(response.status) && epoch === generation) { revisions = Object.create(null); hydrate(); }
          return;
        }
        const data = await response.json();
        if (!data || typeof data !== "object" || Array.isArray(data) || epoch !== generation) return;
        const next = Object.create(null);
        for (const [user, revision] of Object.entries(data)) {
          if (typeof revision === "string" && /^\d{1,20}$/.test(revision)) next[user] = revision;
        }
        revisions = next;
        // A transient image failure may be retried on the next refresh, never in an observer loop.
        failed.clear();
        hydrate();
      } catch (_) { /* Keep the last known pictures when offline. */ }
      finally { pending = null; }
    })();
    return pending;
  }

  async function normalize(file) {
    if (file.size > 20 * 1024 * 1024) throw new Error(aqText("Choose a picture smaller than 20 MB."));
    const bytes = new Uint8Array(await file.slice(0, 8).arrayBuffer());
    const png = [137,80,78,71,13,10,26,10].every((byte, index) => bytes[index] === byte);
    const jpeg = bytes[0] === 255 && bytes[1] === 216 && bytes[2] === 255;
    if (!png && !jpeg) throw new Error(aqText("Choose a JPEG or PNG picture."));
    let bitmap;
    try {
      // A single resize dimension preserves aspect ratio; the canvas crops the center square.
      bitmap = await createImageBitmap(file, { resizeWidth: 1024, resizeQuality: "high", imageOrientation: "from-image" });
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 256;
      const side = Math.min(bitmap.width, bitmap.height);
      canvas.getContext("2d").drawImage(bitmap, (bitmap.width - side) / 2, (bitmap.height - side) / 2, side, side, 0, 0, 256, 256);
      const blob = await new Promise(resolve => canvas.toBlob(resolve, "image/png"));
      if (!blob) throw new Error(aqText("Could not prepare this picture."));
      return blob;
    } catch (_) { throw new Error(aqText("Could not read this picture. Try another JPEG or PNG.")); }
    finally { bitmap?.close(); }
  }

  function open({ user, display = user, token = "" }) {
    if (editor || locked) return;
    const native = nativeAccount();
    token = typeof token === "string" ? token.trim() : "";
    if (!token && native?.user === user) token = native.token;
    const dialog = document.createElement("dialog");
    editor = dialog;
    dialog.className = "avatar-editor";
    dialog.setAttribute("aria-labelledby", "avatar-editor-title");
    dialog.innerHTML = aqHtml`<form><h2 id="avatar-editor-title">Profile picture</h2><p>Choose a picture for ${esc(display)}. It will appear wherever your AnkiQuest community sees your profile.</p><div class="avatar-preview">${markup(user, display)}</div><label>Choose a picture<input name="picture" type="file" accept="image/jpeg,image/png"></label><p>JPEG or PNG, up to 20 MB. The center square is used.</p><div data-avatar-auth></div><p class="avatar-status" role="status" aria-live="polite"></p><div class="avatar-actions"><button type="button" data-remove>Remove picture</button><button type="button" data-close>Close</button><button type="submit">Save picture</button></div></form>`;
    document.body.append(dialog);
    const form = dialog.querySelector("form"), fileInput = form.elements.picture;
    const preview = dialog.querySelector(".avatar-preview");
    const status = dialog.querySelector(".avatar-status"), save = dialog.querySelector('[type="submit"]');
    const remove = dialog.querySelector("[data-remove]");
    let selected = null, previewUrl = "", busy = false, closed = false, selection = 0;
    let password = null, cookieOwner = false, checkingSession = !token && !native;
    const controller = new AbortController();
    function message(text, error = false) { status.textContent = text; status.classList.toggle("error", error); }
    function controls() { const waiting = busy || checkingSession; save.disabled = waiting || !selected; remove.disabled = waiting || !revisions[user]; fileInput.disabled = waiting; if (password) password.disabled = waiting; }
    function requestToken() {
      if (password) return;
      dialog.querySelector("[data-avatar-auth]").innerHTML = aqHtml(['<label>Your AnkiQuest token<input name="token" type="password" autocomplete="off" spellcheck="false" required></label><p>Your token stays only in this window’s memory.</p>']);
      password = form.elements.token;
    }
    function releasePreview() { if (previewUrl) URL.revokeObjectURL(previewUrl); previewUrl = ""; }
    function close() {
      if (closed) return;
      closed = true; selection++; controller.abort(); token = ""; selected = null;
      if (password) password.value = "";
      fileInput.value = "";
      releasePreview();
      preview.replaceChildren();
      dialog.close(); dialog.remove(); editor = null; closeEditor = null;
      window.removeEventListener("pagehide", close);
    }
    closeEditor = close;
    fileInput.addEventListener("change", async () => {
      const version = ++selection, file = fileInput.files[0];
      selected = null; releasePreview(); preview.innerHTML = markup(user, display); hydrate(); controls();
      if (!file) { message(""); return; }
      message(aqText("Preparing preview…"));
      try {
        const blob = await normalize(file);
        if (closed || version !== selection) return;
        selected = blob; previewUrl = URL.createObjectURL(blob);
        const image = document.createElement("img"); image.src = previewUrl; image.alt = aqText("New profile picture preview");
        preview.replaceChildren(image); message(aqText("Ready to save.")); controls();
      } catch (error) { if (!closed && version === selection) { message(error.message, true); controls(); } }
    });
    async function update(deleting) {
      if (busy || checkingSession || closed || (!deleting && !selected)) return;
      const credential = token || password?.value.trim();
      if (!credential && !cookieOwner) { message(aqText("Enter your AnkiQuest token to change your picture."), true); password?.focus(); return; }
      busy = true; controls(); message(deleting ? aqText("Removing picture…") : aqText("Saving picture…"));
      try {
        const response = await fetch(`/api/avatar/${encodeURIComponent(user)}`, {
          method: deleting ? "DELETE" : "POST", signal: controller.signal, credentials: "same-origin",
          headers: { ...(credential ? { Authorization: "Bearer " + credential } : {}), "X-Ankiquest-CSRF": "1", ...(!deleting ? { "Content-Type": "image/png" } : {}) },
          ...(!deleting ? { body: selected } : {}),
        });
        if (closed) return;
        if (response.status === 401 || response.status === 403) {
          token = ""; cookieOwner = false; requestToken(); password.value = "";
          throw new Error(credential ? aqText("That token was not accepted for this player. Enter your token to try again.") : aqText("Your member session expired or changed. Enter your token to reconnect and try again."));
        }
        if (!response.ok) throw new Error(response.status === 413 ? aqText("The picture is too large. Choose a smaller one.") : aqText("The picture could not be saved. Please try again."));
        const result = deleting ? null : await response.json();
        if (closed) return;
        if (!deleting && (typeof result?.revision !== "string" || !/^\d{1,20}$/.test(result.revision))) throw new Error(aqText("The server returned an invalid picture response. Please try again."));
        generation++;
        if (deleting) delete revisions[user]; else revisions[user] = result.revision;
        failed.clear(); selected = null; fileInput.value = ""; releasePreview();
        preview.innerHTML = markup(user, display); hydrate();
        message(deleting ? aqText("Picture removed. Your initials are shown again.") : aqText("Profile picture saved."));
      } catch (error) { if (!closed) message(error.message, true); }
      finally { if (!closed) { busy = false; controls(); } }
    }
    form.addEventListener("submit", event => { event.preventDefault(); update(false); });
    remove.addEventListener("click", () => update(true));
    dialog.querySelector("[data-close]").addEventListener("click", close);
    dialog.addEventListener("cancel", event => { event.preventDefault(); close(); });
    dialog.addEventListener("close", close);
    window.addEventListener("pagehide", close);
    // A different native account must not silently reuse the previous browser owner's cookie.
    if (!token && !checkingSession) requestToken();
    controls(); hydrate(); dialog.showModal();
    refresh().then(() => { if (!closed) controls(); });
    if (checkingSession) {
      message(aqText("Checking your member session…"));
      // This lookup must not publish an initial identity event that closes its own editor.
      Promise.resolve().then(() => window.AnkiQuestSite?.status(false, false)).then(access => {
        if (closed) return;
        cookieOwner = access?.member?.user === user;
        if (!cookieOwner) requestToken();
        message("");
      }).catch(() => { if (!closed) { requestToken(); message(""); } }).finally(() => {
        if (!closed) { checkingSession = false; controls(); }
      });
    }
  }

  window.AnkiQuestAvatars = { markup, refresh, open, isOpen: () => !!editor, close: () => closeEditor?.() };
  window.addEventListener("ankiquest:identity", () => closeEditor?.());
  function nativeAccount() {
    const session = window.ankiquestSession;
    return session && typeof session.user === "string" && session.user && typeof session.token === "string" && session.token.trim()
      ? { user: session.user, token: session.token.trim() } : null;
  }
  function nativeIdentityKey() {
    const session = nativeAccount();
    return session ? JSON.stringify([session.user, session.token]) : "";
  }
  let nativeIdentity = nativeIdentityKey();
  window.addEventListener("ankiquest-auth", () => {
    const next = nativeIdentityKey();
    if (!next || next !== nativeIdentity) closeEditor?.();
    nativeIdentity = next;
  });
  window.addEventListener("ankiquest:locked", () => {
    locked = true;
    nativeIdentity = "";
    generation++;
    revisions = Object.create(null);
    failed.clear();
    closeEditor?.();
    hydrate();
  });
  function start() {
    new MutationObserver(hydrate).observe(document.body, { childList: true, subtree: true });
    refresh();
    setInterval(() => { if (!document.hidden) refresh(); }, 30000);
    document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start, { once: true }); else start();
})();
