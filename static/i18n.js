/* Translate authored interface text before interpolating names or messages. */
(() => {
  "use strict";
  const catalog = window.AnkiQuestSpanish || {};
  const normalize = value => String(value || "en").toLowerCase().split(/[-_]/)[0] === "es" ? "es" : "en";
  let previousLanguage;
  try { previousLanguage = sessionStorage.getItem("ankiquestLanguage"); } catch (_) {}
  let language = normalize(window.ankiquestLanguage || previousLanguage || navigator.language);
  const substitute = (source, values) => source.replace(/\{(\d+)\}/g, (match, index) => index < values.length ? String(values[index]) : match);
  function translate(source) {
    const leading = source.match(/^\s*/)[0], trailing = source.match(/\s*$/)[0], key = source.trim();
    return language === "es" && Object.hasOwn(catalog, key) ? leading + catalog[key] + trailing : source;
  }
  function t(source, ...values) {
    if (Array.isArray(source)) return substitute(translate(source.reduce((text, part, index) => text + (index ? `{${index - 1}}` : "") + part, "")), values);
    return translate(String(source));
  }
  function html(strings, ...values) {
    const marker = index => `\u0001AQ${index}\u0002`;
    const source = strings.reduce((text, part, index) => text + (index ? marker(index - 1) : "") + part, "");
    function fragment(value) {
      const slots = [];
      const key = value.replace(/\u0001AQ(\d+)\u0002/g, (match, index) => { slots.push(marker(index)); return `{${slots.length - 1}}`; });
      return substitute(translate(key), slots);
    }
    const translated = source.split(/(<[^>]*>)/g).map(part => part.startsWith("<")
      ? part.replace(/\b(aria-label|title|placeholder)="([^"]*)"/g, (_, attribute, value) => `${attribute}="${fragment(value)}"`)
      : fragment(part)).join("");
    return translated.replace(/\u0001AQ(\d+)\u0002/g, (_, index) => String(values[index]));
  }
  function staticLabels() {
    document.documentElement.lang = language;
    document.querySelectorAll("[data-i18n]").forEach(element => {
      const source = element.dataset.i18n;
      element.textContent = t(source);
    });
    document.querySelectorAll("[data-i18n-attrs]").forEach(element => {
      const attributes = JSON.parse(element.dataset.i18nAttrs);
      for (const [attribute, source] of Object.entries(attributes)) element.setAttribute(attribute, t(source));
    });
  }
  function set(value) {
    const next = normalize(value);
    if (language === next) return;
    language = next;
    // Rebuild module-level UI labels too. Only the language is stored; never credentials.
    let stored = false;
    try { sessionStorage.setItem("ankiquestLanguage", language); stored = true; } catch (_) {}
    staticLabels();
    dispatchEvent(new CustomEvent("ankiquest:language", {detail:{language}}));
    if (stored) location.reload();
  }
  window.AnkiQuestI18n = {t, html, set, get language() {return language;}};
  const originalFetch = window.fetch.bind(window);
  window.fetch = (input, options = {}) => {
    const url = new URL(input instanceof Request ? input.url : input, location.href);
    if (url.origin === location.origin && url.pathname.startsWith("/api/")) {
      const headers = new Headers(options.headers || (input instanceof Request ? input.headers : undefined));
      headers.set("Accept-Language", language);
      options = {...options, headers};
    }
    return originalFetch(input, options);
  };
  const savedLanguages = new Set();
  let cookieUser = null;
  async function saveLanguage() {
    const native = window.ankiquestSession;
    const session = native?.user && native?.token ? native : cookieUser ? {user:cookieUser} : null;
    if (!session?.user) return;
    const key = JSON.stringify([session.user, language]);
    if (savedLanguages.has(key)) return;
    savedLanguages.add(key);
    try {
      const response = await fetch(`/api/language/${encodeURIComponent(session.user)}`, {
        method:"POST", credentials:"same-origin", headers:{"Content-Type":"application/json", "X-Ankiquest-CSRF":"1", ...(session.token ? {Authorization:"Bearer "+session.token} : {})},
        body:JSON.stringify({language}),
      });
      if (!response.ok && response.status !== 404) savedLanguages.delete(key);
    } catch (_) { savedLanguages.delete(key); }
  }
  addEventListener("ankiquest:language", saveLanguage);
  addEventListener("ankiquest:identity", event => { cookieUser = event.detail?.user || null; saveLanguage(); });
  addEventListener("ankiquest:locked", () => { cookieUser = null; });
  addEventListener("ankiquest-auth", () => { if (window.ankiquestLanguage) set(window.ankiquestLanguage); saveLanguage(); });
  document.addEventListener("DOMContentLoaded", staticLabels);
  document.addEventListener("DOMContentLoaded", saveLanguage);
  if (document.readyState !== "loading") staticLabels();
})();
