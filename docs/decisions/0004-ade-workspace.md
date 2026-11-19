# 0004: viz as an ADE: projects, runner-owned settings, viz tools over MCP

Date: 2026-11-18. Reference: t3code (`docs/findings/t3code.md`), gap analysis of 2026-11-18.

## Context

viz had one flat list of chats in `$HOME`, auto-answered every ACP permission request from config, showed the orchestrator's model in the header, and gave ACP agents a single `report_progress` tool. Daily software work needs per-repository chats, per-chat model and effort, diffs, and agents that can delegate further.

## Decision

- **Projects are directories.** A project in `viz.projects` has a name, path, and `defaultWorkspace` (`local` by default). A chat can use the project checkout, start a new worktree, or reuse a viz-managed one. A new worktree starts from an explicit ref, `origin/HEAD`, or the current branch, in that order. Its branch starts with a temporary name and is renamed from the first message if the model returns a usable name. Reused worktrees keep their branch. Branch and status come from Git at read time.
- **Settings belong to the runner.** ACP chats show the runner's own session config options (model, thought level, mode) instead of a viz model catalog: option ids and values differ per agent and only the agent knows them. viz persists the last options and the user's choices, applies them on every session open, and opens the session when the chat is created so the composer has options before the first message. `[harnesses.<name>].model/effort/mode` and `[orchestrator].model` are defaults for new chats, not fixed settings.
- **viz allows every permission request.** ACP agents run with full access in the user's own projects, so approvals add friction without protecting anything: viz answers each request with the first `allow_once` option, else `allow_always`, and cancels a request without either. There is no access setting and no per-harness permission config. (A per-chat access option with pending approvals was built first and removed as unneeded.)
- **viz state beside the view.** pi-durable's conversation view mounts only built-in documents, so viz sends its own per-conversation state (`conversation.state`) whole on every change, and `conversations.changed` tells apps to re-list. Both are cheap and need no delta format.
- **Agents drive viz through MCP.** Every ACP session gets an MCP token scoped to its conversation; tools reach only that conversation's descendants (spawn, list, read, message, cancel subagents). Subagents are chats with a `parent` and a title derived from their task. The parent transcript links MCP calls to the affected child when the runner forwards the tool result.
- **Subagents share the caller's checkout by default.** Reviewers and helpers can see uncommitted edits without creating branches nobody needs to merge. For independent or parallel edits, `spawn_subagent` accepts `isolation: "worktree"` and starts a new branch at the caller's `HEAD`, without those edits. The tool returns the branch and path; the parent integrates the result. viz never merges automatically.
- **Orchestrator delegation is detached.** `spawn_chat` creates a visible, top-level ACP chat in the selected project (or without a project), with the same workspace choices as app chats. Its result reaches the orchestrator as an input; `message_conversation` queues follow-ups, and `cancel_conversation` stops the chat. The orchestrator addresses only top-level chats; search groups descendant matches under them. Legacy task-owned children remain nested and usable through the app.
- **Deletion hides the conversation subtree.** Delete aborts turns, withdraws queued inputs, closes runners, and stores a `viz.meta` tombstone for the chat and its descendants. The orchestrator cannot be renamed, pinned, archived, moved, or deleted.
- **Turn checkpoints are git trees.** Before each ACP prompt in a repository, viz writes the working tree (untracked files included, via a temporary index) as a commit object and records it per prompt. Turn diffs and revert use those; HEAD and the user's index are never touched.

## Consequences

- A chat's options can be stale until its runner reports them again; changing an option on a closed session takes effect when the session opens.
- Nothing stops an agent from changing or deleting files in its cwd or elsewhere under the user's account; checkpoints and revert are the safety net.
- Revert restores files, not the agent's memory of them: the runner's session still contains the reverted turn.
- Deleting a chat removes clean viz-created worktrees no live chat uses. Dirty worktrees are kept and their paths returned; branches are never removed.
- pi-durable has no row deletion for conversations. Deleted transcripts stay in `viz.sqlite`, but viz excludes them from its APIs and drops their derived search entries. Saved attachments are retained.
