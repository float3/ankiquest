# Sharing

Every celebratory moment the server notices becomes a **win**. Each kind of win has a sharing
policy chosen by the player. A win can reach friends in AnkiQuest, or go anywhere as a card image
and link. All clients (Android, desktop add-on, web) use this API. None of them draws its own card.

## Moments

| kind | when | event key | default policy |
|---|---|---|---|
| `daily_goal` | the player's daily goal is reached | `goal:{day}` | `ask` |
| `streak_secured` | today first counts toward the streak | `secured:{day}` | `private` |
| `streak` | a streak milestone | `streak:{start}:{n}` | `ask` |
| `level` | a new level | `level:{n}` | `ask` |
| `achievement` | an achievement unlocks | `achievement:{id}` | `ask` |
| `record` | a personal best | `record:{window}:{day}` | `ask` |
| `quest` | a daily quest is done | `quest:{day}:{i}` | `private` |
| `conquered` | a card that kept failing is conquered | `conquered:{at}` | `ask` |
| `place` | top 3 on the weekly board at the end of the week | `place:{week}:{n}` | `ask` |
| `challenge` | a challenge is completed | `challenge:{id}` | `ask` |

Deck completions keep their own per-deck recipients (`/api/decks`) and are not part of this table.

Policies:
- `private`: only the player sees it.
- `ask`: the player sees it with a Share action.
- `auto`: friends are told right away.

The player still sees their own celebration in every case, as today (`celebrations_enabled` decides
whether it is pushed).

## Daily goal

The goal lives on the server so every client agrees.

`GET /api/goal/{user}` answers
`{ "mode": "off" | "cards" | "minutes" | "backlog", "value": n, "since": d, "chosen": bool }`.
`PUT /api/goal/{user}` takes `{ "mode", "value"?, "restart"? }`. Defaults and limits:

| mode | meaning | default `value` | allowed range |
|---|---|---|---|
| `off` | no goal | — | — |
| `cards` | reviews today, `today.reviews >= value` | 100 | 1–5000 |
| `minutes` | review minutes today, `today.minutes >= value` | 20 | 1–600 |
| `backlog` | clear the overdue backlog in `value` days, on top of today's due cards | 30 | 1–365 |

- The default goal is `backlog`, 30 days.
- `value` may be left out, and then takes the mode's default.
- `{ "mode": "off" }` keeps the last value, which `GET` still returns.
- `PUT` answers 200 with the saved goal. A value out of range or an unknown mode is 400.
- `chosen` is false until the player first saves a goal. Clients ask for one at setup while it is
  false.
- `since` is the Anki day the current plan started. The server sets it to today when `mode` or
  `value` changes, or when a PUT carries `"restart": true`.

### The backlog plan

Only clients know due counts, so they compute the backlog target. It uses three numbers:

- `due`: cards due today that are not overdue, plus learning and new cards still to do
- `backlog`: review cards overdue from earlier days, including those past the daily review limit
- `studied`: cards already studied today

The target for the day is:

```
days_left = value - (today - since)
if days_left < 1: restart the plan (PUT restart) and use days_left = value
extra     = backlog == 0 ? 0 : max(1, ceil(backlog / days_left))
target    = studied + due + extra
```

`backlog` and `due` are read at the first upload of the day, and that day's target stays fixed.
The target spreads what is left over the days left, so the backlog is gone on day `since + value`
even when some days fall short. It doesn't dwindle to a trickle near the end.

### Goal setup and projections

Clients show the player what each choice means for their own collection:

- whenever the goal dialog opens
- once as part of setup while `chosen` is false: right after signing in, or on the next start for
  players who were already signed in

Inputs for the estimates, besides `due` and `backlog`:

- `daily_due`: the average number of cards due per day over the next 7 days, from the
  collection's forecast, not counting the backlog
- `seconds_per_card`: the player's average answer time over the last 30 days, or 8 s with no history

Estimates per choice, updated as the player changes the value:

| choice | cards today | time today | backlog cleared in |
|---|---|---|---|
| backlog, N days | `due + ceil(backlog/N)` | that × `seconds_per_card` | N days |
| cards C | C | C × `seconds_per_card` | `ceil(backlog / (C − daily_due))` days, or never if `C ≤ daily_due` |
| minutes M | `M×60 / seconds_per_card` | M minutes | as for cards, with C = `M×60 / seconds_per_card` |

The dialog also names the smallest goal that shrinks the backlog: `daily_due + 1` cards a day.
With no backlog it says "Nothing overdue" and suggests `daily_due` cards.

Presets for N: 7, 14, 30 and 60 days, plus a custom value.

The server decides `cards` and `minutes` itself, when reviews are uploaded. Only the client knows
due counts, so for `backlog` the client reports
`POST /api/goal/{user}/reached { "day": d }` (owner only). It is accepted once per day, and only
when the mode is `backlog` and `d` is the player's current Anki day. Otherwise it answers 409.

`GET /api/profile/{user}` (owner view) gains `goal`:

```json
{ "mode": "cards", "value": 100, "since": 20391, "progress": 64, "reached": false }
```

`progress` is reviews or minutes so far. For `backlog` it is `null`, and `reached` reflects a
reported reach.

## Sharing settings

- `GET /api/sharing/{user}` answers `{ "policies": { "<kind>": "private"|"ask"|"auto", ... }, "daily_cap": 3 }`
  for every kind in the table.
- `PUT /api/sharing/{user}` takes the same shape and answers 200 with the saved settings. Kinds left
  out keep their value; an unknown kind or policy, or a cap outside 1–10, is 400.
- `daily_cap` (1–10) limits how many automatic wins one player sends each friend per Anki day. Wins
  over the cap still appear in the player's own list and can be shared by hand.

## Wins

Wins are stored per player. `GET /api/wins/{user}?limit=50&before=<id>` (owner only, `limit` 1–200)
answers `{ "wins": [...] }`, newest first. Clients page with `before` set to the last id until a page
comes back shorter than `limit`. A row looks like this:

```json
{
  "id": 17, "kind": "daily_goal", "key": "goal:20391", "day": 20391, "created_at": 1793400000,
  "title": "Daily goal reached!", "body": "120 cards today in 35 minutes.",
  "stats": { "streak": 51, "level": 54, "reviews_today": 120, "minutes_today": 35 },
  "art": "celebrate",
  "policy": "ask", "shared_with_friends": false, "link": null
}
```

- `created_at` is in seconds, like the inbox.
- `art` is one of the companion poses, picked per kind (see Cards).
- `link` is `{ "url": "...", "image": "..." }` once a link exists.

Celebration notifications in the inbox (`kind` `celebration`, `challenge_complete`,
`reminder_weekly_recap`) gain `"win": 17` when they belong to a win. Clients share by win id. A
grouped "N new unlocks" row stands for several wins and has none. Celebrations for the new moments
carry the topics `goal` and `secured`.

## Sharing a win

`POST /api/share/{user}` keeps its current bodies (`{notification}`, `{place}`) and adds:

- `{ "win": 17, "to": "friends" }` sends the win to the circle. It answers
  `{ "recipients": n, "link": { "url": "...", "image": "..." } }`.
- `{ "win": 17, "to": "link" }` answers
  `{ "url": "https://host/w/<token>", "image": "https://host/w/<token>.png", "text": "<invitation text>" }`.
  The link is created once and reused.

Sending to friends also creates the link, because friends see the card through it.

Friends receive a `share` row as today. It has the same `topic` and quoted title/body, plus
`"win": 17` and `"image": "https://host/w/<token>.png"`. Mutes and unsubscriptions are respected, and
each win reaches each friend once.

`{ "notification": id }` for a row that has a `win` shares that win to friends, so it is not sent twice.

`DELETE /api/wins/{user}/{id}/link` revokes the link (204, or 404 for someone else's win). The page and the image then answer 404.

## Automatic sharing and digests

When a win's policy is `auto`, the server sends it to friends in the same tick in which it is created.
If one tick creates several automatic wins for a player, each friend gets **one** `share` row:

- title `"🎉 {display}: {n} wins"`
- body listing the win titles, joined with " · "
- `topic` of the first win
- `win` is the first win's id

The daily cap counts rows sent, not wins.

## Reactions

`POST /api/react/{user} { "notification": 42 }`: `user` reacts with 🎉 to a `share` row in their own
inbox (or a `completion` row). The original sender gets a `reaction` row:

- title `"🎉 {display} cheered"`
- body: the quoted win title
- `sender` is the reactor, so it can be replied to

One reaction per reactor per notification; a repeat answers 200 with `{ "already": true }`.

The share row in the reactor's inbox gains `"reacted": true`.

## Cards

Cards are drawn by the server:

- `GET /w/{token}.png`: a 1080×1080 PNG
- `GET /w/{token}.wide.png`: 1200×630, for link previews
- `GET /api/wins/{user}/{id}/card.png`: the same square card for the owner before any link exists

Card contents:
- art for the kind, in the player's companion (Aki, Ankilope, or none). The poses are:
  - `celebrate`: daily_goal, level, achievement, quest, challenge
  - `streak`: streak, streak_secured
  - `winner`: place, record
  - `freeze`: unused for now
  - Ankilope has no `streak`, `winner` or `freeze` pose and uses `celebrate`. The new art from #88
    replaces these poses later.
- title, body
- up to three stat chips from `stats`
- the player's display name with their avatar
- the date in the player's language
- the server's host in the footer

Colours are the app's light theme. Text is in the player's language.

`GET /w/{token}` is a small public HTML page with the card and an invitation to the server. Its
`og:title`, `og:description`, `og:image` (wide), `twitter:card=summary_large_image` tags make pasted
links preview. Only wins that have a link are public. Tokens are random and unguessable.

## Privacy

`privacy.rs` covers the new tables:
- export includes wins, goal, sharing settings and reactions
- deletion removes them and their links
