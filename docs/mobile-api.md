# member sessions, activity, and challenge events

these additions preserve bearer authentication and the existing notification array. XP, challenges' progress rules, review scheduling, streaks, and scoring are unchanged.

## browser identity

`POST /auth/session` still returns `204` with the `ankiquest_session` cookie. a valid `Authorization: Bearer <member-token>` establishes that member's session. browser JSON `{ "password": "..." }` accepts either the shared site password or a member token and requires `X-Ankiquest-CSRF: 1`. a shared-password session can read shared pages but has no personal permissions. if a secret is both the shared password and a member token, password login remains read-only. a token configured for multiple members cannot implicitly select a browser owner; existing explicit bearer requests retain their previous per-user behavior.

the cookie is HttpOnly, SameSite=Lax, scoped to `/`, and Secure when the configured public URL uses HTTPS. sessions expire after seven days, rotate on sign-in, are revoked by logout, and disappear when the server restarts. tokens never appear in status responses.

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

counts cover all items within the selected window, independently of pagination, push state, and any Android delivery cursor. `unread_count` counts rows with no read timestamp. `action_count` counts still-pending challenge invitations whose challenge is neither cancelled nor past its deadline. reading an invitation does not accept it. a 30-day view counts its own 30-day window; use the default 90-day view for the full retained inbox. follow `next_before` until null to load more.

`POST /api/activity/{user}/read` accepts `{ "ids": [42, 43] }` and returns `204`. at most 200 positive IDs per request; clients mark larger selections in batches. duplicate IDs/retries are safe and preserve the first read timestamp. unknown, expired, push-only, and other members' IDs have no effect. the caller must own `{user}`. read state survives restart and changes neither push acknowledgement nor legacy delivery state.

the server physically retains notifications for 90 days; history already deleted by an older server cannot be reconstructed. the legacy `GET /api/notifications/{user}` remains an oldest-first array of up to 500 notifications from the last seven days, with its existing reminder delivery checks. it gains the four fields `read_at`, `challenge_id`, `route`, and `action_required`; old clients can ignore them. push delivery eligibility also stays seven days. Push-only game events remain excluded from both personal inbox endpoints. new clients can fall back to the legacy endpoint after an Activity `404`, with no inferred unread badge.

## challenge lifecycle

new event kinds are `challenge_invite`, `challenge_accepted`, and `challenge_complete`. the stable ID and relative route above are attached to each event. only selected recipients receive invitations, only the creator receives acceptance events, and only accepted members receive completion events. no challenge creation broadcasts to unrelated members. creation/membership changes, event deduplication, and notification insertion commit or roll back together. completion is reconciled after uploads, in the regular server poll, and before personal challenge/inbox reads; completion notices are deduplicated across retries and restarts. progress continues to use only reviews after joining and before the original deadline. a completion notice records that the goal was observed complete; later review undo or member departure can still alter live progress under the existing rules.

`POST /api/community/challenges/{user}` accepts optional `request_id`, an 8–128 character string of ASCII letters, numbers, hyphens, and underscores. new clients generate a fresh UUID for each deliberate creation and reuse it only when retrying that creation. a repeated key with the same normalized title, kind, cooperative flag, target, duration, and recipient set returns the existing challenge list without another challenge or event. reusing it with different data returns `400`. keys are scoped to the creator. older clients may omit the key and retain their original create behavior.

repeated successful accept, decline, leave, and creator cancel operations are idempotent. accept retries never reset `joined_at`. accepting or declining an invitation also marks that invitation read. acting as a nonmember is forbidden; all challenge endpoints still require the URL owner's credential. clients should reload challenge details before presenting available actions, since Activity can outlive an invitation, membership, or the challenge deadline.
