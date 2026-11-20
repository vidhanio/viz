# ACP (Agent Client Protocol)

Based on the installed 2.7.0 SDK, the ACP site, adapter repos, and the registry. Unverified items are marked.

## 0. Bottom line

- ACP = JSON-RPC 2.0, newline-delimited over stdio. No batches. Agent process stdout must contain only ACP messages (stderr is for logs). `protocolVersion: 1` is stable; `2` is draft and already spoken by `claude-agent-acp` when a client asks for it.
- The installed TS SDK is **2.7.0**. Its **app API** provides `client()` / `agent()` builders with `connect()` / `connectWith()`, plus `ndJsonStream`, `methods`, `RequestError`, `PROTOCOL_VERSION`, and typed handlers.
- Stable v1 session surface: `session/new`, `session/load`, `session/list`, `session/delete`, `session/resume`, `session/close`, `session/set_mode`, `session/set_config_option`, `session/prompt`, `session/cancel`. `session/fork`, `session/archive|unarchive`, `session/status`, `session/options`, `session/set_title`, providers, `auth/status`, subagents, usage-on-prompt, plan operations, prompt suggestions are **unstable** in 2.7.0 (available from `@agentclientprotocol/sdk/unstable`, whose types are generated from `schema/v1/schema.unstable.json`).
- For a meta-harness, the only standard way to give _any_ ACP agent tools is **MCP servers declared in `session/new`** (stdio is mandatory for all agents; HTTP/SSE optional). MCP-over-ACP (`type: "acp"`, `mcp/message`) is the clean no-extra-process option when the agent advertises `mcpCapabilities.acp` (Claude, Codex do; OpenCode explicitly does not).
- ACP session IDs are agent/process-scoped. viz must namespace them (`adapter/sessionId`) and keep its own conversation mapping; `session/list` only lists sessions that agent instance knows about.
- Registry entries carry distribution metadata only — **no capability flags**. Capabilities must be discovered with `initialize`.

## 1. Versions and sources

| Thing                 | Version / URL                                                                                                                                                                        |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Spec (stable v1)      | https://agentclientprotocol.com/protocol/v1/overview.md                                                                                                                              |
| Spec (draft v2)       | https://agentclientprotocol.com/protocol/v2/overview.md, `/migration.md`                                                                                                             |
| RFD index/status      | https://agentclientprotocol.com/rfds/updates.md                                                                                                                                      |
| Installed TS SDK      | `backend/node_modules/@agentclientprotocol/sdk` v2.7.0 (`package.json`)                                                                                                              |
| TS SDK repo / example | https://github.com/agentclientprotocol/typescript-sdk; `src/examples/client.ts`                                                                                                      |
| Rust crate            | `agent-client-protocol` **5.4.0** (2026-11-18), Rust 1.88, edition 2024, features `process`, `stdio`, `schemars`, `unstable_*`; repo https://github.com/agentclientprotocol/rust-sdk |
| Registry              | https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json; source https://github.com/agentclientprotocol/registry                                                         |
| Adapters              | https://github.com/agentclientprotocol/claude-agent-acp, `codex-acp`, https://github.com/svkozak/pi-acp                                                                              |

## 2. Stable v1 client lifecycle (exact shapes)

Source of truth for the installed types: `backend/node_modules/@agentclientprotocol/sdk/dist/v1/schema/types.gen.d.ts`; protocol pages cited per section.

### 2.1 `initialize`

Request (`InitializeRequest`, types.gen.d.ts:2967):

```json
{
  "jsonrpc": "2.0",
  "id": 0,
  "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "clientCapabilities": {
      "fs": { "readTextFile": true, "writeTextFile": true },
      "terminal": true,
      "auth": { "terminal": true },
      "elicitation": { "form": {}, "url": {} },
      "session": { "notices": {}, "compaction": {}, "configOptions": { "boolean": {} } }
    },
    "clientInfo": { "name": "viz", "title": "viz", "version": "1.0.0" },
    "_meta": {}
  }
}
```

Client capability fields (types.gen.d.ts:3001): `fs.readTextFile/writeTextFile?: boolean`; `terminal?: boolean`; `session?: ClientSessionCapabilities` (`notices`, `compaction`, `configOptions.boolean`, unstable `promptSuggestions`); `auth.terminal?: boolean`; `elicitation.form|url`; unstable `subagents`, `plan`; `_meta`.
Agent response (`InitializeResponse`, :1349):

```json
{
  "protocolVersion": 1,
  "agentCapabilities": {
    "loadSession": true,
    "promptCapabilities": { "image": true, "audio": false, "embeddedContext": true },
    "mcpCapabilities": { "http": true, "sse": false, "acp": true },
    "sessionCapabilities": {
      "list": {},
      "delete": {},
      "resume": {},
      "close": {},
      "additionalDirectories": {},
      "systemPrompt": { "mode": { "override": {} } },
      "fork": {},
      "archive": {},
      "status": {},
      "options": {},
      "title": {}
    },
    "auth": { "logout": {} },
    "providers": {}
  },
  "authMethods": [],
  "agentInfo": { "name": "codex-acp", "version": "2.7.0" }
}
```

Agent capability fields (`AgentCapabilities`, :1397): `loadSession?: boolean`, `promptCapabilities`, `mcpCapabilities`, `sessionCapabilities`, `auth`, unstable `providers`, `_meta`. `sessionCapabilities.fork/archive/status/options/title` are UNSTABLE (unstable types :1627-1750). If the agent doesn't support the requested version it returns its latest supported version; the client must disconnect if it can't speak it (`InitializeResponse` doc, :1349).

### 2.2 Authentication

- `authenticate {methodId}` (`AuthenticateRequest`, :3256). `methodId` must be one of `authMethods`.
- Auth method shapes (v1, :1764): terminal `{id,name,description?,args?: string[],env?: {[name]:string},type:"terminal"}`; agent handled `{id,name,description?,type?: "agent"}` (`id`, not v2's `methodId`).
- Do **not** send a terminal method to `authenticate`; launch the configured agent invocation with the method's `args`/`env` interactively, exit 0 = success, then `initialize` again.
- `logout` (empty params) is available when the agent advertises `agentCapabilities.auth.logout`; `session/new`/`session/prompt` may return `-32000 auth_required` until authenticated. Errors: `RequestError.authRequired()` etc. (`dist/core/jsonrpc.d.ts:503-570`).

### 2.3 Session lifecycle

| Method                      | Request                                                                         | Response                                                                                                                   | Gate                             |
| --------------------------- | ------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- | -------------------------------- |
| `session/new`               | `{cwd, mcpServers: McpServer[], additionalDirectories?, systemPrompt?, _meta?}` | `{sessionId, modes?, configOptions?}`                                                                                      | baseline                         |
| `session/load`              | `{sessionId, cwd, mcpServers, additionalDirectories?, systemPrompt?}`           | `{}` after replaying history as `session/update`; may include `modes/configOptions`                                        | `loadSession`                    |
| `session/resume`            | `{sessionId, cwd, mcpServers?, additionalDirectories?, systemPrompt?}`          | `{modes?, configOptions?}`, **no replay**                                                                                  | `sessionCapabilities.resume`     |
| `session/list`              | `{cwd?, cursor?}`                                                               | `{sessions: SessionInfo[], nextCursor?}`; `SessionInfo={sessionId,cwd,additionalDirectories?,title?,updatedAt?,archived?}` | `sessionCapabilities.list`       |
| `session/delete`            | `{sessionId}`                                                                   | `{}`                                                                                                                       | `sessionCapabilities.delete`     |
| `session/close`             | `{sessionId}`                                                                   | `{}`; cancels work first; idempotent, `-32002` may be treated as success                                                   | `sessionCapabilities.close`      |
| `session/set_mode`          | `{sessionId, modeId}`                                                           | `{}`                                                                                                                       | optional; legacy (v2 removed it) |
| `session/set_config_option` | `{sessionId, configId, value:string}` or `{...,type:"boolean",value:boolean}`   | `{configOptions: SessionConfigOption[]}`                                                                                   | required if options advertised   |

Types: `NewSessionRequest` :3295, `LoadSessionRequest` :3506, `ResumeSessionRequest` :3603, `ListSessionsRequest` :3554, `SetSessionConfigOptionRequest` :3697. `NewSessionResponse` :1913 also carries initial `modes` and `configOptions`. Paths must be absolute; line numbers 1-based (`protocol/v1/overview.md` conventions).

MCP server shapes (v1 `McpServer`, :3339):

```json
{"type":"stdio","name":"viz","command":"/abs/viz-mcp","args":[],"env":[{"name":"K","value":"V"}]}
{"type":"http","name":"viz","url":"https://…","headers":[{"name":"Authorization","value":"…"}]}
{"type":"sse","name":"legacy","url":"https://…","headers":[]}
```

`McpServerStdio` requires all four fields; `command` must be an absolute path. **All agents MUST support stdio; HTTP and SSE are optional capabilities; agents SHOULD support HTTP** (`protocol/v1/session-setup.md`, MCP section). Unstable adds `{"type":"acp","name":"viz","serverId":"…"}` (`McpServer`, unstable types :5231; `McpServerAcp` :5327). `session/load` replays the whole transcript (chunks, tool calls, plans, and compactions if `session.compaction` is advertised); notices are live-only (`session-setup.md`).

### 2.4 Prompt turn

Request (`PromptRequest`, :3735): `{sessionId, prompt: ContentBlock[], _meta?}`. Content blocks (`ContentBlock`, :240): `text{text}`, `image{data,mimeType,uri?}`, `audio{data,mimeType}`, `resource_link{uri,name,description?,mimeType?,size?,annotations?}`, `resource{resource:{uri,text|blob,mimeType?},annotations?}`. Baseline is `text` + `resource_link`; image/audio/embeddedContext require `promptCapabilities`.

Response (`PromptResponse`, :2318): `{stopReason}` with `StopReason = "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" | "cancelled"` (:2339).

`session/update` is a client-bound notification `{sessionId, update}` (`SessionNotification` :2400). Stable v1 variants (`SessionUpdate` union, :2428):
`user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk` (all `ContentChunk{content, messageId?}`), `tool_call` (`ToolCall{toolCallId,title,name?,kind?,status?,content?,locations?,rawInput?,rawOutput?}`; kinds `read|edit|delete|move|search|execute|think|fetch|switch_mode|other`; statuses `pending|in_progress|completed|failed`), `tool_call_update` (same keys optional, patched), `plan{entries:[{content,priority:"high"|"medium"|"low",status:"pending"|"in_progress"|"completed"}]}` (whole-plan replacement), `available_commands_update{availableCommands:[{name,description,input?{hint}}]}`, `current_mode_update{currentModeId}`, `config_option_update{configOptions}`, `session_info_update{title?,updatedAt?}`, `usage_update{used,size,cost?:{amount,currency}}`, `notice{severity,title,description?}`, `compaction_update{compactionId,status,summary?,error?}`, `compaction_summary_chunk{compactionId,content}`.
Tool-call content variants: `{type:"content",content}`, `{type:"diff",path,oldText?,newText}`, `{type:"terminal",terminalId}` (:220, :513, :548). `session/update` handlers are dispatched concurrently by the SDK; serialize per session yourself (see 4).

`session/cancel` is a **notification** `{sessionId}`; the agent should end the turn with `stopReason:"cancelled"`. `$/cancel_request {requestId}` cancels one request (SDK constant `PROTOCOL_METHODS.cancel_request`; RFD "Request Cancellation", preview). Don't send a second `session/prompt` until the first stops; OpenCode explicitly rejects concurrent prompts.

### 2.5 Permission requests

Agent → client request (`RequestPermissionRequest`, :120):

```json
{
  "sessionId": "s1",
  "toolCall": { "toolCallId": "t1", "title": "Run tests", "kind": "execute", "status": "pending" },
  "options": [
    { "optionId": "once", "name": "Allow once", "kind": "allow_once" },
    { "optionId": "always", "name": "Always allow", "kind": "allow_always" },
    { "optionId": "reject", "name": "Reject", "kind": "reject_once" }
  ]
}
```

Response (`RequestPermissionResponse`, :3826): `{outcome:{outcome:"selected",optionId}}` or `{outcome:{outcome:"cancelled"}}`. Decision by `kind`, never by index. Cancelling the turn should cancel outstanding requests.

### 2.6 `fs/*` and `terminal/*` (client methods)

Advertise only what viz implements. `fs/read_text_file {sessionId,path,line?,limit?}` → `{content}`; `fs/write_text_file {sessionId,path,content}` → `{}` (:35, :73). Terminals: `terminal/create {sessionId,command,args?,env?:[{name,value}],cwd?,outputByteLimit?}` → `{terminalId}`; `terminal/output {sessionId,terminalId}` → `{output,truncated,exitStatus?{exitCode?,signal?}}`; `terminal/wait_for_exit` → `{exitCode?,signal?}`; `terminal/kill`, `terminal/release` → `{}` (:3891-4030). SDK `AgentContext.createTerminal(...)` returns a `TerminalHandle` with `currentOutput/waitForExit/kill/release` and `await using` (`dist/v1/stable/acp.d.ts`, `TerminalHandle`).

### 2.7 Elicitation

Client request `elicitation/create` (`:797`), one of:

- form: `{mode:"form", message, requestedSchema:{type:"object",properties:{…primitive schemas…},required?}, sessionId?, toolCallId?}` (or `requestId` for pre-session requests)
- url: `{mode:"url", message, elicitationId, url, sessionId?|requestId}`
  Response `{action:"accept", content} | {action:"decline"} | {action:"cancel"}` (:3994); url completion arrives as `elicitation/complete {elicitationId}`.

### 2.8 `_meta` and extension methods

Any type with `_meta` accepts `{[key:string]:unknown}` (`protocol/v1/extensibility.md`). Reserve root keys `traceparent/tracestate/baggage` for W3C trace context. Custom methods **must** start with `_` (`_com.example/foo`); unknown custom requests get `-32601`, unknown custom notifications are ignored. Advertise extension support in capability `_meta`, reverse-DNS keyed. Use `_meta` on `initialize`, `session/new`, `session/prompt` to carry viz conversation/correlation IDs (t3code-style `requestId` propagation is explicitly allowed).

## 3. Unstable / draft surface relevant to a meta-harness

Installed 2.7.0 unstable v1 (types under `dist/v1/unstable/schema/types.gen.d.ts`; entrypoint `@agentclientprotocol/sdk/unstable`; method constants in `dist/v1/acp.d.ts:56-110`):

| Feature              | Methods / notifications                                       | Key shapes / gates                                                                                                                                                                                                                                                                                                                                                                                                                  |
| -------------------- | ------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Fork                 | `session/fork`                                                | `{sessionId,cwd,additionalDirectories?,mcpServers?,systemPrompt?,configOptions?}` → `{sessionId,modes?,configOptions?}`; `sessionCapabilities.fork`; RFD https://agentclientprotocol.com/rfds/session-fork.md. RFD adds `cursorId` + `fork` cursor capability (2026-11-16), **not present in the installed 2.7.0 schema** (grep: no `cursorId`).                                                                                    |
| Archive              | `session/archive`, `session/unarchive`                        | `{sessionId}`; hides from default list, keeps ID/history; `sessionCapabilities.archive`; `SessionInfo.archived`; `session/list.archived` filter                                                                                                                                                                                                                                                                                     |
| Status               | `session/status`                                              | `{sessionId}` → `{status:"open"                                                                                                                                                                                                                                                                                                                                                                                                     | "saved"}`; unknown → `-32002`; no side effects |
| Options              | `session/options`                                             | `{cwd,additionalDirectories?,sessionId?}` → `{modes?,configOptions?}`; lets viz preflight models/modes before creating/forking                                                                                                                                                                                                                                                                                                      |
| Title                | `session/set_title`                                           | `{sessionId,title}` → stored title                                                                                                                                                                                                                                                                                                                                                                                                  |
| Providers            | `providers/list`, `providers/set`, `providers/disable`        | `providers/list` → providers + current config; `set {providerId,apiType,baseUrl,…}`; `AgentCapabilities.providers`                                                                                                                                                                                                                                                                                                                  |
| Auth status          | `auth/status` request; `auth/status_update` notification      | `{state,…}`; no `auth_required`; `AgentAuthCapabilities`                                                                                                                                                                                                                                                                                                                                                                            |
| Subagents/delegation | `subagent_update`, `session_message`, `session_message_chunk` | `subagent_update` is an upsert keyed by child `sessionId` (`title`, `description`, `capabilities.cancel`, `state`, `cwd`, `additionalDirectories`); `session_message` upserts `messageId` + sender/recipient IDs + `content`; chunks append. Client opts in via `clientCapabilities.subagents:{}` (v1) / no marker in v2. RFD https://agentclientprotocol.com/rfds/subagents.md; Codex implements it (`docs/subagent-sessions.md`). |
| Plan operations      | `plan_update`, `plan_removed`                                 | `plan_update{planId,type:...}` replaces by ID; `plan_removed{planId}`; client `plan:{}` capability. Stable v1 `plan` remains the non-ID form. RFD moved to Preview 2026-11-17 (`rfds/updates.md`).                                                                                                                                                                                                                                  |
| Prompt suggestions   | `prompt_suggestions_update`                                   | `{suggestions:[…]}`; client `session.promptSuggestions` capability                                                                                                                                                                                                                                                                                                                                                                  |
| End-turn usage       | `usage` on `PromptResponse`                                   | `Usage{totalTokens,inputTokens,outputTokens,thoughtTokens?,cachedReadTokens?,cachedWriteTokens?}`; separate from stable `usage_update`. RFD `end-turn-token-usage` (draft).                                                                                                                                                                                                                                                         |
| Session cursors      | `cursor_update` + `cursorId`                                  | Draft 2026-11-16; not in installed 2.7.0                                                                                                                                                                                                                                                                                                                                                                                            |

MCP-over-ACP (RFD https://agentclientprotocol.com/rfds/mcp-over-acp.md): draft targets MCP 2026-07-28, no MCP handshake. Agent advertises `mcpCapabilities.acp`. Client declares `{"type":"acp","name":"viz","serverId":"viz"}` in `mcpServers`. Inner MCP requests arrive as `mcp/message` with `{serverId, requestId, method, params?}`; reply `{result}` or `{error:{code,message,data?}}` (MCP error, distinct from ACP error). SDK `/unstable` ships `MCP_MESSAGE_METHOD`, `MCP_PROTOCOL_VERSION`, `McpOverAcpProvider`, `McpOverAcpCaller` (`dist/v1/unstable/mcp.d.ts`).

Proxy chains / conductor (RFD https://agentclientprotocol.com/rfds/proxy-chains.md): a Conductor sits between client and proxies. Internal methods `proxy/initialize` and `proxy/successor`; conductor forwards e.g. `session/prompt` and wraps `$/cancel_request` with the original `requestId`; proxies may rewrite/observe traffic. Rust crate `agent-client-protocol-conductor` (proxy-chain orchestration) and `agent-client-protocol-polyfill` (e.g. adapt MCP-over-ACP to HTTP) are in the rust-sdk workspace (rust crate README).

ACP v2 (draft; negotiation by sending `protocolVersion: 2`):

- `session/prompt` response **acknowledges acceptance**; completion/stopReason arrives as `state_update` (`running|idle|requires_action|unknown`, optional `background` list). Updates are upserts by ID: omitted = unchanged, `null` = cleared, chunks append. `replay_complete` ends `session/resume` replay.
- Renames/removals: `authenticate`→`auth/login`, `logout`→`auth/logout`; `session/load` removed (use `session/resume` with `replayFrom`); `session/set_mode` removed (config option); `fs/*`, `terminal/*` (execution) removed; `tool_call` update removed (first `tool_call_update` creates); new `tool_call/cancel`, `terminal/cancel`, whole-message upserts, `terminal_update`/`terminal_output_chunk`, `subagent_update`/`session_message` are baseline.
- Capabilities become role-agnostic `capabilities`/`info`, support markers are `{}` objects, and `session/list|resume|close` are mandatory when `capabilities.session` is advertised. `session/prompt` gains `idempotencyKey` (keyed retry class).
- `claude-agent-acp` serves v2 when asked (env `CLAUDE_AGENT_ACP_PROTOCOL=v1|v2|auto`), including sessions/fork/status/options/title, delete, resume-with-replay, plans, notices, compaction, display terminals, elicitation, providers, auth, subagents; steering is v1-only, `idempotencyKey` is accepted but ignored, and cursor forks fail `-32602` (its README).

## 4. TS SDK 2.7.0 client API

Import selection (`package.json` exports, `MIGRATION_STABLE_UNSTABLE.md`):

| Import                                                         | Surface                                                                                                                                                 |
| -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `@agentclientprotocol/sdk`                                     | stable v1 only                                                                                                                                          |
| `@agentclientprotocol/sdk/unstable`                            | full v1 incl. fork/status/options/archive/title/providers/auth-status/subagents/plan ops/MCP-over-ACP; types come from `schema/v1/schema.unstable.json` |
| `@agentclientprotocol/sdk/experimental/v2` / `.../v2/unstable` | draft v2                                                                                                                                                |
| `@agentclientprotocol/sdk/schema/v1/schema[-.unstable].json`   | raw JSON Schemas (also shipped)                                                                                                                         |

Exports/anchors in `dist/v1/stable/acp.d.ts`: `ndJsonStream` (:28), `Stream`, `ClientConnection` (:128), `ClientContext` (`request`, `notify`, `buildSession`), `SessionBuilder` (`withMcpServer`, `withAdditionalDirectories`, `start`, `withSession`), `ActiveSession` (:307; `prompt` :339, `nextUpdate` :343, `readText`, `dispose`), `client()` (:679), `ClientApp` (:688; `onRequest`, `onNotification`, `onConnect`, `connect`, `connectWith` :710), `agent()`/`AgentApp` for the reverse role, `methods`, `PROTOCOL_VERSION`, `RequestError`, `MessageTooLargeError`, `DEFAULT_MAX_MESSAGE_BYTES` (32 MiB), all generated types.

Legacy removed: `ClientSideConnection` → `client().onRequest(...).connect(stream)` or `connectWith(...)`; named outbound helpers (`newSession`, `sessionUpdate`, `extMethod`) → `ctx.request(methods.agent.…, params)` / `ctx.notify(...)`; `Agent`/`Client` interfaces → typed app callbacks. Custom methods: `onRequest("_com.example/x", parser, handler)` or fallback `onRequest("*", ({method,params})=>…)`.

Minimal compiling client (matches `src/examples/client.ts` and the installed declarations):

```ts
import { spawn } from "node:child_process";
import { Readable, Writable } from "node:stream";
import * as acp from "@agentclientprotocol/sdk"; // stable v1
// import * as acp from "@agentclientprotocol/sdk/unstable"; // + fork/status/options/…

function decide(options: acp.PermissionOption[]): acp.RequestPermissionResponse {
  const allow = options.find((o) => o.kind === "allow_once") ?? options[0];
  return allow ? { outcome: { outcome: "selected", optionId: allow.optionId } } : { outcome: { outcome: "cancelled" } };
}

const child = spawn("npx", ["-y", "@agentclientprotocol/codex-acp"], {
  stdio: ["pipe", "pipe", "pipe"], // keep stderr separate; never mix into stdout
});
child.stderr.on("data", (b) => process.stderr.write(b));

const stream = acp.ndJsonStream(
  Writable.toWeb(child.stdin!) as WritableStream<Uint8Array>,
  Readable.toWeb(child.stdout!) as ReadableStream<Uint8Array>,
);

const conn = acp
  .client({ name: "viz" })
  .onRequest(acp.methods.client.session.requestPermission, ({ params }) => decide(params.options))
  .onNotification(acp.methods.client.session.update, ({ params }) => {
    enqueueSerialized(params.sessionId, params.update); // SDK dispatches concurrently
  })
  .connect(stream);

const init = await conn.agent.request(acp.methods.agent.initialize, {
  protocolVersion: acp.PROTOCOL_VERSION,
  clientCapabilities: { fs: { readTextFile: true, writeTextFile: true }, terminal: true },
  clientInfo: { name: "viz", version: "0.0.0" },
});

const { sessionId } = await conn.agent.request(acp.methods.agent.session.new, {
  cwd: "/absolute/workspace",
  mcpServers: [{ name: "viz", command: "/absolute/viz-mcp", args: [], env: [] }],
});

const stop = await conn.agent.request(acp.methods.agent.session.prompt, {
  sessionId,
  prompt: [{ type: "text", text: "do the task" }],
});
await conn.agent.notify(acp.methods.agent.session.cancel, { sessionId }); // if needed
conn.close();
child.kill();
```

Notes: `connectWith(stream, async (ctx) => …)` auto-closes and is the usual entry; `ctx.buildSession(cwd).withMcpServer(...).withSession(...)` bundles session setup and update routing; `ActiveSession.nextUpdate()` yields `{kind:"session_update"}` or `{kind:"stop",response,stopReason}`. Do not mix stable and unstable imports in one in-process `connect()`; over a transport they can differ.

## 5. Registry

Schema `agent.schema.json` (repo root): required `id` (`^[a-z][a-z0-9-]*$`), `name`, `version` (semver), `description`, `distribution`; optional `repository`, `website`, `authors`, `license`, `license_url` (required unless id `dimcode`), `icon`, `preview`. Distribution = `binary{<platform>:{archive, sha256?, cmd, args?, env?}}` (platforms: `darwin-aarch64|x86_64`, `linux-aarch64|x86_64` plus `-musl`, `windows-aarch64|x86_64`), `npx{package,args?,env?}`, `uvx{...}`. Registry docs: https://agentclientprotocol.com/get-started/registry.md.

Representative entries (versions as of this snapshot; launch command from `distribution`):

| id               | Agent                  | Launch                                                                                          |
| ---------------- | ---------------------- | ----------------------------------------------------------------------------------------------- |
| `claude-acp`     | Claude Agent 0.108.1   | `npx @agentclientprotocol/claude-agent-acp@0.108.1` (or bundled `./claude-agent-acp`; Node ≥22) |
| `codex-acp`      | Codex 2.7.0            | `npx -y @agentclientprotocol/codex-acp@2.7.0` (bundles `@openai/codex`)                         |
| `gemini`         | Gemini CLI 0.68.0      | `npx @google/gemini-cli@0.68.0 --acp`                                                           |
| `opencode`       | OpenCode 2.3.2         | `opencode acp` (registry ships binaries only)                                                   |
| `pi-acp`         | pi ACP 0.0.34          | `npx pi-acp@0.0.34` (needs `pi` ≥0.81 on PATH; Node engines ≥20, README says 22+)               |
| `amp-acp`        | Amp 0.13.0             | binary `./amp-acp`                                                                              |
| `auggie`         | Auggie CLI 0.37.0      | `npx @augmentcode/auggie@0.37.0 --acp`                                                          |
| `github-copilot` | GitHub Copilot 1.573.0 | `npx @github/copilot-language-server@1.573.0 --acp`                                             |
| `cursor`         | Cursor 2026.11.13      | `cursor-agent acp`                                                                              |
| `kimi-code`      | Kimi Code 2.6.3        | `npx @moonshot-ai/kimi-code@2.6.3 acp`                                                          |
| `qwen-code`      | Qwen Code 0.28.0       | `npx @qwen-code/qwen-code@0.28.0 --acp --experimental-skills`                                   |
| `autohand`       | Autohand Code 0.9.11   | binary `./autohand --acp`                                                                       |

The docs card list also includes Agoragentic, Antigravity, Bob, Cline, Codebuddy, Cortex Code, Corust, crow-cli, DeepAgents, Devin, DimCode, Dirac, Factory Droid, fast-agent, GLM Agent, goose, Grok Build, Harn, Junie, Kilo, Kimchi, Kimi CLI (legacy), Kiro, MiniMax, Minion, Mistral Vibe, Muse Code, Nova, OpenCode V2, Poolside, Qoder, siGit, Stakpak, VT Code (https://agentclientprotocol.com/get-started/registry.md).

Capabilities are **not** in registry entries. Documented examples: OpenCode publishes its `initialize` result — `loadSession:true`, `mcpCapabilities:{http:true,sse:false}`, `promptCapabilities:{embeddedContext:true,image:true}`, `sessionCapabilities:{additionalDirectories:{},close:{},delete:{},fork:{},list:{},resume:{}}`, no MCP-over-ACP, one prompt at a time, `session/load` replays but `session/resume`/`session/fork` do not (`https://opencode.ai/docs/cli/acp.md`). Claude documents images/@-mentions/embedded context, MCP over stdio/HTTP/ACP, nested subagents, v2 surface (`claude-agent-acp` README). Codex documents MCP over stdio/HTTP/ACP, subagent sessions, `_session/steering`, model/effort/sandbox config (`codex-acp` README). pi-acp documents **no** `fs/*`, **no** `terminal/*`, and MCP servers "accepted … but not wired through to pi" (pi-acp README) — a hard constraint for tool exposure.

## 6. Pitfalls and operational facts

- **stdout purity**: agent MUST NOT write non-ACP bytes to stdout; logs to stderr only; messages newline-delimited with no embedded newlines; client likewise for stdin (`protocol/v1/transports.md`). Pipe stderr separately and surface it in viz logs.
- **Message size**: SDK default cap 32 MiB (`DEFAULT_MAX_MESSAGE_BYTES = 32 * 1024 * 1024`), throws `MessageTooLargeError`; raise `NdJsonStreamOptions.maxMessageBytes` deliberately.
- **Serialization**: `onNotification` handlers start in arrival order but are not awaited; async handlers run concurrently. Queue/chain per session, or use `ActiveSession`, which queues updates. Oversized prompts and permission responses can race cancellation.
- **Process lifecycle**: closing stdin exits some agents (OpenCode); kill on teardown. One process can serve many sessions (OpenCode), but `session/list`/`saved` state is process/agent-scoped.
- **cwd/paths**: absolute paths only, 1-based lines; `cwd` of the subprocess is irrelevant to the session; OpenCode rejects load/resume/fork with a different `cwd` than the session's.
- **Node floors**: claude-agent-acp `engines.node >=22`; pi-acp `>=20` (README 22+); codex-acp publishes no engines (UNVERIFIED floor) and bundles a Codex binary. Standalone claude binaries need `libgcc`/`libstdc++` on Alpine; `CLAUDE_CODE_EXECUTABLE` causes MCP env/headers to appear in argv.
- **Auth**: terminal auth invocation is `agent args + method.args`, env overrides; never send terminal method IDs to `authenticate`. OpenCode advertises `opencode-login` and returns `-32000` until `opencode auth login`; Codex supports ChatGPT (browser; `NO_BROWSER=1` hides), `CODEX_API_KEY`/`OPENAI_API_KEY`, custom gateway; Claude adapter needs a Claude login (bundled CLI) and can be limited via `CLAUDE_AGENT_ACP_PROTOCOL`.
- **Adapter gaps**: pi-acp ignores `mcpServers` and does no fs/terminal delegation, so viz tools must be configured inside pi (pi MCP adapter/extension) for pi sessions; OpenCode does not support MCP-over-ACP; SSE support is rare (`sse:false` in OpenCode); Claude v1 steering only, cursors unsupported, v2 `idempotencyKey` ignored; Codex steering is the `_session/steering` extension with an opt-in fallback.
- **v1 vs v2**: don't assume v2. Ask for 1 unless you handle v2's ack/`state_update`/upsert/removed-method semantics. A v1-only agent returns `protocolVersion:1` to a v2 request; a v2-only agent rejects v1 initialize.
- **Errors**: `-32000` auth required/agent error, `-32002` unknown session/resource (close/delete idempotent), `-32800` request cancelled, `-32602` invalid params; `RequestError.parseError/invalidRequest/methodNotFound/invalidParams/internalError/requestCancelled/authRequired/resourceNotFound/sessionNotFound/unsupportedProtocolVersion` (`core/jsonrpc.d.ts:503-570`).
- **MCP specifics**: stdio `env` is `[{name,value}]` (terminal auth `env` is an object — different shape); HTTP headers likewise `[{name,value}]`; stdio `command` absolute. MCP transport is per-session and may replace/reconnect per request; OpenCode keeps client MCP servers for the life of the process/directory.
- **Custom methods**: only `_`-prefixed names are safe; standard types accept `_meta` but you MUST NOT add root fields; unknown notifications should be ignored.

## 7. Design-relevant implications for viz

- Upstream sessions map to viz conversations; keep `(agentId, sessionId, processKey)` and persist `cwd`, `additionalDirectories`, `title`, `updatedAt`, config option snapshot, and the ACP version negotiated. Use `session/load` (replay) on restore when supported, else `session/resume`; `session/list` for discovery; `session/fork` for branching a chat (when advertised; unstable).
- Expose the viz meta-harness toolset as **MCP tools**, not ACP extension methods: ACP agents only know standard client methods; provider sessions will call MCP tools. Preferred order per adapter: (1) MCP-over-ACP (`type:"acp"`, `mcpCapabilities.acp`) — no extra process/socket and request-scoped; (2) stdio MCP server (universal, must be an absolute command); (3) HTTP MCP (Claude/Codex/OpenCode) for pooled/remote agents. For pi-acp, configure the viz MCP server inside pi.
- Because ACP `mcp/message` carries only `serverId/requestId/method/params` (no `sessionId`), correlate tool calls with the ACP connection/session by using a **per-session server registration/connection** or by injecting the conversation ID into tool arguments/`_meta`; do not assume adapters add it.
- Advertise client capabilities deliberately: `fs` and `terminal` if viz will broker file/command access (and log it), `elicitation` if the viz UI can ask structured questions, `session.compaction/notices/configOptions.boolean`, `auth.terminal` if viz can run an interactive login, `subagents`/`plan` if you consume child-session/ID-plan updates.
- Map update streams into viz events with upsert semantics: tool calls by `toolCallId`, compactions by `compactionId`, subagents by child `sessionId`, messages by `messageId`; chunks append. Persist raw updates for replay/debugging.
- Auto-approve by `PermissionOption.kind` and tool `kind`; never by array index. Cancelling a viz turn should send `session/cancel` and cancel outstanding permissions; don't issue a new prompt until the current `PromptResponse`/`stop` arrives.
- Budget for unstable drift: pin the SDK; if you need fork/status/options/subagents now, use `/unstable` (or the raw schema) and feature-detect each capability in `initialize`. `SessionCapabilities` fields are `{}` markers, not booleans — `sessionCapabilities.fork != null`, not `=== true`.

## 8. UNVERIFIED / not covered

- Bun's support for `Readable.toWeb`/`Writable.toWeb` and `node:child_process` piping as used in the snippet (no shell/run in this session); Node 18+ supports it.
- codex-acp and Gemini CLI Node engine floors (codex `package.json` has no `engines`; Gemini's Node requirement not checked).
- Claude v1 `initialize` capability payload and exact auth method IDs (README documents v2 surface; v1 details not captured).
- Whether any adapter attaches an ACP `sessionId` to MCP tool calls outside the schema (schema has only `serverId/requestId/method/params`; treat as UNVERIFIED).
- `session/fork` `cursorId` / `cursor_update` in SDK 2.8.0 and claude-agent-acp 0.108.1's bundled SDK (2.8.0) — the installed 2.7.0 schema has no cursor fields.
- Full registry agent count and per-entry capability claims (registry has none; docs card list read only).
- Exact RFD statuses beyond the updates page excerpt read (session notices/compaction/permissions preview states not re-verified today).
- Rust SDK's exact client handler trait names beyond the README quickstart (`Client.v1().name(...).connect_with(AcpAgent::from_args([...]), async |cx| …)`); docs.rs pages were not read.
