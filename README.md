# AnkiQuest

XP, streaks, daily quests, friends and leaderboards for Anki. Only the timing of your reviews is sent, never the content of your cards.

## Use it

1. Create an account on an AnkiQuest server, such as [ankiquest.rationality-munich.com](https://ankiquest.rationality-munich.com).
2. Connect Anki:
   - **Desktop:** download `ankiquest.ankiaddon` from the [latest release](https://github.com/float3/ankiquest/releases/latest), open it with Anki, then choose **Tools → ankiquest: sign in or create account…**
   - **Android:** install `AnkiDroid-Quest.apk` from the [latest release](https://github.com/float3/AnkiQuest-Android/releases/latest). It installs next to AnkiDroid; sync your collection through AnkiWeb as usual. Then open **Settings → ankiquest → Account & connection → Sign in or create account**.
3. Study as usual. Add friends, or start a group and share its invite link, on the website's **Friends** page.

## Run your own server

```sh
rustup target add wasm32-unknown-unknown
npm ci
cargo run -- ankiquest.json
```

```json
{ "addr": "127.0.0.1:8097", "state_dir": "state", "registration": true }
```

On NixOS, import `inputs.ankiquest.nixosModules.default` and set `services.ankiquest = { enable = true; domain = "anki.example.com"; registration = true; };`.

Configuration, administration, the API and development: [docs/server.md](docs/server.md).

## License

AGPL-3.0-only.
