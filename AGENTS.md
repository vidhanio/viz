# AGENTS.md

viz is an all-in-one personal agent and meta-harness. The **orchestrator** is the only conversation that runs viz's native agent (pi-durable root with unbounded history and compaction). Connectors send to it by default. App chats and all delegations use external harnesses over ACP; pi-durable stores their transcripts and schedules work without running their models.

The code is the documentation: read it before changing anything structural. `TODO.md` is the roadmap; `docs/` holds only what code can't say (see "Where things go").

## Layout

| Path                   | What                                                      | Language                     |
| ---------------------- | --------------------------------------------------------- | ---------------------------- |
| `backend/`             | `viz-backend`: pi-durable harness + HTTP/WebSocket server | TypeScript on Bun            |
| `protocol/schema.json` | Generated JSON Schema of the client protocol              | generated (`bun run schema`) |
| `crates/viz-config`    | Per-client XDG TOML config, `Secret` type                 | Rust                         |
| `crates/viz-client`    | Protocol types + JSON-RPC/WebSocket client                | Rust                         |
| `crates/viz`           | `viz` CLI (`config`, `status`, `send`)                    | Rust                         |
| `crates/viz-discord`   | Discord connector                                         | Rust (serenity)              |
| `crates/viz-imessage`  | iMessage connector (scaffold, BlueBubbles)                | Rust                         |
| `crates/viz-desktop`   | Desktop app                                               | Rust (Rune)                  |
| `ios/`                 | iOS app, built on Linux with xtool                        | Swift/SwiftUI                |
| `nix/home-module.nix`  | Home Manager module (`homeModules.default`)               | Nix                          |
| `flake.nix`            | Packages, checks, dev shell, treefmt                      | Nix                          |

Everything is Rust except the backend (TypeScript, because pi-durable is a TS library) and the iOS app (Swift). New connectors are new crates `crates/viz-<name>` reading `connectors/<name>.toml`.

## Environment

Use the dev shell (`direnv allow`, or `nix develop`); run tools by plain name. It provides the nightly Rust toolchain, Bun + bun2nix, Swift 6.4 + xtool (from nixpkgs PR #548520, the `nixpkgs-xtool` input), Rune's native libraries, and `treefmt`. Entering the shell runs `bun install --frozen-lockfile` in `backend/`.

- Bun only: `bun add`, `bun check` (typecheck), `bun fmt`. Never npm/npx/tsc/prettier. Adding a dependency regenerates `backend/bun.nix` via the `postinstall` script (needs `bun2nix` on `PATH`, i.e. the dev shell); commit both `bun.lock` and `bun.nix`.
- Pin `@earendil-works/*` packages exactly and together (pi-durable is experimental; `main` has breaking changes vs npm).
- `rustfmt.toml` uses unstable options: format with `treefmt` (nightly rustfmt), not stable `cargo fmt`.
- Rune is unpublished and pinned by git rev; when bumping, update `workspace.dependencies.rune` **and** every `[patch.crates-io]` rev in `Cargo.toml` together (see `docs/findings/rune.md`).
- iOS (details: `docs/findings/ios-on-linux.md`): one-time `xtool setup` (Apple ID or App Store Connect key, then the Xcode .xip; `swift sdk list` must show `darwin`); the shell warns while the SDK is missing. `cd ios && xtool dev` builds, signs and installs on a USB-paired device but doesn't launch it (tap the icon or `xtool launch`; the installed bundle id may be prefixed, e.g. `XTL-….io.vidhan.viz`). No simulator or SwiftUI previews on Linux: test on the device; logs via `idevicesyslog`/`idevicedebug run`. Free accounts re-sign every 7 days; push and TestFlight need a paid account and, for release, a macOS runner. Only `swiftc -typecheck` of non-SwiftUI files works in CI. iOS enforces ATS: reach the backend over HTTPS (`tailscale serve`).

## Local config

Use gitignored `viz.local.toml`, `viz.local/`, and `.viz/` for local development. The backend reads `viz.local.toml` via `.envrc`'s `VIZ_CONFIG`: set `[backend].socket` and `[backend].state_dir` under `.viz/`, `[providers.<id>].api_key`, `[orchestrator].model`, and `[memory].compactor`. Put the same `[backend].socket` in `viz.local/cli.toml` and `viz.local/desktop.toml`; `.envrc` exports their `VIZ_CLI_CONFIG` and `VIZ_DESKTOP_CONFIG` when the files exist. Start with `bun backend/src/main.ts`, `cargo run -p viz -- send "..."`, and `cargo run -p viz-desktop`. Never commit local config or print its secrets.

For app chats and delegation, configure a real ACP executable under `[harnesses.<name>]` (for example `command = "omp"`, `args = ["acp"]`). With several configured runners, set `[backend].default_harness`; with only one, it is implicit. Configure the runner's credentials separately; only explicitly configured `[harnesses.<name>].env` credentials are passed from viz. Exercise ACP changes with a real runner and isolated `.viz/` state, without replacing an active backend or touching its SQLite files.

## Verification

**No tests.** Don't write, keep, or run tests of any kind (unit, integration, `bun test`, `cargo test`, `#[cfg(test)]`, test fixtures, faux-model harnesses). A test is a second, worse copy of the logic; read the invariants from the code and get it right there. If a change needs explaining, write it into the code's comments.

Before declaring a change done, run only the static gate (CI runs the same via `nix flake check`):

```sh
treefmt --fail-on-change
(cd backend && bun check)
cargo clippy --workspace --all-targets -- -D warnings
```

After changing `backend/src/protocol.ts`: `cd backend && bun run schema`, then update `crates/viz-client/src/protocol.rs` and `ios/Sources/Viz/Backend.swift` to match.

## Conventions

- Cargo: declare all direct dependencies under `[workspace.dependencies]` in the root `Cargo.toml`. Members inherit them with `dep.workspace = true` or `{ workspace = true, features = [...] }` for crate-specific features.
- Config: the backend layers `config.toml` and `config.d/*.toml` (later wins, tables merge); clients load separate TOML files under the same config directory. Secrets are `"inline" | { file = "..." } | { env = "..." }`. Reference: the `*.example.toml` files.
- State lives in `$XDG_STATE_HOME/viz` (`$STATE_DIRECTORY` under systemd); sockets in `$XDG_RUNTIME_DIR/viz`. Only `viz-backend` writes `viz.sqlite` (pi-durable is single-writer).
- pi-durable extensions and tools are identified by name in storage: never rename an installed extension/tool/task without a migration.
- Commit often, as soon as a coherent change passes verification. Each commit must stand on its own: keep an API or config change with its callers and generated artifacts; split independent work. Use lowercase Conventional Commit subjects. Do not include unrelated work or secrets.

## Where things go

- `TODO.md`: roadmap and open work, checked off as it lands. Update it in the same change.
- `docs/design.md`: product scope (what viz is for and leaves out) and the desktop design system rules. Read before adding a feature or UI.
- `docs/findings/<topic>.md`: research about external things (libraries, protocols, reference projects), dated, with sources. Re-verify before relying on old findings.
- Everything else (how the system fits together, why a choice was made, config keys) lives in code: module doc comments, comments at the decision point, schema descriptions, and the `*.example.toml` files. No architecture docs, decision records or session logs.
