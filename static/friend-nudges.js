/* Friend nudges use the same private account as the community page. */
(() => {
  const plain = (source, ...values) => Array.isArray(source) ? source.reduce((text, part, index) => text + (index ? values[index - 1] : "") + part, "") : source;
  const aqText = (...args) => (window.AnkiQuestI18n?.t || plain)(...args);
  const aqHtml = (...args) => (window.AnkiQuestI18n?.html || plain)(...args);
  const dialog = document.createElement("dialog");
  dialog.className = "dialog";
  dialog.setAttribute("aria-label", aqText("Nudge your friends"));
  document.body.append(dialog);
  let session = null, busy = false, generation = 0;
  const close = () => { generation++; session = null; busy = false; dialog.close(); dialog.replaceChildren(); };
  const current = (captured, epoch) => session === captured && state.session === captured && generation === epoch && dialog.open;
  function render(payload) {
    dialog.innerHTML = aqHtml`<div class="card-head"><h2>Nudge your friends</h2><button type="button" data-nudge-close aria-label="Close">Close</button></div><p>A little encouragement to do Anki. One nudge per friend per Anki day.</p><label class="check"><input type="checkbox" data-nudge-receiving ${payload.receiving ? "checked" : ""}><span>Let friends and AnkiQuest send me nudges</span></label><p class="settings-hint">Phone vibrations follow your alert settings, silent mode, and Do Not Disturb.</p><div class="stack">${payload.friends.map(friend => aqHtml`<div class="list-row">${avatar(friend.user, friend.display)}<div class="row-text"><strong>${esc(friend.display)}</strong><small>${friend.sent_today ? aqText("Nudged today") : friend.enabled ? aqText("Ready for encouragement") : aqText("Not receiving nudges")}</small></div><button type="button" data-nudge-user="${esc(friend.user)}" ${!friend.enabled || friend.sent_today ? "disabled" : ""}>Nudge</button></div>`).join("") || aqHtml(["<p>No friends are available yet.</p>"])}</div><p data-nudge-status role="status"></p>`;
  }
  async function open() {
    if (!state.session) return;
    close(); session = state.session;
    const captured = session, epoch = ++generation;
    dialog.innerHTML = aqHtml(['<p role="status">Loading friends…</p><button type="button" data-nudge-close>Close</button>']);
    dialog.showModal();
    try {
      const payload = await ownerRequest(`/api/friend-nudges/${encodeURIComponent(captured.user)}`, undefined, captured);
      if (current(captured, epoch)) render(payload);
    } catch (error) {
      if (current(captured, epoch)) dialog.innerHTML = aqHtml`<p role="alert">${esc(error.status === 404 ? aqText("Update the server to use friend nudges.") : error.message)}</p><button type="button" data-nudge-close>Close</button>`;
    }
  }
  async function change(control, receiving) {
    if (busy || !session || state.session !== session) return;
    const captured = session, epoch = generation, wanted = control.checked;
    busy = true;
    const controls = [...dialog.querySelectorAll("input,button:not([data-nudge-close])")];
    const disabled = controls.map(item => item.disabled);
    controls.forEach(item => item.disabled = true);
    const status = dialog.querySelector("[data-nudge-status]"); status.textContent = receiving ? aqText("Saving…") : aqText("Sending…");
    try {
      await ownerRequest(`/api/friend-nudges/${encodeURIComponent(captured.user)}${receiving ? "/receiving" : ""}`, receiving ? {enabled:wanted} : {recipient:control.dataset.nudgeUser}, captured);
      if (!current(captured, epoch)) return;
      const payload = await ownerRequest(`/api/friend-nudges/${encodeURIComponent(captured.user)}`, undefined, captured);
      if (current(captured, epoch)) { render(payload); dialog.querySelector("[data-nudge-status]").textContent = receiving ? aqText("Preference saved.") : aqText("Nudge sent!"); }
    } catch (error) {
      if (current(captured, epoch)) {
        if (receiving) control.checked = !wanted;
        controls.forEach((item, index) => item.disabled = disabled[index]);
        status.textContent = error.status === 409 ? aqText("You already nudged this friend today.") : error.status === 403 && !receiving ? aqText("This friend is not receiving nudges.") : "Could not confirm the change. Refresh and try again. " + error.message;
      }
    } finally { if (current(captured, epoch)) busy = false; }
  }
  document.addEventListener("click", event => {
    const button = event.target.closest("button"); if (!button) return;
    if (button.hasAttribute("data-nudge-friends")) open();
    if (button.hasAttribute("data-nudge-close") || button.hasAttribute("data-disconnect")) close();
    if (button.dataset.nudgeUser) change(button, false);
  });
  dialog.addEventListener("change", event => { if (event.target.hasAttribute("data-nudge-receiving")) change(event.target, true); });
  dialog.addEventListener("cancel", close);
  addEventListener("ankiquest:locked", close);
  addEventListener("ankiquest-auth", close);
  addEventListener("pagehide", close);
})();
