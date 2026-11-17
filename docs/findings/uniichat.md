# uniichat (VictorTaelin/uniichat)

_Research snapshot 2026-11-17 (scout). Re-verify against upstream before relying on details._

**URL**: https://github.com/VictorTaelin/uniichat · TypeScript on Bun · Apache-2.0 · 678★ · hosted at uniichat.com. README explicitly says "**This repo is vibe-coded.** AI agents wrote nearly all of it".

### What it is

A self-hostable, single-user-per-account AI agent with **one chat that never ends**. No "new chat": the server owns the conversation; every client (browser tab, `unii` terminal, iOS app) is a live _view_ that can read, send, stop work, and change model. Data lives in `~/.unii/users/<id>` (`$UNII_DIR`), config in `~/.unii/config.json` + `UNII_*` env.

### Core idea (README quotes)

> **One chat, a memory tree.** Every message is summarized into one line, and neighboring lines are merged in pairs, again and again, into a binary tree. Each call sees a view of the whole chat, up to 128 KB: recent messages one line each, older ones covered by fewer, broader lines. Detail fades with age, but nothing is lost.

> **Zoom.** The agent can open any line into the two it was made from, down to the original message. So its context never rots, and it never needs a "compaction" that throws the past away.

> **Built in the background.** Summaries are written by a cheaper model (the compactor) as messages arrive. The view changes only at its end, so each call reads most of its input from the vendor's prompt cache.

> **Every window is a view.** The server owns the chat. A browser tab, or `unii` in a terminal, shows it live; any of them can send a message, stop the work, or change the model.

> **Agents and tools.** … It can spawn subagents, which work in the background and report back as messages.

> **Import.** Bring your past Claude and ChatGPT chats … they join the tree.

### Data model (verified in source)

- **Messages** (`main/`, one JSON line per message, file per day; `main.index` gives line slot, 16-byte slots):
  `type Msg = { kind; text; date?; images?: string[]; imported?: boolean; line?: string; more?: boolean; act?: string }` — `Memory.ts:79`
  Kinds (`AskAI.ts:29`, restated in `prompt.txt`): `"user" | "work" | "mind" | "unii" | "tool" | "echo" | "note"` (imported chats add `ai` in the prompt's taxonomy).
- **Tree nodes** (`tree/`): node `(0,i)` = message `i` (or the line it was logged with) capped at `NODE = 512` bytes; node `(l,i)` merges `(l-1,2i)` and `(l-1,2i+1)`. A source that fits _is_ its node; otherwise a **compaction** call writes it. Header comment `Memory.ts:1-56`.
- **View**: an ordered list of `Part = {l,i}` (`[id,n]`, n a power of two) covering the chat to its end, oldest first, rendered as lines `id+n|text`. Past `HIGH = 128000` bytes a batch merges adjacent built siblings down to half; compaction views use `SEEN = 32000`. `BLOCK = 4` lines per request block; the last whole block is marked as the prompt-cache prefix. Constants `Memory.ts:140-150`; `view()` `Memory.ts:1009`.
- **Zoom** (`Memory.ts:1028`): `zoom(id,n)` → for `n=1` the whole message (paginated by `at` chars, images on first page); else the two halves `n/2` each. `zoom("Name")` returns an agent's whole chat; `date(id)` its timestamp.
- **Persistence/durability**: `view.json` keeps `{parts, batch, seen?, main, tree, pick, goal?, work?}`; `main/` doubles as the **act log** — an act is done once its JSON line is recorded, idempotent by id (`Chat.act`/`Chat.apply`), so a crash resumes and a deploy can hand a chat to another process with in-flight calls continuing where they started (`Server.ts:1-24`, `Memory.ts:1-56`).
- **Images** by hash under `docs/images/`; **agent logs** under `docs/agent/<Name>.jsonl`.

### Chats, threads, agents

- One `Memory`/`Chat` per signed-in user; one turn at a time; messages arriving mid-turn are injected between tool calls or wait for the next call (`Server.ts` header).
- **Subagents** are separate AI loops with their own log; each call gets the parent's view + the user's rules + its own full log, so it survives restarts. Tool surface (MCP over a random localhost port, authenticated by the chat's secret, caller `main` or agent name; `Tools.ts:1-24`, `Tools.ts:196-211`):
  - `zoom`, `date` — navigate the tree.
  - `spawn(tasks[], effort?)` — start one subagent per task _in parallel_, return names immediately; each report arrives as a `"[Name] report"` message.
  - `tell(name, message)` — message a running/idle agent; reply returns as `"[Name] reply"`.
  - `computer(device, task)` — visible-screen agent (screenshots, mouse, keyboard) on a device marked "computer use".
  - `shell`, `read`, `write`, `edit` run on the `device` named in the call (each `unii` client is a device: `host`, `host-2`, …).
- **Compaction** is just another call: cheaper model, same system prompt minus tools, own view ending at the node, no tools; task text asks for ≤512-byte line (`Memory.ts:171-190`). `JOBS = 64` concurrent builds per side.
- **Imports** become a separate "side" of the tree with their own build queue/payer; chat turns never wait on them.

### Wire protocol (client ⇄ server)

Not ACP: a server-owned WebSocket/HTTP protocol in `View.ts` (`WIRE = 2`, 4 MiB frame cap).

- `Client` acts (`View.ts:126-139`): `say`, `import`, `voice`, `stop`, `goal`, `go`, `pick`, `out` (job output), `zoom`, `root`, `agent`, `guess`.
- `Server` messages (`View.ts:97-113`): `know`, `view`, `zoom`, `root`, `entry`, `delta`, `live`, `state`, `agent`, `note`, `ack`, `jobs`, `job`.
- Turn state (`busy`, `wait`, `agents`, `pick`, `models`, `credits`, …) (`View.ts:60-73`).

**Takeaway for viz**: implement an append-only message log + summary tree where every node is addressable and expandable on demand; background compaction with a cheap model; server-owned per-user chat with idempotent acts; subagents as independent sessions whose only channel back is a message. The view is a _derived_ artifact recomputed at batch boundaries to maximize prompt-cache hits — not stored context.

---
