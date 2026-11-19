//! Conversation tree, its filter and disclosure state.

mod actions;
mod worktree;

use std::collections::{HashMap, HashSet};

use rune::{El, TextEdit, TextStyle, Tx, col, icon, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant, TreeItem, TreeRow, shell::Sidebar};
use viz_client::protocol::{ConversationId, ConversationKind, ConversationSummary, Project};

use crate::{
    app::{Desktop, Status, chat_title},
    theme::{self, Theme},
    ui,
};

#[derive(Debug)]
pub struct Navigation {
    filter: TextEdit,
    archived: bool,
    collapsed_projects: HashSet<String>,
    collapsed_chats: HashSet<ConversationId>,
    menu: Option<actions::Context>,
    edit: Option<actions::Edit>,
    value: TextEdit,
    worktree: worktree::Form,
}

impl Default for Navigation {
    fn default() -> Self {
        Self {
            filter: TextEdit::new(""),
            archived: false,
            collapsed_projects: HashSet::new(),
            collapsed_chats: HashSet::new(),
            menu: None,
            edit: None,
            value: TextEdit::new(""),
            worktree: worktree::Form::default(),
        }
    }
}

impl Navigation {
    pub fn reveal(&mut self, id: ConversationId, summaries: &[ConversationSummary]) {
        let mut cursor = Some(id);
        // Parents are older than their children. Reject malformed links rather
        // than looping.
        while let Some(id) = cursor {
            let Some(chat) = summaries.iter().find(|chat| chat.id == id) else {
                break;
            };
            self.collapsed_chats.remove(&id);
            if let Some(project) = &chat.project {
                self.expand_project(project);
            }
            cursor = chat.parent.filter(|parent| *parent < id);
        }
    }

    pub fn expand_project(&mut self, project: &str) {
        self.collapsed_projects.remove(project);
    }

    fn toggle_chat(&mut self, id: ConversationId) {
        if !self.collapsed_chats.remove(&id) {
            self.collapsed_chats.insert(id);
        }
    }

    fn toggle_project(&mut self, id: &str) {
        if !self.collapsed_projects.remove(id) {
            self.collapsed_projects.insert(id.to_owned());
        }
    }
}

/// Pre-index visible chats so each row is visited once, not once per parent.
struct Outline<'a> {
    state: &'a Desktop,
    children: HashMap<ConversationId, Vec<&'a ConversationSummary>>,
    projects: HashMap<&'a str, Vec<&'a ConversationSummary>>,
    loose: Vec<&'a ConversationSummary>,
    pinned: Vec<&'a ConversationSummary>,
    query: String,
}

impl<'a> Outline<'a> {
    fn new(state: &'a Desktop) -> Self {
        let query = state.navigation.filter.text.trim().to_lowercase();
        let mut outline = Self {
            state,
            children: HashMap::new(),
            projects: HashMap::new(),
            loose: Vec::new(),
            pinned: Vec::new(),
            query,
        };
        let chats: HashMap<_, _> = state
            .conversations
            .iter()
            .filter(|chat| state.navigation.archived || !chat.archived)
            .map(|chat| (chat.id, chat))
            .collect();
        let mut keep = HashSet::new();
        if !outline.query.is_empty() {
            let matching_projects: HashSet<_> = state
                .projects
                .iter()
                .filter(|project| outline.matches(&project.name) || outline.matches(&project.path))
                .map(|project| project.id.as_str())
                .collect();
            for chat in chats.values() {
                if outline.matches(chat_title(chat))
                    || chat
                        .project
                        .as_deref()
                        .is_some_and(|id| matching_projects.contains(id))
                {
                    let mut cursor = Some(chat.id);
                    while let Some(id) = cursor.filter(|id| keep.insert(*id)) {
                        cursor = chats
                            .get(&id)
                            .filter(|chat| !chat.pinned)
                            .and_then(|chat| chat.parent);
                    }
                }
            }
        }
        let known_projects: HashSet<_> = state.projects.iter().map(|p| p.id.as_str()).collect();
        for chat in &state.conversations {
            if !chats.contains_key(&chat.id)
                || (!outline.query.is_empty() && !keep.contains(&chat.id))
            {
                continue;
            }
            if chat.pinned {
                outline.pinned.push(chat);
                continue;
            }
            if chat.kind == ConversationKind::Orchestrator {
                continue;
            }
            if let Some(parent) = chat
                .parent
                .filter(|id| *id < chat.id && chats.contains_key(id))
            {
                outline.children.entry(parent).or_default().push(chat);
            } else if let Some(project) = chat
                .project
                .as_deref()
                .filter(|id| known_projects.contains(id))
            {
                outline.projects.entry(project).or_default().push(chat);
            } else {
                outline.loose.push(chat);
            }
        }
        outline
    }

    fn matches(&self, value: &str) -> bool {
        self.query.is_empty() || value.to_lowercase().contains(&self.query)
    }

    fn visible_project(&self, project: &Project) -> bool {
        self.projects.contains_key(project.id.as_str())
            || self.matches(&project.name)
            || self.matches(&project.path)
    }

    fn chat(&self, chat: &ConversationSummary, depth: u32, t: &Theme, layer: &mut Vec<El>) -> El {
        let id = chat.id;
        let s = self.state;
        let selected = s.selected == Some(id);
        let key = format!("chat-{id}");
        let depth = depth.min(5);
        let children = self.children.get(&id);
        let open = children.is_some()
            && (!self.query.is_empty() || !s.navigation.collapsed_chats.contains(&id));
        let glyph = match chat.kind {
            ConversationKind::Orchestrator => &theme::STAR,
            ConversationKind::Chat => &ui::CHAT,
            ConversationKind::Subagent => &theme::BRANCH,
        };
        let mut item = TreeItem::new(chat_title(chat)).icon(glyph);
        if chat.archived {
            item = item.label_color(t.faint);
        } else if chat.unread {
            item = item.label_color(t.text);
        }
        if chat.unread {
            item = item.dot(t.accent);
        }
        if let Some(path) = &chat.worktree {
            item = item.trailing(
                icon(&ui::GIT_BRANCH, 13.0)
                    .key("worktree")
                    .color(t.faint)
                    .help(path)
                    .a11y_hidden(),
            );
        }
        if let Some(status) = ui::status(chat.status, t) {
            item = item.trailing(status.key("status"));
        }
        item = item.trailing(actions::chat_menu(s, chat, &key, selected, t, layer));
        let font = if chat.unread {
            TextStyle {
                weight: t.title.weight,
                ..t.ui
            }
        } else {
            t.ui
        };
        let row = TreeRow::new(&key, depth).selected(selected);
        let mut node = ui::tree_row(&row, item, t, font)
            .hover_group_focus()
            .on_press_tx(move |s: &mut Desktop, tx| s.select(id, tx));
        if chat.unread {
            node = node.description("Unread");
        }
        if s.connected() {
            let key = key.clone();
            node = node.on_secondary_press(move |s: &mut Desktop, at, _| {
                s.navigation.menu = Some(actions::Context::at(&key, at));
            });
        }
        if children.is_some() {
            let (icon, label) = if open {
                (&theme::CHEVRON_OPEN, "Collapse subagents")
            } else {
                (&theme::CHEVRON, "Expand subagents")
            };
            node = node.expanded(open).child(
                ui::icon_button(icon, label, IconButtonVariant::Ghost)
                    .on_press(move |s: &mut Desktop| s.navigation.toggle_chat(id))
                    .build(&t.k)
                    .key("disclose")
                    .size(20.0, 25.0)
                    // Follow the tree row's indent lane.
                    .absolute(
                        f32::from(u16::try_from(depth).unwrap_or_default()).mul_add(14.0, 4.0),
                        2.0,
                    ),
            );
        }
        let mut branch = col().key(format!("branch-{id}")).w_full().child(node);
        if let Some(children) = children.filter(|_| open) {
            branch = branch.children(
                children
                    .iter()
                    .map(|child| self.chat(child, depth + 1, t, layer)),
            );
        }
        branch
    }

    fn project(&self, project: &Project, t: &Theme, layer: &mut Vec<El>) -> El {
        let s = self.state;
        let open = !self.query.is_empty() || !s.navigation.collapsed_projects.contains(&project.id);
        let id = project.id.clone();
        let key = format!("project-{id}");
        let item = TreeItem::new(&project.name)
            .icon(&ui::FOLDER)
            .label_color(t.text);
        let item = if let Some(branch) = &project.branch {
            item.trailing(
                ui::chip(branch, t)
                    .max_w(68.0)
                    .shrink()
                    .clip_rounded(4.0)
                    .help(branch),
            )
        } else {
            item
        };
        let item = item.trailing(actions::project_menu(s, project, &key, t, layer));
        let mut node = ui::tree_row(&TreeRow::new(&key, 0).expanded(Some(open)), item, t, t.ui)
            .hover_group_focus()
            .help(&project.path)
            .on_press(move |s: &mut Desktop| s.navigation.toggle_project(&id));
        if s.connected() {
            node = node.on_secondary_press(move |s: &mut Desktop, at, _| {
                s.navigation.menu = Some(actions::Context::at(&key, at));
            });
        }
        let mut branch = col()
            .key(format!("project-branch-{}", project.id))
            .w_full()
            .child(node);
        if let Some(chats) = self.projects.get(project.id.as_str()).filter(|_| open) {
            branch = branch.children(chats.iter().map(|chat| self.chat(chat, 1, t, layer)));
        }
        branch
    }
}

fn open_match(s: &mut Desktop, tx: &mut Tx) {
    let filter = s.navigation.filter.text.trim().to_lowercase();
    if filter.is_empty() {
        return;
    }
    let id = s
        .conversations
        .iter()
        .filter(|chat| s.navigation.archived || !chat.archived)
        .find(|chat| {
            chat_title(chat).to_lowercase().contains(&filter)
                || s.projects.iter().any(|project| {
                    Some(&project.id) == chat.project.as_ref()
                        && (project.name.to_lowercase().contains(&filter)
                            || project.path.to_lowercase().contains(&filter))
                })
        })
        .map(|chat| chat.id);
    if let Some(id) = id {
        s.navigation.filter.clear();
        s.select(id, tx);
    }
}

pub fn sidebar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let tree = conversation_tree(s, t, layer);
    Sidebar::new()
        .label("Workspace")
        .header_padding(0.0, 8.0)
        .header(controls(s, t))
        .child(tree)
        .footer(footer(s, t))
        .build(&t.k)
}

/// Mounts sidebar dialogs independently of whether the sidebar is visible.
pub fn overlays(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    actions::dialog(s, t, layer);
    worktree::dialog(s, t, layer);
}

/// Opens the add-project dialog from the sidebar or palette.
pub fn open_add_project(s: &mut Desktop, tx: &mut Tx) {
    actions::add_project(s, tx);
}

/// Opens the worktree dialog for the current project, if it has a Git branch.
pub fn open_worktree(s: &mut Desktop, tx: &mut Tx) {
    if let Some(id) = s.project_id().map(str::to_owned) {
        worktree::open(s, &id, tx);
    }
}

/// Shows the worktree form for the current project's preview scene.
pub fn preview_worktree(s: &mut Desktop) {
    if let Some(form) = s.project().and_then(worktree::Form::for_project) {
        s.navigation.worktree = form;
    }
}

/// The shell toolbar's new-chat button.
pub fn new_chat_button(s: &Desktop, t: &Theme) -> El {
    ui::button("New chat", ButtonVariant::Outline)
        .icon(&theme::PLUS)
        .disabled(!s.connected() || s.new_chat.is_some())
        .on_press_tx(Desktop::create_here)
        .build(&t.k)
        .key("new-chat")
}

fn controls(s: &Desktop, t: &Theme) -> El {
    col().w_full().pt(8.0).child(
        ui::text_field(&s.navigation.filter, "Filter chats", "Filter chats")
            .leading_icon(t.k.glyph.search)
            .on_edit(|s: &mut Desktop| &mut s.navigation.filter)
            .on_submit(open_match)
            .build(&t.k)
            .key("chat-filter")
            .border(1.0, t.hairline),
    )
}

fn conversation_tree(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let outline = Outline::new(s);
    let mut tree = col()
        .key("conversation-tree")
        .role("tree")
        .label("Conversations")
        .w_full()
        .gap(1.0);
    if let Some(root) = s.root.and_then(|root| s.summary(root)).filter(|root| {
        !root.pinned
            && (outline.matches(chat_title(root)) || outline.children.contains_key(&root.id))
    }) {
        tree = tree.child(outline.chat(root, 0, t, layer));
    }
    if !outline.pinned.is_empty() {
        tree = tree
            .child(
                row()
                    .key("pinned-heading")
                    .w_full()
                    .h(28.0)
                    .px(8.0)
                    .mt(9.0)
                    .center_y()
                    .child(ui::section("Pinned", t)),
            )
            .children(
                outline
                    .pinned
                    .iter()
                    .map(|chat| outline.chat(chat, 0, t, layer)),
            );
    }
    tree = tree.child(
        row()
            .key("projects-heading")
            .w_full()
            .h(28.0)
            .px(8.0)
            .mt(9.0)
            .center_y()
            .child(ui::section("Projects", t).grow(1.0))
            .child(
                ui::icon_button(&theme::PLUS, "Add project", IconButtonVariant::Ghost)
                    .disabled(!s.connected())
                    .on_press_tx(open_add_project)
                    .build(&t.k)
                    .key("add-project"),
            ),
    );
    tree = tree.children(
        s.projects
            .iter()
            .filter(|p| outline.visible_project(p))
            .map(|p| outline.project(p, t, layer)),
    );
    if s.projects.is_empty() && outline.query.is_empty() {
        tree = tree.child(
            col()
                .key("no-projects")
                .w_full()
                .px(8.0)
                .pt(2.0)
                .gap(8.0)
                .child(
                    ui::button("Add project", ButtonVariant::Outline)
                        .icon(&theme::PLUS)
                        .disabled(!s.connected())
                        .on_press_tx(open_add_project)
                        .build(&t.k)
                        .key("add-first-project")
                        .w_full(),
                ),
        );
    }
    if !outline.loose.is_empty() {
        tree = tree
            .child(
                ui::section("Chats", t)
                    .key("chats-heading")
                    .px(8.0)
                    .mt(13.0)
                    .mb(4.0),
            )
            .children(
                outline
                    .loose
                    .iter()
                    .map(|chat| outline.chat(chat, 0, t, layer)),
            );
    }
    tree
}

fn footer(s: &Desktop, t: &Theme) -> El {
    let toggle = ui::button(
        if s.navigation.archived {
            "Hide archived"
        } else {
            "Show archived"
        },
        ButtonVariant::GhostMuted,
    )
    .on_press(|s: &mut Desktop| s.navigation.archived = !s.navigation.archived)
    .build(&t.k);
    col()
        .gap(3.0)
        .child(toggle.key("archived").self_start())
        .child(ui::rule(t))
        .child(connection(s, t))
}

/// The footer: a status dot, one word, and a retry button while
/// disconnected (the error in its tooltip).
fn connection(s: &Desktop, t: &Theme) -> El {
    let (color, label, error) = match &s.status {
        Status::Connecting => (t.warning, "Connecting", None),
        Status::Connected { .. } => (t.success, "Connected", None),
        Status::Error(message) => (t.danger, "Offline", Some(message)),
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
        .child(ui::dot(color, 6.0))
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
            ui::button("Retry", ButtonVariant::Ghost)
                .on_press_tx(Desktop::reconnect)
                .build(&t.k)
                .key("retry"),
        );
    }
    footer
}
