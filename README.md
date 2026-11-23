# viz

viz runs persistent ACP threads on one or more configured environments. Threads can delegate tasks locally or to another server; each environment runs its own harnesses and stores its own history.

- Server: Rust, Tokio, and SQLite via Toasty. Server and Rust clients share their Serde wire types.
- Clients: Rust (desktop app on [Rune](https://github.com/anomalyco/rune)).

Status: early.

## Run

```sh
mkdir -p ~/.config/viz
cp config.example.toml ~/.config/viz/config.toml
# Enable the ACP harness you want in config.toml.
nix run .#server
# In another terminal, open the desktop app to create a thread:
nix run .#desktop
# Send a message to that thread:
nix run .#viz -- send --thread 42 "hello"
```

One file configures everything: see [`config.example.toml`](config.example.toml). The server reads `[server]`; the desktop and CLI read the shared `[client]`, and the desktop also reads `[desktop]`; override the path with `VIZ_SERVER_CONFIG`, `VIZ_DESKTOP_CONFIG`, or `VIZ_CLI_CONFIG`.

All clients use a base URL, defaulting to `http://127.0.0.1:7420`. To connect across hosts, use an HTTPS reverse proxy and a bearer token. Link environments in both directions in their server configs. No VPN is required.

The server runs in the foreground, supports `Type=notify`, and shuts down on SIGTERM. A systemd TCP activation unit can pass in one `ListenStream` matching `server.listen`; without one the daemon binds the address itself. Configuration changes take effect on restart.

With Home Manager, import `inputs.viz.homeModules.default`. Set `programs.viz.enable = true` and translate the example into `programs.viz.settings`. See [`nix/home-module.nix`](nix/home-module.nix) for the module's options and examples.

## Client diagnostics

The CLI and desktop write warnings to stderr by default. Use `RUST_LOG` to
enable connection diagnostics without changing command output:

```sh
RUST_LOG=viz_client=debug nix run .#viz -- status
RUST_LOG=viz_desktop=debug,viz_client=debug nix run .#desktop
```
