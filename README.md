# ankiquest

XP, levels, streaks, daily quests, achievements and a leaderboard for Anki. Clients send review log rows (card id, timestamp, previous interval, time taken, review type), never card content. Deck names and daily counts are only sent by players who use deck completion notifications.

XP never depends on which answer button was pressed, so there is no incentive to grade dishonestly.

## Run

```sh
cargo run -- ankiquest.json
```

```json
{
  "addr": "127.0.0.1:8097",
  "state_dir": "state",
  "ntfy": "https://ntfy.sh",
  "remind_hour": 20,
  "public_url": "https://anki.example.com",
  "week_timezone": "Europe/Berlin",
  "week_rollover_hour": 4,
  "users": {
    "hill": { "display": "hill", "ntfy_topic": "some-secret-topic", "token_file": "hill.token" }
  }
}
```

Open `/#<user>` for a profile, `/` for the leaderboard. The leaderboard week runs Monday to Sunday in `week_timezone` and turns over at `week_rollover_hour` for everyone at once; each Anki day counts towards the week it started in. Streaks, quests and "today" still follow each player's own Anki day.

## Getting reviews in

Both clients upload new review rows after each answer and show XP feedback while reviewing. Sync itself can stay on AnkiWeb.

- AnkiDroid: install the [fork](https://github.com/float3/Anki-Android/tree/ankiquest) and fill in Settings → ankiquest.
- Desktop: zip the contents of `addon/` into `ankiquest.ankiaddon`, open it with Anki, then set `url`, `user` and `token` under Tools → Add-ons → ankiquest → Config.

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

## Deck completion notifications

Notifications are off by default for every deck. To set them up, send your deck list once: in AnkiDroid open **Settings → ankiquest → Deck completion notifications**, on desktop use **Tools → ankiquest deck notifications…**, which also opens your dashboard profile. There, choose **Manage deck notifications**, enter your upload token, enable the decks you want to share and pick the people to notify.

After you review at least one card in a deck and finish its scheduled work for the day, selected people receive a message such as “cerro has finished their Spanish studies for today.” A parent deck includes its subdecks. Daily limits are respected, and learning cards due later that day still count as unfinished work. Each deck is announced at most once per Anki day, using your Anki day rollover, even across retries or a server restart. Turning sharing on after finishing a deck does not send a retrospective announcement.

Recipients receive announcements through the updated AnkiDroid client's background notification checks (roughly every 15 minutes, subject to Android's background limits), and through their configured ntfy topic when available (the server checks every 20 seconds). Deck names and recipient preferences are private to the authenticated player; only selected recipients receive the completion message. The dashboard keeps the token only while the settings window is open.

The server and the client used to study must both be updated. Clients only report progress for decks you share, so players who never enable a deck send no deck data. Sending the deck list again replaces the stored one, which removes deleted decks.

Clients may include a `decks` array in the review upload. Each entry has `id` (a string), `name`, `remaining`, `reviewed_today`, and `day` (the local Anki day number, days since the Unix epoch after applying timezone and rollover). Omit this field when a reliable snapshot is unavailable. Initial silent uploads populate the deck list without announcing completions. Set `"catalog": true` when `decks` is the full deck list; decks missing from it are removed.

`GET /api/decks/<user>` with the user's bearer token returns private deck preferences and available recipients. `POST` to the same endpoint accepts `{"decks":[{"id":"123","enabled":true,"recipients":["hill"]}]}`. `GET /api/notifications/<user>` with the recipient's bearer token returns their recent completion announcements, with `id`, `title`, `body`, `day`, and `created_at` (Unix seconds). These endpoints never expose another player's deck settings or notification inbox without that player's token.

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
