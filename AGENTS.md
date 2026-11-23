# AGENTS.md

viz is a single-user workspace for running ACP harnesses on code across environments: run agents, review and ship their work (Git and GitHub), follow delegation, and find things. Each environment stores its projects, threads, and durable runs in SQLite and can delegate tasks to peers over HTTP(S). External harnesses run every model over ACP; viz owns no model loop.

Active single-user development: change configs, wire types, and clients directly without backward-compatibility shims. Prefer improving an existing surface over adding one, remove what becomes redundant in the same change, and pick a good default over adding a setting. Process-wide config is one TOML file with a table per program (`[server]`, `[client]`, `[desktop]`, `[cli]`); project settings live in-app.

## Development commands

Enter the dev shell (`direnv allow` or `nix develop`) for nightly `rustfmt` (`treefmt`) and native libraries.

```sh
# Static gate (mirrors `nix flake check`)
treefmt --fail-on-change
cargo clippy --workspace --all-targets -- -D warnings

# Regenerate wire schema after protocol changes
cargo run -p viz-server -- schema --output protocol/schema.json

# Local runs (gitignored .viz.local/{config/config.toml,state/}; .envrc exports VIZ_*_CONFIG and STATE_DIRECTORY)
cargo run -p viz-server
cargo run -p viz-desktop
cargo run -p viz -- status

# Headless desktop rendering (debug builds; inspect light and dark before finishing UI work)
cargo run -p viz-desktop -- --list-scenes
VIZ_SCENE_THEME=github-dark cargo run -p viz-desktop -- --snap out.png --scene <name>
# Optional flags: --size WxH --scale N --crop <key> --tree out.json --script steps.json --video out.mp4 --trace out.jsonl

# Package builds
nix build .#server .#desktop
```

## Rules

- **No tests.** Do not write or run unit or integration tests, `cargo test`, test fixtures, or faux-model harnesses. Verify invariants from code and exercise real binaries/harnesses against a temporary state directory without touching a running server's state.
- **Doc comments only.** Use `///` and `//!`. No line comments (`//`): put rationale in an item's doc comment or make the code express it.
- **No standalone docs.** No `docs/`, roadmaps, ADRs, or session logs. Architecture and design rationale live in module/item doc comments and `*.example.toml`.
- **Workspace dependencies.** Declare direct Rust dependencies under `[workspace.dependencies]` in root `Cargo.toml` and inherit with `.workspace = true`.
- **Migrations.** Add every SQLite schema change as the next numbered file under `crates/viz-server/migrations/` and register it.
- **Commits.** Commit often once a coherent change passes verification, keeping API/config changes with callers and generated artifacts. Use lowercase Conventional Commit subjects, signed commits, and linear history (rebase and fast-forward; no merge commits). Never commit local config or print secrets.

## Where to look

- Crate boundaries and architecture: root `Cargo.toml` and each crate's `src/lib.rs` or `src/main.rs` (`//!` docs).
- Configuration keys and `Secret` syntax: `config.example.toml` and `crates/viz-config`.
- Desktop UI conventions, tokens, motion, and components: `crates/viz-desktop/src/main.rs`, `theme`, `motion`, and `ui`.
- Updating Rune: bump `workspace.dependencies.rune` and every `[patch.crates-io]` rev in root `Cargo.toml` together.
- Packaging and Home Manager: `flake.nix` and `nix/home-module.nix`.
