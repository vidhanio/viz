# Pi Durable — API research (for the viz Bun backend)


_Research snapshot 2026-11-17 (scout), against `@earendil-works/pi-durable@1.5.2`._
Read-only investigation. No files created/modified; no commands executed (remote sources only).

## Sources read

| Source | URL |
|---|---|
| durable pkg + exports | `https://raw.githubusercontent.com/earendil-works/pi/main/packages/durable/package.json` |
| durable README (1160 lines) | `https://raw.githubusercontent.com/earendil-works/pi/main/packages/durable/README.md` |
| durable CHANGELOG | `https://raw.githubusercontent.com/earendil-works/pi/main/packages/durable/CHANGELOG.md` |
| durable root exports | `https://raw.githubusercontent.com/earendil-works/pi/main/packages/durable/src/index.ts` |
| Harness/conversation/tool types | `https://raw.githubusercontent.com/earendil-works/pi/main/packages/durable/src/harness/types.ts` |
| Conversation view | `https://raw.githubusercontent.com/earendil-works/pi/main/packages/durable/src/harness/view.ts` |
| Harness.open | `https://raw.githubusercontent.com/earendil-works/pi/main/packages/durable/src/harness/harness.ts` |
| Documents | `https://raw.githubusercontent.com/earendil-works/pi/main/packages/durable/src/documents.ts` |
| SQLite storage | `.../packages/durable/src/storage/sqlite/{index,node,storage}.ts` |
| Subagent example | `https://raw.githubusercontent.com/earendil-works/pi/main/packages/durable/test/examples/21-subagent-foreground.ts` |
| Remoting example | `https://raw.githubusercontent.com/earendil-works/pi/main/packages/durable/test/examples/36-durable-views.ts` |
| chord sqlite facade/Node adapter | `.../packages/chord/src/sqlite/{database,node}.ts`, `.../chord/src/sqlite/index.ts` |
| chord delta/ops | `https://raw.githubusercontent.com/earendil-works/pi/main/packages/chord/src/delta/README.md` |
| chord sync | `.../packages/chord/src/sync/index.ts`, `client.ts` |
| pi-ai README/types/oauth | `.../packages/ai/README.md`, `.../packages/ai/src/types.ts:148`, `.../packages/ai/src/auth/oauth/load.ts`, `.../packages/ai/src/bun-oauth.ts` |
| coding-agent RPC | `.../packages/coding-agent/docs/rpc.md` |
| npm registry | `https://registry.npmjs.org/@earendil-works/pi-durable`, `.../pi-ai`, `.../chord`, `https://registry.npmjs.org/pi-acp/latest` |
| pi-acp | `https://raw.githubusercontent.com/svkozak/pi-acp/main/README.md` |
| Bun sqlite | `https://bun.com/blog/bun-v1.4`, `https://bun.com/docs/runtime/sqlite` |

## Summary

- **Published:** `@earendil-works/pi-durable@1.5.2`, `@earendil-works/pi-ai@1.5.2`, `@earendil-works/chord@1.5.2` (dist-tag `latest`). durable 1.5.2 dated 2026-11-10; all `@earendil-works/pi-*` deps are pinned exactly `1.5.2`; `engines.node >=22.19.0`; package self-describes as **Experimental** (README line 2).
- **No Bun SQLite export.** SQLite adapters are `storage/sqlite/node` (Node `node:sqlite` `DatabaseSync`), `storage/sqlite/cloudflare`, and the portable `SqliteStorage` over the 7-method `SqliteDatabase` facade. Bun ≥1.4 implements `node:sqlite` (100% of Node 26.3’s sqlite tests), so the Node adapter should work unchanged; **UNVERIFIED at runtime here**. Otherwise wrap `bun:sqlite` in ~30 LOC.
- **No `viewState()` / `watchEvents()` and no durable client/server transport.** Realtime API is `Conversation.watch()` / `Harness.watchTaskGraph()` / `observeConversation()`; frames carry a full-ish `ConversationView` plus Chord delta op tuples. You write the HTTP/WS layer (Chord supplies delta `apply` and `StoreStorage`/`self.follow` for Chord-object remoting).
- **Subagents are ordinary conversations** created in the tool’s commit with `ownership: { kind: "task", taskId }`, found via `tx.scanConversations({ ownerTaskId })`, driven with `api.conversation(id)`.
- **ACP is not in the pi monorepo.** Third-party `pi-acp@0.0.34` bridges ACP JSON-RPC over stdio to `pi --mode rpc`; pi itself offers RPC mode + SDK.
- **Single-writer constraint:** one process per storage file, no cross-process locking (`README.md:1020-1080`). viz server must be the sole writer; read replicas use `HarnessOptions.readOnly`.

---

## Q1 — Versions and `exports`

`packages/durable/package.json` (version 1.5.2) exports exactly:

```text
"."  "./env"  "./env/node"  "./codemode"  "./tool-search"  "./tools"
"./images"  "./images/fit"  "./images/node"  "./images/cloudflare"
"./storage/memory"  "./storage/indexeddb"  "./storage/jsonl"  "./storage/jsonl/node"
"./storage/store"  "./storage/sqlite"  "./storage/sqlite/cloudflare"
"./storage/sqlite/node"  "./testing"  "./package.json"
```

Every entry has `source`/`types`/`import`(+`default`). Note: **no** `./storage/sqlite/bun`, no `./client`, no `./server`, no `./acp`.
Deps: chord 1.5.2, pi-ai 1.5.2, pi-codemode 1.5.2, pi-schema 1.5.2, pi-telemetry 1.5.2, `@silvia-odwyer/photon` 0.3.3, `diff` 8.0.4. `sideEffects: false`.

chord 1.5.2 exports (relevant): `./context`, `./delta`, `./json`, `./watch`, `./output`, `./sqlite`, `./sqlite/node`, `./sqlite/cloudflare`, `./telemetry-sqlite`, `./telemetry-ingest`, `./http`, `./workers`, `./compression`, `./plugins`, `./plugins/runtime`, `./app`, `./web`, `./surfaces`, `./react`, `./tui`, `./sync`. Peer `ws ^8.21.0` optional.

pi-ai 1.5.2 exports: `.`, `./models`, `./session-resources`, `./compat`, `./providers/*`, `./api/*`, `./utils/*`, `./oauth`, `./bedrock-provider`, `./bun-oauth`, `./lazy-bedrock-provider`, `./lazy-oauth`.

## Q2 — SQLite adapters / Bun

No Bun adapter ships. Two routes:

1. **Use Node adapter on Bun:** `openNodeSqliteStorage(path, options?)` → `Promise<SqliteStorage>` (`src/storage/sqlite/node.ts`, full file). Options: `NodeSqliteStorageOptions = NodeSqliteDatabaseOptions & SqliteStorageOptions`, where database options are `{ mode?: "read-write"|"read-only"; walAutoCheckpointPages?: number; busyTimeoutMs?: number }` (defaults 1000 pages / 5000 ms) and storage option `{ prefix?: string }` (table namespacing). It opens `new DatabaseSync(path,{timeout[,readOnly]})`, `mkdir -p` dirname unless `:memory:`, then `PRAGMA journal_mode=WAL`, `synchronous=NORMAL`, `wal_autocheckpoint=N`. Read-only uses `BEGIN`; writer `BEGIN IMMEDIATE`; closes with `wal_checkpoint(TRUNCATE)`.
2. **Wrap `bun:sqlite`** against the portable facade `@earendil-works/chord/sqlite` (`src/sqlite/database.ts:1-60`) and pass it to `SqliteStorage.open(db, { prefix? })` from `@earendil-works/pi-durable/storage/sqlite`:

```ts
type SqliteValue = null | number | bigint | string | Uint8Array;
interface SqliteExecutor {
  exec(sql: string): Promise<void>;
  run(sql: string, ...p: SqliteValue[]): Promise<void>;
  runCounted(sql: string, ...p: SqliteValue[]): Promise<number>;
  get<T extends object>(sql: string, ...p: SqliteValue[]): Promise<T | undefined>;
  all<T extends object>(sql: string, ...p: SqliteValue[]): Promise<T[]>;
}
interface SqliteDatabase extends SqliteExecutor {
  transaction<T>(cb: (tx: SqliteExecutor) => Promise<T>): Promise<T>;
  close(): Promise<void>;
}
```

Contract details (same file): run/exec/all/get are one statement + positional bindings (never interpolate); adapters may cache prepared statements by SQL; unrelated ops and other transactions must queue until a transaction finishes; calling the DB inside a transaction callback must never settle; on callback rejection roll back first, and if rollback fails reject with a *different* error. `runCounted` returns SQLite `changes()` (triggers/index writes excluded). Bun equivalents: `db.exec`, `db.prepare(sql).{run,get,all}(...p)`, `stmt.run().changes`, `db.transaction(fn)`, `db.close()`; enable `safeIntegers` only if you want bigint round-trips. `SqliteValue` includes `bigint`.

Portability statement, `packages/durable/README.md:1020-1080`: the portable cores `/storage/sqlite` and `/storage/jsonl` “run without Node APIs, for example on Bun or in Cloudflare Durable Objects, given an asynchronous `SqliteDatabase` facade”. Bun 1.4 changelog: `node:sqlite` implemented, `DatabaseSync`/`StatementSync`, passes 319/319 Node 26.3 sqlite subtests; docs note Bun’s `Database`/`DatabaseSync` are distinguishable via `db[Symbol.for("sqlite-type")]`.

## Q3 — Remote clients / wire format / transport

- `watch()` exists on a `Conversation` (`src/harness/types.ts`, Conversation interface): `watch(context, window?): Promise<ConversationWatch>`; `ConversationWatch = WatchHandle<ConversationView>`. `ConversationView = { conversation, entries, before?, docs }` (`src/harness/view.ts:29-42`); the window defaults `min: 50`, `max: 100` and supports `from` (`view.ts:44-62`).
- Frames are Chord watches: `watch.value` at attach, then `start(listener)`; listener receives `{ kind: "value", value, ops, retired }` (or `{ kind:"failed", error }` in generic Chord sources; durable watches never send `failed`, they end). Ends: `stopped | aborted | ended | listener_error | failed`.
- Wire encoding of `ops` (`packages/chord/src/delta/README.md`, Operations table): strict-JSON tuples `["r", value]`, `["s", path, value]`, `["d", path]`, `["a", path, text]`, `["t", path, count]`, `["p", path, index, remove, items]`, `["m", path, permutation]`. Paths are keys/non-negative indices. Batches are exact, not canonical. Commits while the listener runs are folded; past 10,000 pending ops the frame becomes `[["r", view]]`. JSON modes (`test/examples/18-json.ts`) emit “raw view operations, or appended entries and `pi.live` per change”.
- No built-in durable HTTP/WS server, no durable client package, no `attach` protocol module. Closest primitives:
  - `harness.observeConversation(id, observer, ctx, window?)`, `observeTaskGraph`, `observeDoc` — synchronous, atomic with memory adoption, intended “for adapters, such as a Chord source over a view”.
  - `StoreStorage` (`storage/store`) puts a Harness inside a Chord host store; `self.follow((observer, ctx) => harness.value.observeConversation(...))` turns a Durable view into a Chord watchable/member/derive (`test/examples/36-durable-views.ts`, full pattern incl. one frame per commit with ops).
  - Chord `./sync` is *session-log replication* (outbox, `encodeBatch`/`encodeSnapshot`/`decodeBatch`, `uploadSession`, `pullSession`, filesystem remote), not a view subscription. Chord `./http` is the generic plugin-host HTTP/WS-upgrade server.
  - Client-side transcript reassembly is provided: `ConversationTranscript` (`merge`, `loadEarlier`, `fillGap`, `resume`) + `conversation.entries(...)`; example 31.
- Practical viz design: one writer process; WebSocket per client; server subscribes via `watch()`, sends `{seq, value?, ops}` frames (initial snapshot = first value; reconnect via `{from: lastEntryId}`); client applies ops with `apply`/`applyImmutable` from `@earendil-works/chord/delta` and reconciles with `ConversationTranscript`. For read-only replicas, open a second Harness with `readOnly: true` (`HarnessOptions.readOnly`; all writes reject `ReadOnlySession`, `kick`/scheduling off).

## Q4 — Conversations, forks, ownership, scans, documents

- `Harness.open(storage, options, context)` (`harness.ts:613-640`); requires a registry containing built-in tasks (`createRegistry()`; `Harness.open` throws “Registry lacks built-in tasks …”).
- `harness.root(context, { agent?, init? })` returns reserved root (constant `ROOT_CONVERSATION_ID`), creating it in one commit. `harness.conversation(id)`, `harness.createConversation({ ownership, agent?, init? })`, `conversation.fork(at: EntryId, options, context)` (sees parent entries ≤ `at`, keeps agent as of that entry, fresh provider session identity; `agent` + `init` applied in creating commit).
- Ownership: `ConversationOwnership = { kind: "ownerless" } | { kind: "task", taskId }`. Aborting a tool call aborts its task-owned children; parent is idle only once children are; `background: true` tasks survive parent abort.
- Listing/scanning: `tx.scanConversations(query, limit)` → `Page<ConversationRecord, Cursor>`; query supports `ownerTaskId` (example 21), see also `ConversationQuery` type. Live state: `harness.inspect()` → `{ scheduling, tasks, submissions }`; `harness.watchTaskGraph(ctx)` → `TaskGraphWatch`; `getTask`, `waitForTask`, `submission(id)`, `abortSubmission`, `abortTask`, `signal`.
- Entries: `Conversation.entries(query, limit, cursor, ctx)` (fork-aware, newest first unless order asc); `conversation.context()` → `ContextView { head, entries, contributions, messages }`; built-in entry tokens `UserEntry`, `AssistantEntry`, `ToolResultEntry`, `SystemEntry`, `CompactionEntry`, `ResetEntry`; custom `defineEntry`.
- Documents: `defineDoc` overloads (`src/documents.ts:36-53`): scope `"session"` (or `LatestConversationSemantics` / `RewindableConversationSemantics` for conversation-scoped), scope `"task"`; `defineDocFamily` adds a string family key + `initial(seed)`. Definitions need `kind`, `version` (positive int), `initial`, optional `migrate(value, fromVersion)`, `checkpointWhen`. Built-ins: `pi.agent`, `pi.provider`, `pi.live`, `pi.inbox`, `pi.usage` (mounted into `ConversationView.docs`).
- Per-conversation agent: `configure(tx, id, AgentChange)` / `conversation.configure(change, ctx)`: `model {provider,modelId}`, `thinkingLevel`, `extensions` (array or `{add,remove}`), `tools`, `modelTools`, `instructions`, `cwd`, `maxTurns`, `compaction`. Stored as names; resolved per phase from the live registry.

## Q5 — pi-ai providers, auth, OAuth

- `createModels(options?)`: `{ credentials?: CredentialStore; modelsStore?: ModelsStore; authContext?: AuthContext; identity?: ClientIdentity }`; `createModels()` defaults to **in-memory** credential and model stores (`src/models.ts`, CreateModelsOptions). `MutableModels`: `setProvider`, `deleteProvider`, `clearProviders`. `Models`: `getAuth`, `checkAuth`, `getAvailableOfType`, `login(providerId, type, interaction, options?)`, `logout`, `stream`, `complete`, `streamSimple`, `generateImages`, `classify`, `refresh`.
- Providers (README `:62-100`): OpenAI, Ant Ling, Azure OpenAI, OpenAI Codex, Radius, TypeSafe, DeepSeek, NVIDIA NIM, Anthropic, Google, Vertex AI, Mistral, Groq, Cerebras, Cloudflare AI Gateway/Workers AI, xAI, OpenRouter, Vercel AI Gateway, ZAI, MiniMax, Together, Baseten, Hugging Face, Moonshot, GitHub Copilot, Amazon Bedrock, OpenCode Zen/Go, Fireworks, Kimi For Coding, Meta, Qwen Token Plan, Xiaomi MiMo, plus any OpenAI-compatible endpoint (Ollama/vLLM/LM Studio).
- Env auth table (README `:494-525`, source `packages/ai/src/env-ai-keys.ts`): `OPENAI_API_KEY`, `ANTHROPIC_API_KEY` **or** `ANTHROPIC_OAUTH_TOKEN`, `GEMINI_API_KEY`, `GOOGLE_CLOUD_API_KEY`/ADC, `OPENROUTER_API_KEY`, `XAI_API_KEY`, `GROQ_API_KEY`, `DEEPSEEK_API_KEY`, `MISTRAL_API_KEY`, `TOGETHER_API_KEY`, `FIREWORKS_API_KEY`, `OPENCODE_API_KEY`, `KIMI_API_KEY`, `META_API_KEY`, plus Bedrock’s `AWS_PROFILE`/`AWS_ACCESS_KEY_ID`+`AWS_SECRET_ACCESS_KEY`/`AWS_BEARER_TOKEN_BEDROCK`/ECS/IRSA env. Anthropic auth helper builds headers from `ANTHROPIC_OAUTH_TOKEN`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_SERVICE_ACCOUNT_ID`, `ANTHROPIC_WORKSPACE_ID` etc.
- OAuth (`README.md:1802-1893`, `src/auth/oauth/load.ts`, `src/bun-oauth.ts`): providers with `OAuthAuth` on `provider.auth.oauth`: **Anthropic** (Claude Pro/Max), **OpenAI (Sign in with ChatGPT)**, **OpenAI Codex**, **GitHub Copilot**, **OpenRouter** (PKCE→API key); loader also bundles kimiCoding, meta, xai, radius. API: `OAuthAuth.login(interaction)`, `refresh(credential, signal)`, `toAuth(credential)`; `AuthInteraction.prompt()` types `text|secret|select|manual_code`, `notify()` events `info|auth_url|device_code|progress`. Refresh is automatic + lock-protected inside `CredentialStore.modify`. `CredentialStore` contract: `read(providerId)`, `list()` (metadata only), `modify(providerId, fn)`, `delete(providerId)`. Stored credential *owns* its provider (env fallback is not used for it). For single-file Bun binaries, call `registerBunOAuthFlows()` from `@earendil-works/pi-ai/bun-oauth` after bundling.
- `Transport = "sse" | "websocket" | "websocket-cached" | "auto"` (`src/types.ts:148`); Codex WebSocket pooling keyed by `sessionId`, expires after 5 min idle; `cleanupSessionResources(sessionId)`.

## Q6 — ACP

- **No official ACP package/module in `earendil-works/pi`**: `packages/` = agent, ai, chord, codemode, coding-agent, durable, env, evals, mcp, schema, telemetry, tui. No `acp` docs (`packages/coding-agent/docs/acp.md` → HTTP 404); coding-agent README advertises interactive/print/JSON/**RPC**/SDK only.
- `packages/coding-agent/docs/rpc.md`: `pi --mode rpc` = **JSONL over stdin/stdout**, strict framing (LF only, don’t use Node `readline`), command/response with `id` correlation, session events, `agent_settled` as idle signal, extension UI records; exported `RpcClient` for subprocess integrations. SDK for in-process Node/Bun.
- Third-party ACP: `pi-acp@0.0.34` (Sergii Kozak, MIT, 2026-09), deps `@agentclientprotocol/sdk ^0.26.0`; “communicates **ACP JSON-RPC 2.0 over stdio** … and spawns `pi --mode rpc`”, maps assistant chunks, tool_call/tool_call_update (locations, edit diffs), session load via `~/.pi/pi-acp/session-map.json`; requires pi’s `get_session_stats` with `contextUsage`; MVP, Zed-centric.
- For viz: no pi-side ACP adapter to reuse for a *Durable* harness. viz must implement the ACP **client** in Bun (`@agentclientprotocol/sdk`; latest version UNVERIFIED — only 0.26.0 confirmed via pi-acp) and bridge an external harness session into a durable conversation (custom entry kinds + tool results / a task that proxies prompts). `pi-acp` is only useful if viz spawns the plain pi coding agent as an external harness.

## Q7 — Minimal Bun bootstrap (SQLite + subagent tool)

This is directly adapted from the tested `test/examples/21-subagent-foreground.ts`, swapping `MemoryStorage` for SQLite and adding host close.

```ts
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { createModels } from "@earendil-works/pi-ai/models";
import { anthropicProvider } from "@earendil-works/pi-ai/providers/anthropic";
import { obj, str } from "@earendil-works/pi-schema";
import {
  AssistantEntry, Harness, configure, createRegistry, defineExtension, defineTool,
  type AssistantMessage, type ConversationId, type EntryId, type Extension,
  type ToolExecutionApi,
} from "@earendil-works/pi-durable";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";

const context = BACKGROUND_CONTEXT;

async function answerText(api: ToolExecutionApi, answer: EntryId): Promise<string> {
  const entry = await api.commit((tx) => tx.entry(AssistantEntry, answer), context);
  const message = entry?.model?.[0] as AssistantMessage;
  return message.content.flatMap((c) => (c.type === "text" ? [c.text] : [])).join("");
}

const Subagent: Extension = defineExtension({
  name: "subagent",
  tools: [defineTool({
    name: "subagent",
    description: "Delegate a self-contained task to a subagent and get its answer back.",
    parameters: obj({ task: str() }),
    replay: "safe", // rerun after crash finds the same child + submission
    execute: async (args, api) => {
      const child = await api.commit(async (tx) => {
        const existing = (await tx.scanConversations({ ownerTaskId: api.taskId }, 1)).items[0];
        if (existing) return existing.id;
        const created = await tx.createConversation({ ownership: { kind: "task", taskId: api.taskId } });
        await configure(tx as any, created.id, { extensions: { remove: [Subagent] } });
        return created.id;
      }, context);
      await api.details({ conversationId: child }, context);
      const handle = (await api.conversation(child, context))!;
      const settled = await (await handle.submit(
        { type: "input", content: args.task, requestId: `subagent:${api.taskId}` }, context,
      )).wait(context);
      if (settled.status !== "done" || settled.type !== "input") throw new Error(`subagent ${settled.status}`);
      return { output: [{ type: "text", text: await answerText(api, settled.answer) }], details: { conversationId: child } };
    },
  })],
});

const models = createModels();
models.setProvider(anthropicProvider());               // ANTHROPIC_API_KEY or ANTHROPIC_OAUTH_TOKEN
const registry = createRegistry();
registry.install(Subagent);

const storage = await openNodeSqliteStorage("/var/lib/viz/viz.sqlite", { prefix: "viz_" }); // prefix optional
const harness = await Harness.open(storage, { models, registry }, context);
const root = await harness.root(context, {
  agent: { model: { provider: "anthropic", modelId: "claude-sonnet-4-5" }, instructions: "You are viz." },
});

const sub = await root.submit({ type: "input", content: "Use the subagent to name three primes." }, context);
await sub.wait(context);
await harness.waitForIdle(context);
await harness.close(context);
```

`configure` in a `tx` is the transaction form re-exported by durable (`configure(tx, conversationId, change)`); example 21 calls it exactly like this (`await configure(tx, created.id, …)`) — my `as any` above is only to avoid importing `Tx`; drop it in real code.

## Architecture notes / gotchas for viz

- `Harness` = one `Storage` (commit log) + `Registry` (extensions/tools/hooks/tasks) + `Models` + optional `env`/telemetry. All writes go through `tx` commits; transcript entries are immutable; model-visible context is derived (resets/compaction/edits). Tasks are durable state machines (`pi.generation` owns `pi.tool` tasks), so crashes resume mid-turn; storage errors are fatal (`SessionFailed`) → supervise/restart the process.
- One writer per storage, **no cross-process locking**; use `readOnly: true` for viewers/replicas. `HarnessOptions.now` clock must be identical across processes opening the same storage (sleep deadlines).
- Streaming progress commits default ≤100 ms and are persisted together at phase boundaries or `settings.progress.persistWithinMs` (default 1,000 ms) → crash can lose ~1 s of text/tool output (`README.md:578-630`).
- `HarnessOptions.onWake`/`setTimer` exist for hibernating hosts; with systemd, normal timers are fine, but `onWake` is the hook for wake-on-sleep semantics.
- npm 1.5.2 vs `main`: CHANGELOG `[Unreleased]` has breaking changes (`ToolExecutionResult | ToolPark`, `pi.tool` v4, `HookTask` rename, codemode namespaces from extensions). Pin 1.5.2 until viz deliberately tracks main.
- Prompt-cache/durability rationale: images are fitted *before* storage so transcripts/prompt cache never change later (`images/fit`, `README.md:300-330`).
- Registry is host-owned and not stored; conversations store extension *names*. After restart reinstall identical names, else tasks block (`missing_task`).
- `Harness.open` requires built-in tasks, i.e. `createRegistry()` (or an equivalent registry that includes them).
- Tool API gives `api.commit`, `api.details` (UI-visible, e.g. child conversation id), `api.conversation(id)`, `api.executeTool` (nested calls, optional `wait:false` + park), `api.createTask`, `defineSignal`/`harness.signal` for external events (webhooks, Discord replies), `SubmissionDraft.notify` for park-until-subagent-answers (`README.md`, examples 38-46).
- `README.md:1020-1080` also: `storage/sqlite/cloudflare` for DO SQLite; JSONL grows memory with full history (avoid for long-running agent); portable SQLite/JSONL cores run on Bun given the async facade.

**UNVERIFIED:** actual runtime behavior of `openNodeSqliteStorage` under the installed Bun build on this machine; Bun version installed; ACP SDK current version/API beyond `@agentclientprotocol/sdk ^0.26.0`; any performance numbers.