# AGENTS.md

viz is an ACP workspace. Each environment stores its own projects, threads, and durable runs in SQLite. Configured environments can delegate durable tasks to each other over HTTP(S). External harnesses run every model; viz owns no model loop.

viz is in active development with a single user. Do not preserve backward compatibility for configs, stored state, protocol versions, or clients: change them directly and delete local state when needed.

The code is the documentation: read it before changing anything structural. `TODO.md` is the roadmap; `docs/` holds only what code can't say (see "Where things go").

## Layout

| Path                   | What                                                      |
| ---------------------- | --------------------------------------------------------- |
| `crates/viz-server`    | Tokio/Axum server, Toasty SQLite workspace, ACP and MCP   |
| `crates/viz-client`    | Shared Serde protocol types and JSON-RPC WebSocket client |
| `protocol/schema.json` | JSON Schema generated from the Rust wire types            |
| `crates/viz-config`    | XDG config and `Secret`                                   |
| `crates/viz`           | Clap CLI (`config`, `status`, `send`)                     |
| `crates/viz-discord`   | Discord connector (serenity)                              |
| `crates/viz-imessage`  | iMessage connector scaffold (BlueBubbles)                 |
| `crates/viz-desktop`   | Rune desktop app                                          |
| `ios/`                 | Swift/SwiftUI app, built on Linux with xtool              |
| `nix/home-module.nix`  | Home Manager module (`homeModules.default`)               |
| `flake.nix`            | Packages, checks, dev shell, treefmt                      |

New connectors are crates named `viz-<name>` and read `connectors/<name>.toml`.

## Environment

Use the dev shell (`direnv allow` or `nix develop`). It provides nightly Rust, Swift 6.4 + xtool (from `nixpkgs-xtool`), Rune's native libraries, and `treefmt`.

- `rustfmt.toml` uses unstable options: format with `treefmt` (nightly rustfmt), not stable `cargo fmt`.
- Rune is unpublished and pinned by git rev; when bumping, update `workspace.dependencies.rune` **and** every `[patch.crates-io]` rev in `Cargo.toml` together (see `docs/findings/rune.md`).
- iOS (details: `docs/findings/ios-on-linux.md`): one-time `xtool setup` (Apple ID or App Store Connect key, then the Xcode .xip); `swift sdk list` must show `darwin`. `cd ios && xtool dev` builds, signs and installs on a USB-paired device; tap the icon or use `xtool launch`. No simulator or SwiftUI previews on Linux; only non-SwiftUI files can be checked with `swiftc -typecheck`. iOS enforces ATS; reach the server over HTTPS.

## Local config

Use gitignored `viz.local.toml`, `viz.local/`, and `.viz/` for local development. `.envrc` exports `VIZ_CONFIG`, `VIZ_CLI_CONFIG`, and `VIZ_DESKTOP_CONFIG` when their files exist. In the server config, set `[server].state_dir = ".viz/state"`; change `[server].listen` and the clients' `[server].url` together if the defaults (`127.0.0.1:7420` and `http://127.0.0.1:7420`) are in use. Start with `cargo run -p viz-server`, `cargo run -p viz-desktop`, and `cargo run -p viz -- send --thread 42 "..."`. Never commit local config or print its secrets.

For app chats and delegation, configure a real ACP executable under `[harnesses.<name>]` (for example `command = "omp"`, `args = ["acp"]`). With several configured runners, set `[server].default_harness`; with only one, it is implicit. Configure the runner's credentials separately; only explicitly configured `[harnesses.<name>].env` credentials are passed from viz. Exercise ACP changes with a real runner and a fresh temporary state directory, without replacing an active server or touching its SQLite files.

## Verification

**No tests.** Don't write or run unit or integration tests, `cargo test`, fixtures, or faux-model harnesses. Read the invariants from the code; explain non-obvious decisions in comments.

Run the static gate (CI runs it via `nix flake check`):

```sh
treefmt --fail-on-change
cargo clippy --workspace --all-targets -- -D warnings
```

After changing shared wire types, run `cargo run -p viz-server -- schema --output protocol/schema.json` and update `ios/Sources/Viz/Server.swift` to match.

## Conventions

- Cargo: declare all direct dependencies under `[workspace.dependencies]` in the root `Cargo.toml`. Members inherit them with `dep.workspace = true` or `{ workspace = true, features = [...] }` for crate-specific features.
- Config: the server layers `config.toml` and `config.d/*.toml` (later wins, tables merge); clients load separate TOML files under the same config directory. Secrets are `"inline" | { file = "..." } | { env = "..." }`. Reference: the `*.example.toml` files.
- State lives in `$XDG_STATE_HOME/viz` (`$STATE_DIRECTORY` under systemd); only the owning server writes its environment's workspace database. Storage changes do not use migration shims.
- Commit often, as soon as a coherent change passes verification. Each commit must stand on its own: keep an API or config change with its callers and generated artifacts; split independent work. Use lowercase Conventional Commit subjects. Do not include unrelated work or secrets.

## Where things go

- `TODO.md`: roadmap and open work, checked off as it lands. Update it in the same change.
- `docs/design.md`: product scope (what viz is for and leaves out) and the desktop design system rules. Read before adding a feature or UI.
- `docs/findings/<topic>.md`: research about external things (libraries, protocols, reference projects), dated, with sources. Re-verify before relying on old findings.
- Everything else (how the system fits together, why a choice was made, config keys) lives in code: module doc comments, comments at the decision point, schema descriptions, and the `*.example.toml` files. No architecture docs, decision records or session logs.
