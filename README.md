# viz

An all-in-one personal agent and meta-harness. One orchestrator conversation with unbounded history receives everything (desktop, iOS, Discord, iMessage, ...), delegates to subagents and, via ACP, to other agent harnesses.

- Backend: TypeScript on Bun, built on [pi-durable](https://github.com/earendil-works/pi/tree/main/packages/durable).
- Clients and connectors: Rust (desktop app on [Rune](https://github.com/anomalyco/rune)); iOS in Swift, built with [xtool](https://xtool.sh).

Status: early. See `TODO.md` for the roadmap and `docs/design.md` for what viz is for.

## Run

```sh
nix develop                           # or: direnv allow
mkdir -p ~/.config/viz
cp config.example.toml ~/.config/viz/config.toml
ANTHROPIC_API_KEY=... bun backend/src/main.ts
cargo run -p viz -- send "hello"
```

See the examples for the [backend](config.example.toml), [CLI](cli.example.toml), [desktop](desktop.example.toml), [Discord](connectors/discord.example.toml), and [iMessage](connectors/imessage.example.toml).

With Home Manager, import `inputs.viz.homeModules.default`. Set `programs.viz.enable = true` and translate the backend example into `programs.viz.settings`. See [`nix/home-module.nix`](nix/home-module.nix) for the module's options and examples.
