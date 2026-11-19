# t3code: subagent UI

Source: `github.com/pingdotgg/t3code`, branch `main`, read 2026-11-19 via GitHub raw/API (scout). Paths are repo-relative; prefix `https://github.com/pingdotgg/t3code/blob/main/`. Line anchors drift.

t3code web has **no subagent-only floating widget**. The two surfaces are:

1. **Inline timeline turn items/cards**: every orchestration-v2 `subagent` item renders as a link row, or as a collapsible group card when one provider turn spawned several. Rows show avatar + status dot, title, one-line progress/result preview, elapsed time, and a hover card; clicking opens the child thread.
2. **Top-right “Thread details” popover/inline card** (the “floating panel”): a tabbed card anchored to the chat header, containing a **Lineage** section that groups current vs previous subagent/side-chat threads. It is the closest match, but also contains workspace/git/usage sections, and is not subagent-exclusive.

Child runs are deliberately **excluded from sidebar thread lists**. App-owned (`delegate_task`) children are normal messageable threads; provider-native subagent children are read-only and show a composer-replacing bar.

---

## 1. Subagents in the parent timeline

### Row wiring

`apps/web/src/components/chat/MessagesTimeline.logic.ts`

- `~900-990`: a single subagent gets a `TimelineSubagentRow`; consecutive subagents from a run are grouped.
- `~1490-1520`: `runningSubagentsAtRunTail` counts running subagents at the current run tail.
- `~2100-2290`: groups contiguous `subagent` timeline items sharing the same `runId`, `subagent.parentProviderTurnId`, and `subagent.provider` into a group keyed by `runningCompositionRunId ?? subagentCompositionRunId`.
- `~2390-2415`: if the parent is waiting for its run tail, the end-of-timeline working row gets `waitingOnSubagentsLabel(waitingOn)` → `"Waiting on a subagent"` / `"Waiting on {count} subagents"`.

`apps/web/src/components/chat/MessagesTimeline.tsx`

- `1195-1242`: child threads render a `TimelineSystemDivider` at the top:
  - label `"Subagent of"` (side chat: `"Context from"`)
  - detail is the parent title, or `"{title} on {environment}"`
  - icon `BotIcon` (`MessagesSquareIcon` for side chats)
  - action label `"Open parent thread"`.

### Card component

`apps/web/src/components/chat/V2TimelineEventRow.tsx` — `V2TimelineEventRow()` delegate, `V2SubagentGroupRow`, `SubagentTimelineLink`, `SubagentCardAvatar`, `SubagentLinkDetail`.

Single-subagent row structure:

```tsx
<button className="group/subagent flex w-full min-w-0 items-center gap-2.5 rounded-md px-2 py-1.5 text-left">
  <SubagentCardAvatar subagent={subagent} instanceBadge={instanceBadge} className="size-6" />
  <span className="min-w-0 flex-1">
    <span className="flex min-w-0 items-center gap-1.5">
      <span className="truncate text-xs font-medium text-foreground">{title}</span>
      {environmentLabel && <span className="... text-xs text-muted-foreground">{environmentLabel}</span>}
    </span>
    {detail && <span className="text-2xs leading-relaxed ...">{detail}</span>}
  </span>
  {statusLabel && <span className="text-xs">{statusLabel}</span>}
  <span className="text-xs text-muted-foreground tabular-nums">{trailing}</span>
  {isOpenable && (
    <ChevronRightIcon className="size-3.5 text-muted-foreground/60 group-hover/subagent:text-foreground" />
  )}
</button>
```

- **Avatar is the status carrier**: 24 px rounded avatar with `absolute -right-px size-2 rounded-full ring-2 ring-background` dot; color from subagent status (`bg-info`, `bg-success`, `bg-destructive`, muted for stopped/idle).
- **Provider/instance badge**: `ProviderInstanceIcon` overlay at `-right-1 -bottom-0.5`, shown only when `showInstanceBadge`.
- **Title**: `formatSubagentDisplayTitle()` strips a leading `"Subagent: "` and converts `/root/...` paths to a prettified name (`subagentDisplay.ts`).
- **Detail**: `subagentDetailPreview()` picks `result` when terminal, else `progress`, falling back to the other; strips Markdown-ish syntax; clamps to 280 chars + `…`; `subagentCardDetail()` hides `"Child task ended with status ..."`, strips links/backticks/bullets. Path-looking details use `MiddleTruncate`.
- **Elapsed**: `AgentElapsed` while it can infer a start; otherwise the subagent `createdAt` timestamp via `formatTime`.
- **Status text**: shown only when there is a detail line and (`event` or status !== completed); labels come from `V2LifecycleRow.tsx`:
  - `Queued` (info), `Running` (info), `Waiting` (info), `Idle · resumable` (muted), `Completed` (success), `Failed` (destructive), `Stopped` (muted).

### Hover preview

`apps/web/src/components/chat/SubagentTooltipContent.tsx`, inside `ThreadHoverCard` (`apps/web/src/components/ThreadHoverCard.tsx`, max-w-80, glass tooltip):

- provider icon + model label (`size-3`), qualifier/effort/speed, `subProvider`
- status icon + label with color (`Completed` success, `Failed`/`Cancelled` destructive, `Running`/`Waiting`/`Queued` info, `Stopped` muted)
- elapsed, start time
- workspace rows when child differs from parent: `Project`, `Branch`, `Worktree`, `Workspace`
- preview section with `TerminalIcon` and text `result` for terminal states else `progress`, truncated ~200 chars.

### Group card (2+ subagents in one turn)

Same file, `V2SubagentGroupRow`:

```tsx
<Collapsible open={expanded}>
  <CollapsibleTrigger className="group/subagent flex w-full items-center gap-2.5 rounded-md py-2 pr-3.25 text-left">
    <span className="flex shrink-0 -space-x-1.5">
      {preview.map(avatar size-6)} {overflow > 0 && <span className="... size-6 rounded-full bg-muted text-3xs ring-2 ring-background">+{overflow}</span>}
    </span>
    <span className="truncate text-xs font-semibold">{count} subagents</span>
    <span className="truncate text-3xs ...">{statusSummary}</span>
    <span className="... text-xs text-muted-foreground tabular-nums">{elapsed}</span>
    <ChevronDownIcon className={cn("size-3.5", !expanded && "-rotate-90")} />
  </CollapsibleTrigger>
  <CollapsiblePanel className="mt-1 mb-1 rounded-lg border border-border/60 bg-card/30 p-1">
    …individual SubagentTimelineLink rows…
  </CollapsiblePanel>
</Collapsible>
```

- Label is exactly `{count} subagents` (singular handled elsewhere; “Kicked off {count} subagents” / “Ran {count} subagents” exists in `subagentGroupSummary()` but is not the visible group header in web).
- Summary: `summarizeSubagentStatuses()` → `"{n} working · {n} done · {n} failed · {n} stopped · {n} idle"`, joined with `·`.
- Group is `opacity-55` collapsed; active or expanded `opacity-100 text-foreground`; failed group colors summary `text-destructive`; active summary `text-info-foreground`.
- Expansion is stored in the timeline work-group expansion state, so it survives re-renders.
- The card itself is a `V2EventCard`/`TurnFoldRow` with rounded border, hover background; no border on the inner link row.

### What it does not show

The timeline card does **not** show the child transcript or a prompt block. It shows title, provider/model, progress/result excerpt, elapsed, and status only. The child transcript is a separate view opened by clicking the row.

---

## 2. “Floating” top-right agents panel

### Actual identity

This is **Thread details**, not an agents-only panel:

- `apps/web/src/components/chat/PanelLayoutControls.tsx:40-95` — header toggle:
  - icon `SquareMenuIcon className="size-4"` in a ghost `Toggle`
  - `aria-label={gt("Toggle thread details panel")}`
  - tooltip `gt("Toggle thread details ({shortcut})")`
  - when `threadPanelPresentation === "popover"`, it is a `PopoverTrigger`.
- `apps/web/src/components/ChatView.tsx:3379-3395` — the popover anchor is the entire `<header data-chat-header>`.
- `apps/web/src/components/chat/ThreadDetailsCard.tsx`:
  - popover: `side="bottom"`, `align="end"`, `alignOffset`, `collisionAvoidance`, `variant="panel"`, anchored under the header at the top-right.
  - inline mode: `<aside className="absolute z-20">` positioned with `right: placement.right; top: placement.y; width: placement.width; maxHeight: height` — literally a floating card at the top right of the chat canvas. Height = container height − 52.
  - card chrome: `dropdown-glass isolate contain-paint grid ... overflow-hidden rounded-2xl`; scroll fade; density `full | compact | essential` based on measured height.
- Keyboard: `mod+shift+b` → `threadPanel.toggle` (`packages/shared/src/keybindings.ts:68`, `when: "!terminalFocus"`). This toggles the floating details panel, not a subagent list.

### Content and grouping

`apps/web/src/components/chat/ThreadDetailsPanel.tsx` renders `ThreadRelationshipsPanel` only at `density === "full"` and when `visible.lineage || visible.sideChats`.

`apps/web/src/components/chat/ThreadRelationshipsControl.tsx`:

- Groups (`~637-663`):
  - `related` — parent/related rows, unlabeled, always expanded.
  - `active` — subagent threads not in a finished state, unlabeled, expanded.
  - `previous` — label `gt("Previous agents")`, collapsed by default `expanded: false`.
  - remote children added to `active` / `previous`.
- Finished set: `["completed", "failed", "error", "cancelled", "interrupted", "idle"]`.
- Heading:
  - `titleActive = gt("Lineage") + " · " + gt("{count} running")`
  - `failedCount` → `<SectionHeaderStatus>{failedCount} failed</SectionHeaderStatus>` on the group control.
  - Collapsed group shows row count at the right (`{props.rows.length}`), plus `ChevronDownIcon` (`-rotate-90` when collapsed).
- Pagination: after N initial rows, `"Show {Math.min(hiddenCount, PAGE_COUNT)} more"` with `PlusIcon`.
- Empty state: **none**. If nothing visible, `ThreadRelationshipsPanel` returns `null`; `ThreadDetailsSection` also self-hides (`has-[>div:last-child:empty]:hidden`).
- Row anatomy (`~380-445`, `ThreadDetailsControl` styles in `threadDetailsPanelStyles.ts`):
  - `h-8 min-w-0 rounded-lg ... text-[13px] font-medium text-foreground/80`
  - 16 px relationship icon/avatar (`THREAD_DETAILS_PANEL_ICON_CLASS = "size-4 ..."`), status dot/spinner for active
  - title truncate, right-side status/label (e.g. remote `on {environmentLabel}`)
  - remote/delegated parent rows show `CornerLeftUpIcon` and `on {label}`
  - tooltip on hover.
- **Click behavior**: `openThread()` calls `resolveThreadOpenRoute()` and `navigate()` to the child thread route. For side chats, `resolveThreadOpenRoute` opens the parent route and the side chat as a right-panel tab (`apps/web/src/lib/sideChatPane.ts`). Subagent threads always navigate as full threads.
- Live updates: `ThreadRelationshipsPanel` subscribes to the thread graph/projection atoms (`useQuietThreadShell`, `pendingRequestsAtom`, projection `subagents`). Active/previous grouping and `runningCount` recompute on shell/projection updates; running rows show a spinner/status dot.
- Dismiss/collapse:
  - popover: Base UI `Popover`; closes on `outside-press` or `focus-out` (with a find-bar exception), persisted per-thread in `rightPanelStore.setThreadPanelOpen(..., "popover", open)`.
  - inline: visibility toggled via `rightPanelStore` (`threadPanelOpen`), no outside-click dismissal; it is a docked aside.
  - groups collapse independently.

There is **no count badge on the header button** in web; the running count lives in the Lineage heading, and failed counts appear per group.

---

## 3. Sidebar presence and navigation/messaging

### Hidden from sidebar

`packages/contracts/src/orchestrationV2.ts:155-190`:

```ts
// Subagents and side chats live inside their parent's view, never in thread lists.
export const isListedThreadLineage = (lineage) =>
  lineage.relationshipToParent !== "subagent" && lineage.sideChat !== true;
```

`packages/client-runtime/src/state/threadShell.ts:66-72`:

```ts
export function isShownUnderParent(ref, lineage, delegatedUnderParent) {
  return !isListedThreadLineage(lineage) || delegatedUnderParent.has(threadKey(ref));
}
```

- Subagents and side chats are excluded from navigation/thread list rows.
- A task delegated from an environment that this client lists is also hidden under its parent (`delegatedUnderParentAtom`, `~732-760`); if the parent is unlisted/archived, the task returns to the list so it is reachable.
- An open child’s list selection resolves to its nearest listed ancestor; broken chains resolve to itself.

### Navigation to/from a child

- Parent timeline row/group → click opens child route.
- Parent header details → Lineage row → `openThread()`.
- Child timeline header divider:
  - `"Subagent of"` + parent title, with action `"Open parent thread"`.
- Child header breadcrumb: `Project / {parent title} / {current title}`; parent title is clickable (`ChatHeader.tsx:298-330`, `onOpenThread(parentThreadLink.threadId)`).
- Back/forward: `mod+[` / `mod+]` (`navigation.back` / `navigation.forward`).
- Parent side: children are not in the sidebar, only in Lineage and timeline items.

### Can the user message a child?

Two child classes:

1. **App-owned child** (T3 `delegate_task`/orchestration child): a normal server thread. It has a composer and can receive messages. The header shows a clickable parent breadcrumb; side chats are disallowed: `gt("Subagent threads cannot start side chats.")` (`ChatView.tsx:2846-2876`).
2. **Provider-native subagent** (Claude/Codex-style native child): `isProviderNativeSubagentThread(serverThread.source)`. At `ChatView.tsx:1778-1781`:

```ts
// Provider-native subagents cannot take messages; approvals and questions go to the top-level parent.
const showProviderSubagentBar = isProviderSubagent;
const composerMounted = !showProviderSubagentBar;
```

The composer is replaced by `ProviderSubagentBar` (`apps/web/src/components/chat/ProviderSubagentBar.tsx`):

- provider icon, model name, effort/speed, live status
- right-side text `"Runs on its own"`
- button `"Open parent"` with `ArrowUpLeftIcon`
- screen-reader status: `"{model} subagent: {status}"`.

`NativeChildWorkDialog.tsx` states: `"Read only. These subagents cannot need messages."` / `"Read only. These subagents cannot take messages."` (shown in a dialog footer; transcript rows read-only). It renders a provider-native child’s `threadId` work log without a composer.

---

## 4. Shortcuts, notifications, approvals

### Shortcuts

- `mod+shift+b` — toggle thread details panel (the only subagent-related binding).
- `mod+[` / `mod+]` — back/forward when navigating child → parent.
- No dedicated “open subagent”, “next subagent”, or approval shortcut.

### Notifications / attention

`apps/web/src/components/chat/ThreadActivityBanners.tsx:52-101` — `childInputNowItem`:

- only when a child needs input and the parent is not itself awaiting the user
- icon: `ShieldQuestionIcon` if pending approval, else `MessageCircleIcon`, both `text-info-foreground`
- title via `childThreadInputsTitle()`:
  - one: `"Subagent needs input"` / `"Subagent needs approval"` / `"Subagent needs you"` (side chat variants)
  - many: `"{count} subagents need input/approval/you"` (or `threads`, or `side chats`)
- description: inline buttons naming each child; clicking opens it
- action: `"Open approval"` or `"Open question"`
- rendered at end of parent timeline (`ThreadNow`), included in scroll-to-end summary (`"Subagent waiting"` / `"Side chat waiting"`).
- `ThreadActivityAnnouncer` handles screen-reader announcements for approvals, waiting, status; no toast for subagent completion.

Also end-of-run row label: `"Waiting on a subagent"` / `"Waiting on {count} subagents"`.

### Approvals

`apps/web/src/components/chat/ChildThreadApprovals.tsx` — rendered above the parent composer in `ChatView`’s composer overlay:

- accumulator `childThreadApprovalsAtom` (`apps/web/src/state/threads.ts:45-48`) → `createChildThreadApprovalsAtom` (`packages/client-runtime/src/state/threadRequests.ts:373-414`) flattens approvals only from children with `hasPendingApprovals`.
- card:
  - header: `"Approval needed from"` + `entry.title` (tiny 11px label, truncating title)
  - body: `approval.reason` or JSON-stringified `approval.request` (`<pre>` for JSON, line-clamped, `text-foreground/75`)
  - actions: `ComposerPendingApprovalActions`
  - error state: `role="alert"`, `"Couldn't respond to this approval."`, error text, `"Retry"`.
- `ComposerPendingApprovalActions` default options: `Cancel`, `Decline`, `Always allow this session`, `Approve`; `Decline`/`Approve` are primary buttons, the rest in an ellipsis “More approval options” menu. Warning options get a `TriangleAlertIcon` and tooltip.
- In the child thread itself, `TimelinePendingApprovalRow` and the composer hold the same approval actions; nested provider-native approvals are surfaced on the **top-level parent** instead.
- `ThreadRelationshipsPanel` also supports stopping subagents/workflows:
  - `"Could not stop subagent"`, `"Could not stop workflow"` error toasts.
  - `"Could not merge back"`, `"Could not disconnect agent session"` for workflow/delegation actions.
- Provider-native read-only children: the only action is `"Open parent"`; approvals go to the parent.

---

## Visual reference table

| Element                 | t3code web                                                                                                        |
| ----------------------- | ----------------------------------------------------------------------------------------------------------------- |
| Subagent avatar         | 24 px circle, muted bg, `ring-2 ring-background`; status dot 8 px at right; provider badge overlaid bottom-right  |
| Card row                | `rounded-md px-2 py-1.5`, gap 10 px, `text-xs font-medium` title, `text-2xs` muted detail                         |
| Chevron                 | `size-3.5`, right, only if openable                                                                               |
| Group card              | avatar stack `-space-x-1.5`, `+N` badge 24 px, `text-xs font-semibold` title, `text-3xs` summary, chevron rotates |
| Group body              | `rounded-lg border border-border/60 bg-card/30 p-1`, 4 px row spacing                                             |
| Details panel           | glass, `rounded-2xl`, scroll fade, 13 px rows, 32 px height, 16 px icons, 2.5 gap, soft 3.5%/8% hover fill        |
| Details width/placement | inline `width` from canvas preferred placement, `right`/`top` measured; popover anchored to header, align end     |
| Panel heading           | `text-2xs font-medium text-muted-foreground`, 32 px min height, section divider `border-border/65`                |
| Status colors           | running/queued/waiting info; completed success; failed destructive; stopped/idle muted                            |

---

## Recommended adaptation for viz (flat native desktop)

**Two surfaces, same data:**

1. **Timeline card** — 56–64 px row: 24 px rounded status avatar (provider initial or icon), title 13 px semibold, one-line muted preview (progress while live, result when done), right-aligned elapsed, 8 px status dot (running includes subtle pulse/spinner), chevron. Click → open child thread in the main chat view. Avoid glass/translucency; use `surface`/`border` tokens, hover fill only.
2. **Top-right panel** — pin it to the chat content’s top-right, under the header, 300–340 px wide. Button: `SquareMenuIcon` 16 px, tooltip `Thread details (⌘⇧B)`. Panel header `Agents` + `{n} running`; sections:
   - **Running** (expanded): rows with spinner/status dot, title, model, elapsed.
   - **Finished** (collapsed by default, count on right, “Show N more”): rows with terminal status icon/color, duration, one-line result preview.
   - Never list child runs in the sidebar. Keep the parent row only.

**Rows:** 32 px height, 16 px avatar/icon, 13 px medium title, 11 px muted status right-aligned; click navigates to the full child view, not an embedded transcript. Sidebar remains flat — no nested tree.

**Keep both classes of child:**

- `editable: true` → normal composer in the child view; back breadcrumb `Parent / Child`.
- `readonly/providerNative: true` → replace composer with a flat bar: `Runs on its own` + `Open parent` button; no message input. Show a read-only work-log view.
- In both cases add a divider at the top of the child timeline: `Subagent of {parent}`, action `Open parent thread`.

**Approvals:** aggregate pending child approvals into cards above the parent composer (`Approval needed from {title}` + Approve/Decline/More). Add a subtle attention dot to the Agents button; in the child show the same approval card. End-of-timeline status `Subagent waiting` with an `Open approval` action is enough — no system toasts.

**Live updates:** derive rows from child-thread records (`parentId`, `status`, `startedAt`, `finishedAt`, `progress`, `result`, `model`, `provider`, `readonly`); recompute grouping on status change; tick elapsed only while active. Keep running count on the panel header, failed count per section, no global badge.
