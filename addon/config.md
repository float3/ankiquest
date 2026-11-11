AnkiQuest needs Anki 23.10 or newer with Qt 6.

An unconnected profile offers sign-in when it opens. **Not now** dismisses the offer for that profile. You can sign in later from **Tools → AnkiQuest → Settings → Account** or the link under the deck list.

The sign-in dialog starts with the main AnkiQuest server. If you self-host, enter your server address. Use HTTPS for a server outside your local network. After sign-in the add-on keeps a token for this computer.

Use **Switch account…** or **Sign out** in **Tools → AnkiQuest → Settings**. Signing out clears the saved connection and account data shown in the add-on. Your upload checkpoint stays with that server and account in this Anki profile, so signing back in continues with new reviews.

Open **Tools → AnkiQuest → Settings** to change your preferences. **Manage account on the website…** opens the account settings for your server, including devices, sign-in methods and your data. **Manual connection** contains the server, player and token fields and the connection check.

The settings dialog has four groups and scrolls within the window:

- **Account:** sign in, switch accounts, sign out, choose your study companion and manage your account on the website. **Data** holds the last upload status and **Upload everything again**. This recovery action quietly replays your history; ordinary uploads resume from their saved checkpoint.
- **Notify me about:** rank changes, streak warnings, local goal celebrations, server nudges and celebrations, deck completion alerts and unsubscriptions.
- **Sharing:** choose decks and people for completion notifications, set who hears about each kind of win, and choose whether you appear on the global leaderboard. This switch reads the server's current choice; it is hidden on older servers without the field.
- **App and privacy:** update channel, deck markers, session recap, review heatmap, daily goal, streak protection and the server's privacy notice.

Server choices follow your account across devices. Only changed choices are sent when you save. After switching or editing the connection, save and reopen settings to load that account's choices.

The local options below match `config.json`. Account choices stay on the server; internal bookkeeping keys are not user preferences.

- `notify_rank`: say something when your place on the leaderboard changes
- `streak_hours`: warn you this many hours before your streak ends (0 turns it off)
- `update_channel`: `stable` for `addon-<n>` releases, `nightly` for the build of every change (untested)

once a day, and from **Tools → AnkiQuest → Check for updates**, the add-on looks for a newer build on that channel and offers to install it; restart Anki afterwards. copies not installed from a release package never update themselves.

a line under the deck list shows your weekly place, level and streak. its links and **Tools → AnkiQuest → Website** open the website in a window inside Anki, already signed in with your token, where you can see the leaderboard, reply to messages and join challenges.

review log rows (card id, timestamp, previous and new interval, time taken, review type) and deck progress (deck IDs, names, daily remaining counts and review counts) are sent to your server, never card content. deck names and progress are available privately through your upload token.

when a card you forgot at least four times finally earns an interval of three weeks or more, it joins your moments. after studying, its native card shows lapses, answers and days since the first answer. the label suggestion comes from the rendered front, with markup and sounds removed, or a readable note field. it stays on your computer until **Save label**; **Remove label** deletes the saved label. **Tell friends** shares the conquest, and **Undo** is available while the server allows it. a friend’s reply or cheer can close that window early.

to share daily deck completions, open **Tools → AnkiQuest → Settings → Sharing → Deck completion notifications**, tick the decks you want to share and the people who should hear about them. ticking a deck ticks its subdecks. sharing is off by default. recipients receive at most one completion message per deck per Anki day; later learning steps still count as unfinished work.

**Tools → AnkiQuest → Inbox** (Ctrl+Shift+M) lists your recent messages in a window: reply with a quick pick or your own words, mark them read, open them on the website, share your own wins, and cheer a friend's shared win with 🎉; their card picture shows in the row.

your wins wait in a queue for this account and profile. they appear after studying in one card with **and N more**, together with the session recap and the once-per-day streak cue. starting another review defers the card; dismissing it marks those moments shown. later arrivals wait for the next session or **Your moments** on Today. routine quiet wins add to that badge, except the daily streak cue. celebrations switched off remain available through the badge.

**Share** offers text, or the server's picture when ready. copying or saving a picture publishes nothing; **Copy public link** creates a page anyone with the link can see (remove it from the website’s Today page). conquered cards can **Check for picture** while the server prepares their win. their share text uses the saved label, deck and page in your own voice. sharing choices in Settings say which wins stay private, ask you first, or automatically reach friends.

with a server set, the block under the deck list is a Today panel: your companion's mood, streak and level, the next deck with a Study button, today's streak minimum and daily goal, the daily quests, a top-5 leaderboard for the period you pick (your own row is always there), and links to Today, History, Reminders and the website's settings.

Today also shows saved freezes, a freeze earned today and your latest weekly recap for seven days, with one sentence per line. An AnkiQuest entry in the top toolbar and beside Browse's search shows the current streak and day state; click it to return to Today. If the server is unreachable, Today keeps the cached profile, explains what is waiting, and offers **Retry connection**.

**History** and **View progress** open a native Progress window: choose a year of uploaded study days or inspect records in the other tab. Each day keeps the server's met, short, frozen, skipped or no-study label. Refresh retries a failed read; **Open website** remains available for older servers. Changing accounts or closing the Anki profile closes the window and discards late replies.

the daily goal lives on the server, so every device agrees. pick it with **Change goal** on Today: clear your overdue backlog in a number of days (on top of what is due), a number of cards or minutes a day, or no goal. the dialog estimates each choice from your own collection: cards and time today, and when the backlog would be gone. it opens once by itself while you have not picked a goal. a goal from an older version of the add-on moves to the server the first time.

- `deck_status`: mark decks in the deck list as done today, learning later today, or ready
- `session_summary`: show a recap with your reviews, time and cards left when you leave a study session
- `goal_celebrations`: add the once-per-day streak and goal cues to the moments card, with the session recap below them. a server with wins supplies the daily goal moment; the add-on supplies the streak cue
- `review_heatmap`: show the review heatmap under the deck list, as AnkiQuest for Android does: a year of reviews per day, the next 90 days' forecast and your streaks. it works without an account; with one, the days ankiquest judged are coloured by its verdict. click a day to open its cards in the browser

to give friends a copy of a deck, open **Tools → AnkiQuest → Send a deck**, pick the deck and the friends. they get an independent `.apkg` with cards, notes, media and deck options, never your review history; copies over 25 MB are refused, so use Anki's normal export for those. decks friends send you wait under **Tools → AnkiQuest → Decks offered to me** (the number shows how many) for 30 days, where **Import** opens Anki's usual import dialog and **Dismiss** removes the offer.

the settings window also switches streak protection on or off, turns all deck completion alerts you receive on or off, unsubscribes you from specific people (people you muted before show as unsubscribed), and shows when reviews were last uploaded. these choices live on the server and apply across your devices.

**Ctrl+Shift+K** opens the ankiquest website window. with a server and player but no token, reviews are only previewed: you still see the XP feedback if the server shows that player publicly, and nothing is stored.

For a manual connection, set `url` to your server address, `user` to your player name and `token` to your upload token. When the server advertises a different main HTTPS address, the add-on switches after verifying your token there.
