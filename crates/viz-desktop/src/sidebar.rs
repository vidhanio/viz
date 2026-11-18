//! The sidebar, on rune-kit's `Sidebar`: a new-chat row, the orchestrator,
//! then chats, each with its subagents nested under it, and the connection's
//! state at the bottom.

use std::collections::HashSet;

use rune::{El, Tx, icon, row, text};
use rune_kit::{
    Button, ButtonVariant, ControlSize,
    shell::{RowStatus, Sidebar, SidebarHeader, SidebarRow},
};
use viz_client::protocol::{ConversationId, ConversationKind};

use crate::{
    app::{Desktop, Status},
    theme::{self, Theme},
    thread::{Item, ToolStatus},
};

/// The deepest a subagent row indents.
const MAX_DEPTH: u8 = 4;

/// The sidebar's content.
pub fn sidebar(s: &Desktop, t: &Theme) -> El {
    let busy = busy(s);
    let new_chat = SidebarRow::new("New chat")
        .icon(&theme::PLUS)
        .on_select(|s: &mut Desktop, tx: &mut Tx| s.create(tx))
        .build(&t.k)
        .key("new-chat");
    let mut sidebar = Sidebar::new()
        .label("Conversations")
        .header_padding(0.0, 10.0)
        .header(new_chat)
        .footer(connection(s, t));
    let mut rows = Vec::new();
    if let Some(root) = s.root {
        rows.push(conversation_row(s, root, 0, &busy, t));
        children(s, root, 1, &busy, &mut rows, t);
    }
    let mut chats: Vec<ConversationId> = s
        .conversations
        .iter()
        .filter(|summary| summary.kind == ConversationKind::Chat)
        .map(|summary| summary.id)
        .collect();
    chats.sort_unstable_by(|a, b| b.cmp(a));
    if !chats.is_empty() {
        rows.push(
            SidebarHeader::new("Chats")
                .build(&t.k)
                .key("chats-header")
                .mt(12.0),
        );
    }
    for chat in chats {
        rows.push(conversation_row(s, chat, 0, &busy, t));
        children(s, chat, 1, &busy, &mut rows, t);
    }
    for row in rows {
        sidebar = sidebar.child(row);
    }
    sidebar.build(&t.k)
}

/// `parent`'s subagents, depth first, oldest first.
fn children(
    s: &Desktop,
    parent: ConversationId,
    depth: u8,
    busy: &HashSet<ConversationId>,
    rows: &mut Vec<El>,
    t: &Theme,
) {
    let mut subagents: Vec<ConversationId> = s
        .conversations
        .iter()
        .filter(|summary| summary.parent == Some(parent))
        .map(|summary| summary.id)
        .collect();
    subagents.sort_unstable();
    for child in subagents {
        rows.push(conversation_row(s, child, depth.min(MAX_DEPTH), busy, t));
        children(s, child, depth + 1, busy, rows, t);
    }
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

fn conversation_row(
    s: &Desktop,
    id: ConversationId,
    depth: u8,
    busy: &HashSet<ConversationId>,
    t: &Theme,
) -> El {
    let selected = s.selected == Some(id);
    let mut row = SidebarRow::new(s.title(id))
        .depth(depth)
        .selected(selected)
        .on_select(move |s: &mut Desktop, tx: &mut Tx| s.select(id, tx));
    if Some(id) == s.root {
        row = row
            .avatar(icon(&theme::STAR, 13.0).color(t.accent))
            .title_style(t.ui_medium);
    } else if depth > 0 {
        row = row.icon(&theme::BRANCH);
    }
    if busy.contains(&id) {
        row = row.status(RowStatus::Other(theme::pulse(t.accent, 6.0)));
    }
    let built = row.build(&t.k).key(format!("conversation-{id}"));
    if selected {
        // The tide line marks the open conversation.
        built
            .fill(t.selected)
            .child(t.tide(14.0).absolute(0.0, 7.0))
    } else {
        built
    }
}

/// The footer: a status dot, one word, and a retry button while
/// disconnected (the error in its tooltip).
fn connection(s: &Desktop, t: &Theme) -> El {
    let (color, label, error) = match &s.status {
        Status::Connecting => (t.warning, "connecting…", None),
        Status::Connected { .. } => (t.success, "connected", None),
        Status::Error(message) => (t.danger, "disconnected", Some(message)),
    };
    let detail = match (&s.status, &s.endpoint) {
        (Status::Connected { version }, Some(endpoint)) => {
            format!("viz-backend {version} at {endpoint}")
        }
        (Status::Error(message), _) => message.clone(),
        (_, Some(endpoint)) => endpoint.clone(),
        _ => String::new(),
    };
    let mut footer = row()
        .key("connection")
        .role("status")
        .w_full()
        .h(28.0)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .help(detail)
        .child(theme::dot(color, 6.0))
        .child(
            text(label)
                .key("label")
                .font(t.small)
                .color(t.muted)
                .truncate()
                .grow(1.0)
                .shrink(),
        );
    if error.is_some() {
        footer = footer.child(
            Button::new("Retry")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .on_press_tx(|s: &mut Desktop, tx: &mut Tx| s.reconnect(tx))
                .build(&t.k)
                .key("retry"),
        );
    }
    footer
}
