AnkiQuest needs Anki 23.10 or newer with Qt 6.

An unconnected profile offers sign-in when it opens. **Not now** dismisses the offer for that profile. You can sign in later from **Tools → AnkiQuest: sign in or create account…** or the link under the deck list.

The sign-in dialog starts with the main AnkiQuest server. If you self-host, enter your server address. Use HTTPS for a server outside your local network. After sign-in the add-on keeps a token for this computer.

Open **Tools → ankiquest settings…** to change your preferences. **Manage account on the website…** opens the account settings for your server, including devices, sign-in methods and your data. **Manual connection** contains the server, player and token fields and the connection check.

- `notify_rank`: say something when your place on the leaderboard changes
- `streak_hours`: warn you this many hours before your streak ends (0 turns it off)
- `update_channel`: `stable` for `addon-<n>` releases, `nightly` for the build of every change (untested)

once a day, and from **Tools → AnkiQuest: check for updates…**, the add-on looks for a newer build on that channel and offers to install it; restart Anki afterwards. copies not installed from a release package never update themselves.

a line under the deck list shows your weekly place, level and streak. its links and **Tools → AnkiQuest on the web…** open the website in a window inside Anki, already signed in with your token, where you can see the leaderboard, reply to messages and join challenges.

review log rows (card id, timestamp, previous and new interval, time taken, review type) and deck progress (deck IDs, names, daily remaining counts and review counts) are sent to your server, never card content. deck names and progress are available privately through your upload token.

when a card you forgot at least four times finally earns an interval of three weeks or more, a 🏆 banner says so, and when you leave the reviewer a big card opens where you can share it or tell your friends. it can also show what the card was: the add-on fills in the card's first field for you to edit, and nothing about the card is sent unless you choose **Show it on the card**.

to share daily deck completions, open **Tools → AnkiQuest deck notifications…**, tick the decks you want to share and the people who should hear about them. ticking a deck ticks its subdecks. sharing is off by default. recipients receive at most one completion message per deck per Anki day; later learning steps still count as unfinished work.

**Tools → AnkiQuest inbox…** (Ctrl+Shift+M) lists your recent messages in a window: reply with a quick pick or your own words, mark them read, open them on the website, share your own wins, and cheer a friend's shared win with 🎉; their card picture shows in the row. when a poll announces a win of yours (a celebration, a finished challenge, a weekly recap or a climb into the top three) and celebrations are on, a small window offers to share it with your friends, to copy or save the server's card picture (this publishes nothing), or to copy a public link with an invitation (anyone with the link can see the card; remove it from the dashboard's Today page). the settings window says, for each kind of win, whether only you see it, you are asked, or friends are told automatically, and how many automatic shares each friend gets per day.

with a server set, the block under the deck list is a Today panel: your companion's mood, streak and level, the next deck with a Study button, today's streak minimum and daily goal, the daily quests, a top-5 leaderboard for the period you pick (your own row is always there), and links to Today, History, Reminders and the website's settings.

the daily goal lives on the server, so every device agrees. pick it with **Daily goal…** in the settings window or **Change goal** on the panel: clear your overdue backlog in a number of days (on top of what is due), a number of cards or minutes a day, or no goal. the dialog estimates each choice from your own collection: cards and time today, and when the backlog would be gone. it opens once by itself while you have not picked a goal. a goal from an older version of the add-on moves to the server the first time.

- `goal_setup_shown`: the accounts the goal dialog already opened for by itself
- `deck_status`: mark decks in the deck list as done today, learning later today, or ready
- `session_summary`: show a recap with your reviews, time and cards left when you leave a study session
- `goal_celebrations`: once a day, celebrate when today counts for your streak and when you reach your daily goal; that popup replaces the recap for that session. a server with wins celebrates the daily goal itself (it arrives like your other celebrations), so the add-on then only celebrates the streak
- `review_heatmap`: show the review heatmap under the deck list, as AnkiQuest for Android does: a year of reviews per day, the next 90 days' forecast and your streaks. it works without an account; with one, the days ankiquest judged are coloured by its verdict. click a day to open its cards in the browser

to give friends a copy of a deck, open **Tools → AnkiQuest: send a deck to friends…**, pick the deck and the friends. they get an independent `.apkg` with cards, notes, media and deck options, never your review history; copies over 25 MB are refused, so use Anki's normal export for those. decks friends send you wait under **Tools → AnkiQuest: decks offered to me…** (the number shows how many) for 30 days, where **Import** opens Anki's usual import dialog and **Dismiss** removes the offer.

the settings window also switches streak protection on or off, turns all deck completion alerts you receive on or off, unsubscribes you from specific people (people you muted before show as unsubscribed), and shows when reviews were last uploaded. these choices live on the server and apply across your devices.

**Ctrl+Shift+K** opens the ankiquest website window. with a server and player but no token, reviews are only previewed: you still see the XP feedback if the server shows that player publicly, and nothing is stored.

For a manual connection, set `url` to your server address, `user` to your player name and `token` to your upload token. When the server advertises a different main HTTPS address, the add-on switches after verifying your token there.
