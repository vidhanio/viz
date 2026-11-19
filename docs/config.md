# Configuration reference

Configuration lives in `$VIZ_CONFIG_DIR` or `$XDG_CONFIG_HOME/viz` (`~/.config/viz` by default). Each process reads only its own configuration. Start with the matching `*.example.toml` file.

| Component | File                              | Override                                |
| --------- | --------------------------------- | --------------------------------------- |
| Backend   | `config.toml` + `config.d/*.toml` | `$VIZ_CONFIG` (one file, no drop-ins)   |
| CLI       | `cli.toml`                        | `--config`, then `$VIZ_CLI_CONFIG`      |
| Desktop   | `desktop.toml`                    | `--config`, then `$VIZ_DESKTOP_CONFIG`  |
| Discord   | `connectors/discord.toml`         | `--config`, then `$VIZ_DISCORD_CONFIG`  |
| iMessage  | `connectors/imessage.toml`        | `--config`, then `$VIZ_IMESSAGE_CONFIG` |

Backend drop-ins load in lexical order: tables merge recursively and other values replace earlier ones. A missing client file uses defaults; connectors still require their own credentials and allowlists. `viz config` lists component paths and whether they exist, and shows the CLI's effective connection settings without revealing its token. It does not read other components' configuration.

The default state location is `$XDG_STATE_HOME/viz` (or `$STATE_DIRECTORY` under systemd). The default local socket is `$XDG_RUNTIME_DIR/viz/backend.sock`, falling back to `backend.sock` in the default state directory when `$XDG_RUNTIME_DIR` is unset.

The state directory contains `viz.sqlite`, `memory.sqlite`, the derived `search.sqlite` index, uploaded `attachments/`, and the `worktrees/` created for chats. Git features use `git` on `PATH`; creating a pull request also needs an authenticated `gh`.

**Secret** values are `"inline"`, `{ file = "/path" }` (trailing newline trimmed), or `{ env = "NAME" }`.

## `[backend]` (in `config.toml`)

| Key               | Type          | Default                             |                                                           |
| ----------------- | ------------- | ----------------------------------- | --------------------------------------------------------- |
| `socket`          | path          | `$XDG_RUNTIME_DIR/viz/backend.sock` | Local listener, mode 0600                                 |
| `listen`          | `"host:port"` | unset                               | TCP listener for remote clients; requires `token`         |
| `token`           | Secret        | unset                               | Bearer token for TCP clients                              |
| `state_dir`       | path          | `$XDG_STATE_HOME/viz`               | Backend databases                                         |
| `default_harness` | string        | the sole configured harness, if any | Named ACP runner for new app chats and default delegation |

With several `[harnesses.<name>]` entries, set `default_harness` to create chats or orchestrator subagents without choosing a runner. Existing chats keep their runner. ACP subagents started by an ACP chat use that chat's runner unless specified. Without a configured harness, only the orchestrator can run.

## `[providers.<id>]` (backend)

| Key       | Type   |                                                                                                |
| --------- | ------ | ---------------------------------------------------------------------------------------------- |
| `api_key` | Secret | Key for pi-ai provider `<id>` (e.g. `anthropic`, `opencode-go`); unset: the provider's env var |

## `[orchestrator]` (backend, required)

| Key            | Type                                          | Default                |
| -------------- | --------------------------------------------- | ---------------------- |
| `model`        | `{ provider, model }`                         | required               |
| `thinking`     | `off`/`minimal`/`low`/`medium`/`high`/`xhigh` | unset: the model's own |
| `instructions` | string                                        | unset                  |
| `cwd`          | path                                          | `$HOME`                |

The configured model and thinking level are defaults. Selections made in the orchestrator are saved and reapplied on restart. `[orchestrator].model` and `[memory].compactor` must be set explicitly. Provider credentials come from the environment (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, ...; see `docs/findings/pi-durable.md`, Q5); with Home Manager use `programs.viz.environmentFile`.

When upgrading, remove the old `[subagents].model` setting and configure ACP runners instead. Legacy native app chats keep their visible transcript, but their next input starts an ACP session without transferring their native history. If you use Home Manager's `mutableSettings`, remove the old setting from the writable TOML as well as from `programs.viz.settings`.

## `[memory]` (backend, required)

The orchestrator's uniichat memory (`docs/architecture.md`, "Memory"). The same model writes a commit subject when `git.commit` is called without a message.

| Key         | Type                                          | Default                |
| ----------- | --------------------------------------------- | ---------------------- |
| `compactor` | `{ provider, model }`                         | required               |
| `thinking`  | `off`/`minimal`/`low`/`medium`/`high`/`xhigh` | unset: the model's own |

The compactor writes every summary line, so pick a cheap, fast model (uniichat uses Haiku with high thinking). A turn waits until every message before it is summarized; a compactor that keeps failing (bad credentials, unknown model) is retried every 60 s and stalls turns, with each failure logged by `viz-backend`.

## `[harnesses.<name>]` (backend, optional)

Defines a named ACP runner for app chats and the `subagent` tool. A configured harness runs under the backend's OS account with full access: viz allows every ACP permission request. There is no sandbox.

| Key       | Type                   | Default                                                              |
| --------- | ---------------------- | -------------------------------------------------------------------- |
| `command` | string                 | Required; executable on `PATH` or absolute path                      |
| `args`    | array of strings       | `[]`                                                                 |
| `env`     | table of Secret values | `{}`; provider keys are not automatically inherited from the backend |
| `model`   | string                 | ACP session's default model                                          |
| `effort`  | string                 | ACP session's default thinking level                                 |
| `mode`    | string                 | ACP session's default mode                                           |

Set `model`, `effort`, and `mode` to values reported by the runner. viz matches the option category (`model`, `thought_level`, `mode`), then the option id if there is no matching category. The app's selections override these defaults for that conversation.

The `permissions` key was removed; configs that still set it fail validation like any unknown key, so delete it.

## `[backend]` (in client files)

| Key      | Type   | Behavior                                                            |
| -------- | ------ | ------------------------------------------------------------------- |
| `socket` | path   | Local Unix socket; optional unless the backend uses a custom socket |
| `url`    | URL    | When set, connects remotely instead of using the socket             |
| `token`  | Secret | Required when `url` is set                                          |

For a custom local socket, set `socket` to the same path in both the backend and client files. For remote connections, set `url` and `token` in the client file; there is no fallback to the backend's token.

## `[theme]` (in `desktop.toml`)

The desktop follows the system's light/dark appearance. `dark` and `light` each select a built-in base16 scheme by name (for example `tokyo-night-dark` and `tokyo-night-light`). For a custom scheme, use `[theme.dark.colors]` or `[theme.light.colors]` with all 16 keys `base00` through `base0F`, as `"#RRGGBB"` strings. To load tinted-theming YAML, use a `file` key under `[theme.dark]` or `[theme.light]`. See `desktop.example.toml`.

## Top-level settings in `connectors/discord.toml`

| Key             | Type          |                                                                            |
| --------------- | ------------- | -------------------------------------------------------------------------- |
| `bot_token`     | Secret        | Bot token; enable the Message Content intent in the developer portal       |
| `allowed_users` | integer array | Discord user ids whose DMs and mentions are forwarded; required, non-empty |

## Top-level settings in `connectors/imessage.toml` (scaffold)

| Key               | Type         |                                                        |
| ----------------- | ------------ | ------------------------------------------------------ |
| `server_url`      | URL          | BlueBubbles server                                     |
| `password`        | Secret       | BlueBubbles server password                            |
| `allowed_handles` | string array | Phone numbers / Apple IDs whose messages are forwarded |

## Home Manager

```nix
{
  imports = [ inputs.viz.homeModules.default ];
  programs.viz = {
    enable = true;
    environmentFile = config.sops.templates."viz.env".path; # ANTHROPIC_API_KEY=...
    settings = {
      orchestrator.model = { provider = "anthropic"; model = "claude-opus-5-5"; };
      harnesses.claude = {
        command = "claude-agent-acp";
        env.ANTHROPIC_API_KEY.env = "ANTHROPIC_API_KEY";
      };
      memory.compactor = { provider = "anthropic"; model = "claude-haiku-5-5"; };
      backend = {
        listen = "100.64.0.1:7420";
        token.file = config.sops.secrets."viz/token".path;
        default_harness = "claude";
      };
    };
    connectors.discord = {
      enable = true; # runs viz-discord.service
      settings = {
        bot_token.file = config.sops.secrets."viz/discord".path;
        allowed_users = [ 123456789012345678 ];
      };
    };
    desktop.enable = true;
  };
}
```

`programs.viz.settings` is backend-only. Set `desktop.settings`, `cli.settings` and `connectors.<name>.settings` to generate the corresponding client files. `programs.viz.socket` sets the backend socket and defaults each client's socket; if unset, clients inherit the socket declared in `settings.backend.socket`, if any. When Stylix is available, `desktop.stylix.enable` defaults to `config.stylix.enable` and uses its palette on the appropriate appearance (`dark`, `light`, or both for `either`). An explicit desktop theme for that appearance wins.

Services: `viz-backend.service` (`StateDirectory=viz`, `RuntimeDirectory=viz`) and `viz-<connector>.service`, restarted when their generated config changes. `mutableSettings = true` merges the backend settings into a writable `config.toml` instead of linking it; it does not change client file generation.

### Migrating from the shared document

Move remote connection settings into each client's `[backend]` table and repeat any custom local socket there. Move connector settings to the top level of the connector's own file (or from backend `settings` to `connectors.<name>.settings` in Home Manager). Remove the old client and connector tables from `config.toml`, including a writable `mutableSettings` file or any drop-ins.
