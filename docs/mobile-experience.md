# Mobile Community and personal access

Community now puts current actions before historical reports:

- **Friends** shows invitations first, then current goals. Past goals are expandable. Creating a goal opens a separate dialog, with a default of three study days per person over the next seven days. Custom review targets, shared totals, durations and participant choices remain available.
- **Activity** keeps invitations, study updates and replies available after dismissing a phone notification. Members can mark individual items read, answer messages and open the relevant goal. Read status is separate from notification delivery. The server retains up to 90 days; the page offers 30- and 90-day windows and pagination.
- **History** groups the existing overview, winner calendar, trophies, comparisons, records and year review. No scoring or historical data is changed.
- **Reminders** retains existing preferences, quiet hours and frequency limits.

Personal-target goals require each person to reach their own target. Shared-total goals combine accepted members' contributions, which may be different sizes. Only study after a person accepts counts. Streak freezes do not count as study days, and goals do not change the Anki scheduler.

## Stable destinations

These routes also work with `?embed=1` before the hash:

| Destination | Route |
|---|---|
| Friends and invitations | `/community#challenges` |
| One goal | `/community#challenge-<id>` |
| Activity | `/community#activity` |
| Reminder preferences | `/community#reminders` |
| History overview | `/community#overview` |
| Calendar, trophies, records, year review | `/community#calendar`, `#trophies`, `#records`, `#year` |
| Player profile | `/week#<user>` |

The Activity view validates goal destinations before opening them. Missing or inaccessible goals produce a recovery message rather than exposing another player's data.

## Personal connection

The member session returned by `/auth/session` identifies exactly one player. After connecting once with a member token, Community, deck preferences and streak protection reuse the HttpOnly session cookie. The token is never placed in a URL, local storage or session storage. Cookie writes include the CSRF header and are checked by the server.

A shared website password remains read-only. Seeing a profile does not establish its owner's identity: attempting to change another player's preferences still requires that player's credentials. The signed-in member's **My profile** link stays associated with their account when viewing another profile.

Disconnect/Lock site invalidates the session and clears private page state. Back/forward restoration and returning to the page refresh the access check. Private dialogs and form credentials are cleared on page exit. Failed writes remain visibly unconfirmed; offline Activity refresh preserves already loaded entries with an error message.

Older servers without member identity can retain the existing in-memory token connection while the page/dialog remains open. If the Activity endpoint is absent, the page falls back to the old seven-day notifications list and explicitly states that persistent read status and longer history need a server update.

## Browser verification

`tests/mobile_web.test.ts` starts an isolated server with synthetic players and reviews, then checks real API-backed browser actions. It covers owner/read-only separation, account continuity, challenge ordering/creation/acceptance, no retroactive progress, replies and read persistence, profile settings, deep links, offline recovery, credential storage, logout and layout at 320/390/1440 pixels in light and dark themes.

Run with Node 22.13+ (built-in SQLite), Playwright, and a built AnkiQuest binary. Install a Playwright browser or set `PLAYWRIGHT_CHANNEL` for an installed supported browser.

```sh
ANKIQUEST_BIN=/path/to/ankiquest \
PLAYWRIGHT_MODULE=/path/to/node_modules/playwright \
node --experimental-strip-types tests/mobile_web.test.ts
```

`QA_DIR` selects the report/screenshot folder; otherwise a temporary folder is used. `--source-assets` intercepts only the static assets, serving freshly bundled `web/` code and pages, for UI iteration against a compiled backend. Final verification should run without that flag against the final compiled assets.
