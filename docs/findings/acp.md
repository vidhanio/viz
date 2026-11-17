# ACP (Agent Client Protocol)

_Research snapshot 2026-11-17 (scout)._

**Spec**: https://agentclientprotocol.com · **stable v1** is the target; **v2 is draft** (`/protocol/v2/...`, RFDs). Local agents = subprocess + JSON-RPC 2.0 over stdio (newline-delimited); remote HTTP/WebSocket is WIP. Reuses MCP JSON shapes; default text format Markdown.

### SDKs

| Language   | Package / crate                             | Version                | Notes                                                                                                                                                                                                                        |
| ---------- | ------------------------------------------- | ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| TypeScript | npm `@agentclientprotocol/sdk`              | **2.7.0**              | Apache-2.0; default export = stable v1; `/unstable`, `/experimental/v2`, `/experimental/v2/unstable`; peer `zod ^3.25.76 \|\| ^4`; optional `@modelcontextprotocol/server ^2.4.0`; repo `agentclientprotocol/typescript-sdk` |
| Rust       | crates.io `agent-client-protocol`           | **5.3.0** (2026-11-14) | Apache-2.0, edition 2024, MSRV 1.88; features `process`, `stdio`, `schemars`, `unstable`, `unstable_protocol_v2`; repo `agentclientprotocol/rust-sdk`                                                                        |
| Others     | Kotlin/Java/Python official; community libs | —                      | via `/libraries/*`                                                                                                                                                                                                           |

### Client-side flow (spawn → initialize → session → prompt → updates → permissions)

1. **Spawn** the agent as a child process with `stdin/stdout` pipes (stderr inherited/logged). Nothing on the wire until `initialize`.
2. **`initialize`** (request): `{ protocolVersion: 1, clientCapabilities: { fs: { readTextFile, writeTextFile }, terminal: true, auth: { terminal: true }, session?: { compaction: {}, notices: {} } }, clientInfo: { name, title, version } }`. Response: `{ protocolVersion, agentCapabilities: { loadSession, promptCapabilities: { image, audio, embeddedContext }, mcpCapabilities: { http, sse }, sessionCapabilities… }, agentInfo, authMethods }`. Omitted capability = unsupported.
3. **`session/new`** `{ cwd, mcpServers: [{ name, command, args, env } | { type:"http", url, headers }] }` → `{ sessionId }`. (**`session/load`** replays history via `session/update` if `loadSession`; **`session/resume`** restores without replay if advertised; `session/close` cancels/frees.)
4. **`session/prompt`** `{ sessionId, prompt: ContentBlock[] }` — blocks: `text`, `image`, `audio`, `resource_link`, `resource` (embedded context, gated by capabilities). Response is deferred until the turn ends: `{ stopReason: "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" | "cancelled" }`.
5. **`session/update`** notifications meanwhile: `user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`, `available_commands_update`, `current_mode_update`, `config_option_update`, `session_info_update`, `usage_update`, `notice`, `compaction_update`, `compaction_summary_chunk`. Tool calls carry `toolCallId`, `title`, `name`, `kind` (`read|edit|delete|move|search|execute|think|fetch|switch_mode|other`), `status` (`pending|in_progress|completed|failed`), `content`, `locations`, `rawInput/rawOutput`.
6. **Permissions**: agent → client `session/request_permission` `{ sessionId, toolCall:{toolCallId}, options:[{optionId,name,kind}] }`; client replies `{ outcome: { outcome:"selected", optionId } }` or `{ outcome:{ outcome:"cancelled" } }`. Kinds seen: `allow_once`, `allow_always`, `reject_once`. On cancel, client must answer all pending permission requests with `cancelled`.
7. **Client-side methods you must serve** if advertised: `fs/read_text_file`, `fs/write_text_file`, `terminal/*` (create/output/wait/kill/release), `session/request_permission`, elicitation. Clients can also allow the agent to prompt for terminal-auth.
8. **Cancel**: `session/cancel` (notification), then prompt response `stopReason: "cancelled"`.

**TypeScript (fluent API, from `src/examples/client.ts`):**

```ts
import * as acp from "@agentclientprotocol/sdk";
const stream = acp.ndJsonStream(Writable.toWeb(proc.stdin), Readable.toWeb(proc.stdout));
await acp
  .client({ name: "viz" })
  .onRequest(acp.methods.client.session.requestPermission, (ctx) => approve(ctx.params))
  .onRequest(acp.methods.client.fs.readTextFile, (ctx) => ({ content: "" }))
  .connectWith(stream, async (ctx) => {
    const init = await ctx.request(acp.methods.agent.initialize, {
      protocolVersion: acp.PROTOCOL_VERSION,
      clientCapabilities: { fs: { readTextFile: true, writeTextFile: true } },
    });
    return ctx.buildSession(process.cwd()).withSession(async (session) => {
      session.prompt("Hello");
      for (;;) {
        const m = await session.nextUpdate();
        if (m.kind === "stop") return m.response; // { stopReason }
        handle(m.notification); // session/update
      }
    });
  });
```

(Deprecated `ClientSideConnection`/`AgentSideConnection` classes still exist; use `client()`/`agent()`.)

**Rust (from `examples/yolo_one_shot_client.rs`, crate 5.x):**

```rust
let agent = AcpAgent::from_str(&command)?;           // spawns process (feature "process")
Client.v1()
  .on_receive_notification(async move |n: SessionNotification, _cx| { … }, on_receive_notification!())
  .on_receive_request(async move |req: RequestPermissionRequest, responder, _c| { … }, on_receive_request!())
  .connect_with(agent, |conn: ConnectionTo<Agent>| async move {
      let init = conn.send_request(InitializeRequest::new(ProtocolVersion::V1)).block_task().await?;
      let s = conn.send_request(NewSessionRequest::new(cwd)).block_task().await?;
      let r = conn.send_request(PromptRequest::new(s.session_id, vec![ContentBlock::Text(TextContent::new("hi"))])).block_task().await?;
      Ok(())
  }).await?;
```

### Well-known adapters and commands

| Agent                               | Repo / package                                                                                                                                      | Version              | Command to spawn                                                                                                                                                                       |
| ----------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude Agent (ex-`claude-code-acp`) | `agentclientprotocol/claude-agent-acp`; npm `@agentclientprotocol/claude-agent-acp` (old `@zed-industries/claude-code-acp` **deprecated**, renamed) | 0.108.0 (old 0.16.2) | `npx -y @agentclientprotocol/claude-agent-acp` or binary `claude-agent-acp`; bundles/uses `claude` CLI (`CLAUDE_CODE_EXECUTABLE`); Node ≥22                                            |
| Codex                               | `agentclientprotocol/codex-acp`; npm `@agentclientprotocol/codex-acp`                                                                               | 2.7.0                | `npx -y @agentclientprotocol/codex-acp` / `codex-acp`; bundles `@openai/codex ^0.166.0`                                                                                                |
| Gemini CLI                          | `google-gemini/gemini-cli`                                                                                                                          | —                    | `gemini --acp` (add `--debug` for logs)                                                                                                                                                |
| OpenCode                            | `sst/opencode` (registry: `anomalyco/opencode`)                                                                                                     | —                    | `opencode acp`                                                                                                                                                                         |
| Pi                                  | `svkozak/pi-acp`; npm `pi-acp`                                                                                                                      | 0.0.34               | `npx -y pi-acp` / `pi-acp`; spawns `pi --mode rpc`; needs `@earendil-works/pi-coding-agent` ≥0.81.0, Node ≥20; `pi-acp --terminal-login`                                               |
| Auggie                              | `augmentcode/auggie`                                                                                                                                | 0.37.0               | `npx -y @augmentcode/auggie --acp`                                                                                                                                                     |
| Autohand Code                       | `autohandai/code-cli`                                                                                                                               | 0.9.11               | `autohand --acp`                                                                                                                                                                       |
| IBM Bob                             | npm `@ibm/bob`                                                                                                                                      | 2.1.0                | `npx @ibm/bob acp`                                                                                                                                                                     |
| Amp                                 | `tao12345666333/amp-acp`                                                                                                                            | 0.13.0               | `amp-acp` binary                                                                                                                                                                       |
| GitHub Copilot CLI                  | ACP public preview                                                                                                                                  | —                    | _command UNVERIFIED_ (docs page 404 at time of research)                                                                                                                               |
| Cursor                              | registry `cursor`                                                                                                                                   | —                    | _command UNVERIFIED_ (docs: cursor.com/docs/cli/acp)                                                                                                                                   |
| Others                              | Goose, Cline, Cursor, Qwen Code, Kimi, Kiro, Droid, Mistral Vibe, OpenHands, Stakpak, Junie, …                                                      | —                    | see registry JSON `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json` (`distribution.npx` / `distribution.binary`) and `agentclientprotocol.com/get-started/agents` |

Registry is the machine-readable source: entries carry `id`, `version`, `distribution.npx.package`, `distribution.binary.<platform>.cmd/args/sha256`, auth methods.

### Mapping to viz (concrete)

- viz's orchestrator conversation ↔ one long-lived ACP `session` hosted by a **viz ACP agent** (pi-durable) that the desktop/iOS apps drive; external harnesses (Claude/Codex/Gemini/OpenCode/pi) are spawned by viz as **ACP clients** per subagent conversation, one process per session or per pool, with `session/new` + `cwd` = workspace.
- Subagent results return as messages into the orchestrator's memory tree — exactly uniichat's `spawn`/`[Name] report` pattern, but over ACP `session/prompt` + `session/update` instead of an in-process queue.
- Auto-approve policy maps to `session/request_permission` handling (`allow_once` / `reject_once` by `kind`, never by index); cancellations map to `session/cancel` → `stopReason: cancelled`.
