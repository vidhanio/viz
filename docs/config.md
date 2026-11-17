# Configuration reference

Layering, locations, and rationale: `docs/decisions/0002-config-layering.md`. Example: `config.example.toml`.

| Location | Default | Override |
| --- | --- | --- |
| Config | `$XDG_CONFIG_HOME/viz/config.toml` + `config.d/*.toml` | `$VIZ_CONFIG_DIR` |
| State (database) | `$XDG_STATE_HOME/viz/viz.sqlite` | `$STATE_DIRECTORY` (systemd) |
| Socket | `$XDG_RUNTIME_DIR/viz/backend.sock` (state dir if unset) | `[backend].socket` |

`viz config` prints the resolved locations and the merged document.

**Secret** values are `"inline"`, `{ file = "/path" }` (trailing newline trimmed), or `{ env = "NAME" }`.

## `[backend]` (backend, Rust clients)

| Key | Type | Default | |
| --- | --- | --- | --- |
| `socket` | path | `$XDG_RUNTIME_DIR/viz/backend.sock` | Local listener, mode 0600 |
| `listen` | `"host:port"` | unset | TCP listener for remote clients; requires `token` |
| `token` | Secret | unset | Bearer token for TCP clients |

## `[orchestrator]` (backend)

| Key | Type | Default |
| --- | --- | --- |
| `model` | `{ provider, model }` | `{ provider = "anthropic", model = "claude-opus-5-5" }` |
| `thinking` | `off`/`minimal`/`low`/`medium`/`high`/`xhigh` | `medium` |
| `instructions` | string | unset |
| `cwd` | path | `$HOME` |

Applied to the root conversation on every backend start. Provider credentials come from the environment (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, ...; see `docs/findings/pi-durable.md`, Q5); with Home Manager use `programs.viz.environmentFile`.

## `[subagents]` (backend)

| Key | Type | Default |
| --- | --- | --- |
| `model` | `{ provider, model }` | the orchestrator's model |

## `[harnesses.<name>]` (backend, planned)

ACP agents viz may drive: `command` (string), `args` (string array), `env` (table). Parsed and validated now; not used yet.

## `[client]` (Rust clients)

| Key | Type | Default |
| --- | --- | --- |
| `url` | `http://host:port` | unset: use the local socket |
| `token` | Secret | `[backend].token` |

## `[connectors.discord]` (`viz-discord`)

| Key | Type | |
| --- | --- | --- |
| `bot_token` | Secret | Bot token; enable the Message Content intent in the developer portal |
| `allowed_users` | integer array | Discord user ids whose DMs and mentions are forwarded; required, non-empty |

## `[connectors.imessage]` (`viz-imessage`, scaffold)

| Key | Type | |
| --- | --- | --- |
| `server_url` | URL | BlueBubbles server |
| `password` | Secret | BlueBubbles server password |
| `allowed_handles` | string array | Phone numbers / Apple IDs whose messages are forwarded |

## Home Manager

```nix
{
  imports = [ inputs.viz.homeModules.default ];
  programs.viz = {
    enable = true;
    environmentFile = config.sops.templates."viz.env".path; # ANTHROPIC_API_KEY=...
    settings = {
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
