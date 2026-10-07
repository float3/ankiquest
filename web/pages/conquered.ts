export {};

const {t:aqText,html:aqHtml} = AnkiQuestI18n;
const esc = AnkiQuestSite.escape;
const num = (n: number) => n.toLocaleString(AnkiQuestI18n.language);
const DAY_MS = 86_400_000;

/** `/api/conquests/<user>/<at>` (`Shared` in src/conquests.rs). */
interface Shared {
  user: string;
  display: string;
  conquest: {at: number; lapses: number; answers: number; since: number; leech: boolean};
  /** What the card was, when its owner chose to say. */
  label?: string;
  /** For the owner only. */
  told?: boolean;
  friends?: number;
}

const app = document.getElementById("app")!;
const [, , user = "", at = ""] = location.pathname.split("/").map(decodeURIComponent);
const MAX_LABEL = 120;
/* A client that knows the card (the desktop add-on) can suggest a label in the
   fragment, which never reaches the server. Nothing is kept unless the owner saves
   it, and the fragment is dropped so it cannot leak into a copied link. */
const suggestion = new URLSearchParams(location.hash.slice(1)).get("suggest")?.slice(0, MAX_LABEL) ?? "";
if (location.hash) history.replaceState(null, "", location.pathname + location.search);
const pageUrl = () => location.origin + location.pathname;

/** The browser session, or the account AnkiDroid hands a page it opened. */
async function owner(): Promise<OwnerSession | null> {
  const native = window.ankiquestSession;
  if (native && native.user === user && typeof native.token === "string" && native.token.trim()) return {user, token: native.token.trim()};
  return AnkiQuestSite.member(user).catch(() => null);
}

function headline(data: Shared) {
  return data.conquest.leech ? aqText("Leech tamed!") : aqText("Card conquered!");
}

function shareText(data: Shared, mine: boolean) {
  const lapses = data.conquest.lapses, label = data.label;
  if (label) return mine
    ? aqText`I finally learned “${label}”, a card I had forgotten ${lapses} times. 🏆`
    : aqText`${data.display} finally learned “${label}”, a card they had forgotten ${lapses} times. 🏆`;
  return mine
    ? aqText`I finally learned a card I had forgotten ${lapses} times. 🏆`
    : aqText`${data.display} finally learned a card they had forgotten ${lapses} times. 🏆`;
}

function struggleDays(data: Shared) {
  return Math.max(1, Math.round((data.conquest.at - data.conquest.since) / DAY_MS));
}

function loadImage(src: string) {
  return new Promise<HTMLImageElement | null>(resolve => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => resolve(null);
    image.src = src;
  });
}

/** The companion pose for a conquest: a trophy for a tamed leech. */
function pose(data: Shared): CompanionPose {
  return data.conquest.leech ? "winner" : "celebrate";
}

function companionSource(data: Shared) {
  const choice = AnkiQuestAki.choice();
  if (choice === "none") return null;
  return choice === "ankilope" ? "/ankilope/celebrate.png" : `/aki/${pose(data)}.png`;
}

/** A square picture of the card to post where links do not unfurl. */
async function picture(data: Shared): Promise<Blob | null> {
  const size = 1080, canvas = document.createElement("canvas");
  canvas.width = canvas.height = size;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const [hue, deep] = data.conquest.leech ? ["#6c5ce7", "#4534b8"] : ["#f2a33a", "#c46f12"];
  const gradient = ctx.createLinearGradient(0, 0, size, size);
  gradient.addColorStop(0, hue);
  gradient.addColorStop(1, deep);
  ctx.fillStyle = gradient;
  ctx.fillRect(0, 0, size, size);
  ctx.fillStyle = "rgba(255,255,255,0.12)";
  ctx.beginPath(); ctx.arc(120, 80, 260, 0, Math.PI * 2); ctx.fill();
  ctx.beginPath(); ctx.arc(980, 1000, 200, 0, Math.PI * 2); ctx.fill();
  const source = companionSource(data), art = source ? await loadImage(source) : null;
  // With a label the companion steps back to make room for the card itself.
  const label = data.label, shift = label ? 130 : 0, artSize = label ? 400 : 500;
  ctx.textAlign = "center";
  if (art) {
    ctx.save();
    ctx.shadowColor = "rgba(0,0,0,0.25)"; ctx.shadowBlur = 30; ctx.shadowOffsetY = 14;
    ctx.drawImage(art, (size - artSize) / 2, 30, artSize, artSize);
    ctx.restore();
  } else {
    ctx.font = `${label ? 240 : 300}px system-ui, sans-serif`;
    ctx.fillText("🏆", size / 2, label ? 320 : 420);
  }
  ctx.fillStyle = "#ffffff";
  ctx.font = "800 30px system-ui, sans-serif";
  ctx.fillText(data.display.toUpperCase(), size / 2, 590 - shift);
  // Translated headlines run longer; shrink them to fit rather than clip.
  let headlineSize = 92;
  do ctx.font = `900 ${headlineSize}px system-ui, sans-serif`;
  while (ctx.measureText(headline(data)).width > size - 120 && (headlineSize -= 4) > 40);
  ctx.fillStyle = deep;
  ctx.fillText(headline(data), size / 2, 696 - shift);
  ctx.fillStyle = "#ffffff";
  ctx.fillText(headline(data), size / 2, 690 - shift);
  if (label) {
    // The card itself, as a flashcard: white, slightly tilted, the label as large as fits.
    ctx.save();
    ctx.translate(size / 2, 668);
    ctx.rotate(-0.025);
    ctx.save();
    ctx.shadowColor = "rgba(0,0,0,0.2)"; ctx.shadowBlur = 24; ctx.shadowOffsetY = 10;
    ctx.fillStyle = "#ffffff";
    ctx.beginPath(); ctx.roundRect(-430, -70, 860, 140, 28); ctx.fill();
    ctx.restore();
    let labelSize = 64;
    do ctx.font = `800 ${labelSize}px system-ui, sans-serif`;
    while (ctx.measureText(label).width > 800 && (labelSize -= 4) > 28);
    let text = label;
    while (ctx.measureText(text).width > 800 && text.length > 1) text = text.slice(0, -2) + "…";
    ctx.fillStyle = deep;
    ctx.textBaseline = "middle";
    ctx.fillText(text, 0, 2);
    ctx.restore();
  }
  const stats: [number, string][] = [
    [data.conquest.lapses, aqText("lapses")],
    [data.conquest.answers, aqText("answers")],
    [struggleDays(data), aqText("days")],
  ];
  stats.forEach(([value, label], index) => {
    const x = 90 + index * 310;
    ctx.fillStyle = "rgba(255,255,255,0.92)";
    ctx.beginPath(); ctx.roundRect(x, 760, 280, 170, 32); ctx.fill();
    ctx.fillStyle = deep;
    ctx.font = "900 76px system-ui, sans-serif";
    ctx.fillText(num(value), x + 140, 855);
    ctx.font = "800 26px system-ui, sans-serif";
    ctx.fillText(label.toUpperCase(), x + 140, 900);
  });
  ctx.fillStyle = "rgba(255,255,255,0.85)";
  ctx.font = "800 30px system-ui, sans-serif";
  ctx.fillText("AnkiQuest", size / 2, 1020);
  return new Promise(resolve => canvas.toBlob(resolve, "image/png"));
}

function say(message: string, error = false) {
  const status = document.getElementById("conquest-status");
  if (!status) return;
  status.className = error ? "status error" : "status";
  status.textContent = message;
}

async function share(data: Shared, mine: boolean) {
  const text = shareText(data, mine), url = pageUrl();
  const blob = await picture(data);
  const files = blob ? [new File([blob], "conquered.png", {type: "image/png"})] : [];
  try {
    if (files.length && navigator.canShare?.({files})) await navigator.share({files, text, url});
    else if (navigator.share) await navigator.share({text, url});
    else return copy(data, mine);
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") return;
    return copy(data, mine);
  }
}

async function copy(data: Shared, mine: boolean) {
  try {
    await navigator.clipboard.writeText(`${shareText(data, mine)} ${pageUrl()}`);
    say(aqText("Link copied."));
  } catch (_) {
    say(aqText("Copy this page's address to share it."), true);
  }
}

async function download(data: Shared) {
  const blob = await picture(data);
  if (!blob) return say(aqText("This browser cannot draw the picture."), true);
  const link = document.createElement("a");
  link.href = URL.createObjectURL(blob);
  link.download = "conquered.png";
  link.click();
  setTimeout(() => URL.revokeObjectURL(link.href), 1000);
}

async function tell(data: Shared, session: OwnerSession, button: HTMLButtonElement) {
  button.disabled = true;
  say(aqText("Telling your friends…"));
  try {
    const response = await fetch(`/api/conquests/${encodeURIComponent(user)}/${encodeURIComponent(at)}/tell`, {
      method: "POST",
      credentials: "same-origin",
      headers: AnkiQuestSite.ownerHeaders(session, {}),
      body: "{}",
    });
    await AnkiQuestSite.checkAccess(response, {method: "POST"});
    if (!response.ok) throw new Error(String(response.status));
    const {told} = await response.json() as {told: number};
    button.textContent = aqText("Friends told");
    say(told === 1 ? aqText("One friend will see it in their activity.") : aqText`${told} friends will see it in their activity.`);
  } catch (_) {
    button.disabled = false;
    say(aqText("Your friends could not be told. Please try again."), true);
  }
}

async function label(session: OwnerSession, value: string | null) {
  say(value === null ? aqText("Removing…") : aqText("Saving…"));
  try {
    const response = await fetch(`/api/conquests/${encodeURIComponent(user)}/${encodeURIComponent(at)}/label`, {
      method: value === null ? "DELETE" : "POST",
      credentials: "same-origin",
      headers: AnkiQuestSite.ownerHeaders(session, {}),
      body: value === null ? undefined : JSON.stringify({label: value}),
    });
    await AnkiQuestSite.checkAccess(response, {method: "POST"});
    if (!response.ok) throw new Error(String(response.status));
    await load();
  } catch (_) {
    say(aqText("That did not save. Please try again."), true);
  }
}

/** The owner's choice to say what the card was. Nothing is shared until they save. */
function labelControls(data: Shared) {
  if (data.label) return aqHtml`<p class="conquest-label-controls"><button type="button" id="conquest-label-remove" class="link-button">Stop showing the card</button></p>`;
  return aqHtml`<details class="conquest-label-form"${suggestion ? " open" : ""}><summary>Show what the card was</summary>
    <form id="conquest-label-form"><label class="field"><span>The word or phrase friends will see</span><input name="label" maxlength="${MAX_LABEL}" required autocomplete="off" value="${esc(suggestion)}"></label>
    <p class="settings-hint">Anyone who can open this page will see it, and so will friends you tell. You can take it back later.</p>
    <div class="conquest-actions"><button type="submit">Show it on the card</button></div></form></details>`;
}

function render(data: Shared, session: OwnerSession | null) {
  const mine = data.told !== undefined;
  const c = data.conquest;
  const when = new Date(c.at).toLocaleDateString(AnkiQuestI18n.language, {year: "numeric", month: "long", day: "numeric"});
  const friends = data.friends ?? 0;
  const tellButton = !mine ? "" : data.told
    ? aqHtml`<button type="button" disabled>Friends told</button>`
    : friends > 0
      ? aqHtml`<button type="button" id="conquest-tell">Tell my friends</button>`
      : "";
  const art = AnkiQuestAki.choice() === "none" ? `<div class="conquest-trophy" aria-hidden="true">🏆</div>` : AnkiQuestAki.image(pose(data));
  document.title = `${headline(data)} · AnkiQuest`;
  app.innerHTML = aqHtml`<article class="conquest${c.leech ? " tamed" : ""}">
  <div class="conquest-hero">
    ${art}
    <div class="conquest-badge">${c.leech ? aqText("Card tamed") : aqText("Hard card, learned at last")}</div>
    <h1>${headline(data)}</h1>
    <div class="who">${esc(data.display)} · ${esc(when)}</div>
  </div>
  <div class="conquest-body">
    ${data.label ? `<div class="conquest-label">${esc(data.label)}</div>` : ""}
    <p class="lead">${esc(mine ? aqText`You forgot this card ${c.lapses} times, and learned it anyway. It now stays with you for weeks at a time.` : aqText`${data.display} forgot this card ${c.lapses} times, and learned it anyway. It now stays with them for weeks at a time.`)}</p>
    <div class="conquest-stats">
      <div><b>${num(c.lapses)}</b><span>lapses</span></div>
      <div><b>${num(c.answers)}</b><span>answers</span></div>
      <div><b>${num(struggleDays(data))}</b><span>days</span></div>
    </div>
    <div class="conquest-actions">
      <button type="button" class="primary" id="conquest-share">Share</button>
      ${tellButton}
      <div class="pair"><button type="button" id="conquest-copy">Copy link</button><button type="button" id="conquest-download">Download picture</button></div>
    </div>
    <p id="conquest-status" class="status" role="status"></p>
    ${mine && session ? labelControls(data) : ""}
    <p class="conquest-note">${data.label ? aqText("Its owner chose to show what the card was.") : aqText("Only these numbers are shared, never the card itself.")}</p>
    <p><a href="${AnkiQuestSite.href("/week", "#" + encodeURIComponent(data.user))}">${esc(mine ? aqText("Back to your profile") : aqText`See ${data.display}'s profile`)}</a></p>
  </div>
</article>`;
  // The companion's art loads lazily elsewhere; here it is the point of the page.
  app.querySelector<HTMLImageElement>(".aki-art")?.setAttribute("loading", "eager");
  document.getElementById("conquest-share")!.addEventListener("click", () => share(data, mine));
  document.getElementById("conquest-copy")!.addEventListener("click", () => copy(data, mine));
  document.getElementById("conquest-download")!.addEventListener("click", () => download(data));
  const button = document.getElementById("conquest-tell") as HTMLButtonElement | null;
  if (button && session) button.addEventListener("click", () => tell(data, session, button));
  if (mine && session) {
    document.getElementById("conquest-label-form")?.addEventListener("submit", event => {
      event.preventDefault();
      const value = new FormData(event.target as HTMLFormElement).get("label");
      if (typeof value === "string" && value.trim()) label(session, value.trim());
    });
    document.getElementById("conquest-label-remove")?.addEventListener("click", () => label(session, null));
  }
  app.removeAttribute("aria-busy");
}

async function load() {
  await AnkiQuestI18n.ready;
  if (!user || !/^\d+$/.test(at)) {
    app.innerHTML = aqHtml`<p class="notice error" role="alert">This link is incomplete.</p>`;
    return;
  }
  const session = await owner();
  try {
    const data = await AnkiQuestSite.readJSON<Shared>(`/api/conquests/${encodeURIComponent(user)}/${encodeURIComponent(at)}`, {
      credentials: "same-origin",
      headers: AnkiQuestSite.ownerHeaders(session),
    });
    render(data, session);
  } catch (_) {
    app.innerHTML = aqHtml`<p class="notice error" role="alert">This card could not be found. It may be private, or the review behind it was undone.</p>`;
    app.removeAttribute("aria-busy");
  }
}

addEventListener("ankiquest-auth", () => { load(); });
load();
