//! The toolbar: the open conversation's title (a subagent's under its
//! parent), its run state, and the Agents card.

use rune::{El, col, icon, row, text};
use rune_kit::{IconButtonVariant, Placement, Popover, shell::Toolbar};
use viz_client::protocol::{ConversationId, ConversationStatus, ConversationSummary};

use crate::{
    app::{Desktop, chat_title},
    sidebar,
    theme::{
        self, CARD_WIDTH, HAIRLINE, ICON, ICON_SMALL, READING_INSET, S2, S4, S8, S12, S32, Size,
        TOOLBAR_PAD, Theme,
    },
    ui,
};

/// The widest the parent's title gets in a subagent's breadcrumb.
const BREADCRUMB_MAX: f32 = 272.0;
/// The Agents card's `open_picker` value.
const AGENTS: &str = "agents";
/// The Agents card's "Previous agents" fold, in `Desktop::expanded`.
const PREVIOUS: &str = "agents-previous";
const AGENTS_WIDTH: f32 = CARD_WIDTH;

pub fn toolbar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Toolbar {
    // First in the band, under the title and buttons.
    let mut toolbar = Toolbar::new().center(ui::drag_guard("toolbar-drag-guard").pin(
        Some(0.0),
        Some(0.0),
        Some(0.0),
        Some(0.0),
    ));
    let open = s.sidebar.is_open();
    if !open {
        toolbar = toolbar.leading(sidebar::new_chat_button(s, t));
    }
    let title = title(s, t);
    // With the sidebar open the title starts at the reading inset; closed, it
    // follows the toggle and new-chat buttons at the toolbar's gap.
    let title = if open {
        title.ml(READING_INSET - TOOLBAR_PAD)
    } else {
        title
    };
    let mut toolbar = toolbar.leading(title);
    // The orchestrator owns no agents: no Agents card.
    if !s.orchestrator_open()
        && let Some(agents) = agents(s, t, layer)
    {
        toolbar = toolbar.trailing(agents);
    }
    toolbar.trailing(metadata(s, t).mr(READING_INSET - TOOLBAR_PAD))
}

/// Opens or closes the Agents card, when the open chat has subagents.
pub fn toggle_agents(s: &mut Desktop) {
    s.open_picker = if s.open_picker.as_deref() == Some(AGENTS) {
        None
    } else {
        s.selected
            .filter(|id| {
                !s.orchestrator_open() && !sidebar::subagents(&s.conversations, *id).is_empty()
            })
            .map(|_| AGENTS.to_owned())
    };
}

/// The Agents toggle and, open, its card under the toolbar's end: the open
/// chat's subagents, running ones first, finished ones folded under
/// "Previous agents".
fn agents(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    let parent = s.selected?;
    let children = sidebar::subagents(&s.conversations, parent);
    if children.is_empty() {
        return None;
    }
    let open = s.open_picker.as_deref() == Some(AGENTS);
    let toggle = ui::icon_button(
        &ui::BOT,
        "agents",
        IconButtonVariant::GhostMuted,
        Size::Default,
    )
    .shortcut(ui::AGENTS_SHORTCUT)
    .on_press(toggle_agents)
    .build(&t.k)
    .key("agents-toggle");
    let (running, previous): (Vec<_>, Vec<_>) = children
        .into_iter()
        .partition(|chat| chat.status == ConversationStatus::Working);
    let mut card = col()
        .key("agents-card")
        .w(AGENTS_WIDTH)
        .pb(S4)
        .child(
            row()
                .key("agents-heading")
                .w_full()
                .h(S32)
                .px(S12)
                .gap(S4)
                .center_y()
                .child(ui::section("agents", t))
                .children((!running.is_empty()).then(|| {
                    text(format!("· {} running", running.len()))
                        .font(t.small)
                        .color(t.faint)
                })),
        )
        .child(ui::rule(t).mb(S4))
        .children(running.iter().map(|chat| agent_row(chat, t)));
    if !previous.is_empty() {
        let shown = s.expanded.contains(PREVIOUS);
        card = card
            .child(
                row()
                    .key(PREVIOUS)
                    .role("button")
                    .label("previous agents")
                    .expanded(shown)
                    .h(S32)
                    .mx(S4)
                    .px(S8)
                    .gap(S8)
                    .center_y()
                    .radius(Size::Default.radius())
                    .hover(t.hover)
                    .focusable(true)
                    .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                    .on_press(|s: &mut Desktop| s.toggle(PREVIOUS))
                    .child(ui::lead(
                        icon(
                            if shown {
                                &ui::CHEVRON_OPEN
                            } else {
                                &ui::CHEVRON
                            },
                            ICON_SMALL,
                        )
                        .color(t.faint),
                        S32,
                    ))
                    .child(ui::section("previous agents", t).grow(1.0))
                    .child(
                        text(previous.len().to_string())
                            .font(t.mono_small)
                            .color(t.faint),
                    ),
            )
            .children(
                previous
                    .iter()
                    .filter(|_| shown)
                    .map(|chat| agent_row(chat, t)),
            );
    }
    let toggle = Popover::new("agents")
        .trigger(toggle)
        .label("agents")
        .padding(0.0)
        .placement(Placement::BottomEnd)
        .gutter(S8)
        .open(open)
        .on_close(|s: &mut Desktop| s.open_picker = None)
        .child(card)
        .build(&t.overlay, layer);
    Some(toggle)
}

/// One subagent in the Agents card: its state mark, title and how long ago
/// it last moved (the mark already says whether it runs).
fn agent_row(chat: &ConversationSummary, t: &Theme) -> El {
    let id = chat.id;
    let title = chat_title(chat);
    let (mark, state) = ui::agent_state(chat.status, t);
    let when = theme::ago(chat.updated_at).unwrap_or_else(|| state.to_owned());
    row()
        .key(format!("agent-{id}"))
        .role("link")
        .label(title)
        .description(state)
        .h(S32)
        .mx(S4)
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .hover(t.hover)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.open_picker = None;
            s.select(id, tx);
        })
        .child(ui::lead(mark, S32))
        .child(
            text(title)
                .font(t.ui)
                .color(t.text)
                .truncate()
                .min_w(0.0)
                .shrink()
                .grow(1.0)
                .a11y_hidden(),
        )
        .child(text(when).font(t.small).color(t.faint).a11y_hidden())
}

fn title_text(label: &str, t: &Theme) -> El {
    text(label)
        .key("title")
        .font(t.title)
        .color(t.text)
        .min_w(0.0)
        .truncate()
        .shrink()
        .help_when_truncated(label)
}

/// The orchestrator's mark and name, a subagent's parent and name, or the
/// chat's title.
fn title(s: &Desktop, t: &Theme) -> El {
    let Some(id) = s.selected else {
        let label = if s.new_chat.is_some() {
            "new chat"
        } else {
            "viz"
        };
        return title_text(label, t);
    };
    let group = row()
        .key("header-title")
        .min_w(0.0)
        .shrink()
        .gap(S8)
        .center_y();
    if s.root == Some(id) {
        return group
            .child(icon(&ui::STAR, ICON).color(t.accent).a11y_hidden())
            .child(title_text("orchestrator", t));
    }
    let Some(parent) = s
        .summary(id)
        .and_then(|chat| chat.parent)
        .filter(|parent| *parent != id && s.summary(*parent).is_some())
    else {
        return title_text(s.title(id), t);
    };
    let slash = text("/").font(t.title).color(t.faint).a11y_hidden();
    group
        .gap(S4)
        .child(breadcrumb(s, parent, t))
        .child(slash)
        .child(title_text(s.title(id), t))
}

/// A subagent's top-level chat, as a link back.
fn breadcrumb(s: &Desktop, parent: ConversationId, t: &Theme) -> El {
    let label = s.title(parent);
    row()
        .key("header-parent")
        .role("link")
        .label(format!("back to {label}"))
        .min_w(0.0)
        .max_w(BREADCRUMB_MAX)
        .shrink()
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, S2, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx| s.select(parent, tx))
        .child(
            text(label)
                .font(t.title)
                .color(t.muted)
                .group_color(t.text)
                .min_w(0.0)
                .truncate()
                .shrink()
                .help(label)
                .a11y_hidden(),
        )
}

/// The run state, right-aligned; the sidebar already names the project and
/// harness.
fn metadata(s: &Desktop, t: &Theme) -> El {
    let mut metadata = row().key("header-metadata").center_y();
    if let Some(mark) = s
        .conversation
        .as_ref()
        .and_then(|state| ui::status(state.status, t))
    {
        metadata = metadata.child(mark.key("header-status"));
    }
    metadata
}
