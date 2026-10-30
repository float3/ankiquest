# running AnkiQuest

XP, levels, streaks, daily quests, achievements and a leaderboard for Anki. clients send review log rows (card id, timestamp, previous and new interval, time taken, review type), never card content. deck names and daily counts are only sent by players who use deck completion notifications.

XP never depends on which answer button was pressed, so there is no incentive to grade dishonestly.

`review_weighting` (Nix: `reviewWeighting`) chooses how reviews are weighted. the default, `"flat"`, pays every answer its full rate. `"diminishing"` makes each further answer of the same card on the same day worth half as much, so a card you keep failing cannot out-earn one you learn, and pays learning steps 9 XP instead of 6. XP is recalculated from review history, so changing it applies to past reviews too.

`streak_minutes` (Nix: `streakMinutes`, default 0) is the review time an Anki day needs to count toward the streak. 0 lets a single review count. review time is the revlog's time per answer, which Anki caps (60 s by default). the server records each change of the effective value with the time it saw it, in the `streak_minutes` table, and a day follows the value in force when it began: days before the first change keep the any-review rule, and a change made during a day applies from each player's next Anki day, so existing streaks are never rewritten. a day under the minimum counts as missed, so a freeze can cover it. profiles carry `today.minutes` and `today.streak_minutes`, standings `today_minutes` and `streak_minutes`, and the streak warning and reminders say how many minutes are done, e.g. "6 of 10 minutes today".

## administer

`ankiquest [config.json] admin <command>` (on NixOS: `sudo ankiquest-admin <command>`) works on the state of a running server:

- `settings` shows the settings in effect and where each comes from.
- `set review_weighting diminishing`, `set log_retention_days 365` or `set streak_minutes 10` overrides a setting; `reset <setting>` returns to the configured value. the service applies changes within a minute, without a restart.
- `logs [--since 30m|12h|7d|all] [--level info|error] [--user <player>] [--limit <n>]` prints the server log. errors and setting changes are kept in the state database for `log_retention_days` (default 90; 0 keeps them forever), and still go to stderr for the journal.
- `sql "<query>"` runs a read-only query against the state database.
- `remove-account <user>` deletes a self-service account and everything about it, as if they had deleted it themselves; the service carries it out within a minute. players from the configuration are removed there instead.

## dev console

`"devs": ["hill"]` (Nix: `devs`) lets those players open `/dev` on the website, which is linked in the header only for them. it shows player, activity, review, sign-up, group and retention totals, the last week's server errors, and sends announcements to everyone or to chosen groups. announcements arrive as ordinary inbox messages, are pushed like any other, and are recorded in the server log. names in `devs` cannot be taken by a new sign-up, so create a self-service account before listing it. the endpoints are `GET /api/dev/<user>/stats`, `/groups`, `/errors` and `GET`/`POST /api/dev/<user>/announcements` with `{"title", "body", "groups"?: [id, ...]}`; other players get 403.

## run

requires rust 1.88 or later with the `wasm32-unknown-unknown` target, and esbuild (`npm ci` installs it; build.rs also finds it on `PATH` or in `ESBUILD`).

```sh
cargo run -- ankiquest.json
```

```json
{
  "addr": "127.0.0.1:8097",
  "state_dir": "state",
  "ntfy": "https://ntfy.sh",
  "public_url": "https://anki.example.com",
  "week_timezone": "Europe/Berlin",
  "week_rollover_hour": 4,
  "review_weighting": "flat",
  "log_retention_days": 90,
  "streak_minutes": 0,
  "users": {
    "hill": { "display": "hill", "ntfy_topic": "some-secret-topic", "token_file": "hill.token" }
  }
}
```

open `/#<user>` for a profile, `/` for the leaderboard. `/hour`, `/day`, `/week`, `/month`, `/year` and `/all` show the same board for another period, as does `GET /api/leaderboard?period=<name>`; each standing carries `xp` for the requested period and `periods` with all of them. the hour is the last 60 minutes and counts review XP only, the day is each player's own Anki day, and month and year follow the calendar in `week_timezone`. the leaderboard week runs Monday to Sunday in `week_timezone` and turns over at `week_rollover_hour` for everyone at once; each Anki day counts towards the week it started in. streaks, quests and "today" still follow each player's own Anki day.

`/records` and `GET /api/records` name whoever has had the best hour, day, week, month and year here, with the XP and the review count of each and the two who came closest, along with the longest streak and the most days studied; a profile shows the same as personal bests. the record hour is any 60 minutes, not a clock hour.

## clients

the Android app and the desktop add-on show what the server writes, so they word things the same way. upload and preview responses carry `feedback` (`headlines` and an XP `status` line). profiles carry `day_ends_at` and a `streak_warning` while the streak is at risk. `POST /api/rank/<user>` with `{"previous": [...]}` returns the weekly `order` and a `change` notice for the player. behavior that has to stay in each client, such as the study day, the review row format and undo reconciliation, is pinned by `tests/fixtures/clients.json`, which the server, the add-on and the app all test against.

## community and reminders

`/today` is the member's home page on the website. it puts daily quests, streak and XP progress, friend invitations, recent updates, and a direct study link together. it also shows when study data was last updated, so an old sync is visible. `/history` has a selectable study-day calendar with review counts, time, XP, new cards, streaks, and freeze days; `/settings` brings reminder, nudge, freeze, and deck-notification controls together. sign in with your own player token to use these personal pages. a shared site password can unlock the public areas, but cannot view another member's study history or change their settings.

the latest-session recap on Today groups uploaded reviews from the same Anki day when gaps between their timestamps are under five minutes. it is an estimate from review logs, not a separate live study timer. `GET /api/study/<user>?year=YYYY` supplies the selected year's study days, available years, last review/upload timestamps, and latest estimated session to that member only.

open `/community` for the winners calendar, weekly and monthly results, trophy cabinet, improvement and consistency awards, head-to-head history, comeback recognition, records, and year in review. coverage starts with the earliest retained reviews and is labeled **available history**. historical reconstructions and provisional results are identified; results become permanent after a 24-hour late-sync window. existing review XP weights are unchanged.

choose daily, urgent streak, freeze-used, freeze-refill, milestone, weekly closing, and weekly recap reminders, or invite friends to private challenges and shared goals. in AnkiDroid, **Settings → ankiquest → Community reminders** opens this page using your saved account. in a standalone browser, connect with your own upload token. all seven reminder types are off by default, with personal quiet hours and a daily limit. bearer tokens stay in page memory only. supported servers issue a secure account session that works across pages until you disconnect; older servers keep the connection only for the current page. see [the community guide](community.md) for details and API endpoints.

the old server-wide `remind_hour` / NixOS `remindHour` setting is deprecated; enable personal reminders in `/community` instead. existing device-only AnkiDroid and desktop add-on alarms are controlled separately in each client's settings.

## conquered cards

a card that lapsed at least four times and then earns an interval of 21 days or more is *conquered*; from eight lapses, Anki's default leech threshold, it is a *tamed leech*. a lapse is a review answered into relearning: a negative new interval, or, from clients that do not send the new interval, a relearning step right after a review. without the new interval a card counts once a review finds it after a mature interval and no relearning step follows it within a day. lapses on decks without relearning steps cannot be told from a pass and go uncounted. only retained history counts, so a card's earlier lapses may be missing.

conquests from the last two days are celebrated like achievements: upload feedback carries a `🏆` headline, and the player's inbox and push get a celebration. profiles list the latest twelve in `conquests` (`at`, `lapses`, `answers`, `since`, `leech`) and count them all in `lifetime.conquered`. `/conquered/<user>/<at>` shows one as a big companion card with a shareable picture and link, visible to whoever may see the player's profile. its owner can tell their friends once, which puts it in their friends' activity. `GET /api/conquests/<user>/<at>` returns the card's numbers; `POST /api/conquests/<user>/<at>/tell` needs the owner's token or session. no card id or content is shared.

the owner may also say what the card was. `POST /api/conquests/<user>/<at>/label` with `{"label":"das Eichhörnchen"}` stores one line of plain text of up to 120 characters, shown on the card, in its picture and in the message friends get when told; `DELETE` takes it back. both need the owner's token or session. nothing about the card is stored until the owner saves a label: the desktop add-on offers the card's first field as a suggestion in the page's URL fragment (`#suggest=…`), which browsers never send to the server, and the page drops it from the address once read. friends already told keep the message they received. labels are part of the data download and are deleted with the account.

Re-uploading reviews with their new intervals (**Upload everything again** in the add-on's settings) fills in intervals that older uploads left out.

## deck copies with friends

in AnkiDroid's today screen, choose **Share a deck**, select a deck and up to 20 friends, and send them an independent copy. the app exports cards, deck settings, and media as an Anki package without scheduling or review history. each selected friend sees the offer on Today and chooses whether to import it into their own collection; importing does not keep later edits in sync. offers expire after 30 days and recipients can remove them sooner. packages are limited to 25 MiB, and each sender can have up to ten active offers. for larger decks, use Anki's usual export workflow.

only the sender can create an offer, and only a selected recipient can list, download, or dismiss it. the server stores package bytes in its existing state database. `GET /api/deck-copies/<user>` lists offers and eligible friends; `POST /api/deck-copies/<user>?deck=<name>&recipients=<JSON array>` accepts the raw `.apkg` body; `GET /api/deck-copies/<user>/<id>` downloads it; and `DELETE /api/deck-copies/<user>/<id>` removes a recipient's offer. these endpoints require that member's own token or session, with the usual CSRF protection for browser writes.

## profile pictures

profile pictures are optional. open your profile and choose **Profile picture**, or connect your account in Community and use the same control there. choose a JPEG or PNG, preview its center square, then save with your own AnkiQuest token. **remove picture** restores your initials. photos appear on the leaderboard and community views; compatible Android clients also display them in widgets and offer a picture picker in AnkiQuest settings.

the website accepts files up to 20 MB and reduces them before upload. the server accepts PNG/JPEG bodies up to 2 MiB and 4096 × 4096 pixels, stores a normalized 256 × 256 PNG in its existing database, and strips original file metadata. pictures have the same visibility as the community’s player profiles: private sites require a browser session or a member’s token to read pictures and their revision list. uploading and removing always require the owner’s bearer token; the shared site password and browser session do not grant permission to change someone’s picture. tokens are never stored by the picture editor.

- `GET /api/avatars` returns an object mapping usernames with pictures to revision strings.
- `GET /api/avatar/<user>?v=<revision>` returns the current PNG or 404, with an ETag for conditional requests. like other site data, responses use `Cache-Control: no-store` so photos cannot remain readable through a browser cache after logout.
- `POST /api/avatar/<user>` accepts the raw picture body with `Authorization: Bearer <token>` and returns `{"revision":"1"}`.
- `DELETE /api/avatar/<user>` removes the picture and returns 204.

browser regression checks are in `tests/avatar_layout.test.ts`, `tests/avatar_index_layout.test.ts`, and `tests/avatar_photos.test.ts`. run them with Node and Playwright installed; `PLAYWRIGHT_MODULE` and `PLAYWRIGHT_CHANNEL` optionally select an existing installation/browser. the tests use synthetic users and mocked requests.

## private website access

the leaderboard, profiles, records, and Community share the same navigation, colors, cards, and controls, including light and dark themes. enable private access to put those pages and their data behind a sign-in screen:

```json
{
  "private_site": true,
  "site_password_file": "/run/secrets/ankiquest-site-password",
  "public_url": "https://anki.example.com"
}
```

add these fields to your existing configuration. store the shared member password in the named file, readable only by the service and server administrator; do not commit the password or put it in the Nix store. existing nonempty player upload tokens also unlock the website. the shared password is optional when at least one player has a token. private mode refuses to start without a usable password or token. existing installations remain public until `private_site` is enabled.

for NixOS, add these options to the existing service definition:

```nix
services.ankiquest = {
  privateSite = true;
  sitePasswordFile = "/etc/nixos/secrets/ankiquest-site-password";
};
```

the module loads the password through a systemd credential. use HTTPS and set `public_url` to the actual HTTPS address (the NixOS `domain` option does this). browser sign-in creates an opaque, HttpOnly, SameSite cookie that lasts seven days. select **Lock site** to end the session. restarting the service clears browser sessions; restart after changing the password or token files to load the new credentials. passwords and tokens are never placed in website URLs or browser storage.

the shared password and browser session grant access to community pages and read data. managing profile pictures, reminders, challenges, deck notifications, freezes, or an inbox still requires that player's own upload token; the shared password cannot impersonate members.

update the Android app before enabling private mode: authenticated reads and automatic embedded-page sign-in are required. the desktop add-on already sends its configured token on API requests; when opening the website in an external browser, sign in there once. configure each app with its player's token, not the shared website password. Tokenless clients cannot read a private server. see [the access guide](private-site.md) for API behavior and rollout checks.

## getting reviews in

both clients upload new review rows after each answer and show XP feedback while reviewing. sync itself can stay on AnkiWeb.

- AnkiDroid: install the [fork](https://github.com/float3/Anki-Android/tree/ankiquest) and fill in Settings → ankiquest.
- desktop: zip the contents of `addon/` into `ankiquest.ankiaddon`, open it with Anki, then fill in **Tools → ankiquest settings…**. the leaderboard appears under the deck list, with the period links it shares with the website.

`POST /api/reviews/<user>` with `Authorization: Bearer <token>` and

```json
{
  "reviews": [{ "id": 0, "cid": 0, "last_ivl": 0, "ivl": 0, "time_ms": 0, "kind": 0 }],
  "clock": { "offset_west_min": -120, "rollover_hour": 4 },
  "silent": false
}
```

stores the rows and returns the profile. `POST /api/preview/<user>` takes the same `reviews` without storing anything.

the `clock` is the one a player keeps now. each upload that reports a different `offset_west_min` or `rollover_hour` than the last records an entry in `clock_history`, dated from that upload, and every review is read against the clock that was in force when it happened. moving time zone therefore cannot relabel days already studied: a player who studied every morning in Berlin and then moved to California keeps those Berlin mornings on the Berlin days, instead of having them slide back one and leaving an empty day behind. travelling west gains hours, and the day that repeats counts once; travelling far enough east to step the numbering over a day entirely leaves a day nobody could have studied in, and that day reads as protected rather than missed.

a move made before a server recorded any history cannot be recovered from the review data, since Anki stores only a timestamp. such a player gets one entry holding the clock the server knows about, reaching back over everything they have uploaded. declaring the earlier clock takes two statements: point the existing entry at the instant of the move, and insert the clock kept before it.

```sql
update clock_history set effective_from_ms = <move in epoch ms> where user = '<player>' and effective_from_ms = 0;
insert into clock_history (user, effective_from_ms, offset_west_min, rollover_hour) values ('<player>', 0, -120, 4);
```

pick an instant after their last study session on the old clock and before the first on the new one. the earliest entry always reaches back to the beginning of time, whatever instant it carries.

alternatively set `sync_base` to the `SYNC_BASE` of a self-hosted Anki sync server: every folder in it with a `collection.anki2` becomes a player, and collections are copied before reading and never written.

## streak freezes

streak freezes are on by default and start at zero. open **Settings → ankiquest → Streak protection** in AnkiDroid, or choose **Manage streak freezes** on your dashboard profile, to turn protection off or back on. the updated AnkiDroid dashboard reuses your saved token for your own player; its toolbar's **Settings** action opens all AnkiQuest preferences. in a standalone browser, connect once to use streak, deck, and Community settings. supported servers reuse your account session across pages; older servers reuse the token only within the current page. complete all three daily quests while enabled to earn one freeze per Anki day, up to three stored. this replaces the automatic freeze awarded every seven study days. on an existing server, previously protected days and their streak/XP history are preserved; unused automatic freezes are cleared on the upgrade's Anki day. Re-enabling freezes does not award any for quests completed while protection was off, including earlier today; completing extra reviews or toggling the setting cannot claim that day's reward again. completing the quests with a full inventory does not bank a fourth freeze for later.

when upgrading from the opt-in default, protection turns on for players without a saved choice. recorded opt-outs remain off, and saved freezes and past protection are preserved. the server saves the rollout time once; earlier quest completions are not retroactively rewarded, and restarting does not move that boundary.

when an Anki day ends with no reviews, one available freeze automatically protects an existing streak. a protected day preserves the streak count without adding a study day. consecutive missed days each need one freeze; once there is none available, the next missed day resets the streak. turning freezes off pauses earning and spending, keeps stored freezes, and leaves previously protected days intact. enabling them again cannot repair days missed while they were off. day boundaries follow the player's Anki timezone and rollover, not midnight on the server.

`GET /api/streak-freezes/<user>` and `POST` with `{"enabled":true}` or `{"enabled":false}` require that player's bearer token and return `{"enabled":true,"freezes":0,"capacity":3}`. the server calculates the balance from reviews and the saved preference timeline; clients cannot set it. the public profile includes `freezes_enabled`, `stored_freezes`, and `freeze_earned_today`. its existing `freezes` field is the available balance (zero while disabled), so older clients do not mistake paused stock for active protection. review previews can show a projected reward but do not save it. as with XP and quests, importing or deleting review history recalculates the result.

## deck completion notifications

notifications are off by default for every deck. to set them up, tick the decks you want to share and the people to notify: in AnkiDroid under **Settings → ankiquest → Deck completion notifications**, on desktop under **Tools → ankiquest deck notifications…**, or on the dashboard through **Manage deck notifications** with your upload token. ticking a deck ticks its subdecks.

after you review at least one card in a deck and finish its scheduled work for the day, selected people receive a message such as “cerro has finished their Spanish studies for today.” A parent deck includes its subdecks. daily limits are respected, and learning cards due later that day still count as unfinished work. each deck is announced at most once per Anki day, using your Anki day rollover, even across retries or a server restart. turning sharing on after finishing a deck does not send a retrospective announcement.

when a deck and its subdeck complete together with the same review total, each recipient hears only about the most specific deck they follow. parent notifications remain when they cover additional reviews or different recipients. suppressed parent completions are still recorded for the day, so later uploads cannot announce them again.

recipients receive announcements through the updated AnkiDroid client's background notification checks (roughly every 15 minutes, subject to Android's background limits), on desktop through the add-on's own check every five minutes, and through their configured ntfy topic when available (the server checks every 20 seconds). each announcement can be answered once, with a cheer or your own words: from the Android notification itself, or from **Tools → ankiquest inbox…** on desktop. `POST /api/reply/<user>` with `{"notification": 1, "message": "Good job!"}` delivers the answer, which can be answered in turn. the upload response lists what it just announced, so the client that finished a deck can say who was told. deck names and recipient preferences are private to the authenticated player; only selected recipients receive the completion message. the dashboard reuses the current server account session, or keeps a token only in page memory on older servers, shared between settings dialogs for that player. it never stores the token in URLs, local storage or session storage. AnkiDroid supplies its saved token only to its configured dashboard; browsing another player does not reuse that credential.

to control announcements you receive, open **Notifications i receive** on the leaderboard or your dashboard profile. your signed-in account is reused; otherwise connect with your player name and AnkiQuest token. select a person under **Unsubscribe from specific people** and save to remove yourself from all of their deck-sharing lists. they cannot select you again, including through an older client or an already-open settings window. only you can subscribe again by clearing their name and saving. previous deck selections are restored unless the sender has since edited or deleted them; subscribing never adds you to other private decks. these controls work on the website and the Android dashboard and apply to desktop, Android, and ntfy delivery without a native app update.

receiving stays enabled by default. Unsubscribing cancels that sender's pending completion pushes, including retries, and removes matching completion alerts from the server inbox and Activity. subscribing again does not replay old alerts; a deck already finished when reporting resumes is treated as a baseline. turn off **Receive deck completion notifications** to pause every sender without changing your subscriptions. sender subscriptions remain editable while all alerts are off. previous per-person mutes are preselected in the new editor; saving converts those selections into unsubscriptions. already delivered device notifications cannot be recalled. streak reminders, nudges, messages, replies, and your own outgoing sharing remain unaffected.

players can opt into nudges through **Manage deck notifications** on the dashboard. when a place on the weekly board, their best day ever or the next level is within 150 XP, they hear about it once a day each, in reviews as well as XP. nudges only arrive between 9:00 and 22:00 of a player's own day, and only after they have already reviewed something, so they never tell anyone to start studying.

messages can also be written by hand on the server: `sudo ankiquest-message --from cerro aldanita "you are doing great, keep going"`, or `ankiquest <config> message <player> <text>` without the NixOS module. they arrive like any other notification, and with `--from` the recipient can answer them.

an ntfy push is marked delivered only after a successful HTTP response. urgent streak warnings and existing notification types request high priority; other community reminders use normal priority, subject to the phone's notification settings. failed pushes retry after 20 seconds, backing off to at most 15 minutes; missing ntfy configuration leaves inbox messages pending. the queue and inbox retain messages for seven days. retries take turns between recipients and limit network work to 20 seconds per server check. unlocks retain their existing ntfy-only delivery. community reminders are available through both the inbox and ntfy; their relevance and quiet hours are rechecked before delivery. upgrading retires pending legacy streak warnings. a server restart or a lost HTTP response can occasionally cause a duplicate push, but retries keep the same inbox notification.

the server and the client used to study must both be updated. clients only report progress for decks you share, so players who never enable a deck send no deck data. sending the deck list again replaces the stored one, which removes deleted decks.

AnkiQuest supports English and Spanish. Updated Android and desktop clients pass the language selected in Anki, including when it differs from the phone's language. standalone browsers use their preferred language. other languages currently fall back to English. the catalogs are in `static/translations-*.json` (Spanish: `static/translations-es.json`); Android strings are in `values-es/ankiquest*.xml`. names, deck names, goal titles and written messages are kept as authored.

authenticated profile, upload and inbox requests remember the owner's `Accept-Language` for later server pushes. public profile views do not change it. `GET` and `POST /api/language/<user>` also require the owner's bearer token or personal session; cookie writes require CSRF protection. a POST accepts `{"language":"es-ES"}`. stored notification text remains unchanged so each recipient can receive their own translation.

clients may include a `decks` array in the review upload. each entry has `id` (a string), `name`, `remaining`, `reviewed_today`, and `day` (the local Anki day number, days since the Unix epoch after applying timezone and rollover). omit this field when a reliable snapshot is unavailable. initial silent uploads populate the deck list without announcing completions. set `"catalog": true` when `decks` is the full deck list; decks missing from it are removed.

`GET /api/decks/<user>` with the user's bearer token returns private deck preferences and available recipients. `POST` to the same endpoint accepts `{"decks":[{"id":"123","enabled":true,"recipients":["hill"]}]}`. `GET /api/notifications/<user>` with the recipient's bearer token returns their recent completion announcements, with `id`, `title`, `body`, `day`, and `created_at` (Unix seconds). these endpoints never expose another player's deck settings or notification inbox without that player's token.

`GET /api/deck-subscriptions/<user>` requires the recipient's token or personal member session and returns `{"enabled":true,"muted_senders":[],"unsubscribed_senders":[],"sharing_senders":["hill"],"senders":[{"user":"hill","display":"Hill"}]}`. `sharing_senders` lists owners of enabled decks that currently select this recipient, even if the recipient has paused all alerts. the editor shows those senders plus saved unsubscriptions and legacy mutes; `senders` supplies identity labels and validation choices. `POST` accepts `{"enabled":true,"unsubscribed_senders":["hill"]}` and returns the updated settings. at most 100 distinct sender IDs can be selected. IDs must be in the available senders; unknown fields, duplicates, invalid names, and unsubscribing from yourself are rejected. the response exposes identities, never another person's deck names. Personal-session writes require the site's CSRF header. saving subscriptions also clears legacy mutes; the website first includes those mutes in the unsubscribe selection.

the older `GET`/`POST /api/notification-preferences/<user>` API remains compatible with `{"enabled":false,"muted_senders":["hill"]}`. it can still change alert filters, but cannot undo an unsubscribe. older outgoing settings cannot restore a recipient who has unsubscribed either.

## NixOS

```nix
inputs.ankiquest.url = "github:float3/ankiquest";

imports = [inputs.ankiquest.nixosModules.default];

services.ankiquest = {
  enable = true;
  domain = "anki.example.com";
  weekTimezone = "Europe/Berlin";
  ntfy = "https://ntfy.sh";
  users.hill = {
    tokenFile = "/etc/nixos/secrets/ankiquest-hill";
    ntfyTopic = "some-secret-topic";
  };
};
```

the module runs the server in a tight sandbox. systemd holds the port and
passes it in, and the service may not connect to loopback or private
addresses, only out to the internet for ntfy. so `ntfy` must be a public
server. outside systemd, `addr` is bound as usual.

## development

the pages' scripts are TypeScript in `web/`; build.rs bundles them with esbuild and the server embeds the result, with `web/pages/*.ts` inlined where a page has `<script data-entry="pages/…"></script>`. translation lives in `crates/i18n`, which the server links directly and the pages load as `/i18n.wasm` (built from `crates/i18n-wasm`), so both look up the same catalogs the same way.

```sh
rustup target add wasm32-unknown-unknown
npm ci
npm run typecheck
cargo test --workspace
npm run test:web
```
