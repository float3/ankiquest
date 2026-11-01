# API examples

Real responses from the server's handlers for the endpoints the Android app uses. `src/api_examples.rs` builds a small server with fixed data and a clock pinned to Wednesday 28 October 2026, 10:30 UTC, calls each route and compares the answer with these files, so `cargo test` fails when a response changes shape.

Regenerate after an intended change and commit the result:

```sh
UPDATE_API_EXAMPLES=1 cargo test api_examples
```

Random values are replaced, numbered in order of appearance: device tokens by `example-token-N`, win link tokens by `example-link-N`, device ids by `example-device-N`.

| file | request |
|---|---|
| `capabilities.json` | `GET /api/capabilities` |
| `sign_up.json` | `POST /api/accounts` |
| `sign_in.json` | `POST /api/accounts/tokens` |
| `devices.json` | `GET /api/devices/{user}` |
| `upload.json` | `POST /api/reviews/{user}` |
| `share_to_friends.json` | `POST /api/share/{user}` `{"win", "to": "friends"}` |
| `react.json` | `POST /api/react/{user}` |
| `profile.json` | `GET /api/profile/{user}`, owner view with `goal` |
| `leaderboard.json` | `GET /api/leaderboard` |
| `rank.json` | `POST /api/rank/{user}` |
| `notifications.json` | `GET /api/notifications/{user}`: a celebration with `win`, a friend's `share` with `image` and `reacted`, a `reaction` |
| `goal.json` | `GET /api/goal/{user}` |
| `sharing.json` | `GET /api/sharing/{user}` |
| `wins.json` | `GET /api/wins/{user}` |
| `decks.json` | `GET /api/decks/{user}` |
| `companion.json` | `GET /api/companion/{user}` |
| `streak_freezes.json` | `GET /api/streak-freezes/{user}` |

The Android repository's CI fetches these files and parses them with the app's real client code, so a server change that would break the app shows up there before it is deployed.
