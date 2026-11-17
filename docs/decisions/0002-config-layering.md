# 0002: Layered XDG TOML config, written by Home Manager or by hand

Status: accepted (2026-11-17)

## Context

viz must be configurable with and without Nix, from one file that connectors can read their own sections from (`[connectors.discord] bot_token = ...`). The backend is TypeScript and every other component is Rust, so the format must be easy to parse in both. With Home Manager, `xdg.configFile` makes the file a read-only store symlink, which blocks hand edits and app-written settings. Upstream Home Manager solves this two ways (`docs/findings/home-manager-module.md`): app-side drop-in directories (mise's `conf.d`) and activation-time merging into a writable file (`lib.hm.generators.mkImpureConfigMerger`, used by herdr, codex, zed, t3code).

## Decision

- Format: TOML. Location: `$VIZ_CONFIG_DIR` or `$XDG_CONFIG_HOME/viz` (`~/.config/viz`).
- Layers, later wins: `config.toml`, then `config.d/*.toml` in lexical order. Tables merge recursively; other values (including arrays) replace.
- Both loaders (`crates/viz-config`, `backend/src/config.ts`) implement this independently. Each component validates only the sections it owns and ignores the rest.
- Secrets are `"inline"`, `{ file = "/path" }` (sops-nix, agenix, systemd credentials), or `{ env = "NAME" }`; resolved at use, never copied into the store.
- Home Manager module: `programs.viz.settings` generates `config.toml`. Default: read-only link. `mutableSettings = true`: merged into a writable `config.toml` with upstream's `mkImpureConfigMerger` (`$dynamic * $static`, declared values win), plus `mkImpureConfigCleanup` when switching back, exactly like upstream's herdr module. `config.d/` is never managed by Home Manager, so hand-written or app-written overrides survive either mode.
- State goes to `$XDG_STATE_HOME/viz` (`StateDirectory=viz` under systemd), sockets to `$XDG_RUNTIME_DIR/viz`.

## Consequences

- Without Nix: write `~/.config/viz/config.toml` (start from `config.example.toml`).
- With Nix: declarative by default; local experiments go in `config.d/` without fighting the store link.
- Two loader implementations must stay in sync: change both in the same commit.
- `mutableSettings` inherits the merger's limits: no TOML datetimes, comments not preserved, removed declarations linger in the file.
