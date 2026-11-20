//! The sidebar: environments, pinned threads, and other threads
//! either newest first (Recent) or under their projects (Projects).
//! Subagents live in their parent thread, not here.

mod actions;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use rune::{Color, El, TextEdit, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant, shell::Sidebar};
use serde_json::{Value, json};
use viz_client::protocol::{EnvironmentStatus, Project, ThreadId, ThreadSummary};

use crate::{
    app::{Desktop, Status, thread_title},
    theme::{
        self, HAIRLINE, ICON, PROJECT_BOX_INSET, ROW_INSET, S8, SIDEBAR_PAD, Size, TOOLBAR_HEIGHT,
        Theme,
    },
    ui::{self, TreeLine},
};

/// How the sidebar lists top-level threads.
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
    pub(crate) fn dismiss_overlays(&mut self) {
        self.menu = None;
        self.edit = None;
    }

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

    /// Opens the project holding `id`'s top-level thread.
    pub fn reveal(&mut self, id: ThreadId, summaries: &[ThreadSummary], environment: Option<&str>) {
        let top = top_level(summaries, id, environment);
        if let Some(project) = summaries
            .iter()
            .find(|thread| thread.id == top)
            .and_then(|thread| thread.project.as_deref())
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

/// The topmost parent available in this environment.
fn top_level(summaries: &[ThreadSummary], id: ThreadId, environment: Option<&str>) -> ThreadId {
    let mut top = id;
    while let Some(parent) = summaries
        .iter()
        .find(|thread| thread.id == top)
        .and_then(|thread| {
            thread.parent.filter(|_| {
                thread
                    .parent_environment
                    .as_deref()
                    .is_none_or(|parent| Some(parent) == environment)
            })
        })
        .filter(|parent| *parent < top && summaries.iter().any(|thread| thread.id == *parent))
    {
        top = parent;
    }
    top
}

/// Top-level threads by place, filtered once per frame.
struct Outline<'a> {
    state: &'a Desktop,
    /// A delegated thread highlights its top-level parent.
    open: Option<ThreadId>,
    projects: HashMap<&'a str, Vec<&'a ThreadSummary>>,
    loose: Vec<&'a ThreadSummary>,
    pinned: Vec<&'a ThreadSummary>,
    /// Every listed thread but the pinned ones, newest first.
    recent: Vec<&'a ThreadSummary>,
    query: String,
}

impl<'a> Outline<'a> {
    fn new(state: &'a Desktop) -> Self {
        let query = state.navigation.filter.text.trim().to_lowercase();
        let mut outline = Self {
            state,
            open: state
                .selected
                .map(|id| top_level(&state.threads, id, state.environment.as_deref())),
            projects: HashMap::new(),
            loose: Vec::new(),
            pinned: Vec::new(),
            recent: Vec::new(),
            query,
        };
        let known_projects: HashSet<_> = state.projects.iter().map(|p| p.path.as_str()).collect();
        let matching_projects: HashSet<_> = state
            .projects
            .iter()
            .filter(|project| outline.matches(&project.name) || outline.matches(&project.path))
            .map(|project| project.path.as_str())
            .collect();
        for thread in &state.threads {
            let child = thread.parent.is_some_and(|parent| {
                thread
                    .parent_environment
                    .as_ref()
                    .is_none_or(|env| Some(env) == state.environment.as_ref())
                    && parent < thread.id
                    && state.threads.iter().any(|other| other.id == parent)
            });
            if child || (thread.archived && !state.navigation.archived) {
                continue;
            }
            let project = thread
                .project
                .as_deref()
                .filter(|id| known_projects.contains(id));
            if !outline.matches(thread_title(thread))
                && !project.is_some_and(|id| matching_projects.contains(id))
            {
                continue;
            }
            if thread.pinned {
                outline.pinned.push(thread);
            } else {
                outline.recent.push(thread);
                if let Some(project) = project {
                    outline.projects.entry(project).or_default().push(thread);
                } else {
                    outline.loose.push(thread);
                }
            }
        }
        let newest = |threads: &mut Vec<&ThreadSummary>| {
            threads.sort_by_key(|thread| std::cmp::Reverse(thread.updated_at));
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
        self.projects.contains_key(project.path.as_str())
            || self.matches(&project.name)
            || self.matches(&project.path)
    }

    /// A top-level thread's row, its title at rest in `ink`. With `detail`, a
    /// quiet second line names its project.
    fn thread(
        &self,
        thread: &ThreadSummary,
        ink: Color,
        detail: bool,
        t: &Theme,
        layer: &mut Vec<El>,
    ) -> El {
        let id = thread.id;
        let s = self.state;
        let selected = self.open == Some(id);
        let key = format!("thread-{id}");
        let menu_open = actions::menu_open(s, &key);
        let rest = ui::status(thread.status, t).or_else(|| {
            thread.unread.then(|| {
                ui::dot(t.accent)
                    .role("status")
                    .label("Unread")
                    .help("Unread")
            })
        });
        let menu = actions::thread_menu(s, thread, &key, t, layer);
        let place = detail.then(|| {
            thread
                .project
                .as_deref()
                .and_then(|id| s.projects.iter().find(|p| p.path == id))
                .map_or("No project", |project| project.name.as_str())
        });
        // The runner's mark, in the title's ink.
        let mark = {
            let harness = &thread.harness;
            let ink = if thread.archived {
                t.faint
            } else if selected {
                t.text
            } else {
                ink
            };
            icon(ui::harness_icon(harness), ICON)
                .color(ink)
                .help(harness)
                .a11y_hidden()
        };
        let mut node = TreeLine {
            key: &key,
            label: thread_title(thread),
            detail: place,
            depth: 0,
            strong: thread.unread && !thread.archived,
            dim: thread.archived,
            selected,
            ink,
        }
        .build(Some(mark), ui::trailing(rest, Some(menu), menu_open), t)
        .on_press_tx(move |s: &mut Desktop, tx| s.select(id, tx));
        if thread.unread {
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
        let open =
            !self.query.is_empty() || !s.navigation.collapsed_projects.contains(&project.path);
        let id = project.path.clone();
        let key = format!("project-{id}");
        let menu_open = actions::menu_open(s, &key);
        // The folder, turning into the disclosure under the pointer.
        let layer_of = |glyph: &'static rune::Icon, rest: f32, hovered: f32| {
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .center()
                .group_opacity(rest, hovered)
                .child(icon(glyph, ICON).color(t.text))
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
            ink: t.text,
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
        // The project and its threads share one filled box a step above the
        // sidebar, which is the nesting: threads keep the project row's columns
        // (their marks under its folder) instead of stepping in, and only
        // their tree level says they're inside. The header reads in full ink,
        // its threads a step quieter than loose ones. The inset plus a row's
        // radius makes the box's radius, so a highlighted row's corners run
        // parallel to it.
        let mut branch = col()
            .key(format!("project-branch-{}", project.path))
            .w_full()
            .padding(PROJECT_BOX_INSET)
            .radius(Size::Default.radius() + PROJECT_BOX_INSET)
            .fill(t.group)
            .child(node);
        if let Some(threads) = self.projects.get(project.path.as_str()).filter(|_| open) {
            branch = branch.children(threads.iter().map(|thread| {
                self.thread(thread, t.muted.mix(t.faint, 0.5), false, t, layer)
                    .level(2)
            }));
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
                .get(project.path.as_str())
                .and_then(|threads| threads.first())
        }))
        .chain(&outline.loose)
        .next()
        .map(|thread| thread.id);
    if let Some(id) = id.filter(|_| !outline.query.is_empty()) {
        s.navigation.filter.clear();
        s.select(id, tx);
    }
}

pub fn sidebar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let tree = thread_tree(s, t, layer);
    let mut bar = Sidebar::new()
        .label("Workspace")
        .inset(0.0)
        .header_gap(S8)
        .header_padding(0.0, 0.0);
    if !s.environments.is_empty() {
        bar = bar.header(environments(s, t));
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
    actions::imports(s, t, layer);
}

/// Opens the add-project dialog from the sidebar or palette.
pub fn open_add_project(s: &mut Desktop, tx: &mut Tx) {
    actions::add_project(s, tx);
}

/// Opens the confirmation for deleting a thread from the command palette.
pub fn open_delete_thread(s: &mut Desktop, id: ThreadId, tx: &mut Tx) {
    actions::delete_thread(s, id, tx);
}

/// A thread's context menu with its Move to project submenu open, in a preview
/// scene.
pub fn preview_context_menu(s: &mut Desktop, id: ThreadId, x: f32, y: f32) {
    s.navigation.menu = Some(actions::Context::preview(&format!("thread-{id}"), x, y));
}

/// Shows the delete confirmation for `id` in a preview scene.
pub fn preview_delete(s: &mut Desktop, id: ThreadId) {
    s.navigation.edit = Some(actions::Edit::DeleteThread(id));
}

/// Lists threads under their projects in a preview scene.
pub const fn preview_projects(s: &mut Desktop) {
    s.navigation.mode = Mode::Projects;
}

/// The sidebar header's buttons while it's open: add project, then new thread,
/// before the sidebar toggle rune-kit places.
pub fn header_buttons(s: &Desktop, t: &Theme) -> El {
    let add = ui::icon_button(
        &ui::FOLDER_PLUS,
        "Add project",
        IconButtonVariant::GhostMuted,
        Size::Default,
    )
    .disabled(!s.connected())
    .on_press_tx(open_add_project)
    .build(&t.k)
    .key("add-project");
    let import = ui::icon_button(
        &ui::SEARCH,
        "Import thread",
        IconButtonVariant::GhostMuted,
        Size::Default,
    )
    .disabled(!s.connected() || s.import_scope().is_none())
    .on_press_tx(Desktop::browse_imports)
    .build(&t.k)
    .key("import-thread");
    row()
        .key("sidebar-header-buttons")
        .gap(theme::S4)
        .child(add)
        .child(import)
        .child(new_thread_button(s, t))
}

/// New thread, as an icon button: in the sidebar's header row while the sidebar
/// is open, after the sidebar toggle in the toolbar while it's closed.
pub fn new_thread_button(s: &Desktop, t: &Theme) -> El {
    ui::icon_button(
        &ui::COMPOSE,
        "New thread",
        IconButtonVariant::GhostMuted,
        Size::Default,
    )
    .shortcut(format!("{}+N", ui::COMMAND_KEY))
    .disabled(!s.connected() || s.new_thread.is_some())
    .on_press_tx(Desktop::create_here)
    .build(&t.k)
    .key("new-thread")
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

fn environments(s: &Desktop, t: &Theme) -> El {
    col()
        .key("environments")
        .px(ROW_INSET)
        .gap(theme::S4)
        .child(ui::section("Environments", t))
        .children(s.environments.iter().map(|env| {
            let id = env.id.clone();
            let selected = s.environment.as_ref() == Some(&id);
            let (label, color) = match env.status {
                EnvironmentStatus::Local => ("Local", t.success),
                EnvironmentStatus::Reachable => ("Reachable", t.success),
                EnvironmentStatus::Unreachable => ("Unreachable", t.faint),
            };
            row()
                .key(format!("environment-{id}"))
                .role("button")
                .label(id.clone())
                .description(label)
                .help(env.url.as_deref().unwrap_or(label))
                .focusable(true)
                .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                .radius(Size::Small.radius())
                .h(Size::Small.height())
                .px(theme::S4)
                .gap(S8)
                .center_y()
                .fill(if selected {
                    t.hover
                } else {
                    Color::TRANSPARENT
                })
                .hover(t.hover)
                .child(ui::dot(color))
                .child(
                    text(id.clone())
                        .font(t.small)
                        .color(t.text)
                        .truncate()
                        .shrink(),
                )
                .on_press_tx(move |s: &mut Desktop, tx| s.open_in_environment(id.clone(), None, tx))
        }))
}

fn filter(s: &Desktop, t: &Theme) -> El {
    ui::text_field(
        &s.navigation.filter,
        "Filter threads",
        "Filter threads",
        Size::Default,
    )
    .leading_icon(&ui::SEARCH)
    .on_edit(|s: &mut Desktop| &mut s.navigation.filter)
    .on_submit(open_match)
    .build(&t.k)
    .key("thread-filter")
}

fn thread_tree(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let outline = Outline::new(s);
    let recent = s.navigation.mode == Mode::Recent;
    let mut tree = col()
        .key("thread-tree")
        .role("tree")
        .label("Threads")
        .w_full();
    if !outline.pinned.is_empty() {
        tree = tree
            .child(ui::section_row("pinned-heading", "Pinned", None, t))
            .children(
                outline
                    .pinned
                    .iter()
                    .map(|thread| outline.thread(thread, t.muted, recent, t, layer)),
            );
    }
    let archived = s.navigation.archived;
    let show_archived = ui::icon_button(
        &ui::ARCHIVE,
        if archived {
            "Hide archived threads"
        } else {
            "Show archived threads"
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
                .map(|thread| outline.thread(thread, t.muted, true, t, layer)),
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
            .child(ui::section_row("threads-heading", "No project", None, t))
            .children(
                outline
                    .loose
                    .iter()
                    .map(|thread| outline.thread(thread, t.muted, false, t, layer)),
            );
    }
    tree
}

/// The section row that picks how threads are listed, with the archived toggle
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
        .label("List threads by")
        .w_full()
        .h(theme::TREE_ROW)
        .mt(theme::S12)
        .pl(ROW_INSET)
        .gap(theme::S12)
        .center_y()
        .child(tab(Mode::Recent, "Recent"))
        .child(tab(Mode::Projects, "Projects"))
        .child(el().grow(1.0).a11y_hidden())
        .child(ui::trailing(None, Some(archived), true))
}

/// The connection while it's down (a dot and one word, the detail in its
/// tooltip) and a retry while offline. A healthy connection says nothing, so
/// there's no footer (and no rule above it) then.
fn footer(s: &Desktop, t: &Theme) -> Option<El> {
    let (color, label) = match &s.status {
        Status::Connecting => (t.warning, "Connecting…"),
        Status::Connected { .. } => return None,
        Status::Error(_) => (t.danger, "Disconnected"),
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
            quiet("Retry")
                .on_press_tx(Desktop::reconnect)
                .build(&t.k)
                .key("retry"),
        );
    }
    Some(col().w_full().gap(S8).child(ui::rule(t)).child(footer))
}
