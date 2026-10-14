# mobile community and personal access

community now puts current actions before historical reports:

- **friends** shows invitations first, then current goals. past goals are expandable. creating a goal opens a separate dialog, with a default of three study days per person over the next seven days. custom review targets, shared totals, durations and participant choices remain available.
- **activity** keeps invitations, study updates and replies available after dismissing a phone notification. members can mark individual items read, answer messages and open the relevant goal. read status is separate from notification delivery. the server retains up to 90 days; the page offers 30- and 90-day windows and pagination.
- **history** groups the existing overview, winner calendar, trophies, comparisons, records and year review. no scoring or historical data is changed.
- **reminders** retains existing preferences, quiet hours and frequency limits.

Personal-target goals require each person to reach their own target. Shared-total goals combine accepted members' contributions, which may be different sizes. only study after a person accepts counts. streak freezes do not count as study days, and goals do not change the Anki scheduler.

## stable destinations

these routes also work with `?embed=1` before the hash:

| Destination | Route |
|---|---|
| Friends and invitations | `/community#challenges` |
| One goal | `/community#challenge-<id>` |
| Activity | `/community#activity` |
| Reminder preferences | `/community#reminders` |
| History overview | `/community#overview` |
| Calendar, trophies, records, year review | `/community#calendar`, `#trophies`, `#records`, `#year` |
| Player profile | `/week#<user>` |

the activity view validates goal destinations before opening them. missing or inaccessible goals produce a recovery message rather than exposing another player's data.

## personal connection

the member session returned by `/auth/session` identifies exactly one player. after connecting once with a member token, Community, deck preferences and streak protection reuse the HttpOnly session cookie. the token is never placed in a URL, local storage or session storage. cookie writes include the CSRF header and are checked by the server.

a shared website password remains read-only. seeing a profile does not establish its owner's identity: attempting to change another player's preferences still requires that player's credentials. the signed-in member's **My profile** link stays associated with their account when viewing another profile.

disconnect/Lock site invalidates the session and clears private page state. back/forward restoration and returning to the page refresh the access check. private dialogs and form credentials are cleared on page exit. failed writes remain visibly unconfirmed; offline Activity refresh preserves already loaded entries with an error message.

older servers without member identity can retain the existing in-memory token connection while the page/dialog remains open. if the Activity endpoint is absent, the page falls back to the old seven-day notifications list and explicitly states that persistent read status and longer history need a server update.

## browser verification

`tests/mobile_web.test.ts` starts an isolated server with synthetic players and reviews, then checks real API-backed browser actions. it covers owner/read-only separation, account continuity, challenge ordering/creation/acceptance, no retroactive progress, replies and read persistence, profile settings, deep links, offline recovery, credential storage, logout and layout at 320/390/1440 pixels in light and dark themes.

run with Node 22.13+ (built-in SQLite), Playwright, and a built AnkiQuest binary. install a Playwright browser or set `PLAYWRIGHT_CHANNEL` for an installed supported browser.

```sh
ANKIQUEST_BIN=/path/to/ankiquest \
PLAYWRIGHT_MODULE=/path/to/node_modules/playwright \
node --experimental-strip-types tests/mobile_web.test.ts
```

`QA_DIR` selects the report/screenshot folder; otherwise a temporary folder is used. `--source-assets` intercepts only the static assets, serving freshly bundled `web/` code and pages, for UI iteration against a compiled backend. final verification should run without that flag against the final compiled assets.
