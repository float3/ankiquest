// What tests reach inside browser pages: the globals the web scripts share, and the
// top-level renderers of page scripts loaded with `prefix` from ./web.ts.
/// <reference path="../../web/globals.d.ts" />

export {};

declare global {
  // web/pages/index.ts
  var app: HTMLElement;
  function board(rows: object[], me: string): string;
  function view(profile: object, rows: object[], canEditAvatar?: boolean): string;
  // web/pages/community.ts
  var state: {data: unknown};
  function avatar(user: string, display?: string): string;
  function renderOverview(): void;
  function renderYear(): void;
}
