# Install and deploy the server

AnkiQuest accepts review uploads from the desktop add-on and Android app. It
does not replace AnkiWeb or an Anki sync server. Start with a local instance to
check the build; use the NixOS module below for a managed HTTPS service.

## Build and run locally

You need Git, Rust 1.88 or newer, a C compiler, and Node.js 22 with npm. Cargo
builds SQLite from source by default. The website and translations are embedded
in the server binary; there is no separate web server build to deploy.

```sh
git clone https://github.com/float3/ankiquest.git
cd ankiquest
rustup target add wasm32-unknown-unknown
npm ci
cargo build --release --locked
```

Create `ankiquest.json` in that directory:

```json
{
  "addr": "127.0.0.1:8097",
  "state_dir": "state",
  "registration": true
}
```

Then start the server in the foreground:

```sh
./target/release/ankiquest ankiquest.json
```

Open <http://127.0.0.1:8097> and create an account. From another terminal, check
the worker and database:

```sh
curl --fail http://127.0.0.1:8097/api/health
```

A healthy response has `"ok": true`. The state directory is created on startup;
relative paths in the configuration are relative to the process's working
directory. Use absolute paths when running a service. The config path can also
be supplied through `ANKIQUEST_CONFIG`; without either it defaults to
`ankiquest.json`.

This example listens only on loopback. For other devices, use an HTTPS address
and configure each client's server URL. Keep Anki collection sync configured
separately. See [client setup](../README.md#use-it).

## Deploy on NixOS

The flake provides the package and `nixosModules.default`. Add the input to
your existing system flake, pass `inputs` to the host module through
`specialArgs`, and import the module there:

```nix
# In the system flake's inputs:
ankiquest.url = "github:float3/ankiquest";

# In the host module, with inputs in its arguments:
imports = [ inputs.ankiquest.nixosModules.default ];
```

Configure the host, replacing the example domain and email:

```nix
services.ankiquest = {
  enable = true;
  domain = "anki.example.com";
  registration = true;
};

services.nginx.enable = true;
security.acme = {
  acceptTerms = true;
  defaults.email = "admin@example.com";
};
networking.firewall.allowedTCPPorts = [ 80 443 ];
```

Point the domain's DNS records at this host and allow ports 80 and 443 in any
external firewall too. The module creates the nginx virtual host, obtains its
ACME certificate, redirects HTTP to HTTPS, and proxies to a systemd socket on
`127.0.0.1:8097`. Do not expose port 8097. It sets `public_url` from `domain`
and forwards the visitor address for rate limiting.

Build your system configuration before switching it. Replace `HOST` with the
name in your flake's `nixosConfigurations`:

```sh
nixos-rebuild build --flake .#HOST
sudo nixos-rebuild switch --flake .#HOST
```

Then verify the service and the public route:

```sh
sudo systemctl status ankiquest.service ankiquest.socket
sudo journalctl -u ankiquest.service -n 100 --no-pager
curl --fail https://anki.example.com/api/health
sudo ankiquest-admin settings
```

The service uses a dynamic systemd user and stores its data at
`/var/lib/ankiquest` (backed by `/var/lib/private/ankiquest`). Its sandbox permits
outbound connections to public addresses for push notifications and sign-in
providers, but blocks loopback and private networks. A self-hosted ntfy or
GitLab endpoint must therefore be publicly reachable.

## Choose server settings

The minimal example enables self-service registration. These options are
optional; their defaults come from [the module](../flake.nix) and
[JSON configuration](../src/config.rs).

| NixOS option | JSON key | Default / purpose |
| --- | --- | --- |
| `registration` | `registration` | `false`; allow new self-service accounts. |
| `domain` | `public_url` | Unset; public HTTPS address. Required for OAuth. |
| `streakMinutes` | `streak_minutes` | `0`; any review counts. Set a minimum number of review minutes if wanted. |
| `weekTimezone` | `week_timezone` | `"UTC"`; shared leaderboard week timezone. |
| `weekRolloverHour` | `week_rollover_hour` | `4`; local Monday hour when the leaderboard week changes. |
| `ntfy` | `ntfy` | Unset; optional public push server URL. |
| `privacy` | `privacy` | Operator, contact and hosting details displayed on the privacy page. |
| `devs` | `devs` | Empty; accounts allowed to use the dev console. Create each account before listing it. |

Streak and XP rule details, configured players, private-site access and
administration are in [the server reference](server.md). Runtime admin
overrides take precedence over configured values; `ankiquest-admin settings`
shows both the effective values and their sources.

For Google, GitHub or GitLab sign-in, follow the
[provider setup](server.md#sign-in-with-google-github-or-gitlab). Register the
callback for your actual domain and create every configured secret file on the
host **before** deploying. NixOS loads `tokenFile`, `clientSecretFile` and
`sitePasswordFile` as systemd credentials. Keep the secret contents out of Git
and the Nix store; use file-path strings pointing to runtime files, not Nix path
literals that copy files into the store. Restart after changing credential
files.

## Back up and restore

`state_dir/ankiquest.db` contains accounts, review history, settings, uploaded
avatars and shared deck packages. Keep the configuration and runtime secret
files separately as well. Protect backups as account data and keep a copy away
from the server.

For a live database snapshot, use SQLite's backup operation instead of copying
only the main database file while the server is writing. On NixOS, an operator
with the SQLite CLI installed can run:

```sh
sudo install -d -m 0700 /var/backups/ankiquest
sudo sqlite3 /var/lib/ankiquest/ankiquest.db \
  ".backup '/var/backups/ankiquest/ankiquest-YYYY-MM-DD.db'"
sudo chmod 0600 /var/backups/ankiquest/ankiquest-YYYY-MM-DD.db
sudo sqlite3 /var/backups/ankiquest/ankiquest-YYYY-MM-DD.db 'PRAGMA integrity_check;'
```

Choose a new dated filename each time. `integrity_check` should print `ok`.
The automatic snapshots made by destructive admin commands are a safeguard for
those commands, not a scheduled backup policy. Schedule and test your own
backups.

Restore during a maintenance window: stop **both** `ankiquest.socket` and
`ankiquest.service` so socket activation cannot restart the writer. Preserve the
current state directory, then restore a checked snapshot as `ankiquest.db` in a
clean state directory with the service's ownership and permissions. Do not mix
the restored database with old `-wal` or `-shm` files. Restore the matching
configuration and credentials, start the socket and service, and check health,
sign-in and a review upload. Test the procedure on a separate instance before
depending on it for recovery.

## Update and troubleshoot

NixOS pins AnkiQuest in the deployment flake's `flake.lock`. Merging a server PR
does not update a running deployment. Back up first, then update that input and
review the lock-file diff:

```sh
nix flake update ankiquest
git diff -- flake.lock
nixos-rebuild build --flake .#HOST
sudo nixos-rebuild switch --flake .#HOST
```

Build on a separate machine if the server has little memory. The operator can
deploy that system with `nixos-rebuild switch --flake .#HOST --target-host
root@HOSTNAME --use-substitutes`. The project's production host is updated by
its operator through the separate deployment repository.

For a source installation, update the checkout, rerun `npm ci` and
`cargo build --release --locked`, then restart the service with the new binary
and the same configuration and state directory. A system-generation or binary
rollback does not roll back database changes; retain the pre-update snapshot.

| Symptom | Check |
| --- | --- |
| Build cannot find esbuild | Run `npm ci` in the checkout, or put esbuild on `PATH` / set `ESBUILD`. |
| WebAssembly target missing | Run `rustup target add wasm32-unknown-unknown` for the Rust toolchain doing the build. |
| Service fails before startup | Read the journal; check config paths and that all systemd credential files exist. |
| HTTPS or certificate fails | Check DNS, ports 80/443, nginx and ACME logs. |
| Health returns 503 | The database lock or background worker is unhealthy; inspect the journal and `ankiquest-admin logs --level error`. |
| Uploads remain pending | Run the client's **Test connection**, check its server URL, and inspect server logs. Keep the exact error before trying a full upload. |
| Configured setting seems ignored | Run `ankiquest-admin settings`; an admin override may still be active. |

For local development and the test commands, see
[development](server.md#development). For the client contract, see
[mobile-api.md](mobile-api.md).
