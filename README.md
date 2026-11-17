# viz

An all-in-one personal agent and meta-harness. One orchestrator conversation with unbounded history receives everything (desktop, iOS, Discord, iMessage, ...), delegates to subagents and, via ACP, to other agent harnesses.

- Backend: TypeScript on Bun, built on [pi-durable](https://github.com/earendil-works/pi/tree/main/packages/durable).
- Clients and connectors: Rust (desktop app on [Rune](https://github.com/anomalyco/rune)); iOS in Swift, built with [xtool](https://xtool.sh).

Status: scaffold. See `TODO.md` for the roadmap and `docs/architecture.md` for how it fits together.

## Run

```sh
nix develop                           # or: direnv allow
cp config.example.toml ~/.config/viz/config.toml
ANTHROPIC_API_KEY=... bun backend/src/main.ts
cargo run -p viz -- send "hello"
```

With Home Manager: import `inputs.viz.homeModules.default` and set `programs.viz` (`docs/config.md`).
