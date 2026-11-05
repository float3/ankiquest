# member sessions, activity, and challenge events

these additions preserve bearer authentication and the existing notification array. XP, challenges' progress rules, review scheduling, streaks, and scoring are unchanged.


## capabilities

`GET /api/capabilities` needs no sign-in, also on a private site, and may be cached for five minutes (`Cache-Control: max-age=300`). It answers what this server supports, so a client can turn a feature on when the server has it instead of waiting for a deploy:

```json
{ "version": 1, "features": ["activity", "app_sign_in", "cookie_session", "deck_share", "devices", "goal_backlog_days", "mute", "reactions", "review_offsets", "share_cards", "wins"], "public_url": "https://anki.example.test" }
```

`public_url` is the server's main address, without a trailing slash, and is present only when the server has `public_url` configured. A client that saved another address for the same server (an old address that proxies to it) may move to this one: it checks that its saved token works there with one signed-in request, and only then saves the new address and moves anything it keeps per address. Clients only move to an `https` address, keep the saved one when the check fails or they are offline, and ignore the field otherwise.

| feature | meaning |
|---|---|
| `activity` | the durable inbox, `/api/activity/{user}` (below) |
| `app_sign_in` | `POST /api/sign-in/{user}` `{"action": "browser", "provider", "purpose": "link" \| "reauth"}` gives an address to link a provider or sign in with it again in a browser (see `docs/server.md`, sign in with Google, GitHub or GitLab) |
| `cookie_session` | the web dashboard works with the session cookie alone when the native app hands it only `{user}`, no token |
| `deck_share` | telling chosen friends about a deck just finished, `POST /api/decks/{user}/share` (see [sharing.md](sharing.md)) |
| `devices` | password change and the device list and sign-out (`/api/devices/{user}`, see `docs/server.md`) |
| `goal_backlog_days` | the `backlog` goal clears the backlog in `value` days, with `since`, `chosen` and `restart` (see [sharing.md](sharing.md)) |
| `mute` | muting a person, `POST /api/social/{user}` `{ "action": "mute" \| "unmute", "user" }`; `muted` in the social overview (see [community.md](community.md)) |
| `reactions` | cheering a friend's share, `POST /api/react/{user}` |
| `review_offsets` | an uploaded review row may carry `offset_west_min`, the device's UTC offset when the review was made, and the server dates time-zone changes from it (see `docs/server.md`, "getting reviews in"). Older servers ignore the field, so sending it is harmless |
| `share_cards` | server-drawn cards and public pages under `/w/` |
| `wins` | wins, sharing policies and `{ "win", "to" }` shares (see [sharing.md](sharing.md)) |

An account without a password (signed up with a provider) has to sign in with that provider again before it can be deleted or set its first password: those requests answer `403` with `"reauth": true` until it has, and a device token never counts. On a server with `app_sign_in`, apps ask for a browser address with `{"action": "browser", "purpose": "reauth"}`, open it in the system browser, and the sign-in then counts for ten minutes for their token. Older servers offer only **Sign in again** in Settings on the website (`/auth/oauth/{provider}?reauth=1`), which counts for that browser's session. See `docs/server.md`.

A name is never removed or renamed. New names are added as their features land. A server without this endpoint (`404`) has none of them. Clients ignore names they don't know. `version` changes only if the shape of this answer changes.

Real responses for the endpoints the Android app uses are in [api/examples](api/examples/README.md).

## account settings

The Android app's account settings are native and use these endpoints with the device token (all described in `docs/server.md`, "passwords and devices", "sign in with Google, GitHub or GitLab" and the privacy section):

| request | answers |
|---|---|
| `GET /api/sign-in/{user}` | `account`, `password` and `providers` (`provider`, `name`, `linked`) |
| `POST /api/sign-in/{user}` `{"action": "unlink", "provider"}` | the same, after unlinking |
| `POST /api/sign-in/{user}` `{"action": "password", "current", "password", "sign_out_others"}` | the same, after the change |
| `POST /api/sign-in/{user}` `{"action": "browser", "provider", "purpose"}` | `url` to open in the browser, `expires_at` |
| `GET /api/devices/{user}`, `POST` `{"action": "sign_out", "id"}` or `{"action": "sign_out_others"}` | `devices` |
| `GET /api/accounts/{user}` | `user`, `display`, `created_at`, `password`, `confirmed_until` |
| `GET /api/export/{user}` | the data download, saved by the app as it came |
| `DELETE /api/accounts/{user}` `{"password"}`, or no body for an account without a password that signed in again | `204` |

The `403` with `"reauth": true` described under capabilities applies to the first password and to deleting an account without a password.

## browser identity

`POST /auth/session` still returns `204` with the `ankiquest_session` cookie. a valid `Authorization: Bearer <member-token>` establishes that member's session. browser JSON `{ "password": "..." }` accepts either the shared site password or a member token and requires `X-Ankiquest-CSRF: 1`. a shared-password session can read shared pages but has no personal permissions. if a secret is both the shared password and a member token, password login remains read-only. a token configured for multiple members cannot implicitly select a browser owner; existing explicit bearer requests retain their previous per-user behavior.

the cookie is HttpOnly, SameSite=Lax, scoped to `/`, and Secure when the configured public URL uses HTTPS. sessions expire after seven days, rotate on sign-in, are revoked by logout or by signing their device out (see `docs/server.md`, passwords and devices), and disappear when the server restarts. tokens never appear in status responses.

`GET /auth/status` returns:

```json
{
  "private_site": true,
  "authenticated": true,
  "member": { "user": "alice", "display": "Alice" }
}
```

`member` is `null` for a master session, an ambiguous token, or no valid member credential. personal endpoints authorize only the URL's owner, through either their existing bearer token or their member cookie. an explicit invalid or different-member Authorization header never falls back to a cookie. shared viewing and owner permission are separate, including when the server runs in public mode.

all cookie-authenticated mutations require `X-Ankiquest-CSRF: 1`. when origin is present it must match the configured site's origin. the custom header also prevents a cross-site form submission when Origin is absent; the server does not enable cross-origin credentialed requests. native bearer mutations do not require a CSRF header. `/auth/logout` retains its explicit header/origin check. clients must clear personal snapshots and pending actions on logout/account change and scope caches to server plus member plus credential.

## durable activity

`GET /api/activity/{user}` accepts optional `days=90` (default) or `days=30`, `limit=100` (default, 1–200), and `before=<positive notification ID>`. it returns newest IDs first:

```json
{
  "items": [{
    "id": 42,
    "title": "Challenge invitation",
    "body": "alice invited you to Study together.",
    "day": 20718,
    "created_at": 1790000000,
    "sender": "alice",
    "replied": false,
    "kind": "challenge_invite",
    "read_at": null,
    "challenge_id": 8,
    "route": "/community#challenge-8",
    "action_required": true
  }],
  "unread_count": 1,
  "action_count": 1,
  "retention_days": 90,
  "window_days": 90,
  "next_before": null
}
```

both timestamp fields are Unix **seconds**; challenge `start_at` and `end_at` retain their existing Unix **milliseconds** convention. Non-challenge rows have null `challenge_id` and `route`. existing rows acquire null `read_at` and are unread. activity preserves historical reminder text, including reminders that are no longer timely; it is not a source for replaying old system notifications.

`topic` is present only when the server knows what a row is about, so clients can pick matching art. Celebrations and game events carry the first part of their event key: `achievement`, `level`, `streak`, `record`, `conquered`, `secured` or `goal`; a grouped "N new unlocks" row has one only when all of its events share it. A weekly recap carries `place:N` when the player was ranked. Both personal inbox endpoints include it. A `reminder_weekly_recap` body has one sentence a line (reviews and XP, place, friends, change from the previous week, best day, daily wins, streak); show it with its line breaks, or join the lines with spaces where only one line fits. Recaps written before this came as one line.

`moment` is present only when a row is about a win: the player's own celebration, a friend's cheer (`reaction`) for one of the player's wins, or, on a friend's `share` row, that friend's win. It is that win's stable event key, the same key the moment was recorded under, such as `conquered:1793183400000`, `level:4`, `streak:20000:30`, `goal:20012` or `record:day:20000`. A conquered card's key is `conquered:` followed by the `at` the upload response returned for it in `conquests[]`, so a client that already showed that moment can recognise its `celebration` row and not show it twice; check `kind` too, since a cheer for the same win carries the same key. Grouped "N new unlocks" rows and rows not about a win have no `moment`. Unlike `win`, a server id the client cannot know beforehand, the key can be matched against what the client already saw.

`quiet: true` marks a row the server keeps in the inbox without pushing it. Clients list it like any other row but raise no system notification, tooltip, sound or share prompt for it. Today this is "Streak secured!" on an ordinary study day; it is absent (false) on every other row, so older clients that ignore it just announce the row as before.

counts cover all items within the selected window, independently of pagination, push state, and any Android delivery cursor. `unread_count` counts rows with no read timestamp. `action_count` counts still-pending challenge invitations whose challenge is neither cancelled nor past its deadline. reading an invitation does not accept it. a 30-day view counts its own 30-day window; use the default 90-day view for the full retained inbox. follow `next_before` until null to load more.

`POST /api/activity/{user}/read` accepts `{ "ids": [42, 43] }` and returns `204`. at most 200 positive IDs per request; clients mark larger selections in batches. duplicate IDs/retries are safe and preserve the first read timestamp. unknown, expired, push-only, and other members' IDs have no effect. the caller must own `{user}`. read state survives restart and changes neither push acknowledgement nor legacy delivery state.

the server physically retains notifications for 90 days; history already deleted by an older server cannot be reconstructed. the legacy `GET /api/notifications/{user}` remains an oldest-first array of up to 500 notifications from the last seven days, with its existing reminder delivery checks. it gains the four fields `read_at`, `challenge_id`, `route`, and `action_required`; old clients can ignore them. push delivery eligibility also stays seven days. Push-only game events remain excluded from both personal inbox endpoints. new clients can fall back to the legacy endpoint after an Activity `404`, with no inferred unread badge.

## challenge lifecycle

new event kinds are `challenge_invite`, `challenge_accepted`, and `challenge_complete`. the stable ID and relative route above are attached to each event. only selected recipients receive invitations, only the creator receives acceptance events, and only accepted members receive completion events. no challenge creation broadcasts to unrelated members. creation/membership changes, event deduplication, and notification insertion commit or roll back together. completion is reconciled after uploads, in the regular server poll, and before personal challenge/inbox reads; completion notices are deduplicated across retries and restarts. progress continues to use only reviews after joining and before the original deadline. a completion notice records that the goal was observed complete; later review undo or member departure can still alter live progress under the existing rules.

`POST /api/community/challenges/{user}` accepts optional `request_id`, an 8–128 character string of ASCII letters, numbers, hyphens, and underscores. new clients generate a fresh UUID for each deliberate creation and reuse it only when retrying that creation. a repeated key with the same normalized title, kind, cooperative flag, target, duration, and recipient set returns the existing challenge list without another challenge or event. reusing it with different data returns `400`. keys are scoped to the creator. older clients may omit the key and retain their original create behavior.

repeated successful accept, decline, leave, and creator cancel operations are idempotent. accept retries never reset `joined_at`. accepting or declining an invitation also marks that invitation read. acting as a nonmember is forbidden; all challenge endpoints still require the URL owner's credential. clients should reload challenge details before presenting available actions, since Activity can outlive an invitation, membership, or the challenge deadline.

### friend goals for apps

`GET /api/community/challenges/{user}` answers `{ "challenges", "recipients", "weekly_suggestion" }`, newest goal first. `challenges` holds every open goal the player is invited to or has joined, and the latest 100 that are over; a goal the player declined or left is not listed. each goal has `id`, `title`, `kind` (`study_days` or `reviews`), `cooperative`, `creator`, `start_at` and `end_at` (Unix milliseconds), `target`, `progress` (the members' sum), `status`, `weekly`, `started`, `start_when_ready`, and `members`, each with `user`, `display`, `status` (`invited`, `accepted`, `declined` or `left`) and `progress`. a goal's `status` is `active`, `upcoming`, `waiting` (not `started`: it starts once everyone accepts), `complete` (reached, still open until `end_at`), `ended`, `cancelled` (ended early by its creator) or `declined` (a goal waiting for everyone that an invitee declined). an invitation is a goal whose member entry for the player is `invited`. a cooperative goal counts `progress` against `target`, an individual one each accepted member's own. `recipients` are the people the player can invite, as `{ "user", "display" }`. `weekly_suggestion` is null or this week's suggested friend: `week_start`, `expires_at`, `friend`, `target`, `duration_days`, `dismissed` and `challenge_id`, the goal its invitation created.

`POST /api/community/challenges/{user}` creates a goal from `{ "title", "kind", "cooperative", "target", "duration_days", "recipients", "start_when_ready", "request_id" }`: 1–80 characters, 1–31 days, a target of 1 to 100,000 (a study-day target at most the days the participants have), and 1–30 different recipients from `recipients`. `POST /api/community/challenges/{user}/{id}` takes `{ "action" }`: `accept` or `decline` for an invitation, `leave` for a joined goal someone else created, `cancel` for the creator. `POST /api/community/challenges/{user}/weekly` takes `{ "week_start", "action": "invite" | "dismiss" }` for the weekly suggestion. all three answer the same object as the `GET`, so a client redraws from the answer. a refusal is `400` with `{ "error" }` to show, `403` for a goal the player is not part of and `404` for one that does not exist. examples: `challenges.json`, `challenge_create.json` and `challenge_action.json` in [api/examples](api/examples/README.md).

### friends and groups for apps

`GET /api/social/{user}` answers `{ "friends", "incoming", "outgoing", "muted", "groups", "public" }`. `friends`, `incoming` (requests waiting for the player), `outgoing` (the player's requests waiting for an answer) and `muted` are people as `{ "user", "display" }`, by display name. each group has `id`, `name`, `owner` (whether the player owns it), `invite` (its invite code, only for the owner, otherwise null) and `members`. `public` is whether the player's profile is public: anyone may open it, and it is on the global board in full (see the global leaderboard below for the separate opt-in that lists a player there without opening the profile). a group's invite link is `/join/{invite}` on the server's site.

`POST /api/social/{user}` takes one `{ "action", ... }` and answers the same object as the `GET`, so a client redraws from the answer:

| action | fields | |
|---|---|---|
| `befriend` | `user` | sends a request, or accepts one from that player |
| `unfriend` | `user` | removes a friend, declines their request or withdraws the player's own |
| `mute`, `unmute` | `user` | see above |
| `create_group` | `name` | 1–40 characters; the player owns the new group |
| `rename_group` | `group`, `name` | owner only |
| `new_invite` | `group` | owner only; the old code stops working |
| `join` | `invite` | the code from a `/join/{invite}` link; joining again is fine |
| `leave` | `group` | the longest-standing member inherits an owner's group; an empty group is deleted |
| `remove` | `group`, `member` | owner only, not themselves |
| `visibility` | `public` | |

a refusal has `{ "error" }` to show: `400` for a group name, `403` for an owner action by someone else, `404` for an unknown player or invite, `409` when the player is in 20 groups, a group has 200 members, 50 requests wait or 500 people are muted. `GET /api/invites/{code}` needs no credential and answers `{ "name", "members" }` (the member count) to show before joining, or `404`. examples: `social.json`, `group_create.json`, `invite.json` and `social_change.json` in [api/examples](api/examples/README.md). friend requests and group joins send no notifications yet.

## sharing a win

`POST /api/share/{user}` passes a win on to everyone in the player's circle. The body is either `{ "notification": 42 }`, one of the player's own `celebration`, `challenge_complete` or `reminder_weekly_recap` rows, or `{ "place": 2 }`, the player's current place (1 to 3) on this week's board. It answers `{ "recipients": n }`, the friends who got it now.

friends receive a `share` row from the player, with the title and body quoted in the player's own words ("I reached level 4 with 900 XP." rather than "You reached…") and the same `topic` (`place:N` for a place), so it shows the same art and can be replied to.

wins, the daily goal (`/api/goal/{user}`), sharing policies (`/api/sharing/{user}`), `{ "win": id, "to": "friends" | "link" }` shares, `GET /api/wins/{user}`, `DELETE /api/wins/{user}/{id}/link`, automatic digests and `POST /api/react/{user}` are described in [sharing.md](sharing.md). Inbox rows may carry `win` (the win they are about), `share_title` and `share_body` (on the player's own shareable rows, what sharing it tells others, when that differs from `title` and `body`; clients share these and fall back to `title` and `body`), `image` (a friend's shared card) and `reacted: true` (you cheered it); the sender of a cheer gets a replyable `reaction` row. Each win reaches each friend once; friends who muted the player are skipped. Unknown or unshareable notifications are `404`, a place the player doesn't hold is `409`, and anything else is `400`.

## leaderboards and profiles

the app's native leaderboard and player profiles read the same JSON as the website's `/`, `/hour` … `/all` and `/week#<user>` pages.

`GET /api/leaderboard?period=hour|day|week|month|year|all` answers the players the viewer may see, ordered by `xp` in that period, then `xp_total`, then `user`. An unknown or missing `period` is `week`. Each row has `user`, `display`, `level`, `xp` (the requested period), `period`, `periods` (every period's XP), `xp_total`, `week_xp`, `streak`, `streak_state`, and today's `today_reviews`, `today_minutes` and `streak_minutes` (review minutes the streak needs; 0 when any review counts), valid until `day_ends_at` (ms). `closing_xp` is there only while the player's current day began before the weekly reset: the XP still counting toward the week that just ended on the board (see `closing_week` below). Optional `scope` picks the players:

- `friends` (or `circle`): the viewer, their friends and the members of their groups. this is also the board without a `scope`, so older apps see what they always did; a viewer who is not signed in gets `global` instead.
- `group` with `group=<id>` (or `group:<id>`, as on the website's board picker): the members of that group only. a group the viewer is not in, or that does not exist, is `403`; `scope=group` without a numeric `group` is `400`.
- `global`: the global leaderboard, see below.

anything else is `400`. rows come only for players who have synced reviews.

### the global leaderboard

`GET /api/leaderboard?scope=global&period=…` ranks everyone who turned on the global leaderboard, plus public profiles (a profile made public with the `visibility` action, and configured players, who are public unless they say otherwise). a viewer who is not signed in sees only the public profiles. players the viewer muted are left out; muting stays one way, so a player who muted the viewer still sees the viewer and is still seen. the board lists the first 100 places, plus the viewer's own row when it is further down, and every row has `rank`, its place from 1 among everyone on that board. ranks are unique even for equal XP (the order is the one above).

the viewer's own row is always there once they have synced reviews, opted in or not. it carries `"me": true` and `shown`: whether other players see it on this board. with `"shown": false` the app can say "you, not shown to others". other rows have neither field.

rows of players the viewer can already see in full (themselves, their friends and group mates, and public profiles) are the same as on the circle board. any other player (a stranger who only turned the global leaderboard on) has only `rank`, `user`, `display`, `level`, `xp` (the requested period), `period` and `streak`: no `periods`, `xp_total`, `week_xp`, `streak_state`, today's counts, `day_ends_at` or `closing_xp`. `user` is there for the profile picture (`GET /api/avatar/{user}`) and to mute them with `POST /api/social/{user}` `{"action": "mute", "user"}`, which also takes them off the viewer's global board. their profile stays private: `GET /api/profile/{user}` is still `404` for strangers. records and `POST /api/rank` do not include strangers.

the opt-in is `global_leaderboard` in the player's settings, beside `nudges` and `celebrations`: `GET /api/decks/{user}` answers it (false until the player turns it on) and `POST /api/decks/{user}` with `{"decks": [], "global_leaderboard": true}` changes it and keeps everything else, answering the same settings. older servers have no such field; an app should then hide the switch and the global board. examples: `leaderboard_global.json` (a viewer whose stranger row is Ana), `leaderboard_global_hidden.json` (Ana before opting in: her row with `"shown": false`) and `global_leaderboard_on.json` (the settings answer after turning it on).

`GET /api/week` answers `{ "ends_at", "starts_at", "timezone" }`: when the competition week, and so the weekly board, began and resets (ms).

`GET /api/profile/{user}` answers the player's level, XP, `periods`, `records`, streak and `streak_state`, streak freezes (`stored_freezes`, `freezes_enabled`, `freeze_earned_today`), `today`, `lifetime`, daily `quests`, `achievements`, `heatmap` and `conquests`. Only the owner's own profile has `goal`. `closing_week: { "xp", "ends_at" }` is there only while the player's current Anki day began before the weekly reset, so it still counts toward the closing week although `week_xp` already shows the new one: `xp` is what counts toward the closing week, today's included, and `ends_at` (ms) is when it closes for them, the end of their current day. West of `week_timezone` this lasts up to the player's own rollover, about 9 hours in California for a reset at Monday 00:00 UTC; clients show it as "last week still counting" and drop it once `ends_at` has passed. A player the viewer may not see is `404`, the same as an unknown one. The owner's recent wins are `GET /api/wins/{user}` (see [sharing.md](sharing.md)).

## records and study history

the app's native records and study history screens read the same JSON as the website's `/records` and `/history` pages.

`GET /api/records` answers seven boards, in this order: `hour`, `day`, `week`, `month` and `year` (the best rolling hour, 24 hours, 7, 30 and 365 days of XP anyone has had), then `streak` (longest streak) and `days` (days studied). Each is `{ "window", "unit", "holders" }`, `unit` being `xp` or `days`. `holders` are the top three players with anything at all, best first, each with `user`, `display`, `value`, `detail` (the reviews in that window; 0 for `streak` and `days`), `at` and `until` (ms). For a rolling window `at` is its first review and `until` its last; a streak's `at` is when it ended and `days`' when the player first studied, with `until` the same. Optional `scope` and who is included are as for `GET /api/leaderboard`. example: `records.json`.

`GET /api/study/{user}?year=YYYY` is the owner's own study calendar, for the year asked or the player's current one: `{ "year", "years", "days", "last_review_at", "last_received_at", "latest_session" }`. `years` lists every year with history, newest first. `days` has one entry per Anki day from the player's first study day on (days without reviews included, with zeros), each with `date` (`YYYY-MM-DD`, following the player's cutoff), `at` (when that day began, ms), `reviews`, `time_ms`, `xp`, `new_cards`, `streak` (the streak on that day) and `frozen` (a streak freeze covered it). `last_review_at` is the newest uploaded review (ms) and `last_received_at` when reviews last arrived; either is null before the first upload. `latest_session` is null or `{ "started_at", "ended_at", "reviews", "time_ms", "new_cards" }`. Another player's history is `401` or `403`, a year outside the supported range `400`. example: `study.json`.

## community calendar, reminders and conquered cards

the app's native community calendar, reminder settings and conquered-card screens read the same JSON as the website's `/community#calendar`, `/community#reminders` and `/conquered/{user}/{at}` pages.

`GET /api/community?year=YYYY&month=M` needs no credential and answers the competition archive described in [community.md](community.md): `meta` (with `time_zone`, `rollover_hour`, `grace_hours`, `start_date` and `scoring_note`), `calendar` (the days of the month asked), `weeks` and `seasons` (the weeks and months of that year), and `players`, `awards`, `records`, `head_to_head` and `winner_history`. Each period has `key`, `start`, `end`, `kind` (`day`, `week` or `month`), `status` (`ongoing`, `provisional` or `final`), `reconstructed`, `partial`, `winners` (usernames; more than one for a shared win), `winning_margin` (XP) and `standings`, each with `user`, `display`, `xp`, `reviews`, `time_ms`, `new_cards`, `days_active`, `best_streak` and `freezes_used`. A year or month out of range is `400`. example: `community.json`.

`GET /api/community/reminders/{user}` answers the owner's reminder settings and `POST` replaces them all, answering the saved settings: see [community.md](community.md). examples: `reminders.json` and `reminders_save.json`.

`GET /api/conquests/{user}/{at}` answers a conquered card the viewer may see: `user`, `display`, `conquest` (`at`, the answer that conquered it, and `since`, its first answer, both ms; `lapses`, `answers` and `leech`) and `label` when its owner said what the card was. The owner also gets `told` (whether friends have been told), `friends` (how many there are) and, once the server has kept the conquest as a win, `win`: its id, to share its card through `POST /api/share/{user}` with `{ "win", "to": "link" }`. `POST /api/conquests/{user}/{at}/tell` tells the owner's friends once and answers `{ "told" }`, the friends it reached; `POST /api/conquests/{user}/{at}/label` with `{ "label" }` and `DELETE` on the same path set and remove the label and answer `204`. A card that cannot be found or seen is `404`. examples: `conquest.json` and `conquest_tell.json`.

the privacy notice (`/privacy`) is an HTML page filled in with the operator's own details; there is no JSON for it, and apps open it in the browser.
