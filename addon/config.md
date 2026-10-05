Everything here is also under **Tools → ankiquest settings…**, which is easier to use.

New here? **Tools → ankiquest: sign in or create account…** (or the link under the deck list) signs in with a username and password, or creates an account, and fills in the three settings below. The password is not stored; the add-on keeps only a token for this computer.

- `url`: the ankiquest server, e.g. `https://anki.example.com`
- `user`: your player name
- `token`: your upload token
- `notify_rank`: say something when your place on the leaderboard changes
- `streak_hours`: warn you this many hours before your streak ends (0 turns it off)
- `update_channel`: `stable` for `addon-<n>` releases, `nightly` for the build of every change (untested)

Once a day, and from **Tools → ankiquest: check for updates…**, the add-on looks for a newer build on that channel and offers to install it; restart Anki afterwards. Copies not installed from a release package never update themselves.

A line under the deck list shows your weekly place, level and streak. Its links, **Tools → ankiquest on the web…** and **Tools → ankiquest inbox…** open the website in a window inside Anki, already signed in with your token, where you can see the leaderboard, reply to messages and join challenges.

Review log rows (card id, timestamp, previous interval, time taken, review type) and deck progress (deck IDs, names, daily remaining counts and review counts) are sent to your server, never card content. Deck names and progress are available privately through your upload token.

To share daily deck completions, open **Tools → ankiquest deck notifications…**, tick the decks you want to share and the people who should hear about them. Ticking a deck ticks its subdecks. Sharing is off by default. Recipients receive at most one completion message per deck per Anki day; later learning steps still count as unfinished work.
