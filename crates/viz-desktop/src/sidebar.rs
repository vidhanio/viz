//! The sidebar: the orchestrator, then pinned chats and projects with their
//! top-level chats. Subagents live in their parent chat, not here.

mod actions;
mod worktree;

use std::collections::{HashMap, HashSet};

use rune::{El, TextEdit, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant, shell::Sidebar};
use viz_client::protocol::{
    ConversationId, ConversationKind, ConversationStatus, ConversationSummary, Project,
};

use crate::{
    app::{Desktop, Status, chat_title},
    theme::{
        self, HAIRLINE, ICON, ICON_SMALL, ORCHESTRATOR_ROW, ROW_INSET, S8, S32, SIDEBAR_PAD, Size,
        TOOLBAR_HEIGHT, Theme,
    },
    ui::{self, TreeLine},
};

/// The widest a project's branch name gets before it truncates.
const BRANCH_MAX: f32 = S32 * 2.0;

#[derive(Debug)]
pub struct Navigation {
    filter: TextEdit,
    archived: bool,
    collapsed_projects: HashSet<String>,
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
            menu: None,
            edit: None,
            value: TextEdit::new(""),
            worktree: worktree::Form::default(),
        }
    }
}

impl Navigation {
    /// Opens the project holding `id`'s top-level chat.
    pub fn reveal(&mut self, id: ConversationId, summaries: &[ConversationSummary]) {
        let top = top_level(summaries, id);
        if let Some(project) = summaries
            .iter()
            .find(|chat| chat.id == top)
            .and_then(|chat| chat.project.as_deref())
        {
            self.expand_project(project);
        }
    }

    pub fn expand_project(&mut self, project: &str) {
        self.collapsed_projects.remove(project);
    }

    fn toggle_project(&mut self, id: &str) {
        if !self.collapsed_projects.remove(id) {
            self.collapsed_projects.insert(id.to_owned());
        }
    }
}

/// `parent`'s subagents, running first, then newest first.
pub fn subagents(
    summaries: &[ConversationSummary],
    parent: ConversationId,
) -> Vec<&ConversationSummary> {
    let mut children: Vec<_> = summaries
        .iter()
        .filter(|chat| chat.parent == Some(parent) && chat.id != parent)
        .collect();
    children.sort_by_key(|chat| {
        (
            chat.status != ConversationStatus::Working,
            std::cmp::Reverse(chat.updated_at),
        )
    });
    children
}

/// The top-level chat `id` belongs to: itself, or the oldest ancestor its
/// parent links reach. Parents are older than their children; malformed links
/// stop the walk rather than loop.
pub fn top_level(summaries: &[ConversationSummary], id: ConversationId) -> ConversationId {
    let mut top = id;
    while let Some(parent) = summaries
        .iter()
        .find(|chat| chat.id == top)
        .and_then(|chat| chat.parent)
        .filter(|parent| *parent < top && summaries.iter().any(|chat| chat.id == *parent))
    {
        top = parent;
    }
    top
}

/// Top-level chats by place, filtered once per frame.
struct Outline<'a> {
    state: &'a Desktop,
    /// The open chat's top-level chat: subagents highlight their parent.
    open: Option<ConversationId>,
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
            open: state.selected.map(|id| top_level(&state.conversations, id)),
            projects: HashMap::new(),
            loose: Vec::new(),
            pinned: Vec::new(),
            query,
        };
        let known_projects: HashSet<_> = state.projects.iter().map(|p| p.id.as_str()).collect();
        let matching_projects: HashSet<_> = state
            .projects
            .iter()
            .filter(|project| outline.matches(&project.name) || outline.matches(&project.path))
            .map(|project| project.id.as_str())
            .collect();
        for chat in &state.conversations {
            let child = chat.parent.is_some_and(|parent| {
                parent < chat.id && state.conversations.iter().any(|other| other.id == parent)
            });
            if chat.kind == ConversationKind::Orchestrator
                || Some(chat.id) == state.root
                || child
                || (chat.archived && !state.navigation.archived)
            {
                continue;
            }
            let project = chat
                .project
                .as_deref()
                .filter(|id| known_projects.contains(id));
            if !outline.matches(chat_title(chat))
                && !project.is_some_and(|id| matching_projects.contains(id))
            {
                continue;
            }
            if chat.pinned {
                outline.pinned.push(chat);
            } else if let Some(project) = project {
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

    fn chat(&self, chat: &ConversationSummary, depth: u16, t: &Theme, layer: &mut Vec<El>) -> El {
        let id = chat.id;
        let s = self.state;
        let selected = self.open == Some(id);
        let key = format!("chat-{id}");
        let menu_open = actions::menu_open(s, &key);
        let rest = ui::status(chat.status, t).or_else(|| {
            chat.unread.then(|| {
                ui::dot(t.accent)
                    .role("status")
                    .label("Unread")
                    .help("Unread")
            })
        });
        let menu = actions::chat_menu(s, chat, &key, t, layer);
        let worktree = chat.worktree.as_ref().map(|path| {
            icon(&ui::GIT_BRANCH, ICON_SMALL)
                .key("worktree")
                .color(t.faint)
                .help(path)
                .a11y_hidden()
        });
        let mut node = TreeLine {
            key: &key,
            label: chat_title(chat),
            depth,
            strong: chat.unread && !chat.archived,
            dim: chat.archived,
            selected,
        }
        .build(None, worktree, ui::trailing(rest, Some(menu), menu_open), t)
        .on_press_tx(move |s: &mut Desktop, tx| s.select(id, tx));
        if chat.unread {
            node = node.description("Unread");
        }
        if s.connected() {
            node = node.on_secondary_press(move |s: &mut Desktop, at, _| {
                s.navigation.menu = Some(actions::Context::at(&key, at));
            });
        }
        node
    }

    fn project(&self, project: &Project, t: &Theme, layer: &mut Vec<El>) -> El {
        let s = self.state;
        let open = !self.query.is_empty() || !s.navigation.collapsed_projects.contains(&project.id);
        let id = project.id.clone();
        let key = format!("project-{id}");
        let menu_open = actions::menu_open(s, &key);
        // The folder, turning into the disclosure under the pointer.
        let layer_of = |glyph: &'static rune::Icon, rest: f32, hovered: f32| {
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .center()
                .group_opacity(rest, hovered)
                .child(icon(glyph, ICON).color(t.muted))
        };
        let disclosure = if open {
            &ui::CHEVRON_OPEN
        } else {
            &ui::CHEVRON
        };
        let leading = el()
            .size(ICON, ICON)
            .child(layer_of(&ui::FOLDER, 1.0, 0.0))
            .child(layer_of(disclosure, 0.0, 1.0));
        let branch = project.branch.as_ref().map(|branch| {
            text(branch)
                .key("branch")
                .font(t.small)
                .color(t.faint)
                .max_w(BRANCH_MAX)
                .min_w(0.0)
                .truncate()
                .help(branch)
        });
        let menu = actions::project_menu(s, project, &key, t, layer);
        let mut node = TreeLine {
            key: &key,
            label: &project.name,
            depth: 0,
            strong: false,
            dim: false,
            selected: false,
        }
        .build(
            Some(leading),
            branch,
            ui::trailing(None, Some(menu), menu_open),
            t,
        )
        .expanded(open)
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
    let outline = Outline::new(s);
    let id = outline
        .pinned
        .iter()
        .chain(s.projects.iter().filter_map(|project| {
            outline
                .projects
                .get(project.id.as_str())
                .and_then(|chats| chats.first())
        }))
        .chain(&outline.loose)
        .next()
        .map(|chat| chat.id);
    if let Some(id) = id.filter(|_| !outline.query.is_empty()) {
        s.navigation.filter.clear();
        s.select(id, tx);
    }
}

pub fn sidebar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let tree = conversation_tree(s, t, layer);
    let mut bar = Sidebar::new()
        .label("Workspace")
        .inset(0.0)
        .header_gap(S8)
        .header_padding(0.0, 0.0);
    if let Some(orchestrator) = orchestrator(s, t) {
        bar = bar.header(orchestrator);
    }
    bar.header(filter(s, t))
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

/// Opens the confirmation for deleting a chat from the command palette.
pub fn open_delete_chat(s: &mut Desktop, id: ConversationId, tx: &mut Tx) {
    actions::delete_chat(s, id, tx);
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

/// Shows the delete confirmation for `id` in a preview scene.
pub fn preview_delete(s: &mut Desktop, id: ConversationId) {
    s.navigation.edit = Some(actions::Edit::DeleteChat(id));
}

/// New chat, as an icon button: in the sidebar's header row while the sidebar
/// is open, after the sidebar toggle in the toolbar while it's closed.
pub fn new_chat_button(s: &Desktop, t: &Theme) -> El {
    ui::icon_button(
        &ui::COMPOSE,
        "New chat",
        IconButtonVariant::GhostMuted,
        Size::Default,
    )
    .shortcut(format!("{}+N", ui::COMMAND_KEY))
    .disabled(!s.connected() || s.new_chat.is_some())
    .on_press_tx(Desktop::create_here)
    .build(&t.k)
    .key("new-chat")
}

/// Where the sidebar header's new-chat button sits, from the window's top
/// left: one gap before the sidebar toggle, which rune-kit pins to the
/// header's trailing end.
pub fn new_chat_origin(s: &Desktop) -> (f32, f32) {
    let side = Size::Default.height();
    (
        s.sidebar.width() - SIDEBAR_PAD - side - theme::S4 - side,
        (TOOLBAR_HEIGHT - side) / 2.0,
    )
}

/// The orchestrator: its own row above the tree, never pinned or nested.
fn orchestrator(s: &Desktop, t: &Theme) -> Option<El> {
    let root = s.root.and_then(|id| s.summary(id))?;
    let id = root.id;
    let selected = s.selected == Some(id);
    let (detail, color) = match root.status {
        ConversationStatus::Working => ("Working".to_owned(), t.muted),
        ConversationStatus::Failed => ("Last run failed".to_owned(), t.danger),
        ConversationStatus::Idle => (
            theme::ago(root.updated_at)
                .map_or_else(|| "Idle".to_owned(), |ago| format!("Idle · {ago}")),
            t.faint,
        ),
    };
    let rest = ui::status(root.status, t).or_else(|| root.unread.then(|| ui::dot(t.accent)));
    Some(
        row()
            .key("orchestrator")
            .role("link")
            .label("Orchestrator")
            .description(detail.clone())
            .w_full()
            .h(ORCHESTRATOR_ROW)
            .pl(ROW_INSET)
            .gap(S8)
            .center_y()
            .radius(Size::Default.radius())
            .fill(if selected {
                t.selected
            } else {
                rune::Color::TRANSPARENT
            })
            .hover(if selected { t.selected } else { t.hover })
            .hover_group_focus()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .child(ui::lead(
                icon(&ui::STAR, ICON).color(t.accent),
                ORCHESTRATOR_ROW,
            ))
            .child(
                col()
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .child(
                        text("Orchestrator")
                            .font(t.ui_medium)
                            .color(t.text)
                            .a11y_hidden(),
                    )
                    .child(
                        text(detail)
                            .key("orchestrator-status")
                            .font(t.small)
                            .color(color)
                            .truncate()
                            .a11y_hidden(),
                    ),
            )
            .child(ui::trailing(rest, None, false))
            .on_press_tx(move |s: &mut Desktop, tx| s.select(id, tx)),
    )
}

fn filter(s: &Desktop, t: &Theme) -> El {
    ui::text_field(
        &s.navigation.filter,
        "Filter chats",
        "Filter chats",
        Size::Default,
    )
    .leading_icon(&ui::SEARCH)
    .on_edit(|s: &mut Desktop| &mut s.navigation.filter)
    .on_submit(open_match)
    .build(&t.k)
    .key("chat-filter")
}

fn conversation_tree(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let outline = Outline::new(s);
    let mut tree = col()
        .key("conversation-tree")
        .role("tree")
        .label("Conversations")
        .w_full();
    if !outline.pinned.is_empty() {
        tree = tree
            .child(ui::section_row("pinned-heading", "Pinned", None, t))
            .children(
                outline
                    .pinned
                    .iter()
                    .map(|chat| outline.chat(chat, 0, t, layer)),
            );
    }
    tree = tree
        .child(ui::section_row(
            "projects-heading",
            "Projects",
            Some(
                ui::icon_button(
                    &ui::PLUS,
                    "Add project",
                    IconButtonVariant::GhostMuted,
                    Size::Small,
                )
                .disabled(!s.connected())
                .on_press_tx(open_add_project)
                .build(&t.k)
                .key("add-project"),
            ),
            t,
        ))
        .children(
            s.projects
                .iter()
                .filter(|p| outline.visible_project(p))
                .map(|p| outline.project(p, t, layer)),
        );
    if !outline.loose.is_empty() {
        tree = tree
            .child(ui::section_row("chats-heading", "Chats", None, t))
            .children(
                outline
                    .loose
                    .iter()
                    .map(|chat| outline.chat(chat, 0, t, layer)),
            );
    }
    tree
}

/// The connection (a dot and one word, the detail in its tooltip), a retry
/// while offline, and the archived toggle.
fn footer(s: &Desktop, t: &Theme) -> El {
    let (color, label) = match &s.status {
        Status::Connecting => (t.warning, "Connecting"),
        Status::Connected { .. } => (t.success, "Connected"),
        Status::Error(_) => (t.danger, "Offline"),
    };
    let detail = match (&s.status, &s.endpoint) {
        (Status::Connected { version }, Some(endpoint)) => {
            format!("viz-backend {version} at {endpoint}")
        }
        (Status::Error(message), _) => message.clone(),
        (_, Some(endpoint)) => endpoint.clone(),
        _ => String::new(),
    };
    let quiet = |label: &str| ui::button(label, ButtonVariant::GhostMuted, Size::Small);
    let mut footer = row()
        .key("connection")
        .w_full()
        .h(Size::Default.height())
        .pl(ROW_INSET)
        .gap(S8)
        .center_y()
        .child(
            row()
                .role("status")
                .label(label)
                .help(detail)
                .gap(S8)
                .center_y()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .child(ui::lead(ui::dot(color), Size::Default.height()))
                .child(
                    text(label)
                        .key("label")
                        .font(t.small)
                        .color(t.muted)
                        .truncate()
                        .shrink(),
                ),
        );
    if matches!(s.status, Status::Error(_)) {
        footer = footer.child(
            quiet("Retry")
                .on_press_tx(Desktop::reconnect)
                .build(&t.k)
                .key("retry"),
        );
    }
    let archived = s.navigation.archived;
    col().w_full().gap(S8).child(ui::rule(t)).child(
        footer.child(
            quiet(if archived {
                "Hide archived"
            } else {
                "Show archived"
            })
            .on_press(|s: &mut Desktop| s.navigation.archived = !s.navigation.archived)
            .build(&t.k)
            .key("archived")
            .expanded(archived),
        ),
    )
}
