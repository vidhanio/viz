//! Conversation toolbar and the persistent subagent panel.

use rune::{El, ScrollSpec, Transition, col, el, icon, row, text};
use rune_kit::{ProgressCircle, shell::Toolbar};
use viz_client::protocol::{ConversationId, ConversationStatus, ConversationSummary};

use crate::{
    app::{Desktop, chat_title},
    sidebar,
    theme::{
        CARD_WIDTH, HAIRLINE, ICON, ICON_SMALL, RADIUS_CARD, READING_INSET, S2, S4, S8, S12, S16,
        S32, S40, Size, TOOLBAR_PAD, Theme,
    },
    ui,
};

/// The widest the parent's title gets in a subagent's breadcrumb.
const BREADCRUMB_MAX: f32 = 272.0;
/// Leave at least this much for the chat before putting the panel beside it.
const MIN_CHAT_WIDTH: f32 = 560.0;
const PANEL_GAP: f32 = S16;
const PREVIOUS: &str = "agents-previous";
const COMPACT_ROWS: f32 = 2.0;
const WIDE_ROWS: f32 = 6.0;

pub fn toolbar(s: &Desktop, t: &Theme) -> Toolbar {
    // First in the band, under the title and buttons.
    let mut toolbar = Toolbar::new().center(ui::drag_guard("toolbar-drag-guard").pin(
        Some(0.0),
        Some(0.0),
        Some(0.0),
        Some(0.0),
    ));
    let title = if s.sidebar.is_open() {
        title(s, t).ml(READING_INSET - TOOLBAR_PAD)
    } else {
        toolbar = toolbar.leading(sidebar::new_chat_button(s, t));
        title(s, t)
    };
    toolbar
        .leading(title)
        .trailing(metadata(s, t).mr(READING_INSET - TOOLBAR_PAD))
}

/// The panel and its horizontal reserve. In small windows it gets its own
/// band above the transcript instead of covering the latest message.
pub fn agents(s: &Desktop, t: &Theme) -> (Option<El>, f32) {
    let Some(parent) = s.selected else {
        return (None, 0.0);
    };
    let children = sidebar::subagents(&s.conversations, parent);
    if children.is_empty() {
        return (None, 0.0);
    }
    let wide = s.window_width - s.sidebar.shown_width() >= MIN_CHAT_WIDTH + CARD_WIDTH + PANEL_GAP;
    let (running, previous): (Vec<_>, Vec<_>) = children
        .into_iter()
        .partition(|chat| chat.status == ConversationStatus::Working);
    let mut header = row()
        .h(S32)
        .px(S12)
        .gap(S8)
        .center_y()
        .child(icon(&ui::AGENT, ICON_SMALL).color(t.muted).a11y_hidden())
        .child(ui::section("Subagents", t).grow(1.0));
    if !running.is_empty() {
        header = header.child(
            text(format!("{} running", running.len()))
                .font(t.small)
                .color(t.muted),
        );
    }
    let mut list = col()
        .key("subagent-list")
        .w_full()
        .px(S4)
        .pb(S4)
        .max_h(if wide { WIDE_ROWS } else { COMPACT_ROWS } * S40)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .children(running.into_iter().map(|chat| agent_row(chat, t)));
    if previous.len() == 1 {
        list = list.child(agent_row(previous[0], t));
    } else if !previous.is_empty() {
        let shown = s.expanded.contains(PREVIOUS);
        list = list.child(previous_toggle(previous.len(), shown, t));
        if shown {
            list = list.children(previous.into_iter().map(|chat| agent_row(chat, t)));
        }
    }
    let k = &t.overlay;
    let panel = k.shadow_layers(&k.elevation.floating).fold(
        col()
            .key(format!("agents-card-{parent}"))
            .role("group")
            .label("Subagents")
            .w(CARD_WIDTH)
            .radius(RADIUS_CARD)
            .fill(k.c.layer_01)
            .transition(Transition::opacity())
            .child(header)
            .child(list),
        El::box_shadow,
    );
    if wide {
        (
            Some(panel.pin(None, Some(S12), Some(READING_INSET), None)),
            CARD_WIDTH + PANEL_GAP,
        )
    } else {
        let band = row()
            .key("agents-band")
            .w_full()
            .pt(S8)
            .pr(READING_INSET)
            .child(el().grow(1.0).a11y_hidden())
            .child(panel);
        (Some(band), 0.0)
    }
}

fn previous_toggle(count: usize, shown: bool, t: &Theme) -> El {
    row()
        .key(PREVIOUS)
        .role("button")
        .label("Previous subagents")
        .expanded(shown)
        .h(S32)
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .hover(t.hover)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press(|s: &mut Desktop| s.toggle(PREVIOUS))
        .child(
            icon(
                if shown {
                    &ui::CHEVRON_OPEN
                } else {
                    &ui::CHEVRON
                },
                ICON_SMALL,
            )
            .color(t.faint),
        )
        .child(ui::section("Previous subagents", t).grow(1.0))
        .child(text(count.to_string()).font(t.mono_small).color(t.faint))
}

/// The same identity block in the panel and the timeline. Use reported model
/// and effort, never the defaults for that harness.
pub fn agent_identity(chat: &ConversationSummary, t: &Theme) -> El {
    let label = chat_title(chat);
    let detail = match (&chat.model, &chat.thinking) {
        (Some(model), Some(thinking)) => Some(format!("{model} · {thinking}")),
        (Some(model), None) => Some(model.clone()),
        (None, Some(thinking)) => Some(thinking.clone()),
        (None, None) => chat.harness.clone(),
    };
    row()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .center_y()
        .gap(S8)
        .child(
            icon(
                chat.harness.as_deref().map_or(&ui::AGENT, ui::harness_icon),
                ICON,
            )
            .color(t.muted)
            .a11y_hidden(),
        )
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .child(
                    text(label)
                        .font(t.ui_medium)
                        .color(t.text)
                        .truncate()
                        .help_when_truncated(label),
                )
                .children(detail.map(|detail| {
                    text(detail.clone())
                        .font(t.small)
                        .color(t.muted)
                        .truncate()
                        .help_when_truncated(detail)
                })),
        )
}

fn agent_row(chat: &ConversationSummary, t: &Theme) -> El {
    let id = chat.id;
    let (mark, state) = ui::agent_state(chat.status, t);
    row()
        .key(format!("agent-{id}"))
        .role("link")
        .label(chat_title(chat))
        .description(state)
        .h(S40)
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .hover(t.hover)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx| s.select(id, tx))
        .child(agent_identity(chat, t))
        .child(ui::lead(mark, S32))
        .child(icon(&ui::CHEVRON, ICON_SMALL).color(t.faint).a11y_hidden())
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

fn title(s: &Desktop, t: &Theme) -> El {
    let group = row()
        .key("header-title")
        .min_w(0.0)
        .shrink()
        .gap(S8)
        .center_y();
    let Some(id) = s.selected else {
        return s.new_chat.as_ref().map_or_else(
            || title_text("viz", t),
            |draft| {
                let harness = draft
                    .active_harness(&s.harnesses)
                    .map(|runner| runner.name.as_str());
                group
                    .child(harness_mark(harness, t))
                    .child(title_text("New chat", t))
            },
        );
    };
    if s.root == Some(id) {
        return group
            .child(icon(&ui::STAR, ICON).color(t.accent).a11y_hidden())
            .child(title_text("Orchestrator", t));
    }
    let chat = s.summary(id);
    let mark = harness_mark(chat.and_then(|chat| chat.harness.as_deref()), t);
    let title = title_text(s.title(id), t);
    let Some(parent) = chat
        .and_then(|chat| chat.parent)
        .filter(|parent| *parent != id && s.summary(*parent).is_some())
    else {
        return group.child(mark).child(title);
    };
    group
        .child(breadcrumb(s, parent, t))
        .child(icon(&ui::CHEVRON, ICON_SMALL).color(t.faint).a11y_hidden())
        .child(mark)
        .child(title)
}

fn harness_mark(harness: Option<&str>, t: &Theme) -> El {
    let mark = icon(harness.map_or(&ui::HARNESS, ui::harness_icon), ICON).color(t.muted);
    match harness {
        Some(name) => mark.help(name),
        None => mark.a11y_hidden(),
    }
}

fn breadcrumb(s: &Desktop, parent: ConversationId, t: &Theme) -> El {
    let label = s.title(parent);
    row()
        .key("header-parent")
        .role("link")
        .label(format!("Back to {label}"))
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

fn metadata(s: &Desktop, t: &Theme) -> El {
    let mut metadata = row().key("header-metadata").gap(S8).center_y();
    if let Some(state) = &s.conversation {
        if let Some(usage) = &state.usage
            && usage.size > 0
        {
            let fraction = crate::theme::fraction(usage.used, usage.size);
            let percent = format!("{:.0}%", fraction * 100.0);
            let label = format!(
                "Context: {} / {} tokens ({percent})",
                usage.used, usage.size
            );
            metadata = metadata.child(
                row()
                    .center_y()
                    .gap(S4)
                    .label(label.clone())
                    .help(label)
                    .child(
                        ProgressCircle::new(fraction)
                            .size(ICON_SMALL)
                            .build(&t.k)
                            .a11y_hidden(),
                    )
                    .child(text(percent).font(t.small).color(t.muted).a11y_hidden()),
            );
        }
        if let Some(mark) = ui::status(state.status, t) {
            metadata = metadata.child(mark.key("header-status"));
        }
    }
    metadata
}
