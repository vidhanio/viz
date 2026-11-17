# AGENTS.md

viz is an all-in-one personal agent and meta-harness. One **orchestrator** conversation with unbounded history (pi-durable root conversation + compaction) receives everything: messages from connectors (Discord, iMessage, ...) and from full UIs (desktop, iOS). It delegates work to subagent conversations and, via ACP, to other harnesses (Claude Code, Codex, OpenCode, pi, ...).

Read `docs/architecture.md` before changing anything structural. `TODO.md` is the roadmap; `docs/` holds everything else (see "Where things go").

## Layout

| Path                    | What                                                         | Language                  |
| ----------------------- | ------------------------------------------------------------ | ------------------------- |
| `backend/`              | `viz-backend`: pi-durable harness + HTTP/WebSocket server    | TypeScript on Bun         |
| `protocol/schema.json`  | Generated JSON Schema of the client protocol                 | generated (`bun run schema`) |
| `crates/viz-config`     | Layered XDG TOML config, `Secret` type                       | Rust                      |
| `crates/viz-client`     | Protocol types + JSON-RPC/WebSocket client                   | Rust                      |
| `crates/viz`            | `viz` CLI (`config`, `status`, `send`)                       | Rust                      |
| `crates/viz-discord`    | Discord connector                                            | Rust (serenity)           |
| `crates/viz-imessage`   | iMessage connector (scaffold, BlueBubbles)                   | Rust                      |
| `crates/viz-desktop`    | Desktop app                                                  | Rust (Rune)               |
| `ios/`                  | iOS app, built on Linux with xtool                           | Swift/SwiftUI             |
| `nix/home-module.nix`   | Home Manager module (`homeModules.default`)                  | Nix                       |
| `flake.nix`             | Packages, checks, dev shell, treefmt                         | Nix                       |

Everything is Rust except the backend (TypeScript, because pi-durable is a TS library) and the iOS app (Swift). New connectors are new crates `crates/viz-<name>` reading `[connectors.<name>]`.

## Environment

Use the dev shell (`direnv allow`, or `nix develop`); run tools by plain name. It provides the nightly Rust toolchain, Bun + bun2nix, Swift 6.4 + xtool (from nixpkgs PR #548520, the `nixpkgs-xtool` input), Rune's native libraries, and `treefmt`. Entering the shell runs `bun install --frozen-lockfile` in `backend/`.

- Bun only: `bun add`, `bun check` (typecheck), `bun test`, `bun fmt`. Never npm/npx/tsc/prettier. Adding a dependency regenerates `backend/bun.nix` via the `postinstall` script (needs `bun2nix` on `PATH`, i.e. the dev shell); commit both `bun.lock` and `bun.nix`.
- Pin `@earendil-works/*` packages exactly and together (pi-durable is experimental; `main` has breaking changes vs npm).
- `rustfmt.toml` uses unstable options: format with `treefmt` (nightly rustfmt), not stable `cargo fmt`.
- Rune is unpublished and pinned by git rev; when bumping, update `workspace.dependencies.rune` **and** every `[patch.crates-io]` rev in `Cargo.toml` together (see `docs/findings/rune.md`).
- iOS: `cd ios && xtool dev` needs the Darwin SDK (`xtool setup`, Xcode .xip) and a paired device; not available in CI.

## Verification

Run before declaring a change done (CI runs the same via `nix flake check`):

```sh
treefmt --fail-on-change
(cd backend && bun check && bun test)
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

After changing `backend/src/protocol.ts`: `cd backend && bun run schema`, then update `crates/viz-client/src/protocol.rs` and `ios/Sources/Viz/Backend.swift` to match.

## Conventions

- Config: one TOML document from `$XDG_CONFIG_HOME/viz/config.toml` + `config.d/*.toml` (later wins, tables merge). Both loaders (`crates/viz-config`, `backend/src/config.ts`) implement the same layering and XDG rules; change them together and keep their tests in sync. Each component parses only its own sections. Secrets are `"inline" | { file = "..." } | { env = "..." }`. Reference: `docs/config.md`.
- State lives in `$XDG_STATE_HOME/viz` (`$STATE_DIRECTORY` under systemd); sockets in `$XDG_RUNTIME_DIR/viz`. Only `viz-backend` writes `viz.sqlite` (pi-durable is single-writer).
- pi-durable extensions and tools are identified by name in storage: never rename an installed extension/tool/task without a migration.
- Commit with lowercase Conventional Commit subjects; only validated changes.

## Where things go

- `TODO.md`: roadmap and open work, checked off as it lands. Update it in the same change.
- `docs/architecture.md`: how the system fits together (current state, not plans).
- `docs/config.md`: config reference.
- `docs/decisions/NNNN-title.md`: decision records (context, decision, consequences). Add one for any choice a future reader would question.
- `docs/findings/<topic>.md`: research about external things (libraries, protocols, reference projects), dated, with sources. Re-verify before relying on old findings.
- `docs/log/YYYY-MM-DD.md`: progress log per working session: what changed, what was verified, what's next. Append; don't rewrite history.
