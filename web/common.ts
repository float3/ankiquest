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
