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
  settings = lib.recursiveUpdate cfg.settings (
    lib.optionalAttrs applyStylix { desktop.theme = desktopTheme; }
  );
  generatedSettings = tomlFormat.generate "viz-settings.toml" settings;

  stylixColors = config.lib.stylix.colors or null;
  stylixPolarity = config.stylix.polarity or "either";
  applyStylix = cfg.desktop.stylix.enable && stylixColors != null;
  base16 = lib.genAttrs (map (suffix: "base0${suffix}") (lib.stringToCharacters "0123456789ABCDEF")) (
    key: "#${stylixColors.${key}}"
  );
  desktopTheme =
    lib.optionalAttrs (stylixPolarity != "light") { dark.colors = base16; }
    // lib.optionalAttrs (stylixPolarity != "dark") { light.colors = base16; }
    // (cfg.settings.desktop.theme or { });
  settingsSource = pkgs.runCommand "viz-config.toml" { } ''
    settings="$(${lib.getExe pkgs.jaq} --from toml --to toml '.' ${generatedSettings})"
    printf '%s\n' "$settings" > "$out"
  '';
  settingsPath = "${config.xdg.configHome}/viz/config.toml";
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
          server = {
            environment = "workstation";
            listen = "127.0.0.1:7420";
            token.file = config.sops.secrets."viz/token".path;
            default_harness = "claude";
          };
          server.harnesses.claude = {
            name = "Claude Code";
            command = "claude-agent-acp";
            env.ANTHROPIC_API_KEY.env = "ANTHROPIC_API_KEY";
          };
          desktop.theme.dark = "tokyo-night-dark";
        }
      '';
      description = ''
        Configuration written to {file}`$XDG_CONFIG_HOME/viz/config.toml`, shared by
        every viz program: `[server]`, `[client]` and `[desktop]` tables, each read only
        by its own program. See `config.example.toml` for keys and defaults.
        Reference secrets by file or environment variable so the values stay out of
        the Nix store.
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
        `KEY=value` file loaded into the viz server service: provider credentials for the server
        (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, ...) and variables referenced by
        `{ env = "NAME"; }` secrets.
      '';
    };

    server = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = ''
          Run `viz-server.service` as a systemd user service with `StateDirectory=viz`.
          Changes to the generated settings trigger a restart.
        '';
      };
      package = mkOption {
        type = types.package;
        default = vizPackages.server;
        defaultText = lib.literalExpression "viz.packages.\${system}.server";
        description = "The viz-server package.";
      };
    };

    desktop = {
      enable = mkEnableOption "the viz desktop app";
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
  };

  config = mkIf cfg.enable {
    home.packages = [ cfg.package ] ++ lib.optional cfg.desktop.enable cfg.desktop.package;

    xdg.configFile = {
      "viz/config.toml" = mkIf (!cfg.mutableSettings && settings != { }) {
        source = settingsSource;
      };
    };

    home.activation.vizMutableSettings = mkIf (cfg.mutableSettings && settings != { }) (
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

    home.activation.vizImmutableSettings = mkIf (!cfg.mutableSettings && settings != { }) (
      lib.hm.dag.entryBetween [ "linkGeneration" ] [ "writeBoundary" ] (
        lib.hm.generators.mkImpureConfigCleanup {
          file = config.xdg.configFile."viz/config.toml";
        }
      )
    );

    systemd.user.services.viz-server = mkIf cfg.server.enable {
      Unit = {
        Description = "viz server environment";
        After = [ "network.target" ];
        X-Restart-Triggers = [ settingsSource ];
      };
      Service = {
        ExecStart = lib.getExe cfg.server.package;
        Restart = "on-failure";
        RestartSec = 5;
        Type = "notify";
        NotifyAccess = "main";
        KillMode = "mixed";
        UMask = "0077";
        StandardOutput = "journal";
        StandardError = "journal";
        StateDirectory = "viz";
        StateDirectoryMode = "0700";
      }
      // lib.optionalAttrs (cfg.environmentFile != null) { EnvironmentFile = cfg.environmentFile; };
      Install.WantedBy = [ "default.target" ];
    };
  };
}
