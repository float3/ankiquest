# ankiquest

XP, levels, streaks, daily quests, achievements and a leaderboard for Anki. Clients send review log rows (card id, timestamp, previous interval, time taken, review type), never card content. Deck names and daily counts are only sent by players who use deck completion notifications.

XP never depends on which answer button was pressed, so there is no incentive to grade dishonestly.

## Run

Requires Rust 1.88 or later.

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
  "users": {
    "hill": { "display": "hill", "ntfy_topic": "some-secret-topic", "token_file": "hill.token" }
  }
}
```

Open `/#<user>` for a profile, `/` for the leaderboard. `/hour`, `/day`, `/week`, `/month`, `/year` and `/all` show the same board for another period, as does `GET /api/leaderboard?period=<name>`; each standing carries `xp` for the requested period and `periods` with all of them. The hour is the last 60 minutes and counts review XP only, the day is each player's own Anki day, and month and year follow the calendar in `week_timezone`. The leaderboard week runs Monday to Sunday in `week_timezone` and turns over at `week_rollover_hour` for everyone at once; each Anki day counts towards the week it started in. Streaks, quests and "today" still follow each player's own Anki day.

`/records` and `GET /api/records` name whoever has had the best hour, day, week, month and year here, with the XP and the review count of each and the two who came closest, along with the longest streak and the most days studied; a profile shows the same as personal bests. The record hour is any 60 minutes, not a clock hour.

## Community and reminders

Open `/community` for the winners calendar, weekly and monthly results, trophy cabinet, improvement and consistency awards, head-to-head history, comeback recognition, records, and year in review. Coverage starts with the earliest retained reviews and is labeled **available history**. Historical reconstructions and provisional results are identified; results become permanent after a 24-hour late-sync window. Existing review XP weights are unchanged.

Choose daily, urgent streak, freeze-used, freeze-refill, milestone, weekly closing, and weekly recap reminders, or invite friends to private challenges and shared goals. In AnkiDroid, **Settings → ankiquest → Community reminders** opens this page using your saved account. In a standalone browser, connect with your own upload token. All seven reminder types are off by default, with personal quiet hours and a daily limit. Bearer tokens stay in page memory only. Supported servers issue a secure account session that works across pages until you disconnect; older servers keep the connection only for the current page. See [the community guide](docs/community.md) for details and API endpoints.

The old server-wide `remind_hour` / NixOS `remindHour` setting is deprecated; enable personal reminders in `/community` instead. Existing device-only AnkiDroid and desktop add-on alarms are controlled separately in each client's settings.

## Profile pictures

Profile pictures are optional. Open your profile and choose **Profile picture**, or connect your account in Community and use the same control there. Choose a JPEG or PNG, preview its center square, then save with your own AnkiQuest token. **Remove picture** restores your initials. Photos appear on the leaderboard and community views; compatible Android clients also display them in widgets and offer a picture picker in AnkiQuest settings.

The website accepts files up to 20 MB and reduces them before upload. The server accepts PNG/JPEG bodies up to 2 MiB and 4096 × 4096 pixels, stores a normalized 256 × 256 PNG in its existing database, and strips original file metadata. Pictures have the same visibility as the community’s player profiles: private sites require a browser session or a member’s token to read pictures and their revision list. Uploading and removing always require the owner’s bearer token; the shared site password and browser session do not grant permission to change someone’s picture. Tokens are never stored by the picture editor.

- `GET /api/avatars` returns an object mapping usernames with pictures to revision strings.
- `GET /api/avatar/<user>?v=<revision>` returns the current PNG or 404, with an ETag for conditional requests. Like other site data, responses use `Cache-Control: no-store` so photos cannot remain readable through a browser cache after logout.
- `POST /api/avatar/<user>` accepts the raw picture body with `Authorization: Bearer <token>` and returns `{"revision":"1"}`.
- `DELETE /api/avatar/<user>` removes the picture and returns 204.

Browser regression checks are in `tests/avatar_layout.cjs`, `tests/avatar_index_layout.cjs`, and `tests/avatar_photos.cjs`. Run them with Node and Playwright installed; `PLAYWRIGHT_MODULE` and `ANKIQUEST_BROWSER_CHANNEL` optionally select an existing installation/browser. The tests use synthetic users and mocked requests.

## Private website access

The leaderboard, profiles, records, and Community share the same navigation, colors, cards, and controls, including light and dark themes. Enable private access to put those pages and their data behind a sign-in screen:

```json
{
  "private_site": true,
  "site_password_file": "/run/secrets/ankiquest-site-password",
  "public_url": "https://anki.example.com"
}
```

Add these fields to your existing configuration. Store the shared member password in the named file, readable only by the service and server administrator; do not commit the password or put it in the Nix store. Existing nonempty player upload tokens also unlock the website. The shared password is optional when at least one player has a token. Private mode refuses to start without a usable password or token. Existing installations remain public until `private_site` is enabled.

For NixOS, add these options to the existing service definition:

```nix
services.ankiquest = {
  privateSite = true;
  sitePasswordFile = "/etc/nixos/secrets/ankiquest-site-password";
};
```

The module loads the password through a systemd credential. Use HTTPS and set `public_url` to the actual HTTPS address (the NixOS `domain` option does this). Browser sign-in creates an opaque, HttpOnly, SameSite cookie that lasts seven days. Select **Lock site** to end the session. Restarting the service clears browser sessions; restart after changing the password or token files to load the new credentials. Passwords and tokens are never placed in website URLs or browser storage.

The shared password and browser session grant access to community pages and read data. Managing profile pictures, reminders, challenges, deck notifications, freezes, or an inbox still requires that player's own upload token; the shared password cannot impersonate members.

Update the Android app before enabling private mode: authenticated reads and automatic embedded-page sign-in are required. The desktop add-on already sends its configured token on API requests; when opening the website in an external browser, sign in there once. Configure each app with its player's token, not the shared website password. Tokenless clients cannot read a private server. See [the access guide](docs/private-site.md) for API behavior and rollout checks.

## Getting reviews in

Both clients upload new review rows after each answer and show XP feedback while reviewing. Sync itself can stay on AnkiWeb.

- AnkiDroid: install the [fork](https://github.com/float3/Anki-Android/tree/ankiquest) and fill in Settings → ankiquest.
- Desktop: zip the contents of `addon/` into `ankiquest.ankiaddon`, open it with Anki, then fill in **Tools → ankiquest settings…**. The leaderboard appears under the deck list, with the period links it shares with the website.

`POST /api/reviews/<user>` with `Authorization: Bearer <token>` and

```json
{
  "reviews": [{ "id": 0, "cid": 0, "last_ivl": 0, "time_ms": 0, "kind": 0 }],
  "clock": { "offset_west_min": -120, "rollover_hour": 4 },
  "silent": false
}
```

stores the rows and returns the profile. `POST /api/preview/<user>` takes the same `reviews` without storing anything.

Alternatively set `sync_base` to the `SYNC_BASE` of a self-hosted Anki sync server: every folder in it with a `collection.anki2` becomes a player, and collections are copied before reading and never written.

## Streak freezes

Streak freezes are off by default and start at zero. Open **Settings → ankiquest → Streak protection** in AnkiDroid, or choose **Manage streak freezes** on your dashboard profile, to opt in. The updated AnkiDroid dashboard reuses your saved token for your own player; its toolbar's **Settings** action opens all AnkiQuest preferences. In a standalone browser, connect once to use streak, deck, and Community settings. Supported servers reuse your account session across pages; older servers reuse the token only within the current page. Complete all three daily quests while enabled to earn one freeze per Anki day, up to three stored. This replaces the automatic freeze awarded every seven study days. On an existing server, previously protected days and their streak/XP history are preserved; unused automatic freezes are cleared on the upgrade's Anki day. Enabling freezes does not award any for earlier quest completions, including earlier today; completing extra reviews or toggling the setting cannot claim that day's reward again. Completing the quests with a full inventory does not bank a fourth freeze for later.

When an Anki day ends with no reviews, one available freeze automatically protects an existing streak. A protected day preserves the streak count without adding a study day. Consecutive missed days each need one freeze; once there is none available, the next missed day resets the streak. Turning freezes off pauses earning and spending, keeps stored freezes, and leaves previously protected days intact. Enabling them again cannot repair days missed while they were off. Day boundaries follow the player's Anki timezone and rollover, not midnight on the server.

`GET /api/streak-freezes/<user>` and `POST` with `{"enabled":true}` or `{"enabled":false}` require that player's bearer token and return `{"enabled":true,"freezes":0,"capacity":3}`. The server calculates the balance from reviews and the saved preference timeline; clients cannot set it. The public profile includes `freezes_enabled`, `stored_freezes`, and `freeze_earned_today`. Its existing `freezes` field is the available balance (zero while disabled), so older clients do not mistake paused stock for active protection. Review previews can show a projected reward but do not save it. As with XP and quests, importing or deleting review history recalculates the result.

## Deck completion notifications

Notifications are off by default for every deck. To set them up, tick the decks you want to share and the people to notify: in AnkiDroid under **Settings → ankiquest → Deck completion notifications**, on desktop under **Tools → ankiquest deck notifications…**, or on the dashboard through **Manage deck notifications** with your upload token. Ticking a deck ticks its subdecks.

After you review at least one card in a deck and finish its scheduled work for the day, selected people receive a message such as “cerro has finished their Spanish studies for today.” A parent deck includes its subdecks. Daily limits are respected, and learning cards due later that day still count as unfinished work. Each deck is announced at most once per Anki day, using your Anki day rollover, even across retries or a server restart. Turning sharing on after finishing a deck does not send a retrospective announcement.

When a deck and its subdeck complete together with the same review total, each recipient hears only about the most specific deck they follow. Parent notifications remain when they cover additional reviews or different recipients. Suppressed parent completions are still recorded for the day, so later uploads cannot announce them again.

Recipients receive announcements through the updated AnkiDroid client's background notification checks (roughly every 15 minutes, subject to Android's background limits), on desktop through the add-on's own check every five minutes, and through their configured ntfy topic when available (the server checks every 20 seconds). Each announcement can be answered once, with a cheer or your own words: from the Android notification itself, or from **Tools → ankiquest inbox…** on desktop. `POST /api/reply/<user>` with `{"notification": 1, "message": "Good job!"}` delivers the answer, which can be answered in turn. The upload response lists what it just announced, so the client that finished a deck can say who was told. Deck names and recipient preferences are private to the authenticated player; only selected recipients receive the completion message. The dashboard reuses the current server account session, or keeps a token only in page memory on older servers, shared between settings dialogs for that player. It never stores the token in URLs, local storage or session storage. AnkiDroid supplies its saved token only to its configured dashboard; browsing another player does not reuse that credential.

To control announcements you receive, open **Notifications I receive** on the leaderboard or your dashboard profile and enter your player name and AnkiQuest token. Turn off **Receive deck completion notifications** to stop all deck-completion announcements, or mute individual people. These preferences belong to the recipient and apply to AnkiDroid, the desktop add-on, and ntfy. They work before you have uploaded any study history and do not require changes to your own shared decks.

Receiving stays enabled by default for compatibility with existing settings. Muting a person or turning receiving off cancels queued completion pushes and removes those completions from the server inbox, including failed pushes waiting to retry. Turning receiving back on allows new completions only; it does not replay old alerts. Per-person mutes are remembered while the overall switch is off. Already delivered device notifications cannot be recalled. Streak reminders, nudges, messages, replies, and announcements you send have their own controls and are not changed by these recipient preferences.

Players can opt into nudges through **Manage deck notifications** on the dashboard. When a place on the weekly board, their best day ever or the next level is within 150 XP, they hear about it once a day each, in reviews as well as XP. Nudges only arrive between 9:00 and 22:00 of a player's own day, and only after they have already reviewed something, so they never tell anyone to start studying.

Messages can also be written by hand on the server: `sudo ankiquest-message --from cerro aldanita "you are doing great, keep going"`, or `ankiquest <config> message <player> <text>` without the NixOS module. They arrive like any other notification, and with `--from` the recipient can answer them.

An ntfy push is marked delivered only after a successful HTTP response. Urgent streak warnings and existing notification types request high priority; other community reminders use normal priority, subject to the phone's notification settings. Failed pushes retry after 20 seconds, backing off to at most 15 minutes; missing ntfy configuration leaves inbox messages pending. The queue and inbox retain messages for seven days. Retries take turns between recipients and limit network work to 20 seconds per server check. Unlocks retain their existing ntfy-only delivery. Community reminders are available through both the inbox and ntfy; their relevance and quiet hours are rechecked before delivery. Upgrading retires pending legacy streak warnings. A server restart or a lost HTTP response can occasionally cause a duplicate push, but retries keep the same inbox notification.

The server and the client used to study must both be updated. Clients only report progress for decks you share, so players who never enable a deck send no deck data. Sending the deck list again replaces the stored one, which removes deleted decks.

Clients may include a `decks` array in the review upload. Each entry has `id` (a string), `name`, `remaining`, `reviewed_today`, and `day` (the local Anki day number, days since the Unix epoch after applying timezone and rollover). Omit this field when a reliable snapshot is unavailable. Initial silent uploads populate the deck list without announcing completions. Set `"catalog": true` when `decks` is the full deck list; decks missing from it are removed.

`GET /api/decks/<user>` with the user's bearer token returns private deck preferences and available recipients. `POST` to the same endpoint accepts `{"decks":[{"id":"123","enabled":true,"recipients":["hill"]}]}`. `GET /api/notifications/<user>` with the recipient's bearer token returns their recent completion announcements, with `id`, `title`, `body`, `day`, and `created_at` (Unix seconds). These endpoints never expose another player's deck settings or notification inbox without that player's token.

`GET /api/notification-preferences/<user>` requires that recipient's bearer token and returns `{"enabled":true,"muted_senders":[],"senders":[{"user":"hill","display":"Hill"}]}`. `POST` accepts only `{"enabled":false,"muted_senders":["hill"]}` and returns the updated settings plus available sender identities. At most 100 distinct sender IDs may be muted. IDs must be listed in that recipient's available senders; unknown fields, duplicates, and muting yourself are rejected. Sender choices include configured players and people who have shared with or been muted by the recipient, without revealing private deck names. Older clients saving outgoing deck preferences do not overwrite these incoming preferences.

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

The module runs the server in a tight sandbox. systemd holds the port and
passes it in, and the service may not connect to loopback or private
addresses, only out to the internet for ntfy. So `ntfy` must be a public
server. Outside systemd, `addr` is bound as usual.
