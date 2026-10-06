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
  /** For the owner only. */
  told?: boolean;
  friends?: number;
}

const app = document.getElementById("app")!;
const [, , user = "", at = ""] = location.pathname.split("/").map(decodeURIComponent);

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
  const lapses = data.conquest.lapses;
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
  ctx.textAlign = "center";
  if (art) {
    ctx.save();
    ctx.shadowColor = "rgba(0,0,0,0.25)"; ctx.shadowBlur = 30; ctx.shadowOffsetY = 14;
    ctx.drawImage(art, 290, 40, 500, 500);
    ctx.restore();
  } else {
    ctx.font = "300px system-ui, sans-serif";
    ctx.fillText("🏆", size / 2, 420);
  }
  ctx.fillStyle = "#ffffff";
  ctx.font = "800 30px system-ui, sans-serif";
  ctx.fillText(data.display.toUpperCase(), size / 2, 590);
  // Translated headlines run longer; shrink them to fit rather than clip.
  let headlineSize = 92;
  do ctx.font = `900 ${headlineSize}px system-ui, sans-serif`;
  while (ctx.measureText(headline(data)).width > size - 120 && (headlineSize -= 4) > 40);
  ctx.fillStyle = deep;
  ctx.fillText(headline(data), size / 2, 696);
  ctx.fillStyle = "#ffffff";
  ctx.fillText(headline(data), size / 2, 690);
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
  const text = shareText(data, mine), url = location.href;
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
    await navigator.clipboard.writeText(`${shareText(data, mine)} ${location.href}`);
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
    <p class="lead">${mine ? aqText`You forgot this card ${c.lapses} times, and learned it anyway. It now stays with you for weeks at a time.` : aqText`${data.display} forgot this card ${c.lapses} times, and learned it anyway. It now stays with them for weeks at a time.`}</p>
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
    <p class="conquest-note">Only these numbers are shared, never the card itself.</p>
    <p><a href="${AnkiQuestSite.href("/week", "#" + encodeURIComponent(data.user))}">${mine ? aqText("Back to your profile") : aqText`See ${data.display}'s profile`}</a></p>
  </div>
</article>`;
  // The companion's art loads lazily elsewhere; here it is the point of the page.
  app.querySelector<HTMLImageElement>(".aki-art")?.setAttribute("loading", "eager");
  document.getElementById("conquest-share")!.addEventListener("click", () => share(data, mine));
  document.getElementById("conquest-copy")!.addEventListener("click", () => copy(data, mine));
  document.getElementById("conquest-download")!.addEventListener("click", () => download(data));
  const button = document.getElementById("conquest-tell") as HTMLButtonElement | null;
  if (button && session) button.addEventListener("click", () => tell(data, session, button));
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
