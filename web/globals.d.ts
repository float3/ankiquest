/* What the scripts share through `window`. Each page loads several separately
   bundled scripts (site, avatars, then the page's own), so these globals are
   their only common ground. AnkiDroid also sets `ankiquestSession` and
   `ankiquestLanguage` after a page finishes loading, then dispatches
   `ankiquest-auth`. */

import type {I18n} from "./i18n";

export {};

declare global {
  /** Credentials AnkiDroid hands a page it opened; `null` means signed out there. */
  interface NativeSession {
    user?: unknown;
    token?: unknown;
  }

  /** Who a signed-in browser session belongs to, from `/auth/status`. */
  interface Access {
    private_site?: boolean;
    authenticated?: boolean;
    member?: {user?: unknown} | null;
    [key: string]: unknown;
  }

  interface OwnerSession {
    user: string;
    token?: string;
  }

  interface AnkiQuestSiteApi {
    embedded: boolean;
    escape(value: unknown): string;
    href(path: string, hash?: string): string;
    avatar(user: string, display?: string): string;
    kpi(label: string, value: unknown, detail: unknown, tone?: string): string;
    setProfile(user: string, current?: boolean): void;
    checkAccess(response: Response, options?: RequestInit): Promise<void>;
    readJSON<T = unknown>(url: string, options?: RequestInit): Promise<T>;
    status(fresh?: boolean, notify?: boolean): Promise<Access | null>;
    member(user?: string): Promise<{user: string} | null>;
    ownerHeaders(session: OwnerSession | null | undefined, body?: unknown): Record<string, string>;
    connectMember(user: string, token: string, current?: () => boolean): Promise<OwnerSession>;
    disconnectMember(): Promise<Access | null>;
  }

  /** The parts of a profile the companion reacts to. */
  interface CompanionProfile {
    day_ends_at?: number;
    streak_state?: string;
    today?: {reviews?: number};
    quests?: {done?: unknown}[];
  }

  type CompanionPose = "welcome" | "review" | "celebrate" | "streak" | "freeze" | "winner" | "face";

  interface AnkiQuestAkiApi {
    image(pose?: string, extra?: string): string;
    mood(profile: CompanionProfile | null | undefined): CompanionPose;
    profile(value: CompanionProfile | null | undefined): string;
    decorate(root?: ParentNode & Partial<Pick<Element, "matches">>): void;
    setChoice(next: string): void;
    choice(): string;
  }

  interface AvatarEditorOptions {
    user: string;
    display?: string;
    token?: string;
  }

  interface AnkiQuestAvatarsApi {
    markup(user: string, display?: string): string;
    refresh(): Promise<void>;
    open(options: AvatarEditorOptions): void;
    isOpen(): boolean;
    close(): void;
  }

  interface Window {
    AnkiQuestI18n: I18n;
    AnkiQuestSite: AnkiQuestSiteApi;
    AnkiQuestAki: AnkiQuestAkiApi;
    AnkiQuestAvatars?: AnkiQuestAvatarsApi;
    ankiquestSession?: NativeSession | null;
    ankiquestLanguage?: string;
  }

  // Scripts refer to these without `window.`.
  var AnkiQuestI18n: I18n;
  var AnkiQuestSite: AnkiQuestSiteApi;
  var AnkiQuestAki: AnkiQuestAkiApi;
  var AnkiQuestAvatars: AnkiQuestAvatarsApi | undefined;

  interface WindowEventMap {
    "ankiquest:identity": CustomEvent<{user: string | null}>;
    "ankiquest:language": CustomEvent<{language: string}>;
    "ankiquest:companion": CustomEvent<{companion: string}>;
    "ankiquest:locked": Event;
    "ankiquest-auth": CustomEvent<unknown>;
  }
}
