{
  description = "Gamification server for self-hosted Anki sync";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = {
    self,
    nixpkgs,
  }: let
    lib = nixpkgs.lib;
    systems = ["x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin"];
    forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
  in {
    formatter = forAllSystems (pkgs: pkgs.alejandra);

    packages = forAllSystems (pkgs: {
      default = pkgs.rustPlatform.buildRustPackage {
        pname = "ankiquest";
        version = "0.1.0";
        src = lib.cleanSource self;
        cargoLock.lockFile = ./Cargo.lock;
        meta.mainProgram = "ankiquest";
      };
    });

    devShells = forAllSystems (pkgs: {
      default = pkgs.mkShell {
        packages = with pkgs; [cargo clippy rustc rustfmt];
      };
    });

    nixosModules.default = {
      config,
      lib,
      pkgs,
      ...
    }: let
      cfg = config.services.ankiquest;
      syncMount = "/run/ankiquest-sync";
      withToken = lib.filterAttrs (_: u: u.tokenFile != null) cfg.users;
      configFile = pkgs.writeText "ankiquest.json" (builtins.toJSON {
        addr = "127.0.0.1:${toString cfg.port}";
        sync_base =
          if cfg.syncBase == null
          then null
          else syncMount;
        state_dir = "/var/lib/ankiquest";
        ntfy = cfg.ntfy;
        remind_hour = cfg.remindHour;
        week_timezone = cfg.weekTimezone;
        week_rollover_hour = cfg.weekRolloverHour;
        competition_start_date = cfg.competitionStartDate;
        private_site = cfg.privateSite;
        site_trust_proxy = cfg.domain != null;
        site_password_file =
          if cfg.sitePasswordFile == null
          then null
          else "/run/credentials/ankiquest.service/site-password";
        public_url =
          if cfg.domain == null
          then null
          else "https://${cfg.domain}";
        users =
          lib.mapAttrs (name: u: {
            display = u.display;
            ntfy_topic = u.ntfyTopic;
            token_file =
              if u.tokenFile == null
              then null
              else "/run/credentials/ankiquest.service/token-${name}";
          })
          cfg.users;
      });
      user = lib.types.submodule {
        options = {
          display = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
          };
          ntfyTopic = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = "ntfy topic for this player's notifications. Ends up in the world-readable store.";
          };
          tokenFile = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = "File holding the bearer token this player's AnkiDroid uploads reviews with.";
          };
        };
      };
    in {
      options.services.ankiquest = {
        enable = lib.mkEnableOption "ankiquest";
        package = lib.mkOption {
          type = lib.types.package;
          default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        };
        port = lib.mkOption {
          type = lib.types.port;
          default = 8097;
        };
        domain = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Serve through nginx with ACME on this domain.";
        };
        privateSite = lib.mkOption {
          type = lib.types.bool;
          default = false;
          description = "Require a community password or player token to view all site pages and data.";
        };
        sitePasswordFile = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          example = "/run/secrets/ankiquest-site-password";
          description = "Runtime file with the shared read-only site password. Loaded with systemd credentials; the secret never enters the Nix store.";
        };
        syncBase = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          example = "/var/lib/private/anki-sync-server";
          description = "SYNC_BASE of a self-hosted Anki sync server to read reviews from, instead of or besides uploads.";
        };
        ntfy = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          example = "https://ntfy.sh";
          description = "ntfy server to push reminders through. The service may not reach loopback or private addresses, so this must be a public server.";
        };
        weekTimezone = lib.mkOption {
          type = lib.types.str;
          default = "UTC";
          example = "Europe/Berlin";
          description = "Time zone of the shared leaderboard week, which runs Monday to Sunday for every player.";
        };
        weekRolloverHour = lib.mkOption {
          type = lib.types.ints.between 0 23;
          default = 4;
          description = "Local hour in weekTimezone at which the leaderboard week turns over on Mondays.";
        };
        remindHour = lib.mkOption {
          type = lib.types.ints.between 0 23;
          default = 20;
          description = "Deprecated: reminder times and types are configured per player at /community.";
        };
        competitionStartDate = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Optional YYYY-MM-DD competition start date. Null uses the earliest available review history.";
        };
        users = lib.mkOption {
          type = lib.types.attrsOf user;
          default = {};
          description = "Players, keyed by name.";
        };
      };

      config = lib.mkIf cfg.enable {
        assertions = [
          {
            assertion = !cfg.privateSite || cfg.sitePasswordFile != null || withToken != {};
            message = "services.ankiquest.privateSite needs sitePasswordFile or at least one user tokenFile.";
          }
        ];
        # systemd holds the port and passes it in. Connections nginx makes to
        # it belong to the socket unit, so the service's own address rules can
        # shut it out of loopback and the private network entirely.
        systemd.sockets.ankiquest = {
          description = "ankiquest socket";
          wantedBy = ["sockets.target"];
          listenStreams = ["127.0.0.1:${toString cfg.port}"];
        };

        systemd.services.ankiquest = {
          description = "ankiquest";
          wantedBy = ["multi-user.target"];
          requires = ["ankiquest.socket"];
          after = ["network.target" "anki-sync-server.service" "ankiquest.socket"];
          environment.ANKIQUEST_CONFIG = configFile;
          serviceConfig = {
            ExecStart = lib.getExe cfg.package;
            DynamicUser = true;
            StateDirectory = "ankiquest";
            BindReadOnlyPaths = lib.optional (cfg.syncBase != null) "${cfg.syncBase}:${syncMount}";
            LoadCredential =
              lib.mapAttrsToList (name: u: "token-${name}:${toString u.tokenFile}") withToken
              ++ lib.optional (cfg.sitePasswordFile != null) "site-password:${toString cfg.sitePasswordFile}";
            Restart = "always";
            RestartSec = 5;

            # It only ever connects out to the ntfy server, over the internet.
            IPAddressDeny = [
              "localhost"
              "link-local"
              "multicast"
              "0.0.0.0/8"
              "10.0.0.0/8"
              "100.64.0.0/10"
              "172.16.0.0/12"
              "192.168.0.0/16"
              "fc00::/7"
            ];
            # AF_UNIX for name lookups through nscd.
            RestrictAddressFamilies = ["AF_UNIX" "AF_INET" "AF_INET6"];
            # Only its own state directory under /var/lib; the rest is hidden.
            TemporaryFileSystem = "/var/lib:ro";
            InaccessiblePaths = ["-/mnt" "-/media" "-/srv"];
            ProtectHome = true;
            ProtectSystem = "strict";
            PrivateTmp = true;
            PrivateDevices = true;
            DevicePolicy = "closed";
            PrivateUsers = true;
            PrivateIPC = true;
            ProtectProc = "invisible";
            ProcSubset = "pid";
            ProtectClock = true;
            ProtectHostname = true;
            ProtectKernelLogs = true;
            ProtectKernelModules = true;
            ProtectKernelTunables = true;
            ProtectControlGroups = true;
            CapabilityBoundingSet = "";
            NoNewPrivileges = true;
            RestrictNamespaces = true;
            RestrictRealtime = true;
            RestrictSUIDSGID = true;
            LockPersonality = true;
            MemoryDenyWriteExecute = true;
            RemoveIPC = true;
            SystemCallArchitectures = "native";
            SystemCallFilter = ["@system-service" "~@privileged @resources"];
            SystemCallErrorNumber = "EPERM";
            UMask = "0077";
          };
        };

        # The state directory belongs to the service's dynamic user, so writing a
        # message by hand needs root: sudo ankiquest-message aldanita "well done".
        environment.systemPackages = [
          (pkgs.writeShellScriptBin "ankiquest-message" ''
            exec ${lib.getExe cfg.package} ${configFile} message "$@"
          '')
        ];

        services.nginx.virtualHosts = lib.mkIf (cfg.domain != null) {
          ${cfg.domain} = {
            forceSSL = true;
            enableACME = true;
            locations."/" = {
              proxyPass = "http://127.0.0.1:${toString cfg.port}";
              extraConfig = ''
                proxy_set_header X-AnkiQuest-Client-IP $remote_addr;
              '';
            };
          };
        };
      };
    };
  };
}
