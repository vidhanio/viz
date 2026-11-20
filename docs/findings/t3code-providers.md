# How t3code integrates Claude and Codex (2026-11-20)

Source: `github.com/pingdotgg/t3code` `main` @ `1947264`, read via GitHub raw/API. Paths are repo-relative.

## Mechanism

| Agent           | Transport                                                                                                                                 | Where                                                                                                                                             |
| --------------- | ----------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Codex           | spawns `codex app-server -c thread_unload_delay_secs=0` and speaks its JSON-RPC over stdio through a generated typed client               | `apps/server/src/provider/codexLaunchArgs.ts`, `packages/effect-codex-app-server/`, `apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts` |
| Claude          | `@anthropic-ai/claude-agent-sdk` in-process (`query`, `forkSession`, `getSessionMessages`, `getSubagentMessages`); the SDK spawns the CLI | `apps/server/src/orchestration-v2/Adapters/ClaudeAdapterV2.ts`, `apps/server/src/claudeModelOptions.ts`                                           |
| Everything else | generic ACP adapter (local command or the ACP Registry)                                                                                   | `packages/provider-acp/`, `packages/effect-acp/`, `packages/provider-acp-registry/`                                                               |

`claude-agent-acp` / `codex-acp` only run through the generic ACP adapter when a user adds them as local ACP commands; the web maps their registry ids to the Claude/OpenAI icons for branding only (`packages/client-runtime/src/providerClients.ts`).

Their stated policy (`docs/internals/adding-a-provider.md`): an agent gets a dedicated driver only when it needs behavior the ACP spec does not cover.

## What native buys them (each absent or weaker over ACP)

- **Rollback and fork.** Codex `thread/rollback`, `thread/fork`; Claude `forkSession` from a message. `docs/user/providers-acp.md`: "ACP agents cannot rewind their own conversation" (t3 starts a fresh session and replays history instead).
- **Mid-turn steering.** Codex `turn/steer` (and during compaction). ACP has only `session/cancel` + a new prompt.
- **Subagent lineage.** Claude `getSubagentMessages` and tailing Claude's private task output files (`ClaudeTaskOutputTail.ts`); Codex subagent threads and approvals from subagents. "ACP has no portable native subagent-lineage contract."
- **Account, usage, rate limits.** Codex `account/read`, `account/rateLimits/read` + `updated` notifications, `thread/tokenUsage/updated`; Claude SDK `account` (email, subscription, token source) and `SDKRateLimitInfo`. ACP reports none of this portably.
- **Models and per-turn options.** Codex `model/list`, `model/rerouted`, service tier via `thread/settings/update`; Claude per-turn model/effort/fast/thinking compiled from t3's own catalog. Over ACP, options exist only if the adapter reports them as session config options.
- **Richer interaction.** Claude `CanUseTool`/`PermissionUpdate` (persisted allow rules), structured `AskUserQuestion`; Codex `apply_patch` approval, native `/goal` (`thread/goal/*`), background terminals (`thread/backgroundTerminals/*`), compaction (`thread/compact/start`), opting out of `turn/diff`.
- **Isolation and health.** Per-instance `CODEX_HOME` (`provider/Drivers/CodexHomeLayout.ts`), managed Codex installs (`provider/CodexInstallation.ts`), `codex login --device-auth`. Health probes run `<cli> --version` with a timeout, then a protocol handshake for auth/models (`packages/provider-core/src/server/snapshotProbe.ts`, `provider/ClaudeProvider.ts`, `provider/CodexProvider.ts`).

The cost: `CodexAdapterV2.ts` alone is ~388 KB, plus a generated protocol client per agent and tuning of agent internals (unload delays, private task files) that can break on any CLI release.

## Relevance to viz

viz runs both through Zed's adapters (`claude-agent-acp` 0.108.0, `codex-acp` 2.7.0), which already expose what viz's UI uses: session config options (Claude: mode, model, fast, agent, effort; Codex: mode, collaboration_mode, model, fast-mode, reasoning_effort), slash commands, image prompts, tool calls with diffs, plans, and `session/load`/`resume`. Verified by running a prompt through each against a fresh backend. Going native would only pay off for rollback/fork, mid-turn steering, subagent transcripts, or usage/rate-limit display.

Icons: t3 draws an OpenAI knot for Codex and the Claude starburst (`apps/web/src/components/Icons.tsx`, mapped in `apps/web/src/components/chat/ProviderInstanceIcon.tsx`). viz uses the monochrome marks from `@lobehub/icons-static-svg` (Claude, Codex), the pi favicon from `pi.dev`, and oh-my-pi's `assets/icon.svg` (`github.com/can1357/oh-my-pi`), all recoloured to `currentColor`.
