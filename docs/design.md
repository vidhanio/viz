# Design

What viz is for, what it deliberately leaves out, and the rules every UI follows. Read before adding a feature or a screen.

## Product

viz is one person's agent workspace: talk to the orchestrator and run coding agents on projects. Every feature serves one of these jobs:

1. **Ask.** Message the orchestrator from any client or connector; it remembers, delegates, and reports back.
2. **Run agents on code.** Start a chat in a project folder with a harness, pick the agent's model, effort and mode, attach files, use its slash commands, stop it, queue the next message.
3. **Follow delegation.** See the subagents a chat started inside that chat, open them, talk to them.
4. **Find things.** Search every chat; jump anywhere from the palette.

Out of scope until a job above needs it: git (diffs, checkpoints, worktrees, commit/push/PR; the agents do git themselves), permission prompts, in-app settings forms (config is TOML), terminals, a browser, issue trackers, multi-model fan-out, usage dashboards, a theme editor.

### Before adding something

- Name the job it serves. If none, don't add it.
- Prefer improving an existing surface to adding a new one. One obvious place per action, plus the palette and a shortcut where it is frequent.
- Remove what a change makes redundant in the same change.
- A setting is a last resort: pick a good default instead.

## Interface

- **Layout.** Sidebar (orchestrator, pinned, chats) and the chat. A chat with subagents keeps their panel at the top right. Menus, palette, dialogs and toasts are temporary overlays.
- **Sidebar.** Two modes: _Recent_ (chats newest first, project under each title) and _Projects_ (collapsible projects, chats newest first). Subagents live in their parent's panel and timeline, not the sidebar.
- **Activity.** Group reasoning and consecutive tools behind a plain-language summary. While a command runs, show it without making the user open the group. Expanded rows show readable command output and changes, not RPC payloads.
- **Orchestrator.** Visually distinct and never managed like a chat: no pin, archive, rename, move or delete.
- **Copy.** Short, plain, sentence case. No exclamation marks, filler or marketing. Labels name the thing; buttons name the action. Use display names for models, providers and thinking levels, not their wire IDs.

## Design system (desktop)

Everything visual comes from `crates/viz-desktop/src/theme.rs` (tokens) and `crates/viz-desktop/src/ui/` (components). Views compose components; they never set raw sizes, colors or paddings.

- **Tokens:** one semantic palette derived from a base16 scheme per appearance (`desktop.toml` `[theme]`; any scheme must read well), a 4-pt spacing scale, control heights, radii, a type ramp, icon sizes. A new value is a new token, never a literal at a call site.
- **Icons:** [Lucide](https://lucide.dev) only, at token sizes and one stroke width. Never draw custom icons.
- **Flat:** surfaces separated by hairlines and fills, never glow. Overlays (menus, popovers, palette, dialogs, toasts) are borderless, one fill step above what they float over, with a soft layered shadow; nothing else gets a shadow. One accent, used for the primary action, focus and selection.
- **Alignment:** content shares vertical lines: one sidebar inset, one reading column, fixed leading and trailing columns in rows.
- **Verify visually:** render the affected scenes in light and dark (`cargo run -p viz-desktop -- --snap <png> --scene <name>`) and look at them before calling a UI change done.
