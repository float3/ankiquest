# Agent instructions

ankiquest is the AnkiQuest server (Rust), its web pages (TypeScript) and the Anki desktop add-on (Python). The Android app lives in [float3/AnkiQuest-Android](https://github.com/float3/AnkiQuest-Android). These rules apply to every agent (Claude, Codex, others) and to people too.

## Ownership

You are the owner and long-term maintainer of this codebase, and its design is up to you.

Act like it: consolidate duplicated code instead of copying it, fix what you find instead of only reporting it, and leave the code better than you found it. List every change, including small ones, in the PR description; reviewers read the diff against it.

## Bugs: test first

1. Write a regression test that reproduces the bug.
2. Run it and watch it fail for the expected reason.
3. Fix the bug.
4. Run it again and watch it pass, together with the rest of the suite.

Say in the PR that the test failed before the fix. Never skip, disable or loosen a test to get CI green.

## Languages

Write new code in Rust, or TypeScript for the web, when the platform allows it. Use other languages only where the host demands them: Kotlin for the Android app, Python for the Anki desktop add-on. Do not add new Python tools or scripts; when you change an existing one substantially, port it to Rust.

## Code

- Name values: extract recurring or meaningful values into constants or enums; values from a spec (HTTP status codes, for example) always get a constant. Keep obvious one-off values inline.
- Return early; avoid deep nesting.
- Always use braces, even for one-line `if`s.
- Short function names, under 30 characters.
- Enums instead of booleans for function parameters.
- Blank lines between logical blocks, and a short comment saying what a block does and why.
- Keep members private. Widening visibility (private to internal or public) needs the owner's explicit approval; link to it in the PR. Never claim an approval you can't link.
- Layer the code: low-level mechanics (HTTP, database, files, platform APIs) sit behind an abstraction, and each layer talks only to the one directly below it.
- Prefer targeted edits over rewriting whole files.

## Git and pull requests

- Start each session with `git fetch` and work from the latest default branch.
- Never push to the default branch; open a pull request and merge only when every check is green on its final commit.
- Commit messages: subject in the imperative, capitalised, at most 50 characters (72 hard limit), no full stop; a blank line; a body wrapped at 72 characters that explains what and why, not how.
- Disclose AI help with an `Assisted-by: <tool and version>` trailer.
- Never commit secrets, tokens or personal data. Secrets live on the servers, outside the repositories.

## Writing

Write comments, commit messages, issues and PR descriptions with as few words as needed. No praise, no filler, no superlatives. Em dashes take no spaces around them.

Mannered prose substitutes metaphor and flourish for direct statement. Instead of "a parameter worth varying," the mannered writer produces "a dial worth turning." Instead of "this point still matters," they write "this point earns its keep." The phrases exist to display the writer, not to convey the idea, and readers can tell. That is why mannered prose irritates: it makes the reader work harder so the writer can perform. It is also imprecise. Metaphors drag in connotations the writer did not choose and cannot control. The fix is to say what you mean. When a literal phrase is available, use it.

## This repository

- **Layout.** `src/` and `crates/`: the Rust server. `web/`: TypeScript for the pages in `static/`. `addon/`: the Anki desktop add-on, Python because Anki requires it. `tests/`: add-on tests (Python) and web tests (TypeScript). `tools/export_android_art.py` is the last Python tool; port it to Rust the next time it changes.
- **API examples are a contract.** `docs/api/examples/*.json` and `tests/fixtures/*.json` are copied into the Android app at a pinned commit (float3/AnkiQuest-Android#203). Adding an example is fine: Android picks it up through its bump PR. Never remove or reshape an example released apps read (#230); retire it only after Android stops reading it. Document API changes in `docs/mobile-api.md`.
- **Released apps talk to the deployed server.** Keep old endpoints and fields working until no supported client uses them.
- **Deployment** is the NixOS module in `flake.nix`, used from a separate NixOS configuration. Secrets are files on the hosts (passed with systemd `LoadCredential`), never values in this repository.
- **Checks to run** before pushing, for what you changed: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm run typecheck`, `npm run test:web`, `python -m unittest discover -s tests`. Leave the full matrix to CI.
