# Findings: Home Manager conventions for `viz` (TOML config + systemd user services + secrets)

## 0. Checkouts / versions actually read

| Thing | Path / rev |
|---|---|
| Home Manager checkout | `/home/vidhanio/Projects/home-manager` (fork `git@github.com:vidhanio/home-manager.git`, branch `agents-skills`, HEAD `4f870bed211d615a89551fa7b3bec39b3b8b3c89`; upstream `nix-community/home-manager`) |
| HM rev pinned by vidhanix | `nix-community/home-manager` `9d7ba6bef3904a018d38b92f83132eaf01eb8ef3` (`vidhanix/flake.lock:575-594`) |
| sops-nix rev pinned by vidhanix | `Mic92/sops-nix` `7c637afdccdf9bcf35c4a2d67b58a77c1fd843a4` (`vidhanix/flake.lock:1530-1548`) |
| nixpkgs formats | `nixpkgs/pkgs/pkgs-lib/formats.nix:105,123` → `json`, `toml = (import ./formats/toml/default.nix ...).format` |
| agentcord | `/home/vidhanio/Projects/agentcord` (flake-parts + crane Rust project) |
| vidhanix | `/home/vidhanio/Projects/vidhanix` (flake-parts + import-tree) |

`lib.hm.generators.mkImpureConfigMerger` / `mkImpureConfigCleanup` are **present in upstream HM at the vidhanix-pinned rev**, not just the local fork:
`https://github.com/nix-community/home-manager/blob/9d7ba6bef3904a018d38b92f83132eaf01eb8ef3/modules/lib/generators.nix` (lines 1007, 1065, 1088, 1284, 1307).
Local docs call the API **experimental** (`generators.nix:1007-1012`, `1266-1268`).

## 1. Canonical `settings` + `pkgs.formats.toml {}` pattern

Three upstream modules:

**`home-manager/modules/services/listenbrainz-mpd.nix:16,25-37,54-57`** — closest template (TOML + user service):
```nix
tomlFormat = pkgs.formats.toml { };
settings = mkOption {
  inherit (tomlFormat) type;
  default = { };
  description = "Configuration ... $XDG_CONFIG_HOME/listenbrainz-mpd/config.toml";
  example = { submission.token_file = "/run/secrets/listenbrainz-mpd"; };
};
...
xdg.configFile."listenbrainz-mpd/config.toml" = mkIf (cfg.settings != { }) {
  source = tomlFormat.generate "listenbrainz-mpd.toml" cfg.settings;
};
```

**`home-manager/modules/programs/atuin.nix:11,67-100,193-200`** — custom setting type but same shape; adds `force = cfg.forceOverwriteSettings`:
```nix
"atuin/config.toml" = { source = tomlFormat.generate "atuin-config" cfg.settings; force = cfg.forceOverwriteSettings; };
```

**`home-manager/modules/programs/herdr.nix:12-14,62-70,120-127`** — TOML + mutable/immutable split:
```nix
tomlFormat = pkgs.formats.toml { };
generatedSettings = tomlFormat.generate "herdr-settings.toml" cfg.settings;
settingsPath = "${config.xdg.configHome}/herdr/config.toml";
...
xdg.configFile."herdr/config.toml" = mkIf (!cfg.mutableSettings && cfg.settings != { }) {
  source = settingsSource;
  onChange = reloadConfig;
};
```

Mechanics:
- `xdg.configFile` is a `fileType` submodule (`modules/misc/xdg/default.nix:70-81`, `modules/lib/file-type.nix:31-160`): fields `enable`, `target`, `text`, `source`, `executable`, `recursive`, `ignorelinks`, `onChange`, `force`.
- `xdg.configFile.<name>` is merged into `home.file."${config.xdg.configHome}/<name>"` (`modules/misc/xdg/default.nix:187-205`), i.e. it creates a **symlink into the Nix store**.
- `xdg.configHome` default `~/.config`, `xdg.stateHome` default `~/.local/state` (`modules/misc/xdg/default.nix:79-90,138-146`).
- Counter-pattern: `modules/services/spotifyd.nix:12-16,52` generates the TOML and passes it to the daemon as `--config-path ${configFile}`; no user-editable file. Not what viz wants.

## 2. Impure / mutable-config merge patterns

### 2.1 Generic helper (best fit for viz)

`home-manager/modules/lib/generators.nix:1000-1183`:
```
mkImpureConfigMerger :: { pkgs, format, empty, jqOperation, path,
  staticSettings, mode ? null, reader ? null, verboseMsg ? null } -> String
```
- `format` ∈ `"json" | "yaml" | "toml" | "cbor"`; `empty` e.g. `"{}"`; `jqOperation` e.g. `"$dynamic * $static"` (recursive object merge, declared wins).
- Runs inside `home.activation`; reads existing file with `jaq` (`jaq -c '.'` for JSON, `jaq --from toml -c '.'` otherwise), refuses to replace a path resolving into the Nix store, snapshots, writes a candidate temp file, detects concurrent modification, `mv -fT` (falls back to in-place write on `EBUSY`), preserves existing mode, applies `DRY_RUN`/`VERBOSE`, and never clobbers on a failed parse/merge.
- `mode` applies only to a newly created file; docs recommend `"600"` for token-bearing files.
- Limitation: TOML date/time values unsupported by `jaq` 3.1.1 → activation fails without modifying the file (`generators.nix:1000-1010`).

**Herdr is the cleanest complete TOML example** (`modules/programs/herdr.nix:15-30,99-127`):
```nix
settingsSource = pkgs.runCommand "herdr-config.toml" { } ''
  settings="$(${lib.getExe pkgs.jaq} --from toml --to toml '.' ${generatedSettings})"
  printf '%s\n' "$settings" > "$out"
'';
mutableSettingsActivation = lib.hm.generators.mkImpureConfigMerger {
  inherit pkgs; format = "toml"; empty = "{}";
  jqOperation = "$dynamic * $static";
  path = settingsPath; staticSettings = settingsSource; mode = "600";
};
...
home.activation.herdrMutableSettings = mkIf (cfg.mutableSettings && cfg.settings != { }) (
  lib.hm.dag.entryAfter [ "linkGeneration" ] (mutableSettingsActivation + ''run ${reloadConfig}'')
);
home.activation.herdrImmutableSettings = mkIf (!cfg.mutableSettings && cfg.settings != { }) (
  lib.hm.dag.entryBetween [ "linkGeneration" ] [ "writeBoundary" ] (
    lib.hm.generators.mkImpureConfigCleanup {
      file = config.xdg.configFile."herdr/config.toml";
    }
  )
);
```
The `jaq --from toml --to toml` normalization exists so that when switching back to immutable, `mkImpureConfigCleanup` can byte-compare the user file with the generation’s copy.

`mkImpureConfigCleanup` (`generators.nix:1284-1340`) removes only a regular, byte-identical, unchanged file (never symlinks/store sources) before `linkGeneration`, allowing HM to link over it.

DAG semantics: `entryBetween before after` = “run before these, after those” (`modules/lib/dag.nix:105`); `linkGeneration` is `entryAfter [ "writeBoundary" ]` (`modules/files.nix:259`). So cleanup runs **after writeBoundary, before linkGeneration**, while mergers run **after linkGeneration**.

### 2.2 Other real examples

| Module | Mechanism |
|---|---|
| `modules/programs/codex/default.nix:187-230,363-377` | TOML merger with a custom `jqOperation` that strips conflicting MCP transport keys; `mode = "600"`; same immutable cleanup. |
| `modules/programs/t3code.nix:200-247` | Three JSON mergers (`settings.json`, `keybindings.json`, `client-settings.json`), JSON5 `reader = "${lib.getExe json5} --as-json"`, array merge via `$dynamic + $static \| group_by([...]) \| map(reduce ...)`. |
| `modules/programs/github-copilot-cli.nix:470-532` | `mode = "600"`, JSON5 reader because the app writes a comment header; custom `jqOperation` appending trusted folders. |
| `modules/programs/vscode/mutableSettings.nix:1-160` + `vscode/mkVscodeModule.nix:94-116` | Older hand-rolled `vscodeMutableSettings` shell function: JSON5 parse, snapshot, `jq -n '$dynamic * $static'`, temp+rename, conflict detection. Predecessor of the generic helper. |
| `modules/programs/zed-editor.nix:316-366` | Four `mkImpureConfigMerger` activations (settings/keymap/tasks/debug), JSON5 reader, array merges via `group_by`+`reduce`. |
| `modules/programs/mise.nix:11-13,72-95,185-195` | **True `config.d` drop-in**: declarative config goes to `$XDG_CONFIG_HOME/mise/conf.d/50-home-manager.toml` when `mutableSettings = true`, leaving `mise/config.toml` app-owned; activation just `touch`es an empty mutable main file because mise otherwise rewrites the read-only fragment. |
| `modules/programs/opencode.nix:20-40,173-186` | **Separate read-only layer via env**: when `mutableSettings`, config is written to `opencode/home-manager.json` and exported as `OPENCODE_CONFIG` (plus `OPENCODE_TUI_CONFIG`), leaving app files writable. No activation merge. |
| `modules/services/syncthing.nix:129-200,924-940` | Runtime merge through the app’s REST API from a `syncthing-init` oneshot user unit (`ExecStart = updateConfig`, `RuntimeDirectory = "syncthing-init"`), not activation. |

### 2.3 Documented semantics / caveats from herdr’s option text
- Declared values (including arrays) take precedence; undeclared runtime values are preserved.
- Removing a declaration does **not** remove it from the file.
- Comments and formatting are lost.
- New files get `0600`; existing files retain mode.
- Switching to immutable only replaces a byte-identical file; changed files use HM’s normal collision/backup handling.
- Empty `settings` leaves the file unmanaged.
(`modules/programs/herdr.nix:36-58`)

## 3. Secrets

### 3.1 sops-nix HM module (pinned rev)

`https://github.com/Mic92/sops-nix/blob/7c637afdccdf9bcf35c4a2d67b58a77c1fd843a4/modules/home-manager/sops.nix`
- `sops.secrets.<name>` submodule: `name`, `key`, `path` (default `${sops.defaultSymlinkPath}/<name>`), `format` (`yaml|json|binary|ini|dotenv`), `mode` (default `"0400"`), `sopsFile`.
- `sops.defaultSymlinkPath = "${config.xdg.configHome}/sops-nix/secrets"`; `sops.defaultSecretsMountPoint = "%r/secrets.d"`; `keepGenerations = 1`.
- Runs `sops-install-secrets -ignore-passwd <manifest>` via a **oneshot user unit**:
```nix
systemd.user.services.sops-nix = {
  Unit.Description = "sops-nix activation";
  Service = { Type = "oneshot"; Environment = ...; ExecStart = script; };
  Install.WantedBy = if cfg.gnupg.home != null then [ "graphical-session-pre.target" ] else [ "default.target" ];
};
home.activation.sops-nix = ''... ${systemctl} restart --user sops-nix ...'';
```
- Key sources required by assertion: one of `sops.age.keyFile`, `age.sshKeyFile`, `age.sshKeyCmd`, `age.sshKeyPaths`, `gnupg.home`, `gnupg.sshKeyPaths`, Qubes split-gpg.

`https://github.com/Mic92/sops-nix/blob/.../modules/home-manager/templates.nix`
- `sops.templates.<name>`: `content` (or `file`), `path` default `${xdg.configHome}/sops-nix/secrets/rendered/<name>`, `mode` default `"0400"`.
- `sops.placeholder.<secret>` = `"<SOPS:sha256:...:PLACEHOLDER>"`; the installer substitutes these into rendered templates, so secret values never enter the Nix store.

### 3.2 vidhanix wiring

`vidhanix/modules/flake/sops-nix.nix` (full file):
```nix
profiles.base = let
  mkSopsConfig = key: {
    defaultSopsFile = ../../secrets.yaml;
    environment.SOPS_AGE_SSH_PRIVATE_KEY_FILE = key;
    age.sshKeyPaths = [ key ];
  };
in {
  module = { config, ... }: {
    imports = [ inputs.sops-nix.nixosModules.default ];
    sops = mkSopsConfig "${config.persist.persistentStoragePath}/etc/ssh/ssh_host_ed25519_key";
  };
  homeModule = { config, ... }: {
    imports = [ inputs.sops-nix.homeManagerModules.default ];
    sops = mkSopsConfig "${config.home.homeDirectory}/.ssh/id_ed25519";
  };
}
```
Note: the HM side points age at the **user’s** SSH key, and `SOPS_AGE_SSH_PRIVATE_KEY_FILE` is set in `sops.environment`.

Secret usage examples:
- HM, `token_file` style: `modules/programs/noctalia/calendar.nix` →
  `password_file = config.sops.secrets.apple.path;` (`sops.secrets.apple = {}`).
- HM, activation reads secret: `modules/programs/wakatime/default.nix`:
  ```nix
  sops.secrets.wakatime = { };
  home.activation.setWakatimeKey = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
    run ${lib.getExe pkgs.wakatime-cli} --config-write api_key_vault_cmd="cat ${config.sops.secrets.wakatime.path}"
  '';
  ```
- NixOS, template → env file: `modules/services/searxng/default.nix`:
  ```nix
  sops.secrets."searxng/secret-key" = { };
  sops.templates."searxng-env" = {
    owner = "searx"; group = "searx"; mode = "0400";
    content = "SEARXNG_SECRET_KEY=${config.sops.placeholder."searxng/secret-key"}";
  };
  services.searx.environmentFile = config.sops.templates."searxng-env".path;
  ```
  (config later references `$SEARXNG_SECRET_KEY`.)
- NixOS, template → NetworkManager env file: `modules/services/network/default.nix:64-75`.
- NixOS, direct path: `modules/services/tailscale/default.nix` → `authKeyFile = config.sops.secrets.tailscale.path;`
- User passwords: `modules/hosts/default.nix:76-86` → `sops.secrets."passwords/${username}".neededForUsers = true;` and `hashedPasswordFile = config.sops.secrets."passwords/${username}".path`.
- `secrets.yaml` keys present include `passwords`, `networks`, `tailscale`, `apple`, `wakatime`, `opencode`, `searxng`, `vane`, `netrc` (values encrypted; `.sops.yaml` age recipients = vortex/voyager user+root SSH keys).

### 3.3 `environmentFile` / `*_FILE` conventions in HM
- `services/cachix-agent.nix:42-48,64-67`: `credentialsFile` option, default `~/.config/cachix-agent.token`, passed as `Service.EnvironmentFile`, content `CACHIX_AGENT_TOKEN=...`.
- `programs/opencode.nix:327-346,890-900`: `web.environmentFile` (nullOr path, example `/run/secrets/opencode-web`) → `EnvironmentFile`; docs say recommended way to set `OPENCODE_SERVER_PASSWORD`.
- `programs/t3code.nix:200-213,280-288`: same pattern for `t3 serve`.
- `services/restic.nix:49-63,107-131`: `passwordFile` → `RESTIC_PASSWORD_FILE` in the service environment; `repositoryFile` → `RESTIC_REPOSITORY_FILE`.
- `programs/vdirsyncer/accounts.nix:204-210` + `default.nix:132`: `tokenFile` rendered as `token_file = "<path>"` in the app config (path only, not secret value).
- `services/listenbrainz-mpd.nix:33-35`: example `submission.token_file = "/run/secrets/listenbrainz-mpd"` — the app-owned setting points at the secret file.

### 3.4 `LoadCredential`
- Not used anywhere in HM `modules/` (grep for `LoadCredential` returns only a TODO comment in `services/mpdscribble.nix:220`).
- HM’s freeform unit type (`modules/systemd.nix:96-108`) passes any systemd key, so `Service.LoadCredential = "token:/path"` is possible.
- `systemd.exec(5)` documents per-user manager support, including credstore search paths:
  `$XDG_CONFIG_HOME/credstore/`, `$XDG_RUNTIME_DIR/credstore/`, `$HOME/.local/lib/credstore/` (and `.../credstore.encrypted/`), with the credential exposed at `$CREDENTIALS_DIRECTORY`:
  `https://man7.org/linux/man-pages/man5/systemd.exec.5.html` (`LoadCredential=`).
- Exact systemd version on the target is UNVERIFIED (no command execution performed).

### 3.5 agentcord’s `${NAME}` pattern
`agentcord/src/config.rs:162-190,295-340` expands `${NAME}` recursively in every string after TOML parsing via `std::env::var`; missing vars are preserved literally (test at `config.rs:484`). `config.example.toml` documents `bot_token = "${DISCORD_TOKEN}"`. This is why its HM module can keep the token out of the store while the config file itself remains declarative.

## 4. systemd user service conventions in HM

Type (`modules/systemd.nix:96-215`):
```nix
freeformType = attrsOf (attrsOf (either primitive (listOf primitive)));   # Unit/Service/Install arbitrary keys
options.Unit = { Description; Documentation; };
options.Service = { Environment; ExecStart; };
# plus X-Reload-Triggers, X-Restart-Triggers, X-SwitchMethod
```
`Install.WantedBy` etc. are freeform; recursion/Capitalization follows systemd names.

Observed conventions:
- `Install.WantedBy = [ "default.target" ]` for daemons (`listenbrainz-mpd.nix:50`, `spotifyd.nix:36`, `cachix-agent.nix:79`, `agentcord/flake.nix:87`); `[ "graphical-session.target" ]` for GUI/desktop (`vidhanix/modules/programs/valent/default.nix:9`, HM `programs/numlock.nix`, many services).
- `Unit.After = [ "network.target" ]` is the HM norm (`listenbrainz-mpd.nix:41`, `syncthing.nix:889`, `agentcord/flake.nix:74`, `opencode.nix:888`). **No HM module uses `network-online.target`** (grep over `home-manager/modules` returned 0); for GUI, `After`/`PartOf = [ "graphical-session.target" ]`.
- `Restart = "on-failure"` is common (`listenbrainz-mpd`, `syncthing`, `cachix-agent`); `"always"` used for daemons that should never stay down (`spotifyd`, `opencode-web`, `agentcord`). `RestartSec = 5`/`12`.
- Extra robustness used in `syncthing.nix:889-909`: `StartLimitIntervalSec`, `StartLimitBurst`, `SuccessExitStatus`, `RestartForceExitStatus`, sandboxing keys (`NoNewPrivileges`, `PrivateUsers`, `SystemCallFilter`, etc.).
- HM start/stop of new/changed units is handled by `systemd.user.startServices` (default `true`, implemented with `sd-switch`; `modules/systemd.nix:335-360`). `X-Restart-Triggers`/`X-Reload-Triggers`/`X-SwitchMethod` control reload/restart behavior (`modules/systemd.nix:139-176`).

State/runtime directories (user units):
- `systemd.exec(5)` table: for **user units**, `StateDirectory=` → `$XDG_STATE_HOME`, env `$STATE_DIRECTORY`; `RuntimeDirectory=` → `$XDG_RUNTIME_DIR`, `$RUNTIME_DIRECTORY`; `CacheDirectory=` → `$XDG_CACHE_HOME`; `LogsDirectory=` → `$XDG_STATE_HOME/log`; `ConfigurationDirectory=` → `$XDG_CONFIG_HOME`.
  `https://man7.org/linux/man-pages/man5/systemd.exec.5.html` (Table 2).
- `%S` = state-directory root (`$XDG_STATE_HOME` for user managers); `%t` = `$XDG_RUNTIME_DIR`; `%h` = home:
  `https://man7.org/linux/man-pages/man5/systemd.unit.5.html` (§Specifiers).
- HM proof concrete: `modules/services/mpdscribble.nix:216-222`:
  ```nix
  Service = {
    StateDirectory = "mpdscribble";
    RuntimeDirectory = "mpdscribble";
    RuntimeDirectoryMode = "700";
    ExecStartPre = "+${preStart}/bin/mpdscribble-pre-start";
  };
  ```
  Also `services/git-sync.nix:114` (`WorkingDirectory = "${repo.path}"`), `programs/kbfs.nix:56` (`"%h/${cfg.mountPoint}"`), `programs/tahoe-lafs.nix:32` (`%h/.tahoe`).
- `config.xdg.stateHome` is the Nix-side equivalent (`modules/misc/xdg/default.nix:138-146`, default `~/.local/state`; `xdg.stateFile` at `:130`).
- vidhanix persists state via impermanence: `modules/system/persistence.nix` aliases `persist` to `environment.persistence."/persist"` (NixOS) and `home.persistence."/persist"` (HM), then modules declare e.g. `persist.directories = [ ".local/state/rex" ]` (`programs/rex/default.nix`), `[ ".pi/agent" ]` (`pi-coding-agent/default.nix`), `".herdr/worktrees"` + `.config/herdr/session.json` (`herdr/default.nix`).

## 5. agentcord’s Nix module / flake outputs

`agentcord/flake.nix:21-94`: only one Nix re-export:
```nix
flake.homeManagerModules.default = { config, lib, pkgs, ... }: {
  options.programs.agentcord = {
    enable = lib.mkEnableOption "agentcord";
    package = lib.mkOption { type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default; };
    settings = lib.mkOption { inherit (toml) type; default = { }; };
    environmentFile = lib.mkOption { type = lib.types.nullOr lib.types.path; default = null; example = "/run/secrets/agentcord"; };
  };
  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
    xdg.configFile."agentcord/config.toml".source = toml.generate "agentcord-config.toml" cfg.settings;
    systemd.user.services.agentcord = {
      Unit = { Description = "Agentcord Discord client"; After = [ "network.target" ]; };
      Service = { ExecStart = lib.getExe cfg.package; Restart = "always"; RestartSec = 5; }
        // lib.optionalAttrs ((cfg.environmentFile or null) != null) { EnvironmentFile = cfg.environmentFile; };
      Install.WantedBy = [ "default.target" ];
    };
  };
};
```
- No `nixosModules` / `homeModules` alias (grep over the whole repo: only `homeManagerModules`).
- README `:65-90` shows the install snippet (`imports = [ inputs.agentcord.homeManagerModules.default ];`) and states `environmentFile` should contain `KEY=VALUE` lines, referenced from `settings` as `"${DISCORD_TOKEN}"`.
- Build system: flake-parts `perSystem` with crane (`flake.nix:97-300+`), `packages.default` with `meta.mainProgram = "agentcord"`, checks clippy/doc/fmt/deny/nextest, treefmt with `nixfmt`, `statix`, `deadnix`, `rustfmt`, `taplo`.
- Minor quirks: `(cfg.environmentFile or null)` is redundant; no `StateDirectory`/`WorkingDirectory`; `After = [ "network.target" ]` only.

## 6. Implications for `viz` (evidence-based; design inferences marked)

- **Option shape**: `settings = mkOption { type = tomlFormat.type; default = { }; }` + `xdg.configFile."viz/config.toml".source = tomlFormat.generate ...`; add `mutableSettings` (bool, default false) exactly like `herdr`/`codex`.
- **Mutable activation**: use `lib.hm.generators.mkImpureConfigMerger` with `format = "toml"; empty = "{}"; jqOperation = "$dynamic * $static"; mode = "600"; path = "${config.xdg.configHome}/viz/config.toml"`; place `entryAfter [ "linkGeneration" ]`. Add the inverse `mkImpureConfigCleanup` at `entryBetween [ "linkGeneration" ] [ "writeBoundary" ]` so switching back to immutable is clean.
- **TOML normalization**: if using the cleanup path, generate the immutable source through `jaq --from toml --to toml` (as `herdr.nix:15-20` does) so byte-identical comparison works. `[INFERENCE]` skip this only if the cleanup path is omitted.
- **Better than an activation merge (viz owns its code)**: implement app-side layering — e.g. read `$XDG_CONFIG_HOME/viz/config.toml` plus `config.d/*.toml` (mise pattern), or accept `VIZ_CONFIG`/`VIZ_CONFIG_DIR` pointing at a read-only Nix file (opencode pattern). This avoids activation races and comment/format destruction. `[INFERENCE]` based on the limitations above.
- **Service skeleton**: `Unit.After = [ "network.target" ]`; `Service.ExecStart = lib.getExe cfg.package; Restart = "on-failure"; RestartSec = 5;` `Service.StateDirectory = "viz";` `Service.RuntimeDirectory = "viz";` `Install.WantedBy = [ "default.target" ]`. `%S/viz` and `$STATE_DIRECTORY` then work in user units (systemd table above). If a GUI/tray client exists, `PartOf`/`After`/`WantedBy = [ "graphical-session.target" ]` (valent pattern).
- **Secrets**: expose `environmentFile` (nullOr path) → `Service.EnvironmentFile` and/or `tokenFile`-style settings; document sops usage as `sops.secrets."viz/discord-token"` + `sops.templates."viz.env"` → `environmentFile = config.sops.templates."viz.env".path;`. Keep secret values out of `settings`; put only `${ENV_NAME}` placeholders expanded by viz (agentcord precedent) or file paths. For systemd-native use, `LoadCredential` is available but unused by HM/sops-nix.
- **Persistence**: if using impermanence (vidhanix does), put the service state under `persist.directories = [ ".local/state/viz" ]` or use `StateDirectory` with `$XDG_STATE_HOME` persisted.
- **Flake exports**: exporting `homeManagerModules.default` (agentcord) is the minimal convention; vidhanix also uses `flake.homeModules.upstream` for its own bundle (`vidhanix/modules/upstream/default.nix:15-30`) and `home-manager.sharedModules`/`users.<name>.module` wiring (`vidhanix/modules/hosts/default.nix:69-73`, `modules/users/default.nix`).

## 7. Key files read

- HM: `modules/lib/generators.nix` (999-1343), `modules/lib/dag.nix`, `modules/lib/file-type.nix`, `modules/misc/xdg/default.nix`, `modules/home-environment.nix:465-510,690-780`, `modules/systemd.nix:89-253,329-423`, `modules/programs/{herdr,atuin,codex/default,mise,opencode,t3code,vscode/mkVscodeModule,vscode/mutableSettings,github-copilot-cli,zed-editor,pi-coding-agent,vdirsyncer/*}.nix`, `modules/services/{listenbrainz-mpd,spotifyd,mpdscribble,syncthing,cachix-agent,restic}.nix`.
- vidhanix: `flake.nix`, `flake.lock` (HM/sops revs), `modules/flake/{sops-nix,settings}.nix`, `modules/profiles/{default,base,pc}.nix`, `modules/hosts/{default,vortex/default}.nix`, `modules/users/{default,home-manager}.nix`, `modules/system/{persistence,nix}.nix`, `modules/services/{searxng,network,tailscale}/default.nix`, `modules/programs/{wakatime,noctalia/calendar,valent,rex,herdr,pi-coding-agent,agents}/default.nix`, `modules/upstream/_modules/home-manager/programs/omp.nix`, `modules/upstream/_modules/home-manager/agents.nix`, `.sops.yaml`, `secrets.yaml`.
- agentcord: `flake.nix`, `README.md:55-95`, `src/config.rs:150-208,295-340`, `config.example.toml`.
- nixpkgs: `pkgs/pkgs-lib/formats.nix:105,123`.
- External: sops-nix `modules/home-manager/{sops.nix,templates.nix}` at `7c637af`; `systemd.exec(5)`, `systemd.unit(5)` man pages.

## 8. UNVERIFIED / caveats

- Whether the `agents-skills` fork contains local changes to `mkImpureConfigMerger` beyond upstream; only confirmed the helper exists **verbatim** in upstream at `9d7ba6b`.
- Exact systemd version on vortex/voyager and therefore per-user `LoadCredential=`/credstore behavior at runtime.
- agentcord’s public repository URL / whether a NixOS module exists outside this checkout (none in the checkout; only `flake.homeManagerModules.default`).
- Runtime effectiveness of `After = [ "network.target" ]` in user units (HM-wide convention, but user manager has no system target graph); treating as convention, not guarantee.