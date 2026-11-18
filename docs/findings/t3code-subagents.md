# t3code: subagents, toolkit delivery, ACP runtime

Source: `github.com/pingdotgg/t3code`, branch `main`, read 2026-11-18 via GitHub raw/API. Line anchors are from that snapshot and may drift. Repo-relative paths; prefix `https://github.com/pingdotgg/t3code/blob/main/` to open.

## Summary

- t3code's toolkit is **one MCP server** (`apps/server/src/mcp/**`) mounted at `POST /mcp`, authenticated by a **per-scope bearer token** with per-thread state (`McpSessionRegistry`). It is not embedded in the provider process.
- Three delivery paths exist to an ACP agent: direct HTTP MCP, **MCP-native ACP transport** (`type: "acp"`), and a **stdio proxy** (`t3 acp-mcp-bridge`) for agents that need a `command`-based MCP server. The stdio proxy simply pumps newline JSON-RPC to the same HTTP endpoint.
- Deferred exposure (`tool_search`) is a **token-efficiency** feature, not a security boundary; a `t3-tool-exposure: deferred` header hides tools until searched.
- Subagents are ordinary threads plus `parent`/`subagentId` metadata. Completion is delivered by the **event reactor** as a user-role message containing metadata `delegationCompletion`; the parent may be idle. There is no dedicated child→parent question RPC: children report by completing, by ordinary `t3_thread_send`, or by approval. Parent→child steering is `t3_thread_send` with `auto|queue|steer|restart`.
- ACP handling is a normalized adapter over `effect-acp`: process spawn, `initialize`, `session/new` or `loadSession` with replay, permission policy, fs/terminal handlers only when advertised, `session/cancel`, and per-provider `restartRuntimeAfterInterrupt`.
- Native-session import is a first-class flow: list via ACP `session/list` (`sessionCapabilities.list`), then `threadLaunch.launch({ importedNativeThread: { ref: { strength: "strong" } } })`; history arrives via `AgentSessionImporter.importNativeHistory`.

## Files worth opening

| Path                                                                                                     | Contents                                                                                                                           |
| -------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `apps/server/src/mcp/toolkits/orchestrator/tools.ts`, `handlers.ts`                                      | 20 orchestrator tools incl. `delegate_task`, `task_status`, `task_cancel`, `create_threads`, `t3_thread_*`, `t3_background_wait`   |
| `apps/server/src/mcp/toolkits/thread/tools.ts`                                                           | `t3_thread_search/configure/transfer/organize`, `t3_pending_request`, `t3_thread_queue`                                            |
| `apps/server/src/mcp/OrchestratorMcpService.ts`                                                          | All handlers; `TASK_WAKE_EVENTS`; `delegatedTaskAwaitsWork`, completion delivery                                                   |
| `apps/server/src/mcp/McpToolExposure.ts`                                                                 | `TOOL_SEARCH`, `DIRECT_TOOLS`, header; 10-result search                                                                            |
| `apps/server/src/mcp/McpInvocationContext.ts`                                                            | `threadId`, `providerSessionId`, `runtimeMode`, `interactionMode`, `subagentId`, `hiddenFromSubagents`, `hiddenFromOutsideClients` |
| `apps/server/src/mcp/McpHttpServer.ts`                                                                   | `/mcp` route, bearer auth, origin rejection, tool call dispatch                                                                    |
| `apps/server/src/mcp/McpSessionRegistry.ts`                                                              | Token hash → thread/provider scope; revocation                                                                                     |
| `apps/server/src/mcp/AcpMcpStdioBridge.ts`                                                               | `acp-mcp-bridge` and `acp-mcp-call`                                                                                                |
| `apps/server/src/orchestration-v2/ThreadManagementService.ts`                                            | Dispatch modes, `delegatedTaskAwaitsWork`, `workflowStatus`, interrupt                                                             |
| `apps/server/src/orchestration-v2/delegatedTaskModes.ts`                                                 | `resolveRuntimeMode`, `resolveInteractionMode`, `taskPrompt`                                                                       |
| `apps/server/src/orchestration-v2/ThreadLaunchService.ts`                                                | `importedNativeThread`, first-run commit, execution modes                                                                          |
| `packages/contracts/src/orchestratorMcp.ts`                                                              | Every tool input/result schema                                                                                                     |
| `packages/contracts/src/orchestrationV2.ts`                                                              | `OrchestrationV2RuntimeRequest`, notification source, `subagent` turn item, imported-thread fields                                 |
| `packages/provider-acp/src/server/AcpSessionRuntime.ts`                                                  | Spawn/initialize/new/load/prompt/cancel/turn lifecycle, session/list/delete/fork/providers                                         |
| `packages/provider-acp/src/server/adapter.ts`                                                            | ACP→V2 normalization, capabilities, MCP server selection, restart, fs/terminal wiring                                              |
| `packages/provider-acp/src/server/{runtimeModel,clientTerminals,clientPolicy,elicitationForm}.ts`        | Update projection, terminals, permission classification, form elicitation                                                          |
| `packages/provider-acp/src/server/mcpOverAcpBridge.ts`                                                   | MCP-over-ACP tunnel implementation                                                                                                 |
| `packages/effect-acp/src/client.ts`                                                                      | Low-level ACP client, handler registration, process transports                                                                     |
| `packages/provider-acp-registry/src/server/{driver,AcpRegistrySupport,AcpRegistryRuntimeCoordinator}.ts` | Registry driver, binary/npx/uvx resolution, installs, session management                                                           |
| `apps/server/src/provider/AcpRegistrySessions.ts`                                                        | Native-session list/import/delete; `importedNativeThread` launch                                                                   |
| `apps/server/src/ws.ts`                                                                                  | `server{List,Import,Delete}AcpRegistrySession` RPCs                                                                                |
| `docs/orchestration-v2/orchestrator-mcp-server.md`                                                       | Authoritative behavior: dispatch modes, wake, work status                                                                          |
| `docs/user/providers-acp.md`                                                                             | ACP provider UX, native sessions, provider switching, subagent approvals                                                           |
| `docs/internals/provider-sessions.md`                                                                    | Provider-session layering and transcript boundary                                                                                  |

## 1. Subagents end to end

**Identity.** A delegated child is a normal thread; the relationship is recorded in scope/turn items, not in a special child process. `McpInvocationContext.McpInvocationContext` carries `subagentId` for a delegated child and `hiddenFromSubagents`/`hiddenFromOutsideClients` flags. `OrchestrationV2TurnItem` for a subagent (contracts:1895-1945) includes `subagentId`, `origin: "provider_native" | "app_owned"`, `driver`, `providerInstanceId`, `childThreadId`, `prompt`, `progress`, and a result field.

**Fresh-session semantics.** `delegate_task` (docs, `orchestrator-mcp-server.md`): “`delegate_task` always creates a T3 child thread and run. The child receives only the supplied task prompt, plus an optional role instruction supplied in the same call. Parent conversation history is not copied into the child.” The prompt is assembled in `delegatedTaskModes.ts`:

```ts
export function taskPrompt(input: OrchestratorMcpDelegateTaskInput): string {
  return input.role === undefined || input.role === "general"
    ? input.task
    : `Act as the ${input.role} sub-agent for this task.\n\n${input.task}`;
}
```

**Inputs / outputs.** `packages/contracts/src/orchestratorMcp.ts` defines `OrchestratorMcpDelegateTaskInput` (`task`, `role`, mode selection, `title`, `target`, `dispatchMode`), and result types `OrchestratorMcpDelegateTaskResult` (`threadId`, `runId`, `status: started`, `messageId`). `create_threads` creates one run per prompt and returns per-index `{ threadId, runId, status }` results. Tool descriptions are the behavioral contract; exact optional-field schemas are in that file.

**Dispatch modes** (`ThreadManagementService.ts`, docs §“Dispatch modes”, `DispatchModeLimit.ts`): limits are per-thread; `steer` and `restart` have no config limit. Semantics:

- `auto`: idle → new turn and start it; working → `steer`.
- `queue`: hold until the current run finishes. If a parent delegates multiple tasks to one child in one call, only the first starts; later ones queue.
- `steer`: send to the live turn; refused if that provider cannot take mid-turn guidance.
- `restart`: interrupt the current run and start a successor with all pending input concatenated. Repeated restarts do not accumulate a backlog.

**Parent→child messages.** `t3_thread_send` on a running child uses the mode; on an idle **top-level** thread `auto` starts a turn. On a **finished child** it fails with “conversation is not running”; the child must be restarted via a new `delegate_task`/`create_threads` (docs). In reply, return the stable link `t3://thread/<threadId>`.

**Child→parent messages.** There is no separate “report to parent” primitive. The child is a thread, so `t3_thread_send` can target any thread it can identify; otherwise:

1. its result is delivered as the completion wake,
2. an approval request stays in the child,
3. or the child ends its turn with questions in its transcript.

viz should therefore add a first-class `report_back` / `message_parent` tool whose parent is fixed by MCP scope, rather than leveraging `thread_send` plus discovery.

**Completion wake.** `delegate_task` seeds the child and returns immediately. On completion the reactor writes a **user-role message** into the parent, text beginning `Completion: ...`, with `metadata.delegationCompletion` and `senderThreadId`; a parent that is idle is woken by it. `docs/orchestration-v2/orchestrator-mcp-server.md` shows the parent-visible form:

```text
Completion: <one-line summary of what the task did>

<final report of the delegate>
```

and the tool-facing representation:

```json
{
  "kind": "delegated_task",
  "status": "completed",
  ...
}
```

`OrchestratorMcpService.ts` defines the event set that can wake a delegating MCP call:

```ts
const TASK_WAKE_EVENTS = [
  { thread: "parent", eventType: "subagent.updated" },
  { thread: "child", eventType: "run.updated" },
  { thread: "child", eventType: "subagent.updated" },
  { thread: "child", eventType: "provider-thread.updated" },
  { thread: "child", eventType: "runtime-request.updated" },
] as const;
```

Only **terminal** child runs count. `delegatedTaskAwaitsWork` (from `RestartBackgroundNote.ts`) requires a non-terminal delegated run and either a current child run or a last run that started; a background task can hold the thread open. Hooks and monitors do not count as work.

**Work states.** `working | waiting_for_children | result_available` (`task_status` and `t3_thread_read`). `waiting_for_children` requires a non-terminal child with live work (or an unstarted last run). A pending approval does **not** produce a wake: “Approvals never end the wait; a pending live approval waits … the task’s completion still wakes this thread”. The parent must `t3_thread_read` the child / poll `t3_pending_request` (docs).

**Results in the parent transcript.** Two representations: (a) the synthetic `subagent` turn item with prompt/progress/result, and (b) the injected `Completion:` message. UI hides the child transcript from the parent by default but links it (`childThreadId`). Role prompts and title generation are parent-controlled.

**`t3_background_wait`.** Contract tool in `orchestrator/tools.ts`; it is a **marker**, not a blocking wait. It registers provider-native background task IDs so auto-settle does not complete the parent while the provider still has work, then the agent ends its turn; provider completion wakes the thread. Availability is provider-gated (Claude Code/Codex background commands; issues show it must reject IDs that are not background work). Implementation lives in provider adapters + `backgroundWorkHoldsCompletion` in `@t3tools/shared`; details are **UNVERIFIED** in the server core.

**Approval bubbling.** `OrchestrationV2RuntimeRequest` (`contracts/orchestrationV2.ts:1280-1310`) is scoped to `nodeId`, `providerTurnId`, `nativeRequestRef`, `kind`, `status`, `responseCapability` (`live | message | not_resumable`), and `decision/feedback/answers`. Notification sources include `{ kind: "subagent", childThreadId }` (`contracts:1315-1319`). For **app-owned** delegated tasks, approval requests belong to the child thread; the parent learns via `t3_thread_read`/`t3_pending_request`, not by automatic forwarding. For **provider-native** subagents (e.g. Claude Code's own Task subagents), `docs/user/providers-acp.md` states approvals “appear in the parent conversation with the subagent's session ID as the thread ID.” Do not conflate the two.

## 2. How the toolkit reaches provider sessions

**One MCP endpoint.** `McpHttpServer` mounts a streamable HTTP MCP route; `server.ts:861` provides `McpHttpServer.layer`. Authentication is a **per-thread bearer token**:

- `McpSessionRegistry` stores a SHA-256 hash of the token and maps it to `McpInvocationScope` (`threadId`, `providerSessionId`, runtime/interaction modes, subagent origin), plus `hiddenFromOutsideClients`.
- `McpInvocationContext.McpInvocationContext` is built per call; `hiddenFromSubagents`, `hiddenFromOutsideClients`, and `toolExposure` gate the tool list.
- The transport rejects browser `Origin` before checking credentials: “The transport allows no browser origin: refuse it before credentials, as the MCP spec requires.” (`McpHttpServer.ts` middleware comment).
- A client outside a thread (CLI/another app) gets a scope with `hiddenFromOutsideClients`; `docs/user/outside-agents.md` is the user-facing contract.

**Deferred exposure.** `McpToolExposure.ts`:

```ts
export const TOOL_SEARCH = "tool_search";
const DIRECT_TOOLS = new Set(["orchestrator.capabilities", "delegate_task"]);
const MAX_ENDPOINT_TOOLS = 256; // endpoint
const MAX_RESULTS = 10; // search results, default
```

When the request header `t3-tool-exposure: deferred` is present, the endpoint lists only `tool_search`, the direct tools, and thread-scoped tools already named by the session; everything else is searchable by `tool_search` (name/title/description/tags; prefix and substring match). `initialize` writes a short instruction saying the full chain is available but tools are hidden until searched. This is purely prompt-size control; authorization stays with the token.

**ACP delivery selection** (`provider-acp/src/server/adapter.ts`, `docs/user/providers-acp.md`):

| Agent advertises                             | t3 injects                                                                                                                              |
| -------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `mcpCapabilities.http`                       | HTTP MCP server `{ type: "http", url, headers: { Authorization: "Bearer …" } }`                                                         |
| `mcpCapabilities.acp` and t3's env allows it | `{ type: "acp", name, id, _meta: { "t3tools/connectionId" } }`; MCP JSON-RPC travels as ACP `mcp/connect                                | message | notification | disconnect` |
| neither                                      | stdio MCP server `{ name, command: <t3 executable>, args: ["acp-mcp-bridge"], env: { T3_ACP_MCP_ENDPOINT, T3_ACP_MCP_AUTHORIZATION } }` |

The default is automatic: “When T3 starts an ACP agent, it sends its MCP server over the connection the agent supports: MCP over HTTP if … or MCP over ACP if … Otherwise T3 Code runs an installed `t3 acp-mcp-bridge` MCP server as a stdio MCP server.” (`providers-acp.md`). If no supported MCP binding is found, t3 reports “could not find a supported MCP binding”.

**Stdio bridge** (`AcpMcpStdioBridge.ts`). It is a line pump: reads newline-delimited JSON-RPC from stdin, forwards to `T3_ACP_MCP_ENDPOINT` with the bearer and handles `mcp-session-id`/`mcp-protocol-version`; responses go back to stdout. `initialize` and `notifications/initialized` are serialized behind a barrier so no call races the handshake. `acp-mcp-call <tool> <json>` is a one-shot variant for shell-driven flows; it returns both `content` and `structuredContent`.

**MCP-over-ACP** (`mcpOverAcpBridge.ts`). This is **not** a different MCP wire protocol: the agent's `mcp/connect` request (transport `acp`) is answered by a bridge that POSTs JSON-RPC to the same HTTP endpoint, keyed by an ACP connection ID. MCP messages become ACP `mcp/message` requests. Limits: 16 connections, 8 MiB per message, 5 s disconnect timeout. Adapter wiring:

```ts
yield* targetRuntime.handleMcpConnect((request) =>
  mcpBridge.connect(request).pipe(mapMcpBridgeError),
);
yield* targetRuntime.handleMcpMessage(...);
yield* targetRuntime.handleMcpNotification(...);
yield* targetRuntime.handleMcpDisconnect(...);
```

The bridge is optional: `wireAcpRuntimeMcpHandlers` returns immediately if no bridge exists. Calls in flight are failed on `stopCalls` so Stop releases the HTTP responses.

**Identifying the caller.** There is no thread ID in the MCP request; identity is the bearer token’s registry entry, resolved to `McpInvocationScope`. The scope is what makes per-thread cwd, runtime/interaction mode, `subagentId`, and hidden-tool decisions. Outside-client tokens are separately marked. This is the exact piece viz needs: one token per conversation, not per process.

## 3. ACP client runtime

**Spawn.** `AcpSessionRuntime.start` takes `AcpSpawnInput` (`command`, `args`, `cwd`, `env`, `extendEnv`, `shell`). On POSIX it resolves `command`; on Windows it wraps in `sh -c` when needed; it spawns via `ChildProcessSpawner`, pipes stderr, and configures `EffectAcpClient.layerChildProcess(child, …)`. `options.spawn.shell === false` is honored; termination kills the process group (along with `taskkill` on Windows). Protocol logging and request logging are injected.

**Initialize / capabilities.** The adapter sends `clientCapabilities` only for features it actually implements (`adapter.ts:2570-2600`):

```ts
clientCapabilities: {
  fs: {
    readTextFile: flavor.clientFileSystem !== undefined,
    writeTextFile: flavor.clientFileSystem?.writeTextFile !== undefined,
  },
  terminal: clientTerminals !== undefined,
  elicitation: { form: {}, ...(flavor.onUrlElicitation ? { url: {} } : {}) },
}
clientInfo: { name: "t3-code", version: "0.0.0" }
```

Returned agent capabilities are mapped (`adapter.ts:868-890`): `loadSession` → `canReadThreadSnapshot`; `sessionCapabilities.fork` → `canForkThread` (+ `canForkFromTurn`); config/model options → `supportsModelSwitchInSession`; `supportsMcpTools: true`. **Do not advertise fs/terminal if viz cannot serve them**; t3 gates advertisement on implementation presence.

**Session creation vs load.** `AcpSessionRuntime` calls `newSession` when no `resumeSessionId`, otherwise `loadSession`; on load it replays `session/update` history unless the update lacks turn grouping/IDs (idle load then must call `session/load` again). `session/fork`, `session/list`, `session/delete`, `session/close`, provider config, and auth are all capability-gated with `-32601` when unadvertised. `session/list`:

```ts
cwd: options.cwd,
...(cursor === undefined ? {} : { cursor }),
```

**Prompt lifecycle.** One active prompt; a second prompt queues. Text deltas are throttled at 50 ms; subagent text at 400 ms (`ACP_SUBAGENT_TEXT_FLUSH_INTERVAL`) because subagent messages persist each snapshot. Prompt inactivity timeout ignores metadata/unknown updates and resets only on listed activity updates (`agent_message`, `agent_thought`, `tool_call`, `plan`, `terminal_output_chunk`, …). `session/cancel` waits for the prompt to be queued, then either interrupts the local prompt and writes cancel, or (with `cancelBehavior: "wait-for-prompt"`) waits for agent completion up to `cancelTimeout` and on timeout **kills the process** with an `AcpTransportError` naming `session/cancel`.

**Permission policy** (`clientPolicy.ts`). `acpPermissionDisposition(runtimePolicy, request)` returns `allow | ask | deny`. Reads (`read/search/think`) are never asked and are allowed unless an unrecognized sandbox says otherwise. Non-reads: `ask` when approval-required (or explicit `approvalPolicy !== "never"`); otherwise sandbox governs: `readOnly` -> deny; `workspaceWrite` -> allow only mutations inside cwd/writableRoots (symlink-resolved containment); `dangerFullAccess`/`externalSandbox` -> allow. `auto-accept-edits` asks for non-mutations when no explicit approval policy is set. Special case: Codex `mcp_tool_call` elicitations are allowed automatically outside approval-required mode because T3's MCP endpoint enforces scope. Execute approvals grant per turn or session via `makeAcpClientPolicyGrants`.

**fs/terminal.** The client registers `handleReadTextFile`/`handleWriteTextFile` only for flavor-provided `clientFileSystem`; reads are bounded to the policy cwd. Terminals use `ChildProcessSpawner` (not a pty), keep up to 16 live terminals, 4 MiB default output (8 MiB max), truncate from the beginning at a UTF-8 boundary per spec, and retain up to 32 released terminals / 16 MiB. The adapter advertises `terminal: true` only when `clientTerminals !== undefined`.

**session/update mapping.** `runtimeModel.ts` parses update variants; `adapter.ts` projects them (`projectAcpRuntimeSessionUpdate`) into the V2 turn model. The activity list in `adapter.ts:196-210` doubles as the canonical set of updates it understands: `agent_message(_chunk)`, `agent_thought(_chunk)`, `tool_call(_update|_content_chunk)`, `terminal_output_chunk`, `terminal_update`, `plan(_update|_removed)`, `compaction_update`, `compaction_summary_chunk`. Unknown tags are preserved for forward compatibility via contract forward-compatible unions. Tool-call output is stored separately; large output can be omitted with a fetch path (`outputOmitted`, `getTurnItem`) — copy this to keep transcripts small.

**Restarts.** `options.restartRuntimeAfterInterrupt` is per-driver. On restart the runtime tears down the process, clears the generation’s pending requests, and starts a replacement runtime with the same MCP token and `resumeSessionId`; `t3_thread_send` `restart` reuses this. `invalidateCaches` clears live model/command state, not transcripts. Hooks and monitors may be lost on restart; the transcript survives.

**Agent registry** (`provider-acp-registry`). A registry entry is an `AcpRegistryAgent` with `distribution: { binary?: { <platform>: { cmd, args, env, sha256 } }, npx?: { package, version, args, env }, uvx?: … }`. Resolution preference is `binary` → `npx` → `uvx`; npx packages must pin an **exact version** (`EXACT_NPX_PACKAGE`). Installs go to an app-owned install root; npm/uv paths are handled per platform. Launch yields `AcpSessionRuntime.AcpSpawnInput`. Versions are registry-selected; t3 does not float `@latest`. The driver exposes `acpSessionManagement` with `listSessions`, `deleteSession`, `listProviders`, `setProvider`, `disableProvider`, `logout` — all capability-gated by the agent.

## 4. Other things a meta-harness should copy

- **Capability-driven adapter contract.** `ProviderAdapterV2` exposes `sessions.supportsModelSwitchInSession`, `threads.canReadThreadSnapshot/canForkThread/canForkFromTurn`, `tools.supportsMcpTools`. UI/commands degrade from these, not from provider names. Copy the flags; don't branch on “is Claude”.
- **Provider switch handoff.** Switching model write-scope preserves the normalized transcript and resumes via `session/load`; native load replay is idempotent keyed by provider turn ordinal. The tool/`session` boundary is the durable transcript, not the provider’s session file.
- **Transcript import.** ACP native sessions are listed with `session/list` (`cwd`, cursor) and imported with `serverImportAcpRegistrySession` (`ws.ts:2402`). Import requires `nativeSessions.canLoad || canResume`; it derives a deterministic thread ID from `{driver, instanceId, sessionId}`, so re-import is idempotent, then launches with:

```ts
importedNativeThread: {
  ref: { driver, nativeId: input.sessionId, strength: "strong" },
  metadata: { itemIdentityVersion: 2, title?, updatedAt? },
}
runtimeMode: "approval-required",
interactionMode: "default",
workspaceStrategy: { type: "root" },
```

History is materialized by `AgentSessionImporter.importNativeHistory({ threadId, workspaceRoot })`; failures surface as `session_import_failed` and are retryable (“repairing”). A strong native ref also suppresses auto-fork/restart until the first delivered turn. Copy this pattern for `claude-agent-acp`/`codex-acp` session adoption.

- **Remote delegation / environment links.** Orchestrator tools accept `environmentId` to forward the call to a linked machine, with the link's access and the caller's modes both applied. `PeerForwarding`/`RemoteDelegation` implement it. This is how “spawn/message an agent on another machine” is expressed without a second protocol. viz may defer this.
- **Token-scoped tool surface.** `hiddenFromSubagents` and `hiddenFromOutsideClients` are evaluated per call from `McpInvocationContext`; `toolExposure` separately controls prompt size. Keep security (scope) and prompt economy (exposure) orthogonal as t3 does.
- **Link/ID stability.** Thread links are `t3://thread/<threadId>`; MCP command IDs are deterministically derived from `{requestNamespace, operation, requestKey, index}` so retries are idempotent. viz's `report_back`/`spawn_agent` should take an idempotency key and return the same conversation ID on retry.
- **Outside-agent contract.** A client with no thread can call the toolkit with explicit `projectId`/`environmentId`; writes outside a thread are surfaced as `hiddenFromOutsideClients`. This is a good model for viz's CLI/ACP-only agents.
- **Human-in-the-loop UX.** Child threads start hidden with an empty UI and only the role+task prompt; the parent shows a subagent card; completion is a normal user message so it is visible in every client. Copy that rather than inventing a separate “results” pane.

## Design implications for viz (sourced, concise)

1. **Expose one HTTP MCP endpoint** from `viz-backend` with per-conversation bearer tokens mapped to a `VizInvocationScope` (`conversationId`, `kind: orchestrator|agent`, `parentConversationId`, modes). This is exactly `McpSessionRegistry` + `McpInvocationContext`; it makes `report_back`/`message_agent`/`list_conversations`/`spawn_agent` secure without trusting a thread ID in the request body.
2. **Advertise MCP via ACP when possible** (`mcpCapabilities.http` or `.acp`), and keep a **stdio proxy** for pi/claude/codex agents that only accept `command` servers. The proxy needs just `POST /mcp` + bearer + session-id forwarding — t3's `AcpMcpStdioBridge` is ~150 lines and is the cheapest path to “every ACP agent gets viz tools”.
3. **Implement `report_back` explicitly.** t3code relies on completion wake plus `t3_thread_send`; it has no scoped parent channel. viz's orchestrator needs child questions/reports that arrive as durable messages, so fix the parent in the MCP scope and add `report_back({ status, summary, detail?, question? })` returning `{ delivered: true, messageId }`; mirror the `delegationCompletion` metadata shape so the orchestrator transcript renders the same as t3's completion wake.
4. **Keep work states** `working | waiting_for_children | result_available`; only terminal child runs wake; a child waiting on approval is not terminal. Provide `wait_for_agents` analogous to `t3_thread_wait` with “timeout with `next` ends your turn” semantics.
5. **Defer tool exposure** with `tool_search` from day one; the orchestrator toolkit plus ACP-agent toolkit will exceed a comfortable prompt. Copy t3's direct-tool set (capabilities + delegate/spawn) and prefix search.
6. **Capability-gate fs/terminal/load/fork** and never advertise what viz cannot serve. ACP agents _will_ call fs/terminal when advertised, and a half-implemented handler is worse than no capability.

## UNVERIFIED

- Exact optional-field JSON schemas of every `OrchestratorMcp*Input/Result` type; only field names observed via handlers/tool text were quoted above.
- How the stdio bridge conveys deferred exposure (`t3-tool-exposure`) since headers do not cross stdio; likely the endpoint only defers for HTTP-flavor servers. No code path observed setting an env/header in `AcpMcpStdioBridge`.
- Whether `t3_background_wait` is implemented as a server-side no-op marker vs. provider-specific registration beyond `backgroundWorkHoldsCompletion`; only tool contract/docs/issues were read.
- Exact `agentCapabilities.mcpCapabilities` field names in `effect-acp` (`http`/`acp` booleans) were inferred from docs and adapter usage; verify in `packages/effect-acp/src/compat` before implementing.
- `OrchestratorMcpService` completion-message producer line numbers: function names and metadata keys were confirmed via contracts/docs, but the write site was not read line-by-line.
- Whether delegated children see the full orchestrator toolkit: `hiddenFromSubagents` exists and is honored, but the annotation on each tool was not exhaustively enumerated.
