# Configuration reference

Layering, locations, and rationale: `docs/decisions/0002-config-layering.md`. Example: `config.example.toml`.

| Location | Default                                                  | Override                                                 |
| -------- | -------------------------------------------------------- | -------------------------------------------------------- |
| Config   | `$XDG_CONFIG_HOME/viz/config.toml` + `config.d/*.toml`   | `$VIZ_CONFIG_DIR`; `$VIZ_CONFIG` (one file, no drop-ins) |
| State    | `$XDG_STATE_HOME/viz/`                                   | `$STATE_DIRECTORY` (systemd); `[backend].state_dir`      |
| Socket   | `$XDG_RUNTIME_DIR/viz/backend.sock` (state dir if unset) | `[backend].socket`                                       |

`viz config` prints the resolved locations and the merged document.

The state directory contains `viz.sqlite`, `memory.sqlite`, the derived `search.sqlite` index, uploaded `attachments/`, and the `worktrees/` created for chats. Git features use `git` on `PATH`; creating a pull request also needs an authenticated `gh`.

**Secret** values are `"inline"`, `{ file = "/path" }` (trailing newline trimmed), or `{ env = "NAME" }`.

## `[backend]` (backend, Rust clients)

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

## `[client]` (Rust clients)

| Key     | Type               | Default                     |
| ------- | ------------------ | --------------------------- |
| `url`   | `http://host:port` | unset: use the local socket |
| `token` | Secret             | `[backend].token`           |

## `[connectors.discord]` (`viz-discord`)

| Key             | Type          |                                                                            |
| --------------- | ------------- | -------------------------------------------------------------------------- |
| `bot_token`     | Secret        | Bot token; enable the Message Content intent in the developer portal       |
| `allowed_users` | integer array | Discord user ids whose DMs and mentions are forwarded; required, non-empty |

## `[connectors.imessage]` (`viz-imessage`, scaffold)

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
      connectors.discord = {
        bot_token.file = config.sops.secrets."viz/discord".path;
        allowed_users = [ 123456789012345678 ];
      };
    };
    connectors.discord.enable = true; # runs viz-discord.service
    desktop.enable = true;
  };
}
```

Services: `viz-backend.service` (`StateDirectory=viz`, `RuntimeDirectory=viz`) and `viz-<connector>.service`, restarted when the generated config changes. `mutableSettings = true` merges `settings` into a writable `config.toml` instead of linking it.
