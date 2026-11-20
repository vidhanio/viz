//! The sidebar: the orchestrator, then pinned chats, then top-level chats
//! either newest first (Recent) or under their projects (Projects).
//! Subagents live in their parent chat, not here.

mod actions;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use rune::{El, TextEdit, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant, shell::Sidebar};
use serde_json::{Value, json};
use viz_client::protocol::{
    ConversationId, ConversationKind, ConversationStatus, ConversationSummary, Project,
};

use crate::{
    app::{Desktop, Status, chat_title},
    theme::{
        self, HAIRLINE, ICON, ORCHESTRATOR_ROW, PROJECT_BOX_INSET, ROW_INSET, S8, SIDEBAR_PAD,
        Size, TOOLBAR_HEIGHT, Theme,
    },
    ui::{self, TreeLine},
};

/// How the sidebar lists top-level chats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Newest first, each with its project under the title.
    #[default]
    Recent,
    /// Under their collapsible projects, newest first.
    Projects,
}

impl Mode {
    const fn name(self) -> &'static str {
        match self {
            Self::Recent => "recent",
            Self::Projects => "projects",
        }
    }
}

#[derive(Debug)]
pub struct Navigation {
    filter: TextEdit,
    archived: bool,
    mode: Mode,
    collapsed_projects: HashSet<String>,
    menu: Option<actions::Context>,
    edit: Option<actions::Edit>,
    value: TextEdit,
    /// Where the mode and collapsed projects persist, once restored from it.
    store: Option<PathBuf>,
}

impl Default for Navigation {
    fn default() -> Self {
        Self {
            filter: TextEdit::new(""),
            archived: false,
            mode: Mode::default(),
            collapsed_projects: HashSet::new(),
            menu: None,
            edit: None,
            value: TextEdit::new(""),
            store: None,
        }
    }
}

impl Navigation {
    /// Takes the sidebar's mode and collapsed projects from
    /// `$XDG_STATE_HOME/viz/desktop.json` and keeps them there from now on.
    /// A missing or unreadable file leaves the defaults.
    pub fn restore(&mut self) {
        let Ok(paths) = viz_config::Paths::from_env() else {
            return;
        };
        let path = paths.state_dir.join("desktop.json");
        if let Some(saved) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        {
            self.mode = match saved.get("sidebar").and_then(Value::as_str) {
                Some("projects") => Mode::Projects,
                _ => Mode::Recent,
            };
            self.collapsed_projects = saved
                .get("collapsedProjects")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
        }
        self.store = Some(path);
    }

    /// Writes what `restore` reads. Failing to is harmless: the next launch
    /// starts from the defaults.
    fn persist(&self) {
        let Some(path) = &self.store else {
            return;
        };
        let mut collapsed: Vec<_> = self.collapsed_projects.iter().collect();
        collapsed.sort_unstable();
        let saved = json!({ "sidebar": self.mode.name(), "collapsedProjects": collapsed });
        if let Some(dir) = path.parent() {
            _ = std::fs::create_dir_all(dir);
        }
        _ = std::fs::write(path, saved.to_string());
    }

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
        if self.collapsed_projects.remove(project) {
            self.persist();
        }
    }

    fn toggle_project(&mut self, id: &str) {
        if !self.collapsed_projects.remove(id) {
            self.collapsed_projects.insert(id.to_owned());
        }
        self.persist();
    }

    fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.persist();
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
    /// Every listed chat but the pinned ones, newest first.
    recent: Vec<&'a ConversationSummary>,
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
            recent: Vec::new(),
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
            } else {
                outline.recent.push(chat);
                if let Some(project) = project {
                    outline.projects.entry(project).or_default().push(chat);
                } else {
                    outline.loose.push(chat);
                }
            }
        }
        let newest = |chats: &mut Vec<&ConversationSummary>| {
            chats.sort_by_key(|chat| std::cmp::Reverse(chat.updated_at));
        };
        newest(&mut outline.pinned);
        newest(&mut outline.recent);
        newest(&mut outline.loose);
        outline.projects.values_mut().for_each(newest);
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

    /// A top-level chat's row. With `detail`, a quiet second line names its
    /// project.
    fn chat(
        &self,
        chat: &ConversationSummary,
        depth: u16,
        detail: bool,
        t: &Theme,
        layer: &mut Vec<El>,
    ) -> El {
        let id = chat.id;
        let s = self.state;
        let selected = self.open == Some(id);
        let key = format!("chat-{id}");
        let menu_open = actions::menu_open(s, &key);
        let rest = ui::status(chat.status, t).or_else(|| {
            chat.unread.then(|| {
                ui::dot(t.accent)
                    .role("status")
                    .label("unread")
                    .help("unread")
            })
        });
        let menu = actions::chat_menu(s, chat, &key, t, layer);
        let place = detail.then(|| {
            chat.project
                .as_deref()
                .and_then(|id| s.projects.iter().find(|p| p.id == id))
                .map_or("no project", |project| project.name.as_str())
        });
        // The runner's mark, in the title's ink.
        let mark = chat.harness.as_deref().map(|harness| {
            let ink = if chat.archived {
                t.faint
            } else if selected {
                t.text
            } else {
                t.muted
            };
            icon(ui::harness_icon(harness), ICON)
                .color(ink)
                .help(harness)
                .a11y_hidden()
        });
        let mut node = TreeLine {
            key: &key,
            label: chat_title(chat),
            detail: place,
            depth,
            strong: chat.unread && !chat.archived,
            dim: chat.archived,
            selected,
        }
        .build(mark, ui::trailing(rest, Some(menu), menu_open), t)
        .on_press_tx(move |s: &mut Desktop, tx| s.select(id, tx));
        if chat.unread {
            node = node.description("unread");
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
        let menu = actions::project_menu(s, project, &key, t, layer);
        let mut node = TreeLine {
            key: &key,
            label: &project.name,
            detail: None,
            depth: 0,
            strong: false,
            dim: false,
            selected: false,
        }
        .build(Some(leading), ui::trailing(None, Some(menu), menu_open), t)
        .expanded(open)
        .help(&project.path)
        .on_press(move |s: &mut Desktop| s.navigation.toggle_project(&id));
        if s.connected() {
            node = node.on_secondary_press(move |s: &mut Desktop, at, _| {
                s.navigation.menu = Some(actions::Context::at(&key, at));
            });
        }
        // The project and its chats share one hairline box, which is the
        // nesting: chats keep the project row's columns (their marks under its
        // folder) instead of stepping in, and only their tree level says
        // they're inside. The inset plus a row's radius makes the box's radius,
        // so a highlighted row's corners run parallel to it.
        let mut branch = col()
            .key(format!("project-branch-{}", project.id))
            .w_full()
            .padding(PROJECT_BOX_INSET)
            .radius(Size::Default.radius() + PROJECT_BOX_INSET + HAIRLINE)
            .border(HAIRLINE, t.hairline)
            .child(node);
        if let Some(chats) = self.projects.get(project.id.as_str()).filter(|_| open) {
            branch = branch.children(
                chats
                    .iter()
                    .map(|chat| self.chat(chat, 0, false, t, layer).level(2)),
            );
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
        .label("workspace")
        .inset(0.0)
        .header_gap(S8)
        .header_padding(0.0, 0.0);
    if let Some(orchestrator) = orchestrator(s, t) {
        bar = bar.header(orchestrator);
    }
    bar = bar.header(filter(s, t)).child(tree);
    if let Some(footer) = footer(s, t) {
        bar = bar.footer(footer);
    }
    bar.build(&t.k)
}

/// Mounts sidebar dialogs independently of whether the sidebar is visible.
pub fn overlays(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    actions::dialog(s, t, layer);
}

/// Opens the add-project dialog from the sidebar or palette.
pub fn open_add_project(s: &mut Desktop, tx: &mut Tx) {
    actions::add_project(s, tx);
}

/// Opens the confirmation for deleting a chat from the command palette.
pub fn open_delete_chat(s: &mut Desktop, id: ConversationId, tx: &mut Tx) {
    actions::delete_chat(s, id, tx);
}

/// A chat's context menu with its Move to project submenu open, in a preview
/// scene.
pub fn preview_context_menu(s: &mut Desktop, id: ConversationId, x: f32, y: f32) {
    s.navigation.menu = Some(actions::Context::preview(&format!("chat-{id}"), x, y));
}

/// Shows the delete confirmation for `id` in a preview scene.
pub fn preview_delete(s: &mut Desktop, id: ConversationId) {
    s.navigation.edit = Some(actions::Edit::DeleteChat(id));
}

/// Lists chats under their projects in a preview scene.
pub const fn preview_projects(s: &mut Desktop) {
    s.navigation.mode = Mode::Projects;
}

/// The sidebar header's buttons while it's open: add project, then new chat,
/// before the sidebar toggle rune-kit places.
pub fn header_buttons(s: &Desktop, t: &Theme) -> El {
    let add = ui::icon_button(
        &ui::FOLDER_PLUS,
        "add project",
        IconButtonVariant::GhostMuted,
        Size::Default,
    )
    .disabled(!s.connected())
    .on_press_tx(open_add_project)
    .build(&t.k)
    .key("add-project");
    row()
        .key("sidebar-header-buttons")
        .gap(theme::S4)
        .child(add)
        .child(new_chat_button(s, t))
}

/// New chat, as an icon button: in the sidebar's header row while the sidebar
/// is open, after the sidebar toggle in the toolbar while it's closed.
pub fn new_chat_button(s: &Desktop, t: &Theme) -> El {
    ui::icon_button(
        &ui::COMPOSE,
        "new chat",
        IconButtonVariant::GhostMuted,
        Size::Default,
    )
    .shortcut(format!("{}+N", ui::COMMAND_KEY))
    .disabled(!s.connected() || s.new_chat.is_some())
    .on_press_tx(Desktop::create_here)
    .build(&t.k)
    .key("new-chat")
}

/// Where [`header_buttons`] sit, from the window's top left: one gap before
/// the sidebar toggle, which rune-kit pins to the header's trailing end.
pub fn header_origin(s: &Desktop) -> (f32, f32) {
    let side = Size::Default.height();
    (
        s.sidebar.width() - SIDEBAR_PAD - 3.0f32.mul_add(side, 2.0 * theme::S4),
        (TOOLBAR_HEIGHT - side) / 2.0,
    )
}

/// The orchestrator: its own row above the tree, never pinned or nested.
fn orchestrator(s: &Desktop, t: &Theme) -> Option<El> {
    let root = s.root.and_then(|id| s.summary(id))?;
    let id = root.id;
    let selected = s.selected == Some(id);
    let (detail, color) = match root.status {
        ConversationStatus::Working => ("working".to_owned(), t.muted),
        ConversationStatus::Failed => ("last run failed".to_owned(), t.danger),
        ConversationStatus::Idle => (
            theme::ago(root.updated_at)
                .map_or_else(|| "idle".to_owned(), |ago| format!("idle · {ago}")),
            t.faint,
        ),
    };
    let rest = ui::status(root.status, t).or_else(|| root.unread.then(|| ui::dot(t.accent)));
    Some(
        row()
            .key("orchestrator")
            .role("link")
            .label("orchestrator")
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
                        text("orchestrator")
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
        "filter chats",
        "filter chats",
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
    let recent = s.navigation.mode == Mode::Recent;
    let mut tree = col()
        .key("conversation-tree")
        .role("tree")
        .label("conversations")
        .w_full();
    if !outline.pinned.is_empty() {
        tree = tree
            .child(ui::section_row("pinned-heading", "pinned", None, t))
            .children(
                outline
                    .pinned
                    .iter()
                    .map(|chat| outline.chat(chat, 0, recent, t, layer)),
            );
    }
    let archived = s.navigation.archived;
    let show_archived = ui::icon_button(
        &ui::ARCHIVE,
        if archived {
            "hide archived"
        } else {
            "show archived"
        },
        IconButtonVariant::GhostMuted,
        Size::Small,
    )
    .expanded(archived)
    .on_press(|s: &mut Desktop| s.navigation.archived = !s.navigation.archived)
    .build(&t.k)
    .key("archived");
    tree = tree.child(mode_switch(s.navigation.mode, show_archived, t));
    if recent {
        return tree.children(
            outline
                .recent
                .iter()
                .map(|chat| outline.chat(chat, 0, true, t, layer)),
        );
    }
    tree = tree.child(
        col().key("project-boxes").w_full().gap(theme::S4).children(
            s.projects
                .iter()
                .filter(|p| outline.visible_project(p))
                .map(|p| outline.project(p, t, layer)),
        ),
    );
    if !outline.loose.is_empty() {
        tree = tree
            .child(ui::section_row("chats-heading", "no project", None, t))
            .children(
                outline
                    .loose
                    .iter()
                    .map(|chat| outline.chat(chat, 0, false, t, layer)),
            );
    }
    tree
}

/// The section row that picks how chats are listed, with the archived toggle
/// in its trailing slot.
fn mode_switch(current: Mode, archived: El, t: &Theme) -> El {
    let tab = |mode: Mode, label: &'static str| {
        let active = mode == current;
        ui::section(label, t)
            .key(mode.name())
            .role("radio")
            .label(label)
            .checked(active)
            .color(if active { t.text } else { t.faint })
            .hover_group()
            .group_color(if active { t.text } else { t.muted })
            .focusable(true)
            .focus_ring(HAIRLINE, theme::S2, t.accent)
            .on_press(move |s: &mut Desktop| s.navigation.set_mode(mode))
    };
    row()
        .key("sidebar-mode")
        .role("radiogroup")
        .label("list chats by")
        .w_full()
        .h(theme::TREE_ROW)
        .mt(theme::S12)
        .pl(ROW_INSET)
        .gap(theme::S12)
        .center_y()
        .child(tab(Mode::Recent, "recent"))
        .child(tab(Mode::Projects, "projects"))
        .child(el().grow(1.0).a11y_hidden())
        .child(ui::trailing(None, Some(archived), true))
}

/// The connection while it's down (a dot and one word, the detail in its
/// tooltip) and a retry while offline. A healthy connection says nothing, so
/// there's no footer (and no rule above it) then.
fn footer(s: &Desktop, t: &Theme) -> Option<El> {
    let (color, label) = match &s.status {
        Status::Connecting => (t.warning, "connecting…"),
        Status::Connected { .. } => return None,
        Status::Error(_) => (t.danger, "disconnected"),
    };
    let detail = match (&s.status, &s.endpoint) {
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
            quiet("retry")
                .on_press_tx(Desktop::reconnect)
                .build(&t.k)
                .key("retry"),
        );
    }
    Some(col().w_full().gap(S8).child(ui::rule(t)).child(footer))
}
