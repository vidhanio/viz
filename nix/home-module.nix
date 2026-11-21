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
  backendSettings = cfg.settings;
  generatedSettings = tomlFormat.generate "viz-settings.toml" backendSettings;

  stylixColors = config.lib.stylix.colors or null;
  stylixPolarity = config.stylix.polarity or "either";
  applyStylix = cfg.desktop.stylix.enable && stylixColors != null;
  base16 = lib.genAttrs (map (suffix: "base0${suffix}") (lib.stringToCharacters "0123456789ABCDEF")) (
    key: "#${stylixColors.${key}}"
  );
  desktopSettings =
    cfg.desktop.settings
    // lib.optionalAttrs applyStylix {
      theme =
        lib.optionalAttrs (stylixPolarity != "light") { dark.colors = base16; }
        // lib.optionalAttrs (stylixPolarity != "dark") { light.colors = base16; }
        // (cfg.desktop.settings.theme or { });
    };
  cliSettings = cfg.cli.settings;
  connectorSettings = lib.mapAttrs (_: connector: connector.settings) cfg.connectors;
  connectorSources = lib.mapAttrs (
    name: settings: tomlFormat.generate "viz-${name}.toml" settings
  ) connectorSettings;

  settingsOption =
    file:
    mkOption {
      inherit (tomlFormat) type;
      default = { };
      description = ''
        Client settings written to {file}`$XDG_CONFIG_HOME/viz/${file}`.
        See `${lib.removeSuffix ".toml" file}.example.toml` for keys and defaults.
        Reference secrets by file or environment variable, not inline.
      '';
    };
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
      restartTriggers,
    }:
    {
      Unit = {
        Description = description;
        After = [ "network.target" ] ++ after;
        Wants = after;
        X-Restart-Triggers = restartTriggers;
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
        settings = settingsOption "connectors/${name}.toml";
        package = mkOption {
          type = types.package;
          default = vizPackages.${name};
          defaultText = lib.literalExpression "viz.packages.\${system}.${name}";
          description = "The viz-${name} package.";
        };
      };
    };

  enabledConnectors = lib.filterAttrs (_: c: c.enable) cfg.connectors;
in
{
  options.programs.viz = {
    enable = mkEnableOption "viz, a persistent ACP workspace";

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
          backend = {
            environment = "workstation";
            listen = "127.0.0.1:7420";
            token.file = config.sops.secrets."viz/token".path;
            default_harness = "claude";
          };
          harnesses.claude = {
            name = "Claude Code";
            command = "claude-agent-acp";
            env.ANTHROPIC_API_KEY.env = "ANTHROPIC_API_KEY";
          };
        }
      '';
      description = ''
        Backend configuration written to {file}`$XDG_CONFIG_HOME/viz/config.toml`. Only
        viz-backend reads it, along with unmanaged drop-ins under
        {file}`$XDG_CONFIG_HOME/viz/config.d/`. See `config.example.toml` for keys
        and defaults. Reference secrets by file or environment variable so the
        values stay out of the Nix store. Configure clients separately with
        {option}`programs.viz.cli.settings`, {option}`programs.viz.desktop.settings`,
        or {option}`programs.viz.connectors.<name>.settings`.
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
        Client file generation is unchanged.
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
        description = ''
          Run `viz-backend.service` as a systemd user service with `StateDirectory=viz`.
          Changes to the generated backend settings trigger a restart.
        '';
      };
      package = mkOption {
        type = types.package;
        default = vizPackages.backend;
        defaultText = lib.literalExpression "viz.packages.\${system}.backend";
        description = "The viz-backend package.";
      };
    };

    desktop = {
      enable = mkEnableOption "the viz desktop app";
      settings = settingsOption "desktop.toml";
      stylix.enable = mkOption {
        type = types.bool;
        default = config.stylix.enable or false;
        defaultText = lib.literalExpression "config.stylix.enable or false";
        description = ''
          Default to the Stylix base16 palette for the configured appearance
          (`dark`, `light`, or both for `either`). An explicit desktop theme
          for that appearance takes precedence.
        '';
      };
      package = mkOption {
        type = types.package;
        default = vizPackages.desktop;
        defaultText = lib.literalExpression "viz.packages.\${system}.desktop";
        description = "The viz-desktop package.";
      };
    };

    cli.settings = settingsOption "cli.toml";

    connectors = mkOption {
      type = types.attrsOf (types.submodule connectorModule);
      default = { };
      example = lib.literalExpression ''
        {
          discord.enable = true;
          discord.settings = {
            bot_token.file = config.sops.secrets."viz/discord".path;
            allowed_users = [ 123456789012345678 ];
            thread = 42;
          };
        }
      '';
      description = ''
        Connector settings and services. Each reads `connectors/<name>.toml` and runs
        as `viz-<name>.service` when enabled. Changes to its generated settings
        trigger a restart. Known connectors: `discord`, `imessage`.
      '';
    };
  };

  config = mkIf cfg.enable {
    home.packages = [ cfg.package ] ++ lib.optional cfg.desktop.enable cfg.desktop.package;

    xdg.configFile = {
      "viz/config.toml" = mkIf (!cfg.mutableSettings && backendSettings != { }) {
        source = settingsSource;
      };
      "viz/desktop.toml" = mkIf (desktopSettings != { }) {
        source = tomlFormat.generate "viz-desktop.toml" desktopSettings;
      };
      "viz/cli.toml" = mkIf (cliSettings != { }) {
        source = tomlFormat.generate "viz-cli.toml" cliSettings;
      };
    }
    // lib.mapAttrs' (
      name: _settings:
      lib.nameValuePair "viz/connectors/${name}.toml" {
        source = connectorSources.${name};
      }
    ) (lib.filterAttrs (_: settings: settings != { }) connectorSettings);

    home.activation.vizMutableSettings = mkIf (cfg.mutableSettings && backendSettings != { }) (
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

    home.activation.vizImmutableSettings = mkIf (!cfg.mutableSettings && backendSettings != { }) (
      lib.hm.dag.entryBetween [ "linkGeneration" ] [ "writeBoundary" ] (
        lib.hm.generators.mkImpureConfigCleanup {
          file = config.xdg.configFile."viz/config.toml";
        }
      )
    );

    systemd.user.services = {
      viz-backend = mkIf cfg.backend.enable (service {
        description = "viz backend environment";
        inherit (cfg.backend) package;
        restartTriggers = [ settingsSource ];
        extraService = {
          Type = "notify";
          NotifyAccess = "main";
          KillMode = "mixed";
          UMask = "0077";
          StandardOutput = "journal";
          StandardError = "journal";
          # $XDG_STATE_HOME/viz is exported as $STATE_DIRECTORY.
          StateDirectory = "viz";
          StateDirectoryMode = "0700";
        };
      });
    }
    // lib.mapAttrs' (
      name: connector:
      lib.nameValuePair "viz-${name}" (service {
        description = "viz ${name} connector";
        inherit (connector) package;
        after = lib.optional cfg.backend.enable "viz-backend.service";
        restartTriggers = lib.optional (connectorSettings.${name} != { }) connectorSources.${name};
      })
    ) enabledConnectors;
  };
}
