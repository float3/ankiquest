// The web pages as the server serves them, for tests that run them without a server:
// scripts bundled from web/ the way build.rs bundles them, pages with their inline
// scripts filled in, and the translation module.
import {buildSync, transformSync} from "esbuild";
import {execFileSync} from "node:child_process";
import fs from "node:fs";
import {createRequire} from "node:module";
import path from "node:path";
import vm from "node:vm";
import type {Page, Route} from "playwright";

export const root = path.resolve(import.meta.dirname, "..", "..");

/** Playwright, or the module named by PLAYWRIGHT_MODULE, resolved from tests/ as `require` did. */
export const {chromium} = createRequire(path.join(root, "tests", "index.js"))(process.env.PLAYWRIGHT_MODULE || "playwright") as typeof import("playwright");

const scripts = new Map<string, string>();

/** A web/ entry such as "site", "avatars" or "pages/index", bundled like build.rs does. */
export function script(entry: string): string {
  let code = scripts.get(entry);
  if (code === undefined) {
    const result = buildSync({
      entryPoints: [path.join(root, "web", `${entry}.ts`)],
      bundle: true,
      format: "iife",
      target: "es2022",
      charset: "utf8",
      write: false,
      logLevel: "warning",
    });
    code = result.outputFiles[0]!.text;
    scripts.set(entry, code);
  }
  return code;
}

/**
 * The top of a page script, up to (not including) `end`, as plain JavaScript for a
 * classic <script>: its top-level functions and constants become page globals, so
 * tests can call the real renderers without starting the page.
 */
export function prefix(entry: string, end: string): string {
  const source = fs.readFileSync(path.join(root, "web", `${entry}.ts`), "utf8");
  const index = source.indexOf(end);
  if (index < 0) throw new Error(`web/${entry}.ts no longer contains ${JSON.stringify(end)}`);
  const code = transformSync(source.slice(0, index), {loader: "ts", target: "es2022", charset: "utf8"}).code;
  // `export {}` only marks the file as a module for the type checker.
  return code.replace(/^export\s*\{\s*\};?$/gm, "");
}

/** A page from static/ with each `<script data-entry>` placeholder replaced by its code. */
export function page(name: string): string {
  const source = fs.readFileSync(path.join(root, "static", `${name}.html`), "utf8");
  return source.replace(/<script data-entry="([^"]+)"><\/script>/g, (_, entry: string) => `<script>\n${script(entry)}</script>`);
}

let wasm: Buffer | undefined;

/** The translation module, built with the same profile as the server's. */
export function wasmBytes(): Buffer {
  if (!wasm) {
    execFileSync("cargo", ["build", "--quiet", "--package=ankiquest-i18n-wasm", "--target=wasm32-unknown-unknown", "--profile=wasm"], {cwd: root, stdio: "inherit"});
    wasm = fs.readFileSync(path.join(root, "target/wasm32-unknown-unknown/wasm/ankiquest_i18n_wasm.wasm"));
  }
  return wasm;
}

let compiled: WebAssembly.Module | undefined;

/** The compiled translation module, which `web/i18n.ts` accepts as `AnkiQuestI18nModule`. */
export function wasmModule(): WebAssembly.Module {
  compiled ??= new WebAssembly.Module(new Uint8Array(wasmBytes()));
  return compiled;
}

/** A translation catalog from static/. */
export function catalog(language: "es" | "fr" | "de" | "pt"): Record<string, string> {
  return JSON.parse(fs.readFileSync(path.join(root, "static", `translations-${language}.json`), "utf8")) as Record<string, string>;
}

/** Globals that are not part of JavaScript itself, for scripts run in a `vm` context. */
export function hostGlobals(): Record<string, unknown> {
  return {URL, URLSearchParams, Request, Headers, Response, DOMException, Event, AbortController, TextEncoder, TextDecoder, console, setTimeout, clearTimeout, queueMicrotask, AnkiQuestI18nModule: wasmModule()};
}

/** Runs a bundled entry in a fresh context. The context doubles as `window`. */
export function run(entry: string, context: Record<string, unknown>): vm.Context {
  context.window ??= context;
  vm.createContext(context);
  vm.runInContext(script(entry), context);
  return context;
}

/**
 * Answers a Playwright route for a file the server builds or embeds, or returns
 * false. Pages that load site.js need /i18n.wasm too.
 */
export async function fulfillAsset(route: Route, pathname: string): Promise<boolean> {
  const files: Record<string, [string, () => string | Buffer]> = {
    "/site.js": ["text/javascript", () => script("site")],
    "/avatars.js": ["text/javascript", () => script("avatars")],
    "/personal.js": ["text/javascript", () => script("personal")],
    "/i18n.wasm": ["application/wasm", wasmBytes],
    "/site.css": ["text/css", () => fs.readFileSync(path.join(root, "static/site.css"), "utf8")],
    "/avatars.css": ["text/css", () => fs.readFileSync(path.join(root, "static/avatars.css"), "utf8")],
    "/personal.css": ["text/css", () => fs.readFileSync(path.join(root, "static/personal.css"), "utf8")],
  };
  const file = files[pathname];
  if (!file) return false;
  await route.fulfill({contentType: file[0], body: file[1]()});
  return true;
}

/**
 * Opens `body` at a fake origin that answers only the built assets, so a page can
 * load site.js and fetch /i18n.wasm. Every other request is aborted.
 */
export async function open(page: Page, body: string, url = "https://ankiquest.test/"): Promise<void> {
  await page.route("**/*", async route => {
    const request = route.request();
    if (request.url() === url && request.resourceType() === "document") return route.fulfill({contentType: "text/html", body});
    if (!await fulfillAsset(route, new URL(request.url()).pathname)) await route.abort();
  });
  await page.goto(url);
}

/** Adds site.js (translations, companion, site helpers) and waits until translations are ready. */
export async function addSite(page: Page): Promise<void> {
  await page.addScriptTag({content: script("site")});
  await page.evaluate(() => AnkiQuestI18n.ready);
}
