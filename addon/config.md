- `url`: the ankiquest server, e.g. `https://anki.example.com`
- `user`: your player name
- `token`: your upload token

Review log rows (card id, timestamp, previous interval, time taken, review type) and deck progress (deck IDs, names, daily remaining counts and review counts) are sent to your server, never card content. Deck names and progress are available privately through your upload token.

To share daily deck completions, open your profile on the ankiquest website and choose **Manage deck notifications**. Enter your upload token, enable each deck you want to share, and select its recipients. Sharing is off by default. Recipients receive at most one completion message per deck per Anki day; later learning steps still count as unfinished work.
