# viz

viz runs persistent ACP threads on one or more configured environments. Threads can delegate tasks locally or to another server; each environment runs its own harnesses and stores its own history.

- Server: Rust, Tokio, and SQLite via Toasty. Server and Rust clients share their Serde wire types.
- Clients and connectors: Rust (desktop app on [Rune](https://github.com/anomalyco/rune)); iOS in Swift, built with [xtool](https://xtool.sh).

Status: early. See `TODO.md` for the roadmap and `docs/design.md` for what viz is for.

## Run

```sh
mkdir -p ~/.config/viz
cp config.example.toml ~/.config/viz/config.toml
# Enable the ACP harness you want in config.toml.
nix run .#server
# In another terminal, open the desktop app to create or import a thread:
nix run .#desktop
# Send a message to that thread:
nix run .#viz -- send --thread 42 "hello"
```

The connectors are available as `nix run .#discord` and `nix run .#imessage`.

See the examples for the [server](config.example.toml), [CLI](cli.example.toml), [desktop](desktop.example.toml), [Discord](connectors/discord.example.toml), and [iMessage](connectors/imessage.example.toml).

All clients use a base URL. Rust clients default to `http://127.0.0.1:7420`; set the URL on iOS. To connect across hosts, use an HTTPS reverse proxy and a bearer token. Link environments in both directions in their server configs. No VPN is required.

The server runs in the foreground, supports `Type=notify`, and shuts down on SIGTERM. A systemd TCP activation unit can pass in one `ListenStream` matching `server.listen`; without one the daemon binds the address itself. Configuration changes take effect on restart.

Use **Import thread** to bring sessions available in an ACP harness into viz.

With Home Manager, import `inputs.viz.homeModules.default`. Set `programs.viz.enable = true` and translate the server example into `programs.viz.settings`. See [`nix/home-module.nix`](nix/home-module.nix) for the module's options and examples.
