/* Translate authored interface text before interpolating names or messages.
   The catalogs and the lookup live in the ankiquest-i18n crate, which the server
   also uses, loaded here as WebAssembly. Until it has loaded, `t` and `html`
   return English; pages wait for `ready` before their first render. */

interface Exports {
  memory: WebAssembly.Memory;
  alloc(len: number): number;
  dealloc(ptr: number, len: number): void;
  normalize(ptr: number, len: number): bigint;
  translate(languagePtr: number, languageLen: number, ptr: number, len: number): bigint;
  translate_html(languagePtr: number, languageLen: number, ptr: number, len: number): bigint;
}

export interface I18n {
  /** `t("Text")`, or as a tag: t`Review ${count} cards` with `{0}` in the catalog. */
  t(source: string | TemplateStringsArray, ...values: unknown[]): string;
  /** A tag for HTML templates: translates text and labelling attributes, never the values. */
  html(strings: TemplateStringsArray, ...values: unknown[]): string;
  set(value: string): void;
  readonly language: string;
  /** Settles once translations are available, or English is certain. */
  readonly ready: Promise<void>;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();
let wasm: Exports | null = null;

function pass(value: string): [number, number] {
  const wasmExports = wasm!;
  const bytes = encoder.encode(value);
  const ptr = wasmExports.alloc(bytes.length);
  new Uint8Array(wasmExports.memory.buffer, ptr, bytes.length).set(bytes);
  return [ptr, bytes.length];
}

function receive(packed: bigint): string {
  const wasmExports = wasm!;
  const ptr = Number(packed >> 32n), len = Number(packed & 0xffffffffn);
  const value = decoder.decode(new Uint8Array(wasmExports.memory.buffer, ptr, len));
  wasmExports.dealloc(ptr, len);
  return value;
}

async function instantiate(): Promise<Exports> {
  // Tests running outside a browser hand over a compiled module.
  const given = (globalThis as {AnkiQuestI18nModule?: WebAssembly.Module}).AnkiQuestI18nModule;
  if (given) return (await WebAssembly.instantiate(given)).exports as unknown as Exports;
  const response = fetch("/i18n.wasm", {credentials: "same-origin"});
  try {
    return (await WebAssembly.instantiateStreaming(response)).instance.exports as unknown as Exports;
  } catch (_) {
    // Some proxies change the content type; compiling from the bytes still works.
    const bytes = await (await response).arrayBuffer();
    return (await WebAssembly.instantiate(bytes)).instance.exports as unknown as Exports;
  }
}

let previousLanguage: string | null = null;
try { previousLanguage = sessionStorage.getItem("ankiquestLanguage"); } catch (_) {}
/** What the device or player asked for, before normalizing. The server normalizes the same way. */
let requested = String(window.ankiquestLanguage || previousLanguage || navigator.language || "en");
let language: string | null = null;
let requestedBeforeReady = false;
let warned = false;

function current(): string {
  if (language !== null) return language;
  if (!warned) {
    warned = true;
    console.error("AnkiQuestI18n was used before it was ready; showing English");
  }
  return "en";
}

function normalize(value: string): string {
  if (!wasm) return "en";
  const [ptr, len] = pass(value);
  return receive(wasm.normalize(ptr, len));
}

function call(fn: "translate" | "translate_html", source: string): string {
  const active = current();
  if (!wasm || active === "en") return source;
  const [languagePtr, languageLen] = pass(active);
  const [ptr, len] = pass(source);
  return receive(wasm[fn](languagePtr, languageLen, ptr, len));
}

const substitute = (source: string, values: readonly unknown[]): string =>
  source.replace(/\{(\d+)\}/g, (match, index: string) => Number(index) < values.length ? String(values[Number(index)]) : match);

function t(source: string | TemplateStringsArray, ...values: unknown[]): string {
  if (Array.isArray(source)) {
    const parts = source as TemplateStringsArray;
    return substitute(call("translate", parts.reduce((text, part, index) => text + (index ? `{${index - 1}}` : "") + part, "")), values);
  }
  return call("translate", String(source));
}

function html(strings: TemplateStringsArray, ...values: unknown[]): string {
  const marker = (index: number) => `\u0001AQ${index}\u0002`;
  const source = strings.reduce((text, part, index) => text + (index ? marker(index - 1) : "") + part, "");
  return call("translate_html", source).replace(/\u0001AQ(\d+)\u0002/g, (_, index: string) => String(values[Number(index)]));
}

function staticLabels(): void {
  document.documentElement.lang = current();
  document.querySelectorAll<HTMLElement>("[data-i18n]").forEach(element => {
    element.textContent = t(element.dataset.i18n ?? "");
  });
  document.querySelectorAll<HTMLElement>("[data-i18n-attrs]").forEach(element => {
    const attributes = JSON.parse(element.dataset.i18nAttrs ?? "{}") as Record<string, string>;
    for (const [attribute, source] of Object.entries(attributes)) element.setAttribute(attribute, t(source));
  });
}

const ready: Promise<void> = instantiate().then(
  exports => { wasm = exports; },
  error => { console.error("translations could not be loaded; showing English", error); },
).then(() => {
  language = normalize(requested);
  if (requestedBeforeReady) {
    try { sessionStorage.setItem("ankiquestLanguage", language); } catch (_) {}
  }
  if (document.readyState !== "loading") staticLabels();
});

function set(value: string): void {
  requested = String(value);
  if (language === null) { requestedBeforeReady = true; return; } // `ready` applies it.
  const next = normalize(requested);
  if (language === next) return;
  language = next;
  // Rebuild module-level UI labels too. Only the language is stored; never credentials.
  let stored = false;
  try { sessionStorage.setItem("ankiquestLanguage", language); stored = true; } catch (_) {}
  staticLabels();
  dispatchEvent(new CustomEvent("ankiquest:language", {detail: {language}}));
  if (stored) location.reload();
}

window.AnkiQuestI18n = {t, html, set, ready, get language() { return current(); }};

const originalFetch = window.fetch.bind(window);
window.fetch = (input: RequestInfo | URL, options: RequestInit = {}) => {
  const url = new URL(input instanceof Request ? input.url : input, location.href);
  if (url.origin === location.origin && url.pathname.startsWith("/api/")) {
    const headers = new Headers(options.headers || (input instanceof Request ? input.headers : undefined));
    // The server normalizes this exactly as the page does, so no need to wait for `ready`.
    headers.set("Accept-Language", language ?? requested);
    options = {...options, headers};
  }
  return originalFetch(input, options);
};

const savedLanguages = new Set<string>();
let cookieUser: string | null = null;
async function saveLanguage(): Promise<void> {
  await ready;
  const native = window.ankiquestSession;
  const session: {user: string; token?: string} | null =
    native && typeof native.user === "string" && native.user && typeof native.token === "string" && native.token
      ? {user: native.user, token: native.token}
      : cookieUser ? {user: cookieUser} : null;
  if (!session?.user) return;
  const key = JSON.stringify([session.user, language]);
  if (savedLanguages.has(key)) return;
  savedLanguages.add(key);
  try {
    const response = await fetch(`/api/language/${encodeURIComponent(session.user)}`, {
      method: "POST", credentials: "same-origin",
      headers: {"Content-Type": "application/json", "X-Ankiquest-CSRF": "1", ...(session.token ? {Authorization: "Bearer " + session.token} : {})},
      body: JSON.stringify({language}),
    });
    if (!response.ok && response.status !== 404) savedLanguages.delete(key);
  } catch (_) { savedLanguages.delete(key); }
}
addEventListener("ankiquest:language", () => void saveLanguage());
addEventListener("ankiquest:identity", event => { cookieUser = (event as CustomEvent<{user?: string | null}>).detail?.user || null; void saveLanguage(); });
addEventListener("ankiquest:locked", () => { cookieUser = null; });
addEventListener("ankiquest-auth", () => { if (window.ankiquestLanguage) set(window.ankiquestLanguage); void saveLanguage(); });
document.addEventListener("DOMContentLoaded", () => { void ready.then(staticLabels); });
document.addEventListener("DOMContentLoaded", () => void saveLanguage());
