# t3code: worktrees

Source: `github.com/pingdotgg/t3code`, branch `main`, read 2026-11-19 via GitHub raw/API (scout). Paths are repo-relative; prefix `https://github.com/pingdotgg/t3code/blob/main/`. Line anchors drift.

# Summary

t3code treats a worktree as **thread-scoped workspace state**, not a separate kind of agent. The default for a new thread is whatever the project/environment `defaultThreadEnvMode` says; the user can pick **Current checkout**, **New worktree**, or a **Previous worktree** in the composer. A worktree gets an auto-generated branch from the **first user message** (never the thread title), a base ref that must be explicitly chosen, and a staged setup card. Subagents are ordinary child threads: **shared parent checkout by default**, with `delegate_task`'s `isolation:"worktree"` opt-in. There is **no automatic Git merge-back for subagent worktrees**. The only feature named “Bring back”/`thread.merge_back` is a **conversation-context handoff for side chats**, not a branch merge.

---

# 1. New-thread worktree UX

## Workspace mode selector

| Element                       | Behavior                                                                                  | Source                                                              |
| ----------------------------- | ----------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| Composer “Workspace” selector | Menu group label **“Workspace”**, keyboard shortcut `composer.workspace`                  | `apps/web/src/components/BranchToolbarEnvModeSelector.tsx`          |
| Local option                  | **“Current checkout”** when no worktree; **“Current worktree”** when a worktree is active | `apps/web/src/components/BranchToolbar.logic.ts:99-115`             |
| New worktree option           | **“New worktree”**                                                                        | `BranchToolbar.logic.ts:99-100`; `BranchToolbar.tsx:434-449`        |
| Previous worktree option      | **“Previous worktree ({branch})”** or **“Previous worktree”**                             | `BranchToolbar.logic.ts:168-173`; `PreviousWorktreeItemContent.tsx` |
| Locked title                  | Worktree folder basename, or **“Local checkout”**, or **“New worktree”**                  | `BranchToolbar.logic.ts:118-127`                                    |
| Multi-model fan-out           | **“Each model starts in its own worktree.”**                                              | `BranchToolbar.tsx:233-234`                                         |

The same selector is rendered in the composer and in the thread-details panel. On an existing thread, `envModeLocked` turns it into a read-only label with the full worktree path as the tooltip (`BranchToolbarEnvModeSelector.tsx:62-105`).

The docs state the entry points:

> “Its branch and workspace mode come from your configured defaults. To continue in an existing worktree, use **New thread in this worktree** from the branch toolbar.”
> — `docs/user/thread-sidebar.md:11-14`

> “With **New worktree**, each background submission creates its own worktree.”
> — `docs/user/thread-sidebar.md:50-55`

## Base branch picker

`apps/web/src/components/WorktreeBaseBranchPicker.tsx:35-150` is the picker. It:

- loads paginated refs via `usePaginatedBranches`;
- renders a `BranchPicker` with search and an **origin toggle** (`startFromOrigin`);
- trigger labels are `Select ref`, `From {branch}`, or `From {branch} (remote)` (`BranchToolbar.logic.ts:266-292`);
- uses `resolveDefaultWorktreeBaseBranch`: first `ref.isDefault`, else current ref (`BranchToolbar.logic.ts:252-264`).

A base is mandatory for a new worktree on the first send:

> “In worktree mode, require an explicit base branch so we don't silently fall back to local execution when branch selection is missing.”
> — `apps/web/src/components/ChatView.logic.ts:1420-1425`

`planComposerTurn` returns `missingWorktreeBaseBranch` and `baseBranchForWorktree` (`ChatView.logic.ts:1386-1445`). Scratch/no-project threads are the exception; they launch at the scratch root.

## Branch naming

Branches are generated from the **first message**, not the title:

- `ThreadLaunchService.ts:571-635` builds a temporary branch when `workspaceStrategy.branch` is undefined.
- After the worktree exists, `ThreadLaunchService.ts:844-870` calls `generateBranchNameFor(worktreeCwd, initialMessage)` and renames the branch via `git.renameBranch`.
- The prompt is `buildBranchNamePrompt` in `packages/provider-core/src/server/textGenerationPrompts.ts:319-351`:
  - “You generate concise git branch names.”
  - “The user message describes work for another agent. Do not answer it, start the work, read files, or run commands; name the branch from the message and its attachments.”
- Modes (`packages/contracts/src/settings.ts:1084-1092,1547-1551`):
  - `static` — default; prefix defaults to `t3`, e.g. `t3/add-search`; prefix sanitized, slash optional.
  - `semantic` — model chooses `feat/`, `fix/`, `refactor/`, etc.
  - `custom` — model returns the complete branch name; “no prefix or suffix is added.”
- Failure behavior:

  > “If generation fails, or a custom name is invalid or already taken, the temporary branch name remains.”
  > — `docs/user/project-settings.md:67-79`

- “Worktree directories keep their original names.” (`project-settings.md:76-77`)

UI wording: **“Choose how new worktree branches are named from your first message.”** — `apps/web/src/components/settings/BranchNamingSettings.tsx`.

## Where worktrees live

- Default: the **`worktrees` folder of the T3 home directory** (`$T3_HOME/worktrees`, typically `~/.t3/worktrees` — the exact home default was not read).
- Per-machine override: **Settings → Worktrees → Worktree location**, an absolute path; resolved by `worktreesDirectory.ts:15-40`.
- Layout comment: “Worktrees can be `<root>/<repository>/<branch>` or explicitly `<root>/<name>`.” — `apps/server/src/storageCleanup.ts:626`.
- Managed-path validation requires exactly **two path segments below a managed root** (`worktreesDirectory.ts:52-75`), so slashed branch names must be flattened/sanitized into one directory segment. Exact flattener was not read.
- Old roots are retained in `previousWorktreesDirectories` for cleanup/history; pages still reference them (`settings.ts:1377-1383`).
- Existing worktrees stay where they are when the setting changes; cleanup covers both roots (`project-settings.md:209-214`).

## Setup scripts and setup card

- Setup work is a project script with `runOnWorktreeCreate: true` (`packages/contracts/src/project.ts:35-45`). Multiple actions can run; `runOnSettle`, `stopOnSettle`, `async` also exist.
- Setup is staged, in order (`packages/contracts/src/worktreeSetup.ts`):
  `fetch`, `checkout`, `submodules`, `setup-script`, `agent`.
- `apps/web/src/components/chat/WorktreeSetupCard.tsx` displays:
  - phase copy **“Creating worktree”**, **“Installing dependencies”**, **“Starting agent”**;
  - base ref;
  - setup script command;
  - elapsed time;
  - failure heading **“Worktree setup failed”** with **Retry**;
  - cancellation **“Worktree setup was cancelled.”**.
- The card can open the setup terminal; it is dismissed after agent handoff.
- `apps/web/src/components/chat/useWorktreeSetup.ts:100-190` blocks sends while setup runs and the agent hasn't started, exposes `retryableRunIds`, `cancel`, `retryPreparation`, and polls `vcsEnvironment.worktreeSetup`.

## What the toolbar shows afterwards

- Branch selector shows the active branch name; in a worktree, the trigger is the branch name, not “New worktree” (`BranchToolbar.logic.ts:266-291`).
- Workspace control shows **“Current worktree”**; tooltip/aria label shows the full path (`BranchToolbar.tsx:208-245`).
- Branch comparison, diff counts, PR badge, terminal actions, and **New thread in this worktree** remain available from the branch toolbar.
- The sidebar card shows a worktree indicator plus the branch (see §4).
- Local-checkout mismatch warning: **“You're currently checked out on another branch.”** (`SidebarThreadRow.tsx:327-332`).

---

# 2. Subagents / delegated tasks

## Default and parameter

`delegate_task` takes `isolation: "shared" | "worktree"`, default **`shared`**:

> `'shared' (default): parent checkout. 'worktree': new prepared checkout from the parent commit, for a separate checkout or PR per child. Uncommitted edits stay out.`
> — `packages/contracts/src/orchestratorMcp.ts:171-184`

Tool description:

> “Start one child agent with only `task`. No parent history. Separate checkout or PR per child: `isolation:'worktree'`. Keep `taskId` for `task_status` and `task_cancel`. No secrets in `task`.”
> — `apps/server/src/mcp/toolkits/orchestrator/tools.ts:89-100`

## Shared (default)

`isolation` omitted means **no `workspaceStrategy` is sent** (`apps/server/src/mcp/OrchestratorMcpService.ts:2808-2856`). The child thread is built by `makeSubagentChildThread`, which spreads the entire parent thread and then overrides identity/title/lineage only:

> `const { delegatedFrom: _delegatedFrom, ...inherited } = input.parentThread; return { ...inherited, ... }`
> — `packages/provider-core/src/server/subagentProjection.ts:53-105`

Therefore the child inherits the parent’s `branch`, `worktreePath`, `workspaceRoot`, project, runtime/interaction modes, and permissions. The launch dispatches the first message with `{ type: "start_immediately" }`, and `cwd` resolves to the parent’s worktree path first, else the project root (`OrchestratorMcpService.ts`, `parentWorkspaceCwd`). This is why the agent can edit the same checkout concurrently.

## Worktree isolation

`isolation: "worktree"` (`OrchestratorMcpService.ts:2808-2840`) does:

1. Resolve parent `HEAD` at `parent.thread.worktreePath ?? project.workspaceRoot`.
2. If the parent is in a worktree, verify the project root contains that commit. Otherwise fail with:
   > `Isolation "worktree" creates the child checkout in project "…" (…), which does not contain the parent checkout's commit. Use isolation "shared" to work in the parent's checkout.`
3. Send `workspaceStrategy: { type: "worktree", baseRef: commitSha, startFromOrigin: false }`.
4. The child starts with `branch: null`, `worktreePath: null` (`Orchestrator.ts:9797-9801`) and a deferred start (`Orchestrator.ts:9918-9924`); normal worktree preparation creates a new checkout from the parent commit.
5. Uncommitted parent edits are explicitly excluded.
6. Retry of an accepted delegation does not create a second worktree (`isRetryOfAcceptedDelegation`).

So: **not always**, not by default. Shared parent workspace is the default; worktree isolation is opt-in per child. This differs from new top-level threads, whose default can be `defaultThreadEnvMode: "worktree"`.

## What the agent sees

- Parent system prompt (`packages/provider-core/src/server/orchestrationInstructions.ts:9-11`):
  > “Use `delegate_task` for another provider's model, user-asked settings, a separate checkout (`isolation:"worktree"`), or `delegate_task` by name.”
  > “Never make a child's checkout in the shell or in `task`.”
- Child system prompt includes:
  > “As a subagent or child, do the work yourself. Start no subagents unless your task says to.”
  > — `orchestrationInstructions.ts:17`
- Child completion is not a Git operation. The event reactor writes a user-role message into the parent beginning **`Completion: …`**, with `metadata.delegationCompletion` and `senderThreadId`; `task_status` returns `working | waiting_for_children | result_available`, and reading marks it delivered (`docs/findings/t3code-subagents.md:76-81`; `tools.ts:105-115`).
- The child is **not given a special “you are in a worktree, branch X” prompt** beyond the inherited `cwd` and thread metadata. Branch/worktree are visible in the parent’s thread data and UI.
- Subagents are hidden from thread lists: `isListedThreadLineage` excludes `relationshipToParent === "subagent"` and side chats (`packages/contracts/src/orchestrationV2.ts:174-178`).

## How results get back

There is **no branch/worktree merge-back action for delegated tasks**.

- `thread.merge_back` exists, but it is **conversation context handoff**:
  - command schema at `packages/contracts/src/orchestrationV2.ts:3557-3563`;
  - context transfer type `"merge_back"` (`orchestrationV2.ts:184`);
  - client `mergeThreadBack` sends `sourcePoint: { type: "run", runId }` (`packages/client-runtime/src/operations/commands.ts:1117-1130`).
- It powers side chats only. **“Bring back”** appears in `apps/web/src/components/chat/SideChatPane.tsx:56-95`:
  - blocked labels: **“This connection cannot change threads.”**, **“Available when the main thread is idle.”**, **“Available after a finished reply.”**
  - failure toast: **“Could not bring back the side chat”**
  - success: **“Brought back”** / “The main thread gets this side chat with your next message there.”
- “Could not merge back” as a literal was **not found**. The equivalent known failure is the side-chat string above.
- For delegated tasks, “merge back” is **the parent reading the completion message and integrating the work itself** — cherry-pick, merge, copy, or open/review a PR. t3code has PR tools and linked-PR state, but no automatic child-worktree → parent merge.
- Worktree cleanup explicitly preserves branches:
  > “Branches and thread history stay; starting another turn recreates the checkout.”
  > — `docs/user/project-settings.md:250`

---

# 3. Lifecycle

## Cleanup trigger and policy

- Policies are **off by default**; cleanup runs **at server startup, when settings change, and hourly** (`project-settings.md:231-233`).
- Settings shape (`packages/contracts/src/settings.ts:1144-1170,1335-1344`):
  - `worktreeCleanup: { mode: "off" } | { mode: "custom", rules } | null`
  - rules: `worktreeKeepWhen`, `worktreeAfterDays`, `worktreeSettledAfterDays`, `worktreeOnMerge`, `worktreeOnDelete`, `worktreeUnchanged`.
- Per-project override: **Inherit / Off / Custom** (`project-settings.md:235-238`).
- Eligibility gates (`apps/server/src/storageCleanup.ts:1120-1500`):
  - thread archived/settled/deleted and policy matches;
  - T3-managed path only;
  - no active session;
  - no open terminal (`waitForStop("open terminal")`);
  - no other thread sharing the canonical path;
  - pending durable thread-deletion effects must finish or be cancelled;
  - worktree still belongs to its recorded branch, unless explicitly reconciled.
- **Archive** makes a thread eligible under settled/archived rules. Archiving also archives side chats and stops their work (`thread-sidebar.md:318-320`).
- **Delete** cleanup is opt-in via **Delete worktrees with deleted threads**. It includes archived threads and previously orphaned worktrees; the server waits for sessions/terminals and retries skipped worktrees after restart (`project-settings.md:255-258`).
- Manual deletion prompt remains when this policy is off. `worktreeCleanup.ts` only offers it when no other thread uses the canonical path; it normalizes trailing slashes and symlinked spellings.

## Dirty worktrees

Default **“Keep worktrees with local changes — Uncommitted changes”**:

- protects tracked edits **and untracked files**;
- deletes ignored files such as `.env` and build output.
- **Any local files** also protects ignored files other than `node_modules`.
- **Edited tracked files** protects only tracked edits; untracked/ignored may be deleted.
  — `project-settings.md:246-250`; settings literals in `settings.ts:1144-1149`.

A dirty worktree is simply **kept**, with a reason in the cleanup report. Orphan folders that no thread claims are reported with sizes but **never removed** (`project-settings.md:260-262`).

## Branch deletion

**None.** `storageCleanup.ts` contains no `deleteLocalBranch`/branch deletion path; only branch reconciliation. The docs explicitly say branches and history stay.

## Reuse of previous worktrees

- Composer: **“Previous worktree ({branch})”** in `PreviousWorktreeItemContent.tsx` / `BranchToolbarEnvModeSelector.tsx`.
- Selection: latest **non-archived** thread, excluding the current worktree, by `updatedAt` (`BranchToolbar.logic.ts:130-176`).
- Toolbar: **“New thread in this worktree”**.
- Server: `workspaceStrategy: { type: "existing_worktree", worktreePath }`; reused record gets a `reusedWorktree: { baseRef }` marker and is **not rewritten**:
  > “Do not rewrite reused worktrees; that could undo the original branch rename.”
  > — `ThreadLaunchService.ts:829-832`
- Picking a branch already checked out in another worktree reuses that path (`resolveBranchSelectionTarget`, `BranchToolbar.logic.ts:330-360`).
- After cleanup removes the checkout, the thread remains; starting another turn recreates it. `apps/server/src/workspace/restoreMissingWorktree.ts` exists for this path (not read line-by-line).

## Cleanup summary

| Event                    | Worktree                                            | Branch | Thread  |
| ------------------------ | --------------------------------------------------- | ------ | ------- |
| Archive                  | eligible per settled/archived policy                | kept   | kept    |
| Delete                   | removed only when delete policy passes safety gates | kept   | deleted |
| Settle, retention 0      | removed next sweep                                  | kept   | kept    |
| Merged PR policy         | removed if merged commits are in remote default     | kept   | kept    |
| Local changes            | kept by default, per `worktreeKeepWhen`             | kept   | kept    |
| Shared by another thread | kept                                                | kept   | kept    |
| Manual delete            | only when no other thread uses path                 | kept   | kept    |

---

# 4. Sidebar / thread display

- Compact/desktop row: branch line with `GitBranchIcon` + `MiddleTruncate value={thread.branch}` (`apps/web/src/components/sidebar/SidebarThreadRow.tsx:321-324`).
- Detailed card meta line: `ThreadWorktreeIndicator` + branch, then PR badge, issue badge, diff `+insertions −deletions`, and terminal status (`SidebarThreadRow.tsx:1266-1305`). The worktree indicator is the only worktree glyph; the full path is not shown in the sidebar row.
- Local checkout on another branch: **“You're currently checked out on another branch.”** (`SidebarThreadRow.tsx:327-332`).
- Thread details/toolbar show full worktree path in the locked workspace control’s tooltip and context menu; branch comparison and PR controls live in the branch toolbar.
- Search covers “title, branch, pull request, or project” (`thread-sidebar.md:227`). Subagents/side chats are excluded from thread lists; side chats are found under **Side chats** in thread details.
- Mobile mirrors this with subagent rows/cards and thread worktree selection (`apps/mobile/src/features/threads/use-worktree-setup.ts`, `worktree-setup-card.tsx`, `use-selected-thread-worktree.ts`).

---

# Recommended policy for viz

## 1. New conversations

- Keep three workspace choices: **Current checkout / New worktree / Previous worktree**.
- Keep `defaultThreadEnvMode` per project; do not silently fall back to local when worktree creation lacks a base. Require an explicit base for Git projects; default to the repo default branch, else current branch.
- Generate the branch from the **first user message**, not the title. Offer static prefix (default `viz/`), semantic, and custom modes; on failure leave the temporary branch and do not block the conversation.
- Store `branch` and `worktreePath` on the conversation. Create at `<state>/worktrees/<project>/<sanitized-branch-or-slug>`; enforce a managed-root check (exactly two segments, no `..`, no symlink escape) as t3 does.
- Add project-level setup actions with `runOnWorktreeCreate`; show a blocking setup card with checkout/submodule/setup/agent stages, cancel/retry, and terminal access. Make setup failures non-destructive: the conversation and branch survive.
- Reuse via an explicit **Previous worktree** / **New thread in this worktree** action; never rewrite a reused worktree’s branch.

## 2. Subagents: do not always use worktrees

**Recommendation: default to the parent checkout, exactly like t3code.**

Reasons:

- t3’s default is `isolation: "shared"`, and viz’s current `spawn_subagent` already creates a child chat in the caller’s cwd. Matching that is the least surprising behavior and avoids worktree/branch sprawl.
- Read-only reviewers, test-runners, and same-change helpers need the parent’s uncommitted edits; a worktree from `HEAD` would hide them.
- Automatic worktree-per-subagent would create branches that nobody merges. t3 explicitly has no merge-back for these.
- Independent/parallel write work and PR-per-child work are the cases that actually need isolation.

Implement the same enum in viz’s MCP tool:

```ts
isolation?: "shared" | "worktree" // default "shared"
```

- `shared`: child uses the caller’s conversation cwd/worktree and inherits branch/worktree metadata.
- `worktree`: resolve the parent `HEAD`, create a fresh checkout from that commit, exclude uncommitted edits, and return the child branch/worktree identity so the parent can hand it off or open a PR.
- If viz wants a stricter default later, make it a project setting (`subagentIsolation`), not a hard-coded always-worktree policy.

## 3. Merge-back: don’t build an automatic Git merge

**Do not implement “subagent merge back” as an automatic merge.** t3 does not have one; its “Bring back” is side-chat context transfer, and `thread.merge_back` is a context handoff command.

For viz:

- **Completion delivery:** child completion posts a durable parent-visible completion event/message containing the result, child branch, worktree path, and links. The parent decides how to integrate.
- **Branch handoff:** expose the child’s branch and worktree path; let the parent cherry-pick, merge manually, or open a PR. If a UI action is desired, make it an explicit **“Merge branch into parent worktree”** operation with preconditions:
  - parent checkout clean (or explicit stash/confirmation);
  - no in-progress merge/rebase/cherry-pick;
  - show conflicts and allow abort;
  - never delete the child branch automatically.
- **Side chats:** if viz copies side chats, use **“Bring back”** only for conversation context, never for Git. Keep the same guard rails: parent idle, finished source run, and errors like “Could not bring back the side chat.”
- Avoid the label “merge back” for Git if you keep it for context—it will confuse users and agents.

## 4. Lifecycle

- Keep worktrees thread-scoped; persist `branch` + `worktreePath`.
- Never delete branches during worktree cleanup. On cleanup, remove only the checkout.
- Default dirty policy: **uncommitted changes keep** touched and untracked files; ignored files may be removed. Offer t3’s three policies.
- Make archive/delete cleanup **opt-in**, run startup/change/hourly, and gate on: no active session, no terminal, no shared canonical path, no pending deletion effects.
- Reuse previous worktrees only on explicit user/agent action; otherwise recreate a deleted checkout on the next turn.
- Sidebar: branch + worktree glyph + diff/PR badges; full path only in tooltip/details.

---

## Gaps / unverified

- Exact default T3 home path (`~/.t3/worktrees` is inferred; code uses `serverConfig.worktreesDir` and docs say “worktrees folder of the T3 home directory”).
- Exact branch→directory flattening for slashed branch names: the managed-path check allows only two segments under the root, but the flattener was not read.
- `WorktreeSetupTracker` stage implementation was not read line-by-line; stage labels and persisted snapshot semantics come from `packages/contracts/src/worktreeSetup.ts`, `WorktreeSetupCard.tsx`, and `useWorktreeSetup.ts`.
- No literal **“Could not merge back”** string was found; the observed equivalent is **“Could not bring back the side chat”** in `SideChatPane.tsx`.
