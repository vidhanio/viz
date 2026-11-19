# Design

What viz is for, what it deliberately leaves out, and the rules every UI follows. Read before adding a feature or a screen.

## Product

viz is one person's agent workspace: talk to the orchestrator, run coding agents on projects, see and ship what they changed. Every feature serves one of these jobs:

1. **Ask.** Message the orchestrator from any client or connector; it remembers, delegates, and reports back.
2. **Run agents on code.** Start a chat in a project (optionally in its own worktree), pick the agent's model, effort and mode, attach files, use its slash commands, stop it, queue the next message.
3. **Follow delegation.** See the subagents a chat started inside that chat, open them, talk to them.
4. **Review and ship.** See what a turn, the worktree or the branch changed; revert a turn; commit, push, open a PR.
5. **Find things.** Search every chat; jump anywhere from the palette.

Out of scope until a job above needs it: permission prompts, in-app settings forms (config is TOML), terminals, a browser, issue trackers, multi-model fan-out, usage dashboards, themes beyond light and dark.

### Before adding something

- Name the job it serves. If none, don't add it.
- Prefer improving an existing surface to adding a new one. One obvious place per action, plus the palette and a shortcut where it is frequent.
- Remove what a change makes redundant in the same change.
- A setting is a last resort: pick a good default instead.

## Interface

- **Layout.** Sidebar (orchestrator, pinned, chats), the chat, and an optional right panel (changes). Overlays: palette, the Agents card, menus, dialogs, toasts. Nothing else.
- **Sidebar.** Two modes: *Recent* (chats newest first, project under each title) and *Projects* (collapsible projects, chats newest first). Subagents never appear in it; they live in their parent chat (timeline cards and the Agents card).
- **Orchestrator.** Visually distinct and never managed like a chat: no pin, archive, rename, move or delete.
- **Copy.** Short, plain, sentence case. No exclamation marks, filler or marketing. Labels name the thing; buttons name the action.

## Design system (desktop)

Everything visual comes from `crates/viz-desktop/src/theme.rs` (tokens) and `crates/viz-desktop/src/ui/` (components). Views compose components; they never set raw sizes, colors or paddings.

- **Tokens:** one semantic palette for both themes, a 4-pt spacing scale, control heights, radii, a type ramp, icon sizes. A new value is a new token, never a literal at a call site.
- **Icons:** [Lucide](https://lucide.dev) only, at token sizes and one stroke width. Never draw custom icons.
- **Flat:** surfaces separated by hairlines and fills, not shadows or glow. Overlays get a hairline border. One accent, used for the primary action, focus and selection.
- **Alignment:** content shares vertical lines: one sidebar inset, one reading column, fixed leading and trailing columns in rows.
- **Verify visually:** render the affected scenes in light and dark (`cargo run -p viz-desktop -- --snap <png> --scene <name>`) and look at them before calling a UI change done.
