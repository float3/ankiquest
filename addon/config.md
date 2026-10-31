everything here is also under **Tools → ankiquest settings…**, which is easier to use.

new here? **tools → ankiquest: sign in or create account…** (or the link under the deck list) signs in with a username and password, or creates an account, and fills in the three settings below. the password is not stored; the add-on keeps only a token for this computer.

- `url`: the ankiquest server, e.g. `https://anki.example.com`
- `user`: your player name
- `token`: your upload token
- `notify_rank`: say something when your place on the leaderboard changes
- `streak_hours`: warn you this many hours before your streak ends (0 turns it off)
- `update_channel`: `stable` for `addon-<n>` releases, `nightly` for the build of every change (untested)

once a day, and from **Tools → ankiquest: check for updates…**, the add-on looks for a newer build on that channel and offers to install it; restart Anki afterwards. copies not installed from a release package never update themselves.

a line under the deck list shows your weekly place, level and streak. its links, **Tools → ankiquest on the web…** and **Tools → ankiquest inbox…** open the website in a window inside Anki, already signed in with your token, where you can see the leaderboard, reply to messages and join challenges.

review log rows (card id, timestamp, previous and new interval, time taken, review type) and deck progress (deck IDs, names, daily remaining counts and review counts) are sent to your server, never card content. deck names and progress are available privately through your upload token.

when a card you forgot at least four times finally earns an interval of three weeks or more, a 🏆 banner says so, and when you leave the reviewer a big card opens where you can share it or tell your friends. it can also show what the card was: the add-on fills in the card's first field for you to edit, and nothing about the card is sent unless you choose **Show it on the card**.

to share daily deck completions, open **Tools → ankiquest deck notifications…**, tick the decks you want to share and the people who should hear about them. ticking a deck ticks its subdecks. sharing is off by default. recipients receive at most one completion message per deck per Anki day; later learning steps still count as unfinished work.

with a server set, the block under the deck list is a Today panel: your companion's mood, streak and level, the next deck with a Study button, today's streak minimum and daily goal, the daily quests, a top-5 leaderboard for the period you pick (your own row is always there), and links to Today, History, Reminders and the website's settings.

- `goal_mode`: `backlog` (cards due today plus a share of the overdue backlog), `cards`, `minutes`, or `off`
- `goal_backlog_percent`: the share of the backlog to work off each day (5, 10, 20, 34 or 100)
- `goal_cards`, `goal_minutes`: the daily goal for the `cards` and `minutes` modes
- `deck_status`: mark decks in the deck list as done today, learning later today, or ready
- `session_summary`: show a recap with your reviews, time and cards left when you leave a study session
- `goal_celebrations`: once a day, celebrate when today counts for your streak and when you reach your daily goal; that popup replaces the recap for that session
