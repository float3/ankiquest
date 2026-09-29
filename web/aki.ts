// @ts-nocheck
/* Companions are presentation only: never change study rules, rewards or notifications. */
(() => {
  "use strict";
  const poses = new Set(["welcome", "review", "celebrate", "streak", "freeze", "winner", "face"]);
  const ankilopePoses = {welcome:"welcome", review:"study", celebrate:"celebrate", streak:"study", freeze:"study", winner:"celebrate", face:"face"};
  const choices = new Set(["aki", "ankilope", "none"]);
  let choice = "aki", identity = null, identityEpoch = 0;
  const escape = value => String(value).replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
  const source = pose => choice === "ankilope" ? `/ankilope/${ankilopePoses[pose] || "welcome"}.png` : `/aki/${pose}.png`;
  const image = (pose = "welcome", extra = "") => {
    pose = poses.has(pose) ? pose : "welcome";
    return `<img class="aki-art ${escape(extra)}" data-companion-pose="${pose}" src="${source(pose)}" width="128" height="128" alt="" aria-hidden="true" decoding="async" loading="lazy">`;
  };
  function setChoice(next) {
    choice = choices.has(next) ? next : "aki";
    document.documentElement.dataset.companion = choice;
    for (const img of document.querySelectorAll("img[data-companion-pose]")) img.src = source(img.dataset.companionPose);
    dispatchEvent(new CustomEvent("ankiquest:companion", {detail:{companion:choice}}));
  }
  function mood(profile) {
    if (profile?.day_ends_at > 0 && profile.day_ends_at <= Date.now()) return "review";
    if (profile?.streak_state === "protected" && !(profile?.today?.reviews > 0)) return "freeze";
    if (profile?.quests?.length && profile.quests.every(q => q.done === true)) return "celebrate";
    if (profile?.streak_state === "studied" && profile?.today?.reviews > 0) return "streak";
    return "review";
  }
  const messages = {
    welcome: "A small step starts with one card.",
    review: "A small step starts with one card.",
    celebrate: "Your daily quests are complete. Look at you go!",
    streak: "You showed up today. That is progress worth keeping.",
    freeze: "Your streak is protected by a freeze. A fresh start is waiting.",
  };
  function profile(value) {
    const pose = mood(value);
    return `<aside class="aki-companion" data-aki-companion="${pose}">${image(pose)}<p>${escape(AnkiQuestI18n.t(messages[pose]))}</p></aside>`;
  }
  function decorate(root = document) {
    const select = selector => [ ...(root.matches?.(selector) ? [root] : []), ...root.querySelectorAll?.(selector) || [] ];
    for (const brand of select(".brand img, .login-brand img")) {
      if (!brand.dataset.aki) { brand.src = source("face"); brand.dataset.aki = "face"; brand.dataset.companionPose = "face"; }
    }
    for (const element of select(".page-hero:not([data-aki]), .empty:not([data-aki]), .loading-copy:not([data-aki]), .login-card:not([data-aki]), #view-trophies .card-head:not([data-aki]), #view-records .card-head:not([data-aki]), #winner-history .win-heading:not([data-aki])")) {
      let pose = "welcome";
      if (element.matches(".page-hero, #view-trophies .card-head, #view-records .card-head, #winner-history .win-heading")) pose = location.pathname === "/records" || element.closest("#view-trophies, #view-records, #winner-history") ? "winner" : "streak";
      if (element.matches(".loading-copy")) pose = "review";
      element.dataset.aki = pose;
      element.insertAdjacentHTML("afterbegin", image(pose));
    }
    // Dialog contents are recreated after close. Mark only the content, not the persistent dialog.
    for (const title of select("#streak-freezes h3:not([data-aki])")) {
      title.dataset.aki = "freeze";
      title.insertAdjacentHTML("beforebegin", image("freeze", "aki-dialog-art"));
    }
  }
  async function loadChoice(user) {
    if (identity !== user) setChoice("aki");
    identity = user;
    const epoch = ++identityEpoch;
    if (!user) return setChoice("aki");
    try {
      const response = await fetch(`/api/companion/${encodeURIComponent(user)}`, {cache:"no-store", credentials:"same-origin"});
      if (!response.ok) return;
      const saved = await response.json();
      if (epoch === identityEpoch) setChoice(saved.companion);
    } catch (_) { /* Keep the last visible choice while offline. */ }
  }
  window.AnkiQuestAki = {image, mood, profile, decorate, setChoice, choice:()=>choice};
  addEventListener("ankiquest:identity", event => loadChoice(event.detail?.user));
  addEventListener("ankiquest:locked", () => loadChoice(null));
  document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible" && identity) loadChoice(identity); });
  document.documentElement.dataset.companion = choice;
  document.addEventListener("DOMContentLoaded", () => {
    decorate();
    if (!document.body || typeof MutationObserver !== "function") return;
    new MutationObserver(records => {
      for (const record of records) for (const node of record.addedNodes) if (node.nodeType === 1) decorate(node);
    }).observe(document.body, {subtree:true, childList:true});
  }, {once:true});
})();
