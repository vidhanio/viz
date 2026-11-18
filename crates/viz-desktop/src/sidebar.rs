//! The sidebar: a new-chat row and the orchestrator pinned on top, chats and
//! subagents in collapsible sections, and the connection's status at the
//! bottom.

use std::collections::HashSet;

use rune::{El, TextStyle, Tx, col, row, text};
use rune_kit::{
    Button, ButtonVariant, ControlSize, Kit,
    shell::{RowStatus, Sidebar, SidebarRow, SidebarSection},
};
use rune_theme_opencode::icons;
use viz_client::protocol::ConversationId;

use crate::{
    app::{Desktop, Listed, Status},
    theme,
    thread::{Item, ToolStatus},
};

/// The new-chat shortcut as the platform writes it.
pub const NEW_CHAT_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘N"
} else {
    "Ctrl N"
};

/// The sidebar's content.
pub fn sidebar(s: &Desktop, k: &Kit) -> El {
    let busy = busy(s);
    let new_chat = SidebarRow::new("New chat")
        .icon(&icons::NEW_SESSION)
        .detail(NEW_CHAT_SHORTCUT)
        .on_select(|s: &mut Desktop, tx: &mut Tx| s.create(tx))
        .build(k)
        .key("new-chat");
    let mut sidebar = Sidebar::new()
        .label("Conversations")
        .header_padding(0.0, 6.0)
        .header(new_chat)
        .footer(connection(s, k));
    if let Some(root) = s.root {
        sidebar = sidebar.header(row_for(s, root, &icons::OUTLINE_BRAIN, &busy, k));
    }
    let (subagents, chats): (Vec<Listed>, Vec<Listed>) = s
        .conversations
        .iter()
        .partition(|listed| listed.owner.is_some());
    if !chats.is_empty() {
        sidebar = sidebar.section(
            SidebarSection::new("Chats")
                .key("chats")
                .count(chats.len())
                .collapsed(s.chats_closed)
                .on_toggle(|s: &mut Desktop, _: &mut Tx| s.chats_closed ^= true)
                .rows(
                    chats
                        .iter()
                        .map(|listed| row_for(s, listed.id, &icons::SPEECH_BUBBLE, &busy, k)),
                ),
        );
    }
    if !subagents.is_empty() {
        sidebar = sidebar.section(
            SidebarSection::new("Subagents")
                .key("subagents")
                .count(subagents.len())
                .collapsed(s.subagents_closed)
                .on_toggle(|s: &mut Desktop, _: &mut Tx| s.subagents_closed ^= true)
                .rows(
                    subagents
                        .iter()
                        .map(|listed| row_for(s, listed.id, &icons::FORK, &busy, k)),
                ),
        );
    }
    sidebar.build(k)
}

/// Conversations known to be working: the watched one while its run lasts,
/// and the children of its running `subagent` calls. Others' state isn't
/// watched, so they show none.
fn busy(s: &Desktop) -> HashSet<ConversationId> {
    let thread = &s.thread;
    let children = thread.items.iter().filter_map(|item| match item {
        Item::Tool(tool) if tool.status == ToolStatus::Running => tool.child,
        _ => None,
    });
    s.selected
        .filter(|_| thread.live.busy)
        .into_iter()
        .chain(children)
        .collect()
}

fn row_for(
    s: &Desktop,
    id: ConversationId,
    icon: &'static rune::Icon,
    busy: &HashSet<ConversationId>,
    k: &Kit,
) -> El {
    let mut row = SidebarRow::new(s.title(id))
        .icon(icon)
        .selected(s.selected == Some(id))
        .on_select(move |s: &mut Desktop, tx: &mut Tx| s.select(id, tx));
    if Some(id) != s.root {
        row = row.detail(format!("#{id}"));
    }
    if busy.contains(&id) {
        row = row.status(RowStatus::Busy);
    }
    row.build(k).key(format!("conversation-{id}"))
}

/// The footer: a status dot and line, the endpoint, and a retry button while
/// disconnected.
fn connection(s: &Desktop, k: &Kit) -> El {
    let c = &k.c;
    let (color, label, detail) = match &s.status {
        Status::Connecting => (c.warning.icon, "Connecting…".to_owned(), None),
        Status::Connected { version } => (c.success.icon, format!("Connected · v{version}"), None),
        Status::Error(message) => (c.danger.icon, "Disconnected".to_owned(), Some(message)),
    };
    let mut state = row()
        .key("state")
        .w_full()
        .gap(8.0)
        .center_y()
        .child(theme::dot(color, 7.0))
        .child(
            text(label)
                .key("label")
                .font(k.ty.ui_medium)
                .color(c.text_base)
                .truncate()
                .grow(1.0)
                .shrink(),
        );
    if detail.is_some() {
        state = state.child(
            Button::new("Retry")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .on_press_tx(|s: &mut Desktop, tx: &mut Tx| s.reconnect(tx))
                .build(k)
                .key("retry"),
        );
    }
    let mut footer = col()
        .key("connection")
        .role("status")
        .w_full()
        .px(8.0)
        .py(8.0)
        .gap(4.0)
        .radius(k.radius.md)
        .fill(c.layer_01)
        .border(1.0, c.border_weak)
        .child(state);
    if let Some(detail) = detail {
        footer = footer.child(
            text(detail.clone())
                .key("error")
                .font(k.ty.small)
                .color(c.text_muted)
                .w_full()
                .max_lines(2)
                .help(detail.clone()),
        );
    }
    if let Some(endpoint) = &s.endpoint {
        footer = footer.child(
            text(endpoint.clone())
                .key("endpoint")
                .font(TextStyle {
                    size: 10.5,
                    line_height: 14.0,
                    ..k.ty.mono
                })
                .color(c.text_faint)
                .w_full()
                .truncate()
                .help(endpoint.clone()),
        );
    }
    footer
}
