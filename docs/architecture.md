# Architecture

Current state as of the scaffold (2026-11-17). Plans live in `TODO.md`; reasons in `docs/decisions/`.

```mermaid
flowchart LR
  subgraph clients[Clients]
    desktop[viz-desktop<br/>Rune]
    ios[iOS app<br/>SwiftUI + xtool]
    cli[viz CLI]
  end
  subgraph connectors[Connectors]
    discord[viz-discord]
    imessage[viz-imessage]
  end
  subgraph backend[viz-backend · Bun]
    server[server.ts<br/>JSON-RPC over WebSocket]
    harness[pi-durable Harness]
    db[(viz.sqlite<br/>$XDG_STATE_HOME/viz)]
  end
  harnesses[ACP harnesses<br/>claude-agent-acp, codex-acp, opencode, pi-acp]

  desktop & cli & discord & imessage -- unix socket --> server
  ios -- TCP + token over tailnet --> server
  server --> harness --> db
  harness -. planned .-> harnesses
```

## Backend (`backend/`)

`viz-backend` is the only process that owns state. It opens a pi-durable `Harness` over SQLite (`openNodeSqliteStorage`, which runs on Bun's `node:sqlite`) at `$STATE_DIRECTORY` or `$XDG_STATE_HOME/viz/viz.sqlite`. pi-durable is single-writer: other processes never open the database; they talk to the backend.

- **Orchestrator**: pi-durable's reserved root conversation (`harness.root()`), with the `CodingTools`, `viz.memory`, `viz.orchestrator` (turns prompt) and `viz.subagent` extensions; model, thinking level, instructions, cwd and extensions are re-applied from config on every start. Its history is unbounded through its memory (below).
- **Subagents**: the `subagent` tool creates a conversation owned by the tool call (`ownership: { kind: "task" }`), so aborting the call aborts it, a crash resumes it, and UIs can attach to it through the call's `details.conversationId`. Runs on `[subagents].model`, without the orchestrator's memory and prompt.
- **App chats**: `conversation.create` makes an ownerless conversation, the sidebar's other entries.
- **Durability**: every model request and tool call is a checkpointed pi-durable task. `requestId` on `conversation.submit` makes admission idempotent (connectors use the platform message id).

## Memory (`backend/src/memory/`)

The orchestrator's memory is a port of uniichat's (`docs/findings/uniichat.md`; source `Memory.ts`, `prompt.txt` at `0500bf6`), on pi-durable's run loop instead of uniichat's:

- `sync.ts` derives messages from the orchestrator's transcript as entries commit (a watch on the root): user inputs `user`, answers `viz`, each tool call with its result as `tool` + `echo`. The cursor is stored with the messages, so the derivation is exactly-once across crashes. The pi-durable transcript stays the source of truth.
- `memory.ts` keeps those messages and the summary tree in `$STATE_DIRECTORY/memory.sqlite`: every message becomes a ≤512-byte line (as is if it fits, else written by the `[memory].compactor` model), adjacent lines merge pairwise up the tree, 64 compactions at a time, a failing one asked again every 60 s. Two views over the tree (turns': 64-128 KB; compactions': 16-32 KB) merge their most-aged pairs down to half when full, so a view changes only at its end between batches and stays prompt-cache friendly.
- `extension.ts`: a turn starts from a fresh context (`VizHarness.submit` resets the idle orchestrator before a new input; inputs while it works are steered into the running turn). Before each model request, `beforeRequest` waits until every message before the turn is summarized and puts the view in front of the turn's first message ("Turn: the user's new message"); on Anthropic, `beforeProviderRequest` marks the view's last whole 4-line block for the prompt cache. Tools: `zoom(id, n, at)` opens a line into its halves or a message whole, `date(id)`.
- `prompt.ts`: uniichat's prompt adapted to viz; compactions use its memory part as their whole system prompt.

Not ported: imports of other chats, images, `zoom` on a subagent's chat, billing holds.

## Protocol (`backend/src/protocol.ts`)

JSON-RPC 2.0 over a WebSocket at `/v1/rpc`, plus `GET /v1/health`. `protocol.ts` is the source of truth (pi-schema builders); `bun run schema` emits `protocol/schema.json` for the Rust and Swift clients. Clients call `hello` first with `{ kind: app | connector | cli, name }`.

Listeners (see `docs/decisions/0001-remote-transport.md`):

- Unix socket `$XDG_RUNTIME_DIR/viz/backend.sock`, mode 0600: local clients, no token.
- Optional TCP `[backend].listen` (a tailnet or LAN address): bearer token required (`Authorization` header, or `?token=` for clients that can't set headers).

## Message routing

Every connector input goes into the orchestrator (or a named conversation) with an `origin` (`connector`, `channel`, optional `thread`/`message`/`author`). The backend stores the origin with the transcript entry (`kind: "viz.input"`, `data.origin`) and prefixes the model-visible text with a tag like `[discord #123 @alice]`, so the orchestrator knows where an input came from.

- **Connectors** receive only `reply` notifications for inputs whose `origin.connector` is their own `hello` name, and render just those replies.
- **Apps** (desktop, iOS) `conversation.watch` conversations and receive the full view (`conversation.view`: full value, then Chord delta ops): every message from every source, tool calls, subagents, live state.

Reply routing is in-memory today: a reply to a connector that is disconnected when the answer lands is dropped (durable outbox: `TODO.md`).

## Configuration

One TOML document, layered from `config.toml` + `config.d/*.toml` in `$XDG_CONFIG_HOME/viz`, read independently by the backend (TS) and every Rust component; each parses only its own sections. Home Manager writes `config.toml` (read-only link, or merged into a writable file with `mutableSettings`). Reference: `docs/config.md`; rationale: `docs/decisions/0002-config-layering.md`.

## Nix

- `packages`: `viz-backend` (bun2nix, `bun build --compile`), `viz`, `viz-discord`, `viz-imessage`, `viz-desktop` (crane; desktop wrapped with Rune's runtime libraries), `default` (all).
- `checks`: clippy, backend `bun check`, treefmt. No tests (`AGENTS.md`).
- `homeModules.default`: `programs.viz` with `settings`, `mutableSettings`, `environmentFile`, `backend.enable`, `desktop.enable`, `connectors.<name>.enable`; systemd user services `viz-backend` (`StateDirectory=viz`, `RuntimeDirectory=viz`) and `viz-<connector>`.
- Dev shell: nightly Rust, Bun, bun2nix, Swift 6.4 + xtool from the `nixpkgs-xtool` input (nixpkgs PR #548520, same as vidhanix), Rune's libraries.

## Relation to reference projects

- **uniichat** (`docs/findings/uniichat.md`): the single never-ending chat, its summary-tree memory with `zoom` (above), and every window being a live view of server-owned state.
- **t3code** (`docs/findings/t3code.md`): the contract-only protocol module, per-thread snapshot + delta subscriptions, idempotent commands, token-authenticated remote access over tailnet, capability-driven harness adapters, and MCP as the tool surface for spawned harnesses.
- **ACP** (`docs/findings/acp.md`): the planned way viz drives other harnesses (viz is the ACP client; each external session maps to a viz conversation).
