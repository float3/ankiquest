/* Small helpers every bundle needs, imported rather than shared through `window`
   so they also work in scripts that run before site.js. */

const entities: Record<string, string> = {"&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;"};

/** Text safe to place in HTML content or a quoted attribute; `null` and `undefined` become empty. */
export function escape(value: unknown): string {
  return String(value ?? "").replace(/[&<>"']/g, char => entities[char]!);
}

/** The account AnkiDroid handed this page, or `null` when it has none or is signed out. */
export function nativeAccount(): {user: string; token: string} | null {
  const value = window.ankiquestSession;
  const token = typeof value?.token === "string" ? value.token.trim() : "";
  return typeof value?.user === "string" && value.user && token ? {user: value.user, token} : null;
}
