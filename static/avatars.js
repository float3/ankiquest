/* Shared profile pictures. Tokens and selected photos live only in the open editor. */
(() => {
  "use strict";
  let revisions = Object.create(null), generation = 0, pending = null, editor = null, closeEditor = null, locked = false;
  const failed = new Set();
  const esc = value => String(value ?? "").replace(/[&<>"']/g, char => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[char]));
  const imageUrl = (user, revision) => `/api/avatar/${encodeURIComponent(user)}?v=${encodeURIComponent(revision)}`;
  const key = (user, revision) => JSON.stringify([user, revision]);

  function markup(user, display = user) {
    const initials = String(display || user).trim().split(/\s+/).map(part => Array.from(part)[0] || "").slice(0, 2).join("").toUpperCase();
    let hash = 0;
    for (const char of String(user)) hash = (hash * 31 + char.charCodeAt(0)) | 0;
    return `<span class="avatar" data-avatar-user="${esc(user)}" style="border-color:hsl(${Math.abs(hash) % 360} 55% 55%)" aria-hidden="true"><span class="avatar-initials">${esc(initials)}</span></span>`;
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

  async function decode(file) {
    if (file.size > 20 * 1024 * 1024) throw new Error("Choose a picture smaller than 20 MB.");
    const bytes = new Uint8Array(await file.slice(0, 8).arrayBuffer());
    const png = [137,80,78,71,13,10,26,10].every((byte, index) => bytes[index] === byte);
    const jpeg = bytes[0] === 255 && bytes[1] === 216 && bytes[2] === 255;
    if (!png && !jpeg) throw new Error("Choose a JPEG or PNG picture.");
    try {
      return await createImageBitmap(file, { resizeWidth: 1024, resizeQuality: "high", imageOrientation: "from-image" });
    } catch (_) { throw new Error("Could not read this picture. Try another JPEG or PNG."); }
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
    dialog.innerHTML = `<form><h2 id="avatar-editor-title">Profile picture</h2><p>Choose a picture for ${esc(display)}. It will appear wherever your AnkiQuest community sees your profile.</p><div class="avatar-preview">${markup(user, display)}</div><div class="avatar-crop-controls" hidden><p>Drag the picture to position it, or use the arrow keys.</p><label>Zoom<input name="zoom" type="range" min="1" max="4" step="0.01" value="1"></label><button type="button" data-reset-crop>Reset crop</button></div><label>Choose a picture<input name="picture" type="file" accept="image/jpeg,image/png"></label><p>JPEG or PNG, up to 20 MB.</p><div data-avatar-auth></div><p class="avatar-status" role="status" aria-live="polite"></p><div class="avatar-actions"><button type="button" data-remove>Remove picture</button><button type="button" data-close>Close</button><button type="submit">Save picture</button></div></form>`;
    document.body.append(dialog);
    const form = dialog.querySelector("form"), fileInput = form.elements.picture;
    const preview = dialog.querySelector(".avatar-preview");
    const status = dialog.querySelector(".avatar-status"), save = dialog.querySelector('[type="submit"]');
    const remove = dialog.querySelector("[data-remove]");
    let selected = null, canvas = null, busy = false, closed = false, selection = 0;
    let zoom = 1, offsetX = 0, offsetY = 0;
    const cropControls = dialog.querySelector(".avatar-crop-controls"), zoomInput = form.elements.zoom;
    let password = null, cookieOwner = false, checkingSession = !token && !native;
    const controller = new AbortController();
    function message(text, error = false) { status.textContent = text; status.classList.toggle("error", error); }
    function controls() { const waiting = busy || checkingSession; save.disabled = waiting || !selected; remove.disabled = waiting || !revisions[user]; fileInput.disabled = waiting; zoomInput.disabled = waiting || !selected; dialog.querySelector("[data-reset-crop]").disabled = waiting || !selected; if (password) password.disabled = waiting; }
    function requestToken() {
      if (password) return;
      dialog.querySelector("[data-avatar-auth]").innerHTML = '<label>Your AnkiQuest token<input name="token" type="password" autocomplete="off" spellcheck="false" required></label><p>Your token stays only in this window’s memory.</p>';
      password = form.elements.token;
    }
    function releasePreview() { selected?.close(); selected = null; canvas = null; cropControls.hidden = true; }
    function renderCrop() {
      if (!selected || !canvas) return;
      const size = canvas.width;
      const scale = Math.max(size / selected.width, size / selected.height) * zoom;
      const width = selected.width * scale, height = selected.height * scale;
      offsetX = Math.max((size - width) / 2, Math.min((width - size) / 2, offsetX));
      offsetY = Math.max((size - height) / 2, Math.min((height - size) / 2, offsetY));
      const context = canvas.getContext("2d");
      context.clearRect(0, 0, size, size);
      context.imageSmoothingQuality = "high";
      context.drawImage(selected, (size - width) / 2 + offsetX, (size - height) / 2 + offsetY, width, height);
    }
    function setZoom(value) {
      zoom = Math.max(1, Math.min(4, value));
      zoomInput.value = String(zoom);
      renderCrop();
    }
    function prepareCanvas(bitmap) {
      selected = bitmap; zoom = 1; offsetX = offsetY = 0; zoomInput.value = "1";
      canvas = document.createElement("canvas");
      canvas.width = canvas.height = 256;
      canvas.className = "avatar-crop-canvas";
      canvas.tabIndex = 0;
      canvas.setAttribute("role", "img");
      canvas.setAttribute("aria-label", "Profile picture crop. Drag to reposition, use arrow keys to move, or use the zoom slider.");
      const pointers = new Map();
      let pinch = null;
      canvas.addEventListener("pointerdown", event => {
        if (busy) return;
        canvas.setPointerCapture(event.pointerId);
        pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
        if (pointers.size === 2) {
          const [a, b] = [...pointers.values()];
          pinch = { distance: Math.hypot(a.x - b.x, a.y - b.y), zoom };
        }
      });
      canvas.addEventListener("pointermove", event => {
        if (!pointers.has(event.pointerId) || busy) return;
        const before = pointers.get(event.pointerId);
        pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
        if (pointers.size === 2 && pinch) {
          const [a, b] = [...pointers.values()];
          if (pinch.distance > 0) setZoom(pinch.zoom * Math.hypot(a.x - b.x, a.y - b.y) / pinch.distance);
        } else {
          const ratio = canvas.width / canvas.getBoundingClientRect().width;
          offsetX += (event.clientX - before.x) * ratio;
          offsetY += (event.clientY - before.y) * ratio;
          renderCrop();
        }
      });
      function endPointer(event) {
        pointers.delete(event.pointerId);
        pinch = null;
      }
      canvas.addEventListener("pointerup", endPointer);
      canvas.addEventListener("pointercancel", endPointer);
      canvas.addEventListener("wheel", event => {
        if (busy) return;
        event.preventDefault();
        setZoom(zoom * Math.exp(-event.deltaY / 500));
      }, { passive: false });
      canvas.addEventListener("keydown", event => {
        const distance = event.shiftKey ? 24 : 8;
        if (event.key === "ArrowLeft") offsetX -= distance;
        else if (event.key === "ArrowRight") offsetX += distance;
        else if (event.key === "ArrowUp") offsetY -= distance;
        else if (event.key === "ArrowDown") offsetY += distance;
        else if (event.key === "+" || event.key === "=") setZoom(zoom * 1.1);
        else if (event.key === "-" || event.key === "_") setZoom(zoom / 1.1);
        else return;
        event.preventDefault();
        renderCrop();
      });
      preview.replaceChildren(canvas);
      cropControls.hidden = false;
      renderCrop();
    }
    function close() {
      if (closed) return;
      closed = true; selection++; controller.abort(); token = "";
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
      releasePreview(); preview.innerHTML = markup(user, display); hydrate(); controls();
      if (!file) { message(""); return; }
      message("Preparing preview…");
      try {
        const bitmap = await decode(file);
        if (closed || version !== selection) { bitmap.close(); return; }
        prepareCanvas(bitmap); message("Ready to save."); controls();
      } catch (error) { if (!closed && version === selection) { message(error.message, true); controls(); } }
    });
    async function update(deleting) {
      if (busy || checkingSession || closed || (!deleting && !selected)) return;
      const credential = token || password?.value.trim();
      if (!credential && !cookieOwner) { message("Enter your AnkiQuest token to change your picture.", true); password?.focus(); return; }
      busy = true; controls(); message(deleting ? "Removing picture…" : "Saving picture…");
      try {
        const image = deleting ? null : await new Promise(resolve => canvas.toBlob(resolve, "image/png"));
        if (!deleting && !image) throw new Error("Could not prepare this picture. Try another JPEG or PNG.");
        if (closed) return;
        const response = await fetch(`/api/avatar/${encodeURIComponent(user)}`, {
          method: deleting ? "DELETE" : "POST", signal: controller.signal, credentials: "same-origin",
          headers: { ...(credential ? { Authorization: "Bearer " + credential } : {}), "X-Ankiquest-CSRF": "1", ...(!deleting ? { "Content-Type": "image/png" } : {}) },
          ...(!deleting ? { body: image } : {}),
        });
        if (closed) return;
        if (response.status === 401 || response.status === 403) {
          token = ""; cookieOwner = false; requestToken(); password.value = "";
          throw new Error(credential ? "That token was not accepted for this player. Enter your token to try again." : "Your member session expired or changed. Enter your token to reconnect and try again.");
        }
        if (!response.ok) throw new Error(response.status === 413 ? "The picture is too large. Choose a smaller one." : "The picture could not be saved. Please try again.");
        const result = deleting ? null : await response.json();
        if (closed) return;
        if (!deleting && (typeof result?.revision !== "string" || !/^\d{1,20}$/.test(result.revision))) throw new Error("The server returned an invalid picture response. Please try again.");
        generation++;
        if (deleting) delete revisions[user]; else revisions[user] = result.revision;
        failed.clear(); fileInput.value = ""; releasePreview();
        preview.innerHTML = markup(user, display); hydrate();
        message(deleting ? "Picture removed. Your initials are shown again." : "Profile picture saved.");
      } catch (error) { if (!closed) message(error.message, true); }
      finally { if (!closed) { busy = false; controls(); } }
    }
    form.addEventListener("submit", event => { event.preventDefault(); update(false); });
    zoomInput.addEventListener("input", () => setZoom(Number(zoomInput.value)));
    dialog.querySelector("[data-reset-crop]").addEventListener("click", () => { offsetX = offsetY = 0; setZoom(1); });
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
      message("Checking your member session…");
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
