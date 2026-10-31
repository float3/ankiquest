/* The one way scripts call the server: owner credentials, the CSRF marker on
   writes, the site's access gate after every response, and the server's own
   explanation when it refuses. */

/** `fetch` options, plus the owner whose token to send and a value to send as JSON. */
export interface SendOptions extends RequestInit {
  session?: OwnerSession | null;
  json?: unknown;
  /** `false` keeps the response from the site's access gate, for callers that handle a lost session themselves. */
  gate?: boolean;
}

/** A refused request: its HTTP status, and the explanation the server gave, if any. */
export interface RequestFailure extends Error {
  status: number;
  detail: string;
}

const reads = new Set(["GET", "HEAD"]);

/** The bearer token of `session`, and the JSON headers when there is a `body`. */
export function ownerHeaders(session: OwnerSession | null | undefined, body?: unknown): Record<string, string> {
  return {
    ...(session?.token ? {Authorization: "Bearer " + session.token} : {}),
    ...(body === undefined ? {} : {"Content-Type": "application/json", "X-Ankiquest-CSRF": "1"}),
  };
}

function requestInit({session, json, headers, gate: _, ...options}: SendOptions): RequestInit {
  const method = (options.method ?? (json === undefined && options.body == null ? "GET" : "POST")).toUpperCase();
  const merged = new Headers(ownerHeaders(session, json));
  if (!reads.has(method)) merged.set("X-Ankiquest-CSRF", "1");
  new Headers(headers).forEach((value, name) => merged.set(name, value));
  return {cache: "no-store", credentials: "same-origin", ...options, method, headers: merged, ...(json === undefined ? {} : {body: JSON.stringify(json)})};
}

/** Sends a request and, unless `gate` is false, lets the site's access gate see the response. */
export async function send(url: string, options: SendOptions = {}): Promise<Response> {
  const init = requestInit(options);
  const response = await fetch(url, init);
  if (options.gate !== false) await window.AnkiQuestSite?.checkAccess(response, init);
  return response;
}

async function explanation(response: Response): Promise<string> {
  try {
    const body: {error?: unknown; message?: unknown} | null = await response.json();
    return typeof body?.error === "string" ? body.error : typeof body?.message === "string" ? body.message : "";
  } catch (_) {
    return "";
  }
}

/** The error for a refused response, worded by the server when it explained itself. */
export async function failure(response: Response): Promise<RequestFailure> {
  const detail = await explanation(response);
  const message = detail || `The server could not complete this request (${response.status}). Please try again.`;
  return Object.assign(new Error(message), {status: response.status, detail});
}

/** The JSON a request returns, `null` for 204 No Content; throws a `RequestFailure` when refused. */
export async function request<T>(url: string, options: SendOptions = {}): Promise<T> {
  const response = await send(url, options);
  if (!response.ok) throw await failure(response);
  return response.status === 204 ? null as T : response.json() as Promise<T>;
}

/** Whether `error` is a refused request rather than a network or script failure. */
export function refused(error: unknown): error is RequestFailure {
  return error instanceof Error && typeof (error as Partial<RequestFailure>).status === "number";
}
