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
A client can also ask right after a deck is finished which friends to tell (see "Telling friends about a finished deck").

Policies:
- `private`: only the player sees it.
- `ask`: the player sees it with a Share action.
- `auto`: friends are told right away.

The player still sees their own celebration in every case, as today (`celebrations_enabled` decides
whether it is pushed).

`streak_secured` happens on every study day, so it is pushed only when the streak was saved late:
in the last three hours of the player's day, or after an urgent streak reminder that day. Otherwise
its inbox row carries `quiet: true` and clients show it without announcing it. Streak milestones
are pushed as their own `streak` celebration.

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
{ "mode": "cards", "value": 100, "since": 20391, "day": 20395, "progress": 64, "reached": false }
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
- `share_title` and `share_body`, when present, are the win as it leaves the player: in their own
  words ("I reached level 4 with 900 XP.", "Today counts toward my 12-day streak."), as friends,
  the card, the public page and the invitation text show it. Wins that already read the same for
  anyone, such as the daily goal or a place, leave them out. The player's own shareable inbox rows
  carry the same two fields.

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

## Sending a moment to chosen friends

`POST /api/share/{user}` (owner only) also takes one body for every moment that is a win:

```json
{ "moment": { "conquest": 1793183400000 }, "to": { "friends": ["cerro"], "groups": [3] }, "note": "Took me a month!" }
```

- `moment` is one of:
  - `{ "win": 17 }`: a win by id.
  - `{ "key": "streak:20000:30" }`: a win by its event key, the `moment` of an inbox row. Records,
    streak milestones, daily goals, levels, achievements, places and challenges are all wins.
  - `{ "notification": 42 }`: one of the player's own shareable rows that has a `win`. A row from
    before wins were kept has none and is `409`; share it with `{ "notification" }` alone.
  - `{ "conquest": at }`: a conquered card. Its win is kept now if the server announced it before
    wins were kept; one the worker has not announced yet is `409`, so ask again shortly.
- `to` is `"circle"`, everyone in the player's circle, or `{ "friends": [...], "groups": [...] }`.
  Friends must be in the player's circle and groups ones the player is in; a group reaches its
  members other than the player. At most 100 friends and 20 groups, none twice, and not both
  empty. Anything else is `400`.
- `note` is optional: one line of at most 140 characters, with runs of spaces and line breaks
  made one space. A note with nothing printable is `400`.

It answers `{ "recipients": n, "link": { "url", "image" }, "win": 17, "undo_until": ms }`.
`recipients` counts those who got it now. Each win reaches each person once, whoever sent it there
and however it was addressed, and people who muted or unsubscribed from the player are skipped.
`undo_until` is left out when nobody got it. A missing moment is `404`.

Recipients get the usual `share` row (see above) with `"note"` when the player wrote one. The link
is made as for `{ "win", "to": "friends" }`, which now does the same as `"to": "circle"` and
answers as before.

`DELETE /api/share/{user}/{win}` takes a win sent by hand back. Rows sent within the last day that
their recipient has not read, cheered or answered leave their activity, and those people can be
sent it again; other rows stay, and a push already delivered stays on their phone. Wins sent by an
`auto` policy are not taken back. It answers `{ "retracted": n }`, `0` when there was nothing to
take back, or `409` when every row was sent more than a day ago. Someone else's win is `404`.

Telling friends about a conquest (`/api/conquests/{user}/{at}/tell`) and a finished deck
(`/api/decks/{user}/share`) keep working as described here. Clients that see `share_moments` in
`/api/capabilities` use this body instead for conquests.

### Conquered cards in the player's words

A `conquered` win reads as the player's own news once it leaves them: on its card, its page, the
invitation text and friends' `share` rows. The player's own list keeps the stored text.

- title: "I just conquered a card", or "I just tamed a leech"
- body: "I forgot it {n} times, and now it stays with me for weeks."

The card itself is never shared unless its owner adds a label for that conquest with
`POST /api/conquests/{user}/{at}/label` (`at` is the number in the win's key `conquered:{at}`).
The body then names it: "I forgot “{label}” {n} times, and now it stays with me for weeks."
`DELETE` on the same path takes it off the card and page again; friends already told keep the row
they got. Clients offer the label per share, off by default, and show the player the exact text first.

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
- `GET /api/wins/{user}/{id}/card.png`: the same square card for the owner before any link exists.
  Clients use it to copy or save the picture, so that doing so publishes nothing; they ask for
  `{ "win", "to": "link" }` only when the player chooses to share a link, and say that anyone
  with the link can see the card.

Card contents:
- art for the kind, in the player's companion (Aki, Ankilope, or none). The poses are:
  - `celebrate`: daily_goal, level, achievement, quest, challenge
  - `streak`: streak, streak_secured
  - `winner`: place, record
  - `freeze`: unused for now
  - Ankilope has no `streak`, `winner` or `freeze` pose and uses `celebrate`. The new art from #88
    replaces these poses later.
- title, body
- up to three stat chips from `stats`: streak, level, and the day's cards and minutes, which say
  "today". A `conquered` card leaves the day's totals off, since it is about one card
- the player's display name with their avatar
- the date in the player's language
- the server's host in the footer

Colours are the app's light theme. Text is in the player's language.

`GET /w/{token}` is a small public HTML page with the card and an invitation to the server. Its
`og:title`, `og:description`, `og:image` (wide), `twitter:card=summary_large_image` tags make pasted
links preview. Only wins that have a link are public. Tokens are random and unguessable.

## Telling friends about a finished deck

Per-deck recipients announce a deck automatically. For other decks the client can ask the player
right after they finish one which friends to tell (players can turn the question off):

`POST /api/decks/{user}/share { "id": "123", "name": "Spanish", "recipients": ["cerro"] }` (owner
only) sends each recipient the usual `completion` row, "{display} has finished their {name}
studies for today.", and answers `{ "recipients": n }`, those who got it now. Recipients must be
among `recipients` of `GET /api/decks/{user}` (the player's circle, without people who
unsubscribed), at most 100 and without repeats; otherwise it is 400. Each recipient hears about a
deck once per Anki day; recipients who paused deck alerts or unsubscribed from the player are
skipped. The name comes from this request only and is not stored with the player's decks. The
server cannot check that the deck is finished, so this is the player's word, like a message.

## Privacy

`privacy.rs` covers the new tables:
- export includes wins, goal, sharing settings and reactions
- deletion removes them and their links
