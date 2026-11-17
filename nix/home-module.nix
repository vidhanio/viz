{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkEnableOption
    mkIf
    mkOption
    types
    ;

  cfg = config.programs.viz;
  vizPackages = self.packages.${pkgs.stdenv.hostPlatform.system};

  tomlFormat = pkgs.formats.toml { };
  generatedSettings = tomlFormat.generate "viz-settings.toml" cfg.settings;
  # Match the impure merger's TOML serialization, so an unchanged mutable file can be replaced by
  # a store link when `mutableSettings` is turned off again (same as upstream's herdr module).
  settingsSource = pkgs.runCommand "viz-config.toml" { } ''
    settings="$(${lib.getExe pkgs.jaq} --from toml --to toml '.' ${generatedSettings})"
    printf '%s\n' "$settings" > "$out"
  '';
  settingsPath = "${config.xdg.configHome}/viz/config.toml";

  service =
    {
      description,
      package,
      after ? [ ],
      extraService ? { },
    }:
    {
      Unit = {
        Description = description;
        After = [ "network.target" ] ++ after;
        Wants = after;
        X-Restart-Triggers = [ settingsSource ];
      };
      Service = {
        ExecStart = lib.getExe package;
        Restart = "on-failure";
        RestartSec = 5;
      }
      // lib.optionalAttrs (cfg.environmentFile != null) { EnvironmentFile = cfg.environmentFile; }
      // extraService;
      Install.WantedBy = [ "default.target" ];
    };

  connectorModule =
    { name, ... }:
    {
      options = {
        enable = mkEnableOption "the viz ${name} connector service";
        package = mkOption {
          type = types.package;
          default = vizPackages."viz-${name}";
          defaultText = lib.literalExpression "viz.packages.\${system}.viz-${name}";
          description = "The viz-${name} package.";
        };
      };
    };

  enabledConnectors = lib.filterAttrs (_: c: c.enable) cfg.connectors;
in
{
  options.programs.viz = {
    enable = mkEnableOption "viz, an all-in-one personal agent";

    package = mkOption {
      type = types.package;
      default = vizPackages.viz;
      defaultText = lib.literalExpression "viz.packages.\${system}.viz";
      description = "The `viz` command-line client.";
    };

    settings = mkOption {
      inherit (tomlFormat) type;
      default = { };
      example = lib.literalExpression ''
        {
          orchestrator = {
            model = { provider = "anthropic"; model = "claude-opus-5-5"; };
            thinking = "high";
          };
          backend = {
            listen = "100.100.1.2:7420";
            token.file = config.sops.secrets."viz/token".path;
          };
          connectors.discord = {
            bot_token.file = config.sops.secrets."viz/discord".path;
            allowed_users = [ 123456789012345678 ];
          };
          harnesses.claude.command = "claude-agent-acp";
        }
      '';
      description = ''
        Configuration written to {file}`$XDG_CONFIG_HOME/viz/config.toml`. Every viz component
        reads this file plus drop-ins in {file}`$XDG_CONFIG_HOME/viz/config.d/*.toml`, which
        override it and are never touched by Home Manager. Pass secrets as `{ file = "..."; }`
        so they stay out of the Nix store. See `docs/config.md`.
      '';
    };

    mutableSettings = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Whether to merge {option}`programs.viz.settings` into a writable
        {file}`$XDG_CONFIG_HOME/viz/config.toml` during activation, instead of linking a
        read-only file. Declared values win; values added by hand or by viz are preserved; removing
        a declaration does not remove it from the file; comments are not preserved. New files get
        mode 0600. Switching back replaces the file only if it is byte-identical to the declared
        settings. Uses Home Manager's `mkImpureConfigMerger`.
      '';
    };

    environmentFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      example = "/run/secrets/viz-env";
      description = ''
        `KEY=value` file loaded into every viz service: provider credentials for the backend
        (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, ...) and variables referenced by
        `{ env = "NAME"; }` secrets.
      '';
    };

    backend = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = "Whether to run viz-backend as a systemd user service on this machine.";
      };
      package = mkOption {
        type = types.package;
        default = vizPackages.viz-backend;
        defaultText = lib.literalExpression "viz.packages.\${system}.viz-backend";
        description = "The viz-backend package.";
      };
    };

    desktop = {
      enable = mkEnableOption "the viz desktop app";
      package = mkOption {
        type = types.package;
        default = vizPackages.viz-desktop;
        defaultText = lib.literalExpression "viz.packages.\${system}.viz-desktop";
        description = "The viz-desktop package.";
      };
    };

    connectors = mkOption {
      type = types.attrsOf (types.submodule connectorModule);
      default = { };
      example = lib.literalExpression "{ discord.enable = true; }";
      description = ''
        Connector services. Each reads `[connectors.<name>]` from the config and runs as
        `viz-<name>.service`. Known connectors: `discord`, `imessage`.
      '';
    };
  };

  config = mkIf cfg.enable {
    home.packages = [ cfg.package ] ++ lib.optional cfg.desktop.enable cfg.desktop.package;

    xdg.configFile."viz/config.toml" = mkIf (!cfg.mutableSettings && cfg.settings != { }) {
      source = settingsSource;
    };

    home.activation.vizMutableSettings = mkIf (cfg.mutableSettings && cfg.settings != { }) (
      lib.hm.dag.entryAfter [ "linkGeneration" ] (
        lib.hm.generators.mkImpureConfigMerger {
          inherit pkgs;
          format = "toml";
          empty = "{}";
          jqOperation = "$dynamic * $static";
          path = settingsPath;
          staticSettings = settingsSource;
          mode = "600";
        }
      )
    );

    home.activation.vizImmutableSettings = mkIf (!cfg.mutableSettings && cfg.settings != { }) (
      lib.hm.dag.entryBetween [ "linkGeneration" ] [ "writeBoundary" ] (
        lib.hm.generators.mkImpureConfigCleanup {
          file = config.xdg.configFile."viz/config.toml";
        }
      )
    );

    systemd.user.services = {
      viz-backend = mkIf cfg.backend.enable (service {
        description = "viz backend (pi-durable orchestrator)";
        inherit (cfg.backend) package;
        extraService = {
          # $XDG_STATE_HOME/viz (exported as $STATE_DIRECTORY) and $XDG_RUNTIME_DIR/viz.
          StateDirectory = "viz";
          StateDirectoryMode = "0700";
          RuntimeDirectory = "viz";
          RuntimeDirectoryMode = "0700";
        };
      });
    }
    // lib.mapAttrs' (
      name: connector:
      lib.nameValuePair "viz-${name}" (service {
        description = "viz ${name} connector";
        inherit (connector) package;
        after = lib.optional cfg.backend.enable "viz-backend.service";
      })
    ) enabledConnectors;
  };
}
