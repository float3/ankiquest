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

**Tools → ankiquest inbox…** (Ctrl+Shift+M) lists your recent messages in a window: reply with a quick pick or your own words, mark them read, open them on the website, or share your own wins. when a poll announces a win of yours (a celebration, a finished challenge, a weekly recap or a climb into the top three) and celebrations are on, a small window offers to share it with your friends or as a picture with an invitation to your server.
