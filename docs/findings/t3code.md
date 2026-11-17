# t3code research report (`github.com/pingdotgg/t3code`, `main` @ 2026‑11‑17, v0.1.10)

Not cloned (no shell tool available in this session); everything below was read directly from GitHub raw/API URLs. Paths are repo‑relative.

## 0. Stack summary

| Piece                                                         | Tech                                                                                                                                                                                               |
| ------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Monorepo                                                      | pnpm workspace (`packageManager: pnpm@12.14.0`), TS everywhere, `vite-plus` (`vp`) build/test, oxc transform, vitest                                                                               |
| Server (`apps/server`, pkg name **`t3`**, bin `dist/bin.mjs`) | Node (**engines `^22.16 \|\| ^23.11 \|\| >=24.10`**), **Effect** (`effect`, `effect/cli`, `effect/http`, `effect/sql`, `effect/ai`), `node-pty`, `playwright-core`, `undici`, `ws` via Effect HTTP |
| Web (`apps/web`, `@t3tools/web`)                              | React + TanStack Router + Tailwind 4 + TipTap + zustand + `@effect/atom-react`, clerk-js, i18n via `gt`                                                                                            |
| Desktop (`apps/desktop`)                                      | **Electron 44.10.1** (`main: dist-electron/boot.cjs`), electron-builder, electron-updater, `electron-store`, keyring, `dbus-next`, `ffi-rs`, playwright-core                                       |
| Mobile (`apps/mobile`)                                        | Expo/React Native **[UNVERIFIED]** — evidence: `apps/server` script `generate:expo-schema`, `docs/internals/mobile-*.md`, `apps/mobile` in tree                                                    |
| Infra                                                         | `infra/relay` = Cloudflare Worker + Durable Objects (T3 Connect), Clerk identity                                                                                                                   |
| DB                                                            | SQLite at `<stateDir>/statev2.sqlite` via **`node:sqlite`** wrapped as `@t3tools/shared/nodeSqliteClient` + Effect `SqlClient`                                                                     |

## 1. Repo layout

```
apps/      server/ web/ desktop/ mobile/ marketing/ apiserver/(UNVERIFIED role)   # http://localhost:3000 `vp run -F t3 start` serves web+api
packages/  contracts/ client-runtime/ shared/ tailscale/ ssh/
           provider-core/ provider-acp/ provider-acp-registry/ provider-cursor/
           provider-grok/ provider-kiro/ provider-muse/ provider-opencode/ provider-pi/
           provider-fx/ provider-testing/
           effect-acp/ effect-codex-app-server/
           secret-provider-core|definitions|onepassword/
           source-control-core|definitions|github|gitlab|bitbucket|azure-devops|forgejo|gitcafe|cursor-origin/
           issue-tracker-linear/
docs/      user/ internals/ orchestration-v2/ operations/ scenarios/
```

Split: **web = the whole UI** (desktop loads the same web build); desktop = Electron shell that boots/bundles a `t3` server child (`--mode desktop`, bootstrap secrets passed over a file descriptor: `apps/server/src/bootstrap.ts`, `--bootstrap-fd`). `docs/internals/overview.md` states the renderer follows the same client/server boundary, and "Local environment" can be turned off to make the desktop a remote‑only client. No Rust, no Tauri.

Shared client domain/connection logic lives in `packages/client-runtime` (connection driver, supervisor, reconnect; `docs/internals/connection-runtime.md`), clients supply platform services only.

## 2. Server ⇄ client transport

- **Single HTTP + WebSocket origin.** `apps/server/src/ws.ts` (~line 3560‑3700) serves `Router.add("GET", "/ws", …)`, plus an Effect RPC `Router.get("getAll")` lane. Handlers are `ServerWsRpcGroup.of({…})`, i.e. **typed Effect RPC over WS with JSON serialization** (`effect/http` + `RpcSerialization`), not hand‑rolled JSON‑RPC.
- Contract file: `packages/contracts/src/rpc.ts` (`ServerWsRpcGroup`), protocol version + method names in `packages/contracts/src/orchestrationV2.ts:3847`:

```ts
export const ORCHESTRATION_V2_WS_METHODS = {
  dispatchCommand: "orchestration.dispatchCommand",
  getTurnDiff,
  getFullThreadDiff,
  searchThreads,
  searchThread,
  searchThreadStream,
  getArchivedShellSnapshot,
  getThreadFamily,
  getThreadTitles,
  getThreadLinkMetadata,
  getWorktreeThreadIds,
  getOlderSettledThreads,
  getThreadProjection,
  getWorkflowScript,
  getNativeChildWork,
  stopWorkflow,
  forceStopThread,
  getTurnItem,
  launchThread,
  subscribeShell,
  subscribeThread,
  subscribeCommandOutput,
  subscribePromptSuggestion,
};
```

- **State delivery = snapshot + cursor subscriptions**, per thread/shell, so a client watching one thread doesn't pay for all history (`subscribeThread`, `subscribeShell`, `subscribeCommandOutput` tail frames `{kind: replace|append, text, truncated, running}`, `subscribePromptSuggestion`).
- Commands are **idempotent by `CommandId`**; dispatch returns the last committed stored‑event sequence; duplicate ids replay the stored receipt without re‑running provider side effects (`CommandReceiptStore`, `persistence/OrchestrationCommandReceipts.ts`).
- Protocol/version negotiation: `ORCHESTRATION_PROTOCOL_VERSION` in `packages/contracts/src/orchestrationV2.ts`; version sent as a WS query param (constant `ORCHESTRATION_PROTOCOL_QUERY_PARAM` referenced; exact spelling **UNVERIFIED**). Lookup endpoint: **`GET /.well-known/t3/environment`** returning `ExecutionEnvironmentDescriptor` (used by `t3 pair` probing).
- MCP endpoint **`/mcp`** (Streamable HTTP, OAuth‑protected) exposes the orchestrator tools to outside agents (§7).

## 3. Auth, pairing, tokens

`packages/contracts/src/auth.ts`, `apps/server/src/auth/*`, `apps/server/src/cli/pair.ts`.

- Auth policies: `desktop-managed-local | loopback-browser | remote-reachable | unsafe-no-auth`. Bootstrap methods: `desktop-bootstrap | one-time-token`. Session methods: `browser-session-cookie | bearer-access-token | dpop-access-token`.
- **Scopes** (`AuthEnvironmentScope`): `orchestration:read`, `orchestration:operate`, `settings:write`, `providers:manage`, `environment:maintain`, `preview:operate`, `diagnostics:read`, `terminal:read`, `terminal:operate`, `filesystem:read`, `filesystem:write`, `source-control:write`, `access:read|write`, `relay:read|write`, legacy `review:write`. Parent‑scope fallback table for old servers (`sessionGrantsScope`).
- **Pairing**: one‑time token, 12 chars from alphabet `23456789ABCDEFGHJKLMNPQRSTUVWXYZ`, default TTL **5 m**, printed as QR + `Pairing URL` + `App link`. MCP client access levels map to prefixes: `READ|SUPERVISED|EDITS|AUTO|FULL-<code>`.
- **OAuth token exchange** (RFC 8693 style, form‑encoded): grant `urn:ietf:params:oauth:grant-type:token-exchange`, subject token `urn:t3:params:oauth:token-type:environment-bootstrap` → `access_token` (Bearer or **DPoP**). Separate **WS tickets** (`AuthWebSocketTicketUse = "rpc" | "media"`, minted at a path segment so the DPoP proof binds it).

CLI (quoted from `apps/server/src/cli/config.ts`, `cli/server.ts`, `cli/pair.ts`):

```
t3 [cwd] | t3 serve | t3 start     # serve = headless, prints pairing details
  --mode <desktop|...>  --port  --host "127.0.0.1|0.0.0.0|a Tailnet IP"
  --base-dir  (≡ T3CODE_HOME; default ~/.t3)  --dev-url  --no-browser
  --bootstrap-fd  --auto-bootstrap-project-from-cwd  --log-ws-events
  --tailscale-serve  --tailscale-serve-port  --lan
t3 pair [--scope <scope>…] [--ttl 5m] [--label …] [--tailscale] [--tailscale-serve-port]
t3 up | t3 connect | t3 status | t3 auth pairing create|session issue | t3 auth --help
t3 environment link <address> --pairing-code <code> --access <mode>
```

Env equivalents: `T3CODE_MODE/PORT/HOST/HOME`, `T3CODE_LAN`, `T3CODE_TAILSCALE_SERVE(_PORT)`, `T3CODE_NO_BROWSER`, `T3CODE_LOG_WS_EVENTS`, `T3CODE_DEV_AUTH_TOKEN` (**≥32 chars**), `T3CODE_PORT_DISCOVERY=0`, `T3CODE_SERVER_BROWSER_SANDBOX=0`.

## 4. Remote access (`docs/internals/remote.md`, `docs/user/remote-access.md`, `docs/internals/t3-connect.md`)

- **Identity is separate from route.** Environment keeps a stable `environmentId` (`environment/ServerEnvironment.ts`); a client stores an **ordered route list** and connects over the first working one, preflighting earlier routes and swapping the session (`packages/client-runtime/src/connection/{driver,supervisor}.ts`). Routes: direct LAN/tailnet, Tailscale Serve HTTPS, SSH, T3 Connect tunnel.
- **LAN mode** = listen on all IPv4 interfaces but reject peers whose _both_ addresses aren't loopback/private/tailnet v4 (keeps a VPS public IP closed). `--lan` ≡ `T3CODE_LAN`.
- **Tailscale**: `t3 pair --tailscale` runs `tailscale serve --https=<port> --bg <local-url>`; dedicated errors for no MagicDNS, no HTTPS certs, non‑operator (`sudo tailscale set --operator=$USER`), port already fronting another environment, target not loopback.
- **T3 Connect** = cloud relay for _discovery/bootstrap only_: Clerk identity, relay mints a one‑time credential bound to the client's DPoP key, client redeems it **directly** with the environment; app traffic then rides the managed tunnel hostname, relay does not proxy HTTP/WS. Optional device approval keyed by DPoP thumbprint; links/sessions last **30 days**; `t3 connect unlink|logout`.
- **Peers / "Linked environments"** (beta): env A signs into env B's `/mcp` as an outside OAuth client with a pairing code, stores the token in its own secret store; `delegate_task` with `target.environmentId` runs the task there. Gated by `linkedEnvironmentsEnabled`.

## 5. Driving other harnesses (provider abstraction)

- Contract: **`ProviderAdapterV2`** in `packages/provider-core/src/server/ProviderAdapter.ts` (≈3.1 k lines). Session runtime methods (lines ~690‑900):

```
openSession / closeSession
ensureThread({threadId, modelSelection, runtimePolicy, providerSessionId?,
              existingProviderThread?, forceNewNativeThread?, resumeFromSession?,
              initialNativeThreadId?, promptSuggestions?})
resumeThread({providerThread, threadId?, modelSelection?, runtimePolicy?})
startTurn | resumeTurn | compactThread | steerTurn({runId, providerTurnId, message})
interrupt({providerThread, providerTurnId, requestRuntimeRestart?})
respondToRuntimeRequest({requestId, decision?: ProviderApprovalDecision, feedback?, answers?})
readThreadSnapshot | rollbackThread({target: thread_start|provider_turn, checkpointId})
forkThread({sourceProviderThread, providerTurnId?, targetThreadId, ownerNodeId?})
injectHistory (portable context handoff; may return false)
applyModelSelection / applyThreadTitle (live process, else next turn)
subscribeEvents → Stream<ProviderAdapterV2Event>; hasPendingBackgroundWork; getModelContextWindow
```

- Typed error classes encode the contract (`ProviderAdapterInterruptError/Timeout`, `…SteerRunUnsupportedError`, `ProviderResumeUnsupportedError`, `ProviderCompactionUnsupportedError`, `…CapabilitiesError`, etc.).
- **Capability-driven, never provider‑name‑driven**: `OrchestrationV2ProviderCapabilities` flags (fork, rollback, steering, subagents…) gate orchestration; unsupported → documented fallback (`docs/orchestration-v2/provider-capability-system.md`, `docs/internals/adding-a-provider.md`).
- Implementations: native adapters `ClaudeProvider.ts` (Anthropic Agent SDK `@anthropic-ai/claude-agent-sdk`), Codex via `effect-codex-app-server` (JSON‑RPC app‑server, spawns `codex app-server`), Cursor (`@cursor/sdk`), OpenCode (`@opencode-ai/sdk`), Pi (`provider-pi`, RPC), Muse (`@muse-code/sdk`), fx/Kiro/Grok/Antigravity as small ACP flavors over `packages/provider-acp/src/server/adapter.ts`; generic ACP agents come from `provider-acp-registry` (`AcpRegistryProbe/Sessions`) with zero per‑agent code. Legacy drivers still in `apps/server/src/provider/Drivers`.
- ACP plumbing exists both directions: `effect-acp` (client side), `packages/provider-acp/src/server/{AcpSessionRuntime,adapter,runtimeModel,clientTerminals,clientPolicy,elicitationForm,mcpOverAcpBridge}.ts`, plus an ACP stdio bridge that exposes **T3 itself** as an ACP agent (`apps/server/src/AcpStdioBridge.ts`, `AcpCodingAgent*`).
- Replay‑fixture testing: `apps/server/scripts/record-*-replay-fixture.ts` for codex/claude/cursor/pi/grok/fx/kiro/muse; shared scenarios `simple, multi_turn, queued_turn, turn_interrupt, message_steering, provider_thread_resume, tool_call_*`.

## 6. State model & persistence (event‑sourced, v2)

`docs/orchestration-v2/README.md`:

```
native provider protocol → rotating raw diagnostics → adapter/normalizer
→ V2 orchestration event store (SQLite) → runtime execution graph
→ conversation projection → UI/API views
```

- Invariants: app ids primary / provider ids are refs; child completion never closes parent run; checkpoints attach to scopes; rollback in app run counts; provider switch = explicit context handoff; forks record lineage first and resolve lazily.
- Graph: `AppThread → Run → ExecutionNode{root, tool, approval, subagent} → ProviderThread → child Run`.
- Domain events (`packages/contracts/src/orchestrationV2.ts:1986`): `thread.created`, `thread.{archived,unarchived,deleted,settled,snoozed,pinned,visited,marked-unread,moved?…, model-selection-updated, provider-switched, tags-updated, pull-request-synced,…}`, `run.created|updated`, `run-attempt.created|updated`, `node.updated`, `subagent.updated`, `provider-session.attached|updated|detached`, `provider-thread.updated`, `provider-turn.updated`, `runtime-request.updated`, `message.updated`, plus `OrchestrationV2Notification`, `…StoredEvent{sequence}`.
- Side effects today via **reactors** observing events; README documents a tracked follow‑up "durable effect outbox" (`ProviderEffectRequest {kind: provider.turn.start | turn.interrupt | runtime-request.respond | thread.rollback | thread.fork}`).
- Files: `apps/server/src/orchestration-v2/` (CommandPolicy, CommandReceiptStore, Checkpoint{Service,RollbackService,CaptureService,RestoreSafety}, CommandOutputHub? (`CommandOutputHub.ts`), AttachmentClaims, RuntimePolicy, projections, `legacy/` importer), `apps/server/src/persistence/` (`OrchestrationEventStore.ts`, `AuthSessions.ts`, `AuthPairingLinks.ts`, `ProviderSessionRuntime.ts`, `Migrations/`, `DatabaseStorage.ts`, `Sqlite.ts`).
- Paths (`apps/server/src/config.ts:147‑175`): `stateDir` default `~/.t3/` + `statev2.sqlite`, `attachments/`, `logs/`, `themes/`, `browser-artifacts/`, `secrets/`, `settings.json`, `keybindings.json`, `server-runtime.json`, `environment-id`. Project scratch dirs for project‑less threads: `~/.t3/scratch/<date>-<slug>-<id>`. Dirs chmod 0700, WAL, 6‑hourly compaction, `t3 doctor` reclaim.

## 7. Abilities exposed to harness instances (MCP)

- Injected per thread as an MCP server; **deferred tool exposure**: `DIRECT_TOOLS = {orchestrator_capabilities, delegate_task, task_status, tool_search}`; others listed only after `tool_search` (`apps/server/src/mcp/McpToolExposure.ts`).
- Tools (`apps/server/src/mcp/toolkits/orchestrator/tools.ts`, `toolkits/thread/tools.ts`): `orchestrator_capabilities`, `delegate_task`, `task_status`, `task_cancel`, `create_threads`, `t3_thread_list`, `t3_thread_read`, `t3_thread_update` (rename/regen title), `t3_thread_send` (`mode: auto|queue|steer|restart`), `t3_thread_wait`, `t3_thread_interrupt`, `t3_thread_launch`, `t3_thread_search`, `t3_thread_organize`, `t3_thread_configure`, `t3_thread_transfer`, `t3_queue`, `t3_pending_request`, `t3_workflow_stop`, `t3_background_wait`, `schedule_task`, `list_scheduled_tasks`, `delete_scheduled_task`, `run_scheduled_task_now`, `request_secret` (one‑use `secretRef` or file delivery), `t3_environment_links`, `t3_environment_link`, `t3_environment_unlink`. Toolkit dirs also cover `attachment, device, environment, html, issues, message, preview, previewControls, project, pullRequests, theme, worktree`.
- Tool metadata drives UX/security: `Tool.Title`, `Readonly`, `Destructive`, `Idempotent`, `OpenWorld`, `McpSchema.EnabledWhen` (e.g. `hiddenFromOutsideClients`, `hiddenWhileLinksOff`). Every tool call is scope‑checked (`auth/RpcAuthorization.ts`), and remote‑targeted tools take `OrchestratorMcpEnvironmentTarget.environmentId` and forward the caller's modes.

## 8. Product features worth copying (concrete)

- **Thread sidebar as a work queue**: Pinned / Active / Snoozed / Settled sections, drag‑to‑pin/settle/wake with server‑persisted order, drafts in the sidebar, undo toasts, archive, unread marks, search, title regeneration (`docs/user/thread-sidebar.md`).
- **Worktrees & branches as first class**: New worktree per thread, “New thread in this worktree”, branch toolbar, base‑branch selection, multi‑model fan‑out starts one thread+worktree per model (`docs/user/source-control.md`, thread-sidebar doc).
- **Turn‑level checkpoints + diffs**: `orchestration.getTurnDiff` / `getFullThreadDiff`, checkpoint capture on each run, rollback to run/checkpoint, restore‑safety checks, files‑viewed state for PR review.
- **Terminals**: server‑owned PTYs (`node-pty`) with ghostty WASM renderer, command‑output tail streaming with cursor/escapes, terminal‑scoped auth (`terminal:read|operate`).
- **Approvals as runtime requests**: unified `runtime-request.updated` (approval decision, free‑text feedback, question answers), subagent approvals surfaced in the parent thread with the subagent's name; permission modes Supervised / Auto‑accept edits / Auto / Full access, admin‑capped by managed policy.
- **Delegation & subagents**: `delegate_task` (fresh child with only `task`, optional worktree isolation), parent woken on completion, `task_status` work states `working|waiting_for_children|result_available`, background‑work liveness, `/fork`, provider switch with context handoff.
- **Scheduled tasks** (cron/webhook driven, run with no active turn), **secret requests** as secure cards, **environment linking** across machines, **previews** (port discovery, HTML renders, browser tabs with per‑tab control/read‑only), **voice input**, **SnapShots** (host screenshots), **prompt suggestions**, command palette action **“Use T3 Code in another app (MCP)”**.
- **Operational niceties**: one‑command `t3 up` (background service prompt), systemd/launchd/docker service management, `t3 doctor`, server self‑update with its own scope, OTEL traces/metrics with rotation, DB reclaim/compaction, `--log-ws-events` debugging.

## 9. Implications for viz (short)

- Copy the **contract‑only boundary**: one `contracts` package owning RPC method strings, schemas, scope enum, and event union; clients regenerate from it (pi‑durable event log can be the `OrchestrationEventStore` analogue, with command receipts for idempotency).
- Copy **snapshot+cursor per‑thread subscriptions** and **route‑list + supervisor** reconnection rather than a single `--host` URL.
- Copy **capability flags on the adapter** instead of per‑harness branches; ACP agents via a registry, native adapters only where the protocol is weaker.
- Copy **MCP as the one tool surface** for both spawned subagents and outside harnesses (deferred `tool_search` listing to keep tool catalogs small).
- Avoid: Electron + cloud relay if viz targets Rune/Swift clients directly; a relay (Clerk + CF Durable Objects) is optional infrastructure you can replace with tailscale‑only reachability.

## Verification notes

- Verified by reading: all paths quoted above (`README.md`, `docs/internals/{overview,remote,t3-connect,adding-a-provider}.md`, `docs/orchestration-v2/README.md`, `docs/user/{remote-access,thread-sidebar,permission-modes,source-control,outside-agents}.md`, `apps/server/package.json`, `apps/web/package.json`, `apps/desktop/package.json`, `apps/server/src/{config.ts,ws.ts,server.ts,bootstrap.ts,cli/{config,server,pair,authScopes}.ts,persistence/{Sqlite,DatabaseStorage,Migrations}.ts,os-jank.ts}`, `apps/server/src/mcp/{McpToolExposure.ts,toolkits/orchestrator/tools.ts,toolkits/thread/tools.ts}`, `packages/contracts/src/{auth,orchestrationV2,rpc}.ts`, `packages/provider-core/src/server/ProviderAdapter.ts`).
- UNVERIFIED: exact WS query‑param constant name/value; `apps/apiserver` purpose; mobile framework; whether `t3_thread_launch` lives in `toolkits/worktree` (name only seen in tool descriptions); concrete SQLite wrapper internals of `@t3tools/shared/nodeSqliteClient`.
- Note: the earlier attempt to read `artifact://24` for `apps/desktop/src` returned an agentclientprotocol.com page instead, so the desktop `src/` subdir contents (`app/`, `backend/`, `electron/`, `ipc/`, `permissions/`, `preview/`, `settings/`, `main.ts`, `preload.ts`, `boot.ts`, `linuxSecretStorage.ts`, `compileCache.ts`) are known only at directory level.
