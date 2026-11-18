# AGENTS.md

viz is an all-in-one personal agent and meta-harness. One **orchestrator** conversation with unbounded history (pi-durable root conversation + compaction) receives everything: messages from connectors (Discord, iMessage, ...) and from full UIs (desktop, iOS). It delegates work to subagent conversations and, via ACP, to other harnesses (Claude Code, Codex, OpenCode, pi, ...).

Read `docs/architecture.md` before changing anything structural. `TODO.md` is the roadmap; `docs/` holds everything else (see "Where things go").

## Layout

| Path                   | What                                                      | Language                     |
| ---------------------- | --------------------------------------------------------- | ---------------------------- |
| `backend/`             | `viz-backend`: pi-durable harness + HTTP/WebSocket server | TypeScript on Bun            |
| `protocol/schema.json` | Generated JSON Schema of the client protocol              | generated (`bun run schema`) |
| `crates/viz-config`    | Layered XDG TOML config, `Secret` type                    | Rust                         |
| `crates/viz-client`    | Protocol types + JSON-RPC/WebSocket client                | Rust                         |
| `crates/viz`           | `viz` CLI (`config`, `status`, `send`)                    | Rust                         |
| `crates/viz-discord`   | Discord connector                                         | Rust (serenity)              |
| `crates/viz-imessage`  | iMessage connector (scaffold, BlueBubbles)                | Rust                         |
| `crates/viz-desktop`   | Desktop app                                               | Rust (Rune)                  |
| `ios/`                 | iOS app, built on Linux with xtool                        | Swift/SwiftUI                |
| `nix/home-module.nix`  | Home Manager module (`homeModules.default`)               | Nix                          |
| `flake.nix`            | Packages, checks, dev shell, treefmt                      | Nix                          |

Everything is Rust except the backend (TypeScript, because pi-durable is a TS library) and the iOS app (Swift). New connectors are new crates `crates/viz-<name>` reading `[connectors.<name>]`.

## Environment

Use the dev shell (`direnv allow`, or `nix develop`); run tools by plain name. It provides the nightly Rust toolchain, Bun + bun2nix, Swift 6.4 + xtool (from nixpkgs PR #548520, the `nixpkgs-xtool` input), Rune's native libraries, and `treefmt`. Entering the shell runs `bun install --frozen-lockfile` in `backend/`.

- Bun only: `bun add`, `bun check` (typecheck), `bun fmt`. Never npm/npx/tsc/prettier. Adding a dependency regenerates `backend/bun.nix` via the `postinstall` script (needs `bun2nix` on `PATH`, i.e. the dev shell); commit both `bun.lock` and `bun.nix`.
- Pin `@earendil-works/*` packages exactly and together (pi-durable is experimental; `main` has breaking changes vs npm).
- `rustfmt.toml` uses unstable options: format with `treefmt` (nightly rustfmt), not stable `cargo fmt`.
- Rune is unpublished and pinned by git rev; when bumping, update `workspace.dependencies.rune` **and** every `[patch.crates-io]` rev in `Cargo.toml` together (see `docs/findings/rune.md`).
- iOS (details: `docs/findings/ios-on-linux.md`): one-time `xtool setup` (Apple ID or App Store Connect key, then the Xcode .xip; `swift sdk list` must show `darwin`); the shell warns while the SDK is missing. `cd ios && xtool dev` builds, signs and installs on a USB-paired device but doesn't launch it (tap the icon or `xtool launch`; the installed bundle id may be prefixed, e.g. `XTL-….io.vidhan.viz`). No simulator or SwiftUI previews on Linux: test on the device; logs via `idevicesyslog`/`idevicedebug run`. Free accounts re-sign every 7 days; push and TestFlight need a paid account and, for release, a macOS runner. Only `swiftc -typecheck` of non-SwiftUI files works in CI. iOS enforces ATS: reach the backend over HTTPS (`tailscale serve`).

## Local config

For running viz from the repo, put a config in `viz.local.toml` (gitignored, with its state in `.viz/`, also gitignored): `.envrc` exports `VIZ_CONFIG` pointing at it, so the backend, `viz` CLI and desktop app in this shell all use it instead of `~/.config/viz`. It should set `[backend].socket` and `[backend].state_dir` under `.viz/`, `[providers.<id>].api_key`, and the three required models. Then: `bun backend/src/main.ts`, `cargo run -p viz -- send "..."`, `cargo run -p viz-desktop`. Never commit it or print its key.

## Verification

**No tests.** Don't write, keep, or run tests of any kind (unit, integration, `bun test`, `cargo test`, `#[cfg(test)]`, test fixtures, faux-model harnesses). A test is a second, worse copy of the logic; read the invariants from the code and get it right there. If a change needs explaining, write it into the code's comments or `docs/`.

Before declaring a change done, run only the static gate (CI runs the same via `nix flake check`):

```sh
treefmt --fail-on-change
(cd backend && bun check)
cargo clippy --workspace --all-targets -- -D warnings
```

After changing `backend/src/protocol.ts`: `cd backend && bun run schema`, then update `crates/viz-client/src/protocol.rs` and `ios/Sources/Viz/Backend.swift` to match.

## Conventions

- Config: one TOML document from `$XDG_CONFIG_HOME/viz/config.toml` + `config.d/*.toml` (later wins, tables merge). Both loaders (`crates/viz-config`, `backend/src/config.ts`) implement the same layering and XDG rules; change them together. Each component parses only its own sections. Secrets are `"inline" | { file = "..." } | { env = "..." }`. Reference: `docs/config.md`.
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
