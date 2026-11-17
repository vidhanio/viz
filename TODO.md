# TODO

Roadmap and open work. Check items off in the same change that lands them; record decisions in `docs/decisions/` and session progress in `docs/log/`.

## Done (scaffold, 2026-11-17)

- [x] Flake: dev shell (nightly Rust, Bun + bun2nix, Swift 6.4 + xtool from `nixpkgs-xtool`, Rune libs, treefmt), packages, checks, `homeModules.default`
- [x] Layered XDG TOML config in Rust (`viz-config`) and TS (`backend/src/config.ts`), secrets as inline/file/env
- [x] Backend: pi-durable harness on SQLite (Bun `node:sqlite`), orchestrator root conversation, `subagent` tool, JSON-RPC/WebSocket server on Unix socket + token-authenticated TCP, connector reply routing, tests with pi-ai's faux provider
- [x] `viz-client` (Rust), `viz` CLI (`config`, `status`, `send`)
- [x] `viz-discord` connector (DMs + mentions from allowed users → orchestrator → reply in place)
- [x] Home Manager module: `settings`, `mutableSettings` (upstream impure merger), `environmentFile`, services for backend and connectors
- [x] iOS app skeleton (xtool project, WebSocket client, sidebar + chat)
- [x] AGENTS.md, architecture, config reference, decision records, findings

## Backend

- [ ] **Durable connector outbox.** Replies to connectors are routed in memory and dropped if the connector is offline or the backend restarts mid-run. Persist pending replies (pi-durable document keyed by connector), deliver on reconnect, let connectors ack; dedupe by submission id.
- [ ] **uniichat memory** (`docs/findings/uniichat.md`): extension that summarizes every message into a ≤512-byte line in the background with a cheap model, merges lines pairwise into a binary tree (pi-durable document), renders the orchestrator's context as a 128 KB view of the tree (`beforeRequest` hook, prompt-cache-friendly blocks), and gives the model `zoom`/`date` tools to expand any node down to the original message. Decide how it coexists with pi-durable compaction (likely: replaces it for the root conversation).
- [ ] **Orchestrator tools** (t3code's thread toolkit, uniichat's `spawn`/`tell`): list/read/search conversations, send to a conversation (`queue`/`steer`), background subagents that report back as messages, abort, schedule tasks (pi-durable timers).
- [ ] **ACP harness manager** (`docs/findings/acp.md`): spawn `[harnesses.<name>]` with `@agentclientprotocol/sdk` (already a dependency), one ACP session per viz conversation; map `session/update` to custom entries so UIs render it; route `session/request_permission` to approvals in the UI; `session/cancel` on abort; `session/load`/`resume` after restart. Tools: start/send/status/cancel a harness session. Capability flags per harness, never name checks.
- [ ] **MCP endpoint** for spawned harnesses (`mcpServers` in `session/new`): expose viz's orchestrator tools to them, deferred listing via a `tool_search` tool (t3code pattern).
- [ ] Conversation metadata document (title, origin, pinned/archived/unread) for the sidebar; title generation.
- [ ] Approvals: `beforeTool` hook + memo for dangerous tools, surfaced as protocol requests.
- [ ] Credentials: persist pi-ai `CredentialStore` in the state dir; OAuth login (Claude Pro/Max, ChatGPT, Copilot) driven from a UI.
- [ ] Auth: per-client tokens, pairing codes (QR for iOS), scopes; `tailscale serve` helper (t3code's `--tailscale-serve`).
- [ ] Expose viz as an ACP agent itself, so editors (Zed) can talk to the orchestrator.
- [ ] Reload config without restart (currently restart via systemd `X-Restart-Triggers`).
- [ ] Decide whether to track pi-durable `main` (breaking changes queued) or stay on npm releases.

## Protocol

- [ ] Generate Rust (`typify`) and Swift (`quicktype` or hand-rolled from schema) types from `protocol/schema.json` instead of mirroring by hand.
- [ ] Methods: abort, compact, configure (model/thinking), fork, archive; approvals; paging older entries (`ConversationTranscript.loadEarlier`).
- [ ] Protocol version negotiation beyond the `hello` equality check.

## Connectors

- [ ] Discord: typing indicator until the reply lands, attachments (images to the model), threads as forked conversations, slash commands (`/abort`, `/model`), reconnect with backoff when the backend restarts.
- [ ] iMessage: implement the BlueBubbles bridge (REST + webhooks/socket), or an alternative if BlueBubbles' API doesn't fit.
- [ ] Shared connector runtime crate once a second real connector exists (reconnect loop, origin helpers, chunking).
- [ ] More connectors: Telegram, email, Matrix, Signal.

## Desktop (Rune)

- [x] Skeleton: sidebar + transcript + input, live view via delta ops (headless snapshots verified; live submit untested)
- [ ] Subagents nested under their tool calls (`details.conversationId`), tool output, diffs, approvals
- [ ] Settings UI writing `config.d/` drop-ins
- [ ] Tray/notifications (Rune `dbus` feature), global shortcut to summon

## iOS

- [ ] `xtool setup` (Apple ID or App Store Connect key, Xcode 27.1 .xip) → `swift sdk list` shows `darwin`; keep `programs.xtool.sdk.enable` off (its link targets `~/.swiftpm`, xtool reads `$XDG_CONFIG_HOME/swiftpm`)
- [ ] First USB device build: Developer Mode, trust the profile, record the installed bundle id prefix; document in AGENTS.md
- [ ] `tailscale serve` HTTPS in front of `[backend].listen` (ATS); or an `Info.plist` ATS exception via `xtool.yml: infoPath`
- [ ] App icon (`xtool.yml: iconPath`, 1024×1024 PNG)
- [ ] Release path: paid account + GitHub macOS runner or Xcode Cloud (Linux-only upload conflicts with the Xcode license)
- [ ] Evaluate isim (kolabs-dev/isim) for simulator runs / rendering `#Preview` on Linux
- [ ] Watch conversations: Chord delta apply in Swift, transcript rendering, sidebar from `conversations.list`
- [ ] Pairing (QR from desktop/CLI), token in Keychain instead of `@AppStorage`
- [ ] Push notifications (needs a relay; out of scope until remote auth exists)

## Nix / ops

- [ ] NixOS VM test for the Home Manager module (services start, socket reachable, connector connects)
- [ ] CI: GitHub Actions running `nix flake check`
- [ ] Drop `nixpkgs-xtool` once NixOS/nixpkgs#548520 merges; drop the bun2nix fork once nix-community/bun2nix#110 merges
- [ ] `cargo deny` config and check
- [ ] Wire into vidhanix (`programs.viz`, sops secrets, `persist.directories = [ ".local/state/viz" ]`)
