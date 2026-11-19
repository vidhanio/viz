# 0004: viz as an ADE: projects, runner-owned settings, viz tools over MCP

Date: 2026-11-18. Reference: t3code (`docs/findings/t3code.md`), gap analysis of 2026-11-18.

## Context

viz had one flat list of chats in `$HOME`, auto-answered every ACP permission request from config, showed the orchestrator's model in the header, and gave ACP agents a single `report_progress` tool. Daily software work needs per-repository chats, per-chat model and effort, approvals, diffs, and agents that can delegate further.

## Decision

- **Projects are directories.** A project is `{ name, path }` in a root document (`viz.projects`); a chat runs in its project's path, or in a viz-created git worktree of it. Branch and status come from git at read time; viz stores no git state except per-turn checkpoints.
- **Settings belong to the runner.** ACP chats show the runner's own session config options (model, thought level, mode) instead of a viz model catalog: option ids and values differ per agent and only the agent knows them. viz persists the last options and the user's choices, applies them on every session open, and opens the session when the chat is created so the composer has options before the first message. `[harnesses.<name>].model/effort/mode` and `[orchestrator].model` are defaults for new chats, not fixed settings.
- **Access is viz's option.** `access` (full / ask / read-only) decides ACP permission requests per chat; `ask` holds the request open as an approval until a client answers. Pending approvals live in memory: they die with the ACP process, so persisting them would show requests nobody can answer.
- **viz state beside the view.** pi-durable's conversation view mounts only built-in documents, so viz sends its own per-conversation state (`conversation.state`) whole on every change, and `conversations.changed` tells apps to re-list. Both are cheap and need no delta format.
- **Agents drive viz through MCP.** Every ACP session gets an MCP token scoped to its conversation; tools reach only that conversation's descendants (spawn, list, read, message, cancel subagents). Subagents an agent spawns are ordinary chats with a `parent`, so the user can open and talk to them too.
- **Turn checkpoints are git trees.** Before each ACP prompt in a repository, viz writes the working tree (untracked files included, via a temporary index) as a commit object and records it per prompt. Turn diffs and revert use those; HEAD and the user's index are never touched.

## Consequences

- A chat's options can be stale until its runner reports them again; changing an option on a closed session takes effect when the session opens.
- Approvals do not survive a backend restart; the interrupted turn is reported as failed, as before.
- Revert restores files, not the agent's memory of them: the runner's session still contains the reverted turn.
- Worktrees are created under the state directory and never deleted automatically.
