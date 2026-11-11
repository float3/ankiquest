# AnkiWeb listing draft

Prepared for the publishing owner. This is not a published AnkiWeb listing.
Use the checked `.ankiaddon` artifact from the intended release commit and
upload the screenshots below. Record the resulting listing ID in the
compatibility documentation before directing users to install it.

## Title

AnkiQuest — study progress, streaks and friends

## Description

Give your regular Anki study sessions a little company. AnkiQuest adds a
study companion, XP, streaks, daily goals, quests and optional social features
to Anki. It works with Anki 23.10 or newer using Qt 6.

- **Today:** see your study progress, next deck, streak status and daily goal
  under the deck list.
- **Progress:** browse your study history and records in a native window.
- **Moments:** celebrate goals and difficult cards after studying. Suggested
  card labels stay on your computer until you choose to save one.
- **Friends:** manage friends and groups, start friend goals, send messages
  and choose whether to appear on the global leaderboard.
- **Reminders:** choose quiet hours and optional system notifications.

After installing, restart Anki and open **Tools → AnkiQuest → Settings →
Account** to sign in or create an AnkiQuest account. You can use the offered
server or your own. AnkiQuest does not replace AnkiWeb synchronization.

AnkiWeb manages updates for this installation. Use **Tools → Add-ons → Check
for Updates**. Do not also install the GitHub package into the same Anki
profile; remove the other installation first. Keep a collection backup and
the same AnkiQuest account when changing installation source.

[Source and support](https://github.com/float3/ankiquest) ·
[Android companion](https://github.com/float3/AnkiQuest-Android) ·
[Self-hosting guide](https://github.com/float3/ankiquest/blob/master/docs/install.md)

## Privacy note

When connected, the add-on uploads review timestamps, card IDs, old and new
intervals, answer time and review type, along with your Anki day settings.
It also uploads deck IDs, names and progress counts to your chosen server.
These are used for progress, XP, streaks, goals and optional deck-completion
sharing. Ordinary review uploads do not contain card text, images or audio.

Your computer keeps a device token after sign-in. The password is not saved
by the add-on. Card labels are sent only when you save them; sending a deck
copy explicitly includes its cards, notes and media. Sharing a public win
link makes that win accessible to anyone with the link.

New profiles are public and listed on the global leaderboard by default.
Review **Public profile** in Friends and **Show me on the global leaderboard**
in Settings → Sharing to choose your visibility. Other players outside your
circle see limited leaderboard rows; a public profile also exposes its
public progress page. Your server's privacy notice explains its operator,
hosting and retention. Open it from **Settings → App and privacy → Privacy
notice**. The website's account settings let you export your data or delete
your account.

## Screenshots and captions

**Today:** your companion, streak status, next deck and progress in Anki.

![AnkiQuest Today](screenshots/desktop-today-light.png)

**Progress:** a native history window with the server's study-day states.

![AnkiQuest Progress](screenshots/desktop-progress.png)

**Inbox:** messages and actions in a native window.

![AnkiQuest Inbox](screenshots/desktop-inbox.png)

## Publication handoff

The publishing owner still needs to choose the AnkiWeb account, create the
listing, upload the verified package and screenshots, and provide its numeric
ID. Then test an AnkiWeb installation and its update path, add package
conflicts for the known ID, and link the live listing from the README.
