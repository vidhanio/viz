# viz

viz runs persistent ACP threads on one or more configured environments. Threads can delegate tasks locally or to another backend; each environment runs its own harnesses and stores its own history.

- Backend: Rust, Tokio, and SQLite via Toasty. Backend and Rust clients share their Serde wire types.
- Clients and connectors: Rust (desktop app on [Rune](https://github.com/anomalyco/rune)); iOS in Swift, built with [xtool](https://xtool.sh).

Status: early. See `TODO.md` for the roadmap and `docs/design.md` for what viz is for.

## Run

```sh
nix develop                           # or: direnv allow
mkdir -p ~/.config/viz
cp config.example.toml ~/.config/viz/config.toml
# Enable the ACP harness you want in config.toml.
cargo run -p viz-backend
# Create or import a thread in the desktop app, then send to its ID:
cargo run -p viz -- send --thread 42 "hello"
```

See the examples for the [backend](config.example.toml), [CLI](cli.example.toml), [desktop](desktop.example.toml), [Discord](connectors/discord.example.toml), and [iMessage](connectors/imessage.example.toml).

All clients use a base URL. Rust clients default to `http://127.0.0.1:7420`; set the URL on iOS. To connect across hosts, use an HTTPS reverse proxy and a bearer token. Link environments in both directions in their backend configs. No VPN is required.

The backend runs in the foreground, supports `Type=notify`, and shuts down on SIGTERM. A systemd TCP activation unit can pass in one `ListenStream` matching `backend.listen`; without one the daemon binds the address itself. Configuration changes take effect on restart.

This version starts a new workspace database. It does not migrate the old backend's config or history. Use **Import thread** for sessions still available in an ACP harness.

With Home Manager, import `inputs.viz.homeModules.default`. Set `programs.viz.enable = true` and translate the backend example into `programs.viz.settings`. See [`nix/home-module.nix`](nix/home-module.nix) for the module's options and examples.
