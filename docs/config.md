# Configuration reference

Layering, locations, and rationale: `docs/decisions/0002-config-layering.md`. Example: `config.example.toml`.

| Location         | Default                                                  | Override                                                 |
| ---------------- | -------------------------------------------------------- | -------------------------------------------------------- |
| Config           | `$XDG_CONFIG_HOME/viz/config.toml` + `config.d/*.toml`   | `$VIZ_CONFIG_DIR`; `$VIZ_CONFIG` (one file, no drop-ins) |
| State (database) | `$XDG_STATE_HOME/viz/{viz,memory}.sqlite`                | `$STATE_DIRECTORY` (systemd); `[backend].state_dir`      |
| Socket           | `$XDG_RUNTIME_DIR/viz/backend.sock` (state dir if unset) | `[backend].socket`                                       |

`viz config` prints the resolved locations and the merged document.

**Secret** values are `"inline"`, `{ file = "/path" }` (trailing newline trimmed), or `{ env = "NAME" }`.

## `[backend]` (backend, Rust clients)

| Key         | Type          | Default                             |                                                   |
| ----------- | ------------- | ----------------------------------- | ------------------------------------------------- |
| `socket`    | path          | `$XDG_RUNTIME_DIR/viz/backend.sock` | Local listener, mode 0600                         |
| `listen`    | `"host:port"` | unset                               | TCP listener for remote clients; requires `token` |
| `token`     | Secret        | unset                               | Bearer token for TCP clients                      |
| `state_dir` | path          | `$XDG_STATE_HOME/viz`               | Backend databases                                 |

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

Applied to the root conversation on every backend start. There is no default model anywhere: the backend refuses to start until `[orchestrator]`, `[subagents]` and `[memory]` name theirs. Provider credentials come from the environment (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, ...; see `docs/findings/pi-durable.md`, Q5); with Home Manager use `programs.viz.environmentFile`.

## `[subagents]` (backend, required)

| Key     | Type                  | Default  |
| ------- | --------------------- | -------- |
| `model` | `{ provider, model }` | required |

## `[memory]` (backend, required)

The orchestrator's uniichat memory (`docs/architecture.md`, "Memory").

| Key         | Type                                          | Default                |
| ----------- | --------------------------------------------- | ---------------------- |
| `compactor` | `{ provider, model }`                         | required               |
| `thinking`  | `off`/`minimal`/`low`/`medium`/`high`/`xhigh` | unset: the model's own |

The compactor writes every summary line, so pick a cheap, fast model (uniichat uses Haiku with high thinking). A turn waits until every message before it is summarized; a compactor that keeps failing (bad credentials, unknown model) is retried every 60 s and stalls turns, with each failure logged by `viz-backend`.

## `[harnesses.<name>]` (backend, planned)

ACP agents viz may drive: `command` (string), `args` (string array), `env` (table). Parsed and validated now; not used yet.

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
      subagents.model = { provider = "anthropic"; model = "claude-sonnet-5-5"; };
      memory.compactor = { provider = "anthropic"; model = "claude-haiku-5-5"; };
      backend = { listen = "100.64.0.1:7420"; token.file = config.sops.secrets."viz/token".path; };
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
