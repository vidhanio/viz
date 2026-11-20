# TODO

Roadmap and open work. Check items off in the same change that lands them.

## Backend

- [x] Rust/Tokio backend, Toasty SQLite workspace, and Serde TOML config with lexical drop-ins.
- [x] Configured environments over ordinary HTTP(S), independent stores, and a URL-only client connection.
- [x] Reusable ACP threads, per-input runs, and durable delegated tasks. Deliver the task result to its parent once, without making later runs part of that task.
- [x] Project metadata keyed by absolute directory, searchable transcripts, paged entries, queue editing, and attachments.
- [x] Discover ACP model and thinking options. Import existing ACP sessions when the harness supports listing and loading.
- [x] Session-scoped MCP tools for delegation, threads, projects, and environment discovery.
- [ ] Durable connector outbox: persist replies when a connector disconnects, and deduplicate on reconnect.
- [ ] Scheduled tasks, editing or forking a thread at a past message, and in-thread find.
- [ ] Real ACP steering if the protocol gains an in-turn submission method.
- [ ] Pairing and per-client tokens.

## Protocol

- [x] Backend and Rust clients share Serde types and generate JSON Schema from them.
- [ ] Generate Swift payload types from the schema.

## Connectors

- [ ] Discord: typing indicator until the reply lands, attachments (images to the model), threads as forked conversations, slash commands (`/abort`, `/model`), reconnect with backoff when the backend restarts.
- [ ] iMessage: implement the BlueBubbles bridge (REST + webhooks/socket), or an alternative if BlueBubbles' API doesn't fit.
- [ ] Shared connector runtime crate once a second real connector exists (reconnect loop, origin helpers, chunking).
- [ ] More connectors: Telegram, email, Matrix, Signal.

## Desktop (Rune)

- [x] Environment picker, project sidebar, paged transcript, and import flow.
- [x] Configurable dark and light base16 schemes and Home Manager Stylix integration.
- [x] Harness icons and configured harness display names; full model and thinking labels, inline thinking buttons, and animated model search.
- [x] Grouped timeline activity and persistent task panel; stream reasoning and running commands.
- [x] Use ACP titles and context-window reports when the runner sends them.
- [ ] Settings UI for desktop preferences in `desktop.toml`
- [ ] Tray, global shortcut to summon

## iOS

- [x] Project-grouped threads, search, live activity, and importing ACP sessions.
- [ ] `xtool setup` (Apple ID or App Store Connect key, Xcode 27.1 .xip) → `swift sdk list` shows `darwin`; keep `programs.xtool.sdk.enable` off (its link targets `~/.swiftpm`, xtool reads `$XDG_CONFIG_HOME/swiftpm`)
- [ ] First USB device build: Developer Mode, trust the profile, record the installed bundle id prefix; document in AGENTS.md
- [ ] Connect a real device over HTTPS and move the bearer token into the Keychain.
- [ ] App icon (`xtool.yml: iconPath`, 1024×1024 PNG)
- [ ] Release path: paid account + GitHub macOS runner or Xcode Cloud (Linux-only upload conflicts with the Xcode license)
- [ ] Evaluate isim (kolabs-dev/isim) for simulator runs / rendering `#Preview` on Linux
- [ ] Push notifications (needs a relay; out of scope until remote auth exists)
- [ ] Replace the hand-rolled JSON-RPC correlation in `Backend.swift` with `ChimeHQ/JSONRPC` (needs a small custom `DataChannel` to send the `Authorization` header); blocked on a device build to verify

## Nix / ops

- [x] Foreground systemd service with readiness notification, optional TCP activation, and graceful shutdown.
- [ ] Deploy the Home Manager module on a host and exercise connector delivery.
- [ ] CI: GitHub Actions running `nix flake check`
- [ ] Drop `nixpkgs-xtool` once NixOS/nixpkgs#548520 merges.
- [ ] `cargo deny` config and check
- [ ] Wire into vidhanix (`programs.viz`, sops secrets, `persist.directories = [ ".local/state/viz" ]`)
