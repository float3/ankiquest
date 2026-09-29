/* Friend nudges use the same private account as the community page. */

interface NudgeFriend { user: string; display: string; enabled: boolean; sent_today: boolean; muted_by_me: boolean; }
interface NudgeSettings { receiving: boolean; automatic_receiving: boolean; friends: NudgeFriend[]; }
type Change = "send" | "friend" | "automatic" | "sender";

/** What the community page shares: its connected account and its authenticated requests. */
export interface NudgeAccount {
  session(): OwnerSession | null;
  request<T>(path: string, body: unknown, session: OwnerSession): Promise<T>;
}

const statusOf = (error: unknown) => error instanceof Error && "status" in error ? error.status : undefined;
const messageOf = (error: unknown) => error instanceof Error ? error.message : String(error);

/** Creates the dialog and its listeners; call once, synchronously at page load. */
export function friendNudges(account: NudgeAccount): void {
  const {t: aqText, html: aqHtml} = AnkiQuestI18n, {escape: esc, avatar} = AnkiQuestSite;
  const dialog = document.createElement("dialog");
  dialog.className = "dialog";
  // The dialog stays closed until the page has rendered, which waits for translations too.
  void AnkiQuestI18n.ready.then(() => dialog.setAttribute("aria-label", aqText("Nudge your friends")));
  document.body.append(dialog);
  let session: OwnerSession | null = null, busy = false, generation = 0;
  const close = () => { generation++; session = null; busy = false; dialog.close(); dialog.replaceChildren(); };
  const current = (captured: OwnerSession, epoch: number) => session === captured && account.session() === captured && generation === epoch && dialog.open;
  function render(payload: NudgeSettings) {
    dialog.innerHTML = aqHtml`
      <div class="card-head"><h2>Nudge your friends</h2><button type="button" data-nudge-close aria-label="Close">Close</button></div>
      <p>A little encouragement to do Anki. One nudge per friend per Anki day.</p>
      <label class="check"><input type="checkbox" data-nudge-receiving ${payload.receiving ? "checked" : ""}><span>Let friends send me nudges</span></label>
      <label class="check"><input type="checkbox" data-nudge-automatic ${payload.automatic_receiving ? "checked" : ""}><span>Let AnkiQuest send me progress nudges</span></label>
      <p class="settings-hint">Choose which friends can nudge you below. Phone vibrations follow your alert settings, silent mode, and Do Not Disturb.</p>
      <div class="stack">${payload.friends.map(friend => aqHtml`
        <div class="list-row">${avatar(friend.user, friend.display)}<div class="row-text">
          <strong>${esc(friend.display)}</strong>
          <small>${friend.sent_today ? aqText("Nudged today") : friend.enabled ? aqText("Ready for encouragement") : aqText("Not receiving nudges")}</small>
          <label class="check"><input type="checkbox" data-nudge-sender="${esc(friend.user)}" aria-label="${esc(aqText("Receive nudges from") + " " + friend.display)}" ${friend.muted_by_me ? "" : "checked"}><span>Receive their nudges</span></label>
        </div><button type="button" data-nudge-user="${esc(friend.user)}" ${!friend.enabled || friend.sent_today ? "disabled" : ""}>Nudge</button></div>`).join("") || aqHtml`<p>No friends are available yet.</p>`}</div>
      <p data-nudge-status role="status"></p>`;
  }
  async function open() {
    const active = account.session();
    if (!active) return;
    close(); session = active;
    const captured = active, epoch = ++generation;
    dialog.innerHTML = aqHtml`<p role="status">Loading friends…</p><button type="button" data-nudge-close>Close</button>`;
    dialog.showModal();
    try {
      const payload = await account.request<NudgeSettings>(`/api/friend-nudges/${encodeURIComponent(captured.user)}`, undefined, captured);
      if (current(captured, epoch)) render(payload);
    } catch (error) {
      if (current(captured, epoch)) dialog.innerHTML = aqHtml`<p role="alert">${esc(statusOf(error) === 404 ? aqText("Update the server to use friend nudges.") : messageOf(error))}</p><button type="button" data-nudge-close>Close</button>`;
    }
  }
  async function change(control: HTMLInputElement | HTMLButtonElement, mode: Change) {
    if (busy || !session || account.session() !== session) return;
    const captured = session, epoch = generation, wanted = control instanceof HTMLInputElement ? control.checked : undefined;
    busy = true;
    const controls = [...dialog.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input,button:not([data-nudge-close])")];
    const disabled = controls.map(item => item.disabled);
    controls.forEach(item => item.disabled = true);
    const status = dialog.querySelector("[data-nudge-status]")!; status.textContent = mode === "send" ? aqText("Sending…") : aqText("Saving…");
    try {
      const base = `/api/friend-nudges/${encodeURIComponent(captured.user)}`;
      const path = mode === "friend" ? "/receiving" : mode === "automatic" ? "/automatic" : mode === "sender" ? `/senders/${encodeURIComponent(control.dataset.nudgeSender!)}` : "";
      await account.request(base + path, mode === "send" ? {recipient:control.dataset.nudgeUser} : {enabled:wanted}, captured);
      if (!current(captured, epoch)) return;
      const payload = await account.request<NudgeSettings>(`/api/friend-nudges/${encodeURIComponent(captured.user)}`, undefined, captured);
      if (current(captured, epoch)) { render(payload); dialog.querySelector("[data-nudge-status]")!.textContent = mode === "send" ? aqText("Nudge sent!") : aqText("Preference saved."); }
    } catch (error) {
      if (current(captured, epoch)) {
        if (mode !== "send" && control instanceof HTMLInputElement) control.checked = !wanted;
        controls.forEach((item, index) => item.disabled = disabled[index]);
        status.textContent = statusOf(error) === 409 ? aqText("You already nudged this friend today.") : statusOf(error) === 403 && mode === "send" ? aqText("This friend is not receiving nudges.") : "Could not confirm the change. Refresh and try again. " + messageOf(error);
      }
    } finally { if (current(captured, epoch)) busy = false; }
  }
  document.addEventListener("click", event => {
    if (!(event.target instanceof Element)) return;
    const button = event.target.closest("button"); if (!button) return;
    if (button.hasAttribute("data-nudge-friends")) open();
    if (button.hasAttribute("data-nudge-close") || button.hasAttribute("data-disconnect")) close();
    if (button.dataset.nudgeUser) change(button, "send");
  });
  dialog.addEventListener("change", event => {
    if (!(event.target instanceof HTMLInputElement)) return;
    if (event.target.hasAttribute("data-nudge-receiving")) change(event.target, "friend");
    if (event.target.hasAttribute("data-nudge-automatic")) change(event.target, "automatic");
    if (event.target.hasAttribute("data-nudge-sender")) change(event.target, "sender");
  });
  dialog.addEventListener("cancel", close);
  addEventListener("ankiquest:locked", close);
  addEventListener("ankiquest-auth", close);
  addEventListener("pagehide", close);
}
