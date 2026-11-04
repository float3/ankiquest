# API examples

Real responses from the server's handlers for the endpoints the Android app uses. `src/api_examples.rs` builds a small server with fixed data and a clock pinned to Wednesday 28 October 2026, 10:30 UTC, calls each route and compares the answer with these files, so `cargo test` fails when a response changes shape.

Regenerate after an intended change and commit the result:

```sh
UPDATE_API_EXAMPLES=1 cargo test api_examples
```

Random values are replaced, numbered in order of appearance: device tokens by `example-token-N`, win link tokens by `example-link-N`, device ids by `example-device-N`, group invite codes by `example-invite-N`, app sign-in tickets by `example-ticket-N`.

| file | request |
|---|---|
| `capabilities.json` | `GET /api/capabilities` |
| `sign_up.json` | `POST /api/accounts` |
| `sign_in.json` | `POST /api/accounts/tokens` |
| `devices.json` | `GET /api/devices/{user}` |
| `upload.json` | `POST /api/reviews/{user}` |
| `share_to_friends.json` | `POST /api/share/{user}` `{"win", "to": "friends"}` |
| `share_to_link.json` | `POST /api/share/{user}` `{"win", "to": "link"}` |
| `deck_share.json` | `POST /api/decks/{user}/share` |
| `react.json` | `POST /api/react/{user}` |
| `profile.json` | `GET /api/profile/{user}`, owner view with `goal` |
| `leaderboard.json` | `GET /api/leaderboard` |
| `leaderboard_day.json` | `GET /api/leaderboard?period=day` |
| `week.json` | `GET /api/week` |
| `player_profile.json` | `GET /api/profile/{user}` for another player, as the leaderboard opens it: no `goal` |
| `rank.json` | `POST /api/rank/{user}` |
| `notifications.json` | `GET /api/notifications/{user}`: a celebration with `win`, a friend's `share` with `image` and `reacted`, a `reaction` |
| `goal.json` | `GET /api/goal/{user}` |
| `sharing.json` | `GET /api/sharing/{user}` |
| `wins.json` | `GET /api/wins/{user}` |
| `decks.json` | `GET /api/decks/{user}` |
| `companion.json` | `GET /api/companion/{user}` |
| `streak_freezes.json` | `GET /api/streak-freezes/{user}` |
| `challenges.json` | `GET /api/community/challenges/{user}`: an invitation for the player, a goal in progress, one reached, one ended early, and the weekly suggestion |
| `challenge_create.json` | `POST /api/community/challenges/{user}` with a `request_id` |
| `challenge_action.json` | `POST /api/community/challenges/{user}/{id}` `{"action": "accept"}` |
| `mute.json` | `POST /api/social/{user}` `{"action": "mute", "user"}` |
| `group_create.json` | `POST /api/social/{user}` `{"action": "create_group", "name"}`: the new group with its invite code |
| `social.json` | `GET /api/social/{user}`: a friend, a request each way, a muted player, a group the player owns and one they joined |
| `invite.json` | `GET /api/invites/{code}` |
| `social_change.json` | `POST /api/social/{user}` `{"action": "befriend", "user"}`, accepting a request |
| `records.json` | `GET /api/records` |
| `study.json` | `GET /api/study/{user}`: three study days and a day off |
| `sign_in_methods.json` | `GET /api/sign-in/{user}`: a password and a linked GitHub |
| `account.json` | `GET /api/accounts/{user}` |
| `password_change.json` | `POST /api/sign-in/{user}` `{"action": "password", "current", "password", "sign_out_others": false}` |
| `devices_sign_out.json` | `POST /api/devices/{user}` `{"action": "sign_out_others"}` |
| `sign_in_browser.json` | `POST /api/sign-in/{user}` `{"action": "browser", "provider", "purpose": "reauth"}` |

The Android repository's CI fetches these files and parses them with the app's real client code, so a server change that would break the app shows up there before it is deployed.
