everything here is also under **Tools → ankiquest settings…**, which is easier to use.

new here? **tools → ankiquest: sign in or create account…** (or the link under the deck list) signs in with a username and password, or creates an account, and fills in the three settings below. the password is not stored; the add-on keeps only a token for this computer.

- `url`: the ankiquest server, e.g. `https://anki.example.com`
- `user`: your player name
- `token`: your upload token
- `notify_rank`: say something when your place on the leaderboard changes
- `streak_hours`: warn you this many hours before your streak ends (0 turns it off)
- `update_channel`: `stable` for `addon-<n>` releases, `nightly` for the build of every change (untested)

once a day, and from **Tools → ankiquest: check for updates…**, the add-on looks for a newer build on that channel and offers to install it; restart Anki afterwards. copies not installed from a release package never update themselves.

a line under the deck list shows your weekly place, level and streak. its links and **Tools → ankiquest on the web…** open the website in a window inside Anki, already signed in with your token, where you can see the leaderboard, reply to messages and join challenges.

review log rows (card id, timestamp, previous and new interval, time taken, review type) and deck progress (deck IDs, names, daily remaining counts and review counts) are sent to your server, never card content. deck names and progress are available privately through your upload token.

when a card you forgot at least four times finally earns an interval of three weeks or more, a 🏆 banner says so, and when you leave the reviewer a big card opens where you can share it or tell your friends. it can also show what the card was: the add-on fills in the card's first field for you to edit, and nothing about the card is sent unless you choose **Show it on the card**.

to share daily deck completions, open **Tools → ankiquest deck notifications…**, tick the decks you want to share and the people who should hear about them. ticking a deck ticks its subdecks. sharing is off by default. recipients receive at most one completion message per deck per Anki day; later learning steps still count as unfinished work.

**Tools → ankiquest inbox…** (Ctrl+Shift+M) lists your recent messages in a window: reply with a quick pick or your own words, mark them read, open them on the website, share your own wins, and cheer a friend's shared win with 🎉; their card picture shows in the row. when a poll announces a win of yours (a celebration, a finished challenge, a weekly recap or a climb into the top three) and celebrations are on, a small window offers to share it with your friends, or as the server's card picture with a link and an invitation. the settings window says, for each kind of win, whether only you see it, you are asked, or friends are told automatically, and how many automatic shares each friend gets per day.

with a server set, the block under the deck list is a Today panel: your companion's mood, streak and level, the next deck with a Study button, today's streak minimum and daily goal, the daily quests, a top-5 leaderboard for the period you pick (your own row is always there), and links to Today, History, Reminders and the website's settings.

the daily goal lives on the server, so every device agrees. pick it with **Daily goal…** in the settings window or **Change goal** on the panel: clear your overdue backlog in a number of days (on top of what is due), a number of cards or minutes a day, or no goal. the dialog estimates each choice from your own collection: cards and time today, and when the backlog would be gone. it opens once by itself while you have not picked a goal. a goal from an older version of the add-on moves to the server the first time.

- `goal_setup_shown`: the accounts the goal dialog already opened for by itself
- `deck_status`: mark decks in the deck list as done today, learning later today, or ready
- `session_summary`: show a recap with your reviews, time and cards left when you leave a study session
- `goal_celebrations`: once a day, celebrate when today counts for your streak and when you reach your daily goal; that popup replaces the recap for that session

to give friends a copy of a deck, open **Tools → ankiquest: send a deck to friends…**, pick the deck and the friends. they get an independent `.apkg` with cards, notes, media and deck options, never your review history; copies over 25 MB are refused, so use Anki's normal export for those. decks friends send you wait under **Tools → ankiquest: decks offered to me…** (the number shows how many) for 30 days, where **Import** opens Anki's usual import dialog and **Dismiss** removes the offer.

the settings window also switches streak protection on or off, turns all deck completion alerts you receive on or off, unsubscribes you from specific people (people you muted before show as unsubscribed), and shows when reviews were last uploaded. these choices live on the server and apply across your devices.

**Ctrl+Shift+K** opens the ankiquest website window. with a server and player but no token, reviews are only previewed: you still see the XP feedback if the server shows that player publicly, and nothing is stored.
