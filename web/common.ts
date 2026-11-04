/* Small helpers every bundle needs, imported rather than shared through `window`
   so they also work in scripts that run before site.js. */

const entities: Record<string, string> = {"&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;"};

/** Text safe to place in HTML content or a quoted attribute; `null` and `undefined` become empty. */
export function escape(value: unknown): string {
  return String(value ?? "").replace(/[&<>"']/g, char => entities[char]!);
}

/**
 * The account AnkiDroid handed this page, or `null` when it has none or is signed out. Current apps
 * hand only the member and have already installed its HttpOnly session cookie, so requests for it
 * carry no token; older apps also hand the token itself.
 */
export function nativeAccount(): OwnerSession | null {
  const value = window.ankiquestSession;
  if (typeof value?.user !== "string" || !value.user) return null;
  const token = typeof value.token === "string" ? value.token.trim() : "";
  return token ? {user: value.user, token} : {user: value.user};
}

/** A count in the page's language; anything that is not a number shows as 0. */
export function number(value: unknown): string {
  return Number(value || 0).toLocaleString(AnkiQuestI18n.language);
}

/** Orders strings by code point, the same in every browser and language. */
export function lexical(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** The 24 `<option>`s of an hour picker, 00:00 to 23:00, with `selected` chosen. */
export function hourOptions(selected: unknown): string {
  return Array.from({length: 24}, (_, hour) => `<option value="${hour}" ${hour === Number(selected) ? "selected" : ""}>${String(hour).padStart(2, "0")}:00</option>`).join("");
}

/**
 * A calendar day such as "2026-09-25" in the page's language, read at noon UTC so no time zone
 * moves it to another day: "—" for none, and the value itself when it is not a date.
 */
export function day(value: unknown, options: Intl.DateTimeFormatOptions = {month: "short", day: "numeric", year: "numeric"}): string {
  if (!value) return "—";
  const parsed = new Date(String(value).slice(0, 10) + "T12:00:00Z");
  return Number.isNaN(parsed.getTime()) ? String(value) : new Intl.DateTimeFormat(AnkiQuestI18n.language, {...options, timeZone: "UTC"}).format(parsed);
}
