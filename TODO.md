# TODO

Roadmap and open work. Check items off in the same change that lands them; record decisions in `docs/decisions/` and session progress in `docs/log/`.

## Done (scaffold, 2026-11-17)

- [x] Flake: dev shell (nightly Rust, Bun + bun2nix, Swift 6.4 + xtool from `nixpkgs-xtool`, Rune libs, treefmt), packages, checks, `homeModules.default`
- [x] Layered XDG TOML config in Rust (`viz-config`) and TS (`backend/src/config.ts`), secrets as inline/file/env
- [x] Backend: pi-durable harness on SQLite (Bun `node:sqlite`), orchestrator root conversation, JSON-RPC/WebSocket server on Unix socket + token-authenticated TCP, connector reply routing
- [x] `viz-client` (Rust), `viz` CLI (`config`, `status`, `send`)
- [x] `viz-discord` connector (DMs + mentions from allowed users → orchestrator → reply in place)
- [x] Home Manager module: `settings`, `mutableSettings` (upstream impure merger), `environmentFile`, services for backend and connectors
- [x] iOS app skeleton (xtool project, WebSocket client, sidebar + chat)
- [x] AGENTS.md, architecture, config reference, decision records, findings

## Backend

- [ ] **Durable connector outbox.** Replies to connectors are routed in memory and dropped if the connector is offline or the backend restarts mid-run. Persist pending replies (pi-durable document keyed by connector), deliver on reconnect, let connectors ack; dedupe by submission id.
- [x] **uniichat memory** for the orchestrator (`backend/src/memory/`, `docs/architecture.md` "Memory"): transcript-derived messages, summary tree in `memory.sqlite`, 128 KB view before each turn, `zoom`/`date`, Anthropic cache mark on the view
- [ ] Memory follow-ups: prompt-cache marks on compaction calls (uniichat marks their view block too); images in messages and `zoom` (uniichat stores them by hash); `zoom` on a subagent's conversation (uniichat's `zoom("Name")`); import past Claude/ChatGPT chats as a separate tree side; tell the user when turns wait on a failing compactor instead of only logging; pi-durable's own compaction is still enabled as a safety net for very long single turns
- [x] **Orchestrator tools** (t3code's thread toolkit, uniichat's `spawn`/`tell`): `list_conversations`, `read_conversation`, `search_conversations`, `spawn_chat` (background, reports back as a `[chat N]` input), `message_conversation` (queues), `cancel_conversation`.
- [ ] Orchestrator tools follow-ups: steer into a running ACP turn, scheduled tasks (pi-durable timers), recover a worktree left by a crash between creation and saving it.
- [x] **Detached ACP delegation**: the orchestrator delegates only through `spawn_chat`; chat results arrive as inputs, and legacy task-owned chats remain usable.
- [x] **Scoped MCP progress** for ACP delegations: loopback HTTP or stdio bridge, per-turn token, no access to unrelated conversations.
- [x] **Persistent ACP app chats**: keep the runner process and session across turns, restore the same session via `session/resume` or `session/load` after restart, and fail explicitly if restoration is unsupported. The orchestrator is the only native model loop.
- [x] **Long-lived ACP subagents**: finished subagents accept follow-ups; ACP agents spawn/list/read/message/cancel their own subagents over MCP. Parent transcript tool records link to the affected child when its runner forwards the result (`docs/decisions/0004-ade-workspace.md`).
- [x] **ACP permissions**: viz allows every request (first `allow_once`, else `allow_always`). Per-chat access and approvals existed briefly and were removed as unneeded.
- [ ] Broader MCP toolkit (see Orchestrator tools above), with deferred discovery via `tool_search`.
- [x] Conversation metadata (`viz.meta`: project, parent, archived, pinned, unread, worktree, timestamps) for the sidebar; rename.
- [x] **Conversation deletion**: tombstone a chat subtree, stop its turns, remove its search hits, and clean up unshared clean worktrees. Protect the orchestrator from thread management.
- [x] **ADE workspace** (t3code parity, 2026-11-18): projects, per-chat model/effort/mode from the runner's ACP config options (config = default), slash commands, `@` file search, stop, queue view and removal, attachments, full-text search, history paging, git worktree chats, per-turn checkpoints with diff and revert, commit (generated message) / push / PR.
- [ ] ADE follow-ups (from the t3code gap analysis): terminal panes (server PTYs; Bun lacks openpty), in-thread find, edit-from-here / fork a chat at a message, plan-mode approve → implement, usage and limits panel, settings UI and rebindable keys, harness probe (capabilities and models per runner before a chat exists), scratch directories for project-less chats, multi-model fan-out to parallel worktrees.
- [ ] Credentials: persist pi-ai `CredentialStore` in the state dir; OAuth login (Claude Pro/Max, ChatGPT, Copilot) driven from a UI.
- [ ] Auth: per-client tokens, pairing codes (QR for iOS), scopes; `tailscale serve` helper (t3code's `--tailscale-serve`).
- [ ] Expose viz as an ACP agent itself, so editors (Zed) can talk to the orchestrator.
- [ ] Reload config without restart (currently restart via systemd `X-Restart-Triggers`).
- [ ] Decide whether to track pi-durable `main` (breaking changes queued) or stay on npm releases.

## Protocol

- [ ] Generate Rust (`typify`) and Swift (`quicktype` or hand-rolled from schema) types from `protocol/schema.json` instead of mirroring by hand.
- [ ] Methods: compact, fork. (Done: abort, configure, archive, delete, paging via `conversation.entries`.)
- [ ] Protocol version negotiation beyond the `hello` equality check.

## Connectors

- [ ] Discord: typing indicator until the reply lands, attachments (images to the model), threads as forked conversations, slash commands (`/abort`, `/model`), reconnect with backoff when the backend restarts.
- [ ] iMessage: implement the BlueBubbles bridge (REST + webhooks/socket), or an alternative if BlueBubbles' API doesn't fit.
- [ ] Shared connector runtime crate once a second real connector exists (reconnect loop, origin helpers, chunking).
- [ ] More connectors: Telegram, email, Matrix, Signal.

## Desktop (Rune)

- [x] Skeleton: sidebar + transcript + input, live view via delta ops
- [x] Redesign on rune-kit + OpenCode theme (light/dark): split shell with collapsible sidebar (Orchestrator, Chats, Subagents, busy spinners, connection footer), thread header (model, thinking, usage), Markdown timeline with grouped collapsible tool rows, subagent rows that open the child, turn dividers, streaming answer, auto-growing composer, toasts
- [x] ADE redesign (2026-11-18): flat theme and `ui/` components; sidebar tree (orchestrator, pinned, projects → chats → subagents, status, unread, filter, archive, rename/move); header without the model; composer with model/effort/mode selects, `/` commands, `@` files, attachments, stop, queue rows; plan checklist; ACP tool rows with diffs; Ctrl/Cmd+K palette with full-text search; Ctrl/Cmd+D changes panel (diffs, branch, commit, push, PR, revert); worktree chats; desktop notifications and window-title status. (Its access select and approval card were later removed with ACP approvals.)
- [x] Delete a chat from its row menu or the command palette, with confirmation and retained-worktree paths.
- [ ] Settings UI writing `config.d/` drop-ins
- [ ] Tray, global shortcut to summon

## iOS

- [x] Delete chats via a destructive swipe with confirmation; no pin, archive, or delete on the orchestrator.
- [ ] `xtool setup` (Apple ID or App Store Connect key, Xcode 27.1 .xip) → `swift sdk list` shows `darwin`; keep `programs.xtool.sdk.enable` off (its link targets `~/.swiftpm`, xtool reads `$XDG_CONFIG_HOME/swiftpm`)
- [ ] First USB device build: Developer Mode, trust the profile, record the installed bundle id prefix; document in AGENTS.md
- [ ] `tailscale serve` HTTPS in front of `[backend].listen` (ATS); or an `Info.plist` ATS exception via `xtool.yml: infoPath`
- [ ] App icon (`xtool.yml: iconPath`, 1024×1024 PNG)
- [ ] Release path: paid account + GitHub macOS runner or Xcode Cloud (Linux-only upload conflicts with the Xcode license)
- [ ] Evaluate isim (kolabs-dev/isim) for simulator runs / rendering `#Preview` on Linux
- [ ] Watch conversations: Chord delta apply in Swift, transcript rendering, sidebar from `conversations.list`
- [ ] Pairing (QR from desktop/CLI), token in Keychain instead of `@AppStorage`
- [ ] Push notifications (needs a relay; out of scope until remote auth exists)
- [ ] Replace the hand-rolled JSON-RPC correlation in `Backend.swift` with `ChimeHQ/JSONRPC` (needs a small custom `DataChannel` to send the `Authorization` header); blocked on a device build to verify

## Nix / ops

- [ ] Deploy the Home Manager module on a real host (services start, socket reachable, connector connects)
- [ ] CI: GitHub Actions running `nix flake check`
- [ ] Drop `nixpkgs-xtool` once NixOS/nixpkgs#548520 merges; drop the bun2nix fork once nix-community/bun2nix#110 merges
- [ ] `cargo deny` config and check
- [ ] Wire into vidhanix (`programs.viz`, sops secrets, `persist.directories = [ ".local/state/viz" ]`)
