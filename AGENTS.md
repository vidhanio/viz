# AGENTS.md

viz is an ACP workspace. Each environment stores its own projects, threads, and durable runs in SQLite. Configured environments can delegate durable tasks to each other over HTTP(S). External harnesses run every model; viz owns no model loop.

viz is in active development with a single user. Do not preserve backward compatibility for configs, stored state, protocol versions, or clients: change them directly and delete local state when needed.

The code is the documentation: read it before changing anything structural. There is no `docs/` or roadmap file; the code, its doc comments and the `*.example.toml` files are the whole context.

## Scope

viz is one person's workspace for running ACP harnesses on code. Every feature serves one of these jobs: run agents on code, review and ship their work (Git, GitHub, terminals), follow delegation, work across environments, find things. ACP is the only way viz talks to a harness.

Out of scope until a job needs it: issue tracker pages (mentioning a GitHub issue or starting a thread from one is in scope), an embedded browser, voice, multi-model fan-out, usage dashboards, a theme editor, settings forms for process-wide config (that is TOML; project settings are in-app). Prefer improving an existing surface to adding one, remove what a change makes redundant in the same change, and pick a good default over adding a setting.

## Layout

| Path                   | What                                                      |
| ---------------------- | --------------------------------------------------------- |
| `crates/viz-server`    | Tokio/Axum server, Toasty SQLite workspace, ACP and MCP   |
| `crates/viz-client`    | JSON-RPC WebSocket client and protocol re-exports        |
| `crates/viz-protocol`  | Shared Serde wire types and method schemas               |
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
- Rune is unpublished and pinned by git rev; when bumping, update `workspace.dependencies.rune` **and** every `[patch.crates-io]` rev in `Cargo.toml` together.
- Rune has no async runtime: other threads reach the UI through `AppHandle::update`. Under X11 it needs `libxkbcommon-x11` at runtime; without a GPU, use software Vulkan (`VK_ICD_FILENAMES=…/lvp_icd.json`).
- iOS: one-time `xtool setup` (Apple ID or App Store Connect key, then the Xcode .xip); `swift sdk list` must show `darwin`. xtool reads the SDK from `$XDG_CONFIG_HOME/swiftpm/swift-sdks`, so keep the NixOS `programs.xtool.sdk.enable` off. `cd ios && xtool dev` builds, signs and installs on a USB-paired device but does not launch it; tap the icon or use `xtool launch` (the installed bundle id may carry an `XTL-…` prefix). First install: enable Developer Mode and trust the profile under Settings → General → VPN & Device Management. Free Apple IDs: profiles expire after 7 days, 3 apps per device. Device logs and screenshots come from libimobiledevice in the dev shell: `idevicesyslog`, `idevicedebug run <bundle-id>`, `idevicescreenshot`. No simulator or SwiftUI previews on Linux; only non-SwiftUI files can be checked with `swiftc -typecheck`. iOS enforces ATS; reach the server over HTTPS.

## Local config

Use gitignored `viz.local.toml`, `viz.local/`, and `.viz/` for local development. `.envrc` exports `VIZ_CONFIG`, `VIZ_CLI_CONFIG`, and `VIZ_DESKTOP_CONFIG` when their files exist. In the server config, set `[server].state_dir = ".viz/state"`; change `[server].listen` and the clients' `[server].url` together if the defaults (`127.0.0.1:7420` and `http://127.0.0.1:7420`) are in use. Start with `cargo run -p viz-server`, `cargo run -p viz-desktop`, and `cargo run -p viz -- send --thread 42 "..."`. Never commit local config or print its secrets.

For app chats and delegation, configure a real ACP executable under `[harnesses.<name>]` (for example `command = "omp"`, `args = ["acp"]`). With several configured runners, set `[server].default_harness`; with only one, it is implicit. Configure the runner's credentials separately; only explicitly configured `[harnesses.<name>].env` credentials are passed from viz. Exercise ACP changes with a real runner and a fresh temporary state directory, without replacing an active server or touching its SQLite files.

Harnesses differ in what they implement; check before relying on one by sending a bare `initialize` and reading `agentCapabilities`:

```sh
(echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}'; sleep 10) | omp acp | grep -m1 '"id":1' | jq .result.agentCapabilities
```

`pi-acp` accepts `mcpServers` but never connects them, so viz's tools are unavailable in pi threads.

## Verification

**No tests.** Don't write or run unit or integration tests, `cargo test`, fixtures, or faux-model harnesses. Read the invariants from the code.

Run the static gate (CI runs it via `nix flake check`):

```sh
treefmt --fail-on-change
cargo clippy --workspace --all-targets -- -D warnings
```

Desktop rendering (debug builds): `cargo run -p viz-desktop -- --snap out.png --scene <name>`; `--list-scenes` lists scenes, `VIZ_SCENE_THEME=<builtin scheme>` picks the scheme, `--size WxH`/`--scale N` set the viewport, and `--crop <element key>` cuts to one element. `--tree out.json` dumps the accessibility tree, `--script steps.json --video out.mp4` records an interaction through ffmpeg, and `--trace out.jsonl` logs frames.

After changing shared wire types, run `cargo run -p viz-server -- schema --output protocol/schema.json` and update `ios/Sources/Viz/Server.swift` to match.

## Conventions

- Cargo: declare all direct dependencies under `[workspace.dependencies]` in the root `Cargo.toml`. Members inherit them with `dep.workspace = true` or `{ workspace = true, features = [...] }` for crate-specific features.
- Config: the server layers `config.toml` and `config.d/*.toml` (later wins, tables merge); clients load separate TOML files under the same config directory. Secrets are `"inline" | { file = "..." } | { env = "..." }`. Reference: the `*.example.toml` files.
- State lives in `$XDG_STATE_HOME/viz` (`$STATE_DIRECTORY` under systemd); only the owning server writes its environment's workspace database. Storage changes do not use migration shims.
- Comments: doc comments only (`///`, `//!`, Swift `///`). No line comments in code: if a decision needs explaining, put it in the doc comment of the item that makes it, or make the code say it.
- Commit often, as soon as a coherent change passes verification. Each commit must stand on its own: keep an API or config change with its callers and generated artifacts; split independent work. Use lowercase Conventional Commit subjects. Do not include unrelated work or secrets. History is linear: rebase and fast-forward, never merge commits.

## Where things go

- How the system fits together and why: module and item doc comments.
- Config keys: Serde doc comments and the `*.example.toml` files.
- Wire types: their doc comments, which become `protocol/schema.json` descriptions.
- Desktop UI rules: the `viz-desktop` crate doc and `theme.rs`; render affected scenes in light and dark (`cargo run -p viz-desktop -- --snap <png> --scene <name>`) and look at them before calling a UI change done.
- No architecture docs, roadmaps, research notes, decision records or session logs.
