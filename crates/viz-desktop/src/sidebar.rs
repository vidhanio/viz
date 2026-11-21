//! The sidebar: pinned threads, other threads either newest first (Recent) or
//! under their projects (Projects), delegated task subtrees under their parent
//! threads, and a footer for connection status and environment switching.

mod actions;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use rune::{Color, El, LineCap, Shape, TextEdit, Tx, canvas, col, el, icon, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant, Menu, MenuItem, Placement, shell::Sidebar};
use serde_json::{Value, json};
use viz_client::protocol::{
    EnvironmentStatus, Project, TaskStatus, ThreadId, ThreadStatus, ThreadSummary,
};

use crate::{
    app::{Desktop, Status, thread_title},
    theme::{
        self, HAIRLINE, ICON, ICON_SMALL, RADIUS_CONTROL, RADIUS_INNER, ROW_INSET, S2, S4, S6, S8,
        S12, Size, TREE_ROW_CHILD, TREE_STEP, Theme,
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
    collapsed_parents: HashSet<ThreadId>,
    env_menu: bool,
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
            collapsed_parents: HashSet::new(),
            env_menu: false,
            menu: None,
            edit: None,
            value: TextEdit::new(""),
            store: None,
        }
    }
}

impl Navigation {
    pub(crate) fn dismiss_overlays(&mut self) {
        self.env_menu = false;
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

    fn toggle_parent(&mut self, id: ThreadId) {
        if !self.collapsed_parents.remove(&id) {
            self.collapsed_parents.insert(id);
        }
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

/// Top-level threads and their delegated child trees, filtered once per frame.
struct Outline<'a> {
    state: &'a Desktop,
    /// Exact open thread (parent or delegated child).
    open: Option<ThreadId>,
    projects: HashMap<&'a str, Vec<&'a ThreadSummary>>,
    loose: Vec<&'a ThreadSummary>,
    pinned: Vec<&'a ThreadSummary>,
    /// Every listed thread but the pinned ones, newest first.
    recent: Vec<&'a ThreadSummary>,
    /// Local delegated child threads indexed by their parent thread id.
    children: HashMap<ThreadId, Vec<&'a ThreadSummary>>,
    query: String,
    now: i64,
}

impl<'a> Outline<'a> {
    fn new(state: &'a Desktop) -> Self {
        let query = state.navigation.filter.text.trim().to_lowercase();
        let mut outline = Self {
            state,
            open: state.selected,
            projects: HashMap::new(),
            loose: Vec::new(),
            pinned: Vec::new(),
            recent: Vec::new(),
            children: HashMap::new(),
            query,
            now: theme::now_ms(),
        };
        let known_projects: HashSet<_> = state.projects.iter().map(|p| p.path.as_str()).collect();
        let matching_projects: HashSet<_> = state
            .projects
            .iter()
            .filter(|project| outline.matches(&project.name) || outline.matches(&project.path))
            .map(|project| project.path.as_str())
            .collect();
        let mut child_ids = HashSet::new();
        for thread in &state.threads {
            if thread.archived && !state.navigation.archived {
                continue;
            }
            if let Some(parent) = thread.parent.filter(|&parent| {
                thread
                    .parent_environment
                    .as_ref()
                    .is_none_or(|env| Some(env) == state.environment.as_ref())
                    && parent < thread.id
                    && state.threads.iter().any(|other| other.id == parent)
            }) {
                child_ids.insert(thread.id);
                outline.children.entry(parent).or_default().push(thread);
            }
        }
        for kids in outline.children.values_mut() {
            kids.sort_by_key(|thread| {
                (
                    thread.status != ThreadStatus::Working,
                    std::cmp::Reverse(thread.updated_at),
                )
            });
        }
        for thread in &state.threads {
            if child_ids.contains(&thread.id) || (thread.archived && !state.navigation.archived) {
                continue;
            }
            let project = thread
                .project
                .as_deref()
                .filter(|id| known_projects.contains(id));
            if !outline.matches(thread_title(thread))
                && !project.is_some_and(|id| matching_projects.contains(id))
                && !outline.descendant_matches(thread.id)
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

    fn descendant_matches(&self, parent: ThreadId) -> bool {
        let Some(kids) = self.children.get(&parent) else {
            return false;
        };
        kids.iter()
            .any(|child| self.matches(thread_title(child)) || self.descendant_matches(child.id))
    }

    fn has_active_or_selected_child(&self, parent: ThreadId) -> bool {
        let Some(kids) = self.children.get(&parent) else {
            return false;
        };
        kids.iter().any(|child| {
            child.status == ThreadStatus::Working
                || self.open == Some(child.id)
                || self.has_active_or_selected_child(child.id)
        })
    }

    fn visible_project(&self, project: &Project) -> bool {
        self.projects.contains_key(project.path.as_str())
            || self.matches(&project.name)
            || self.matches(&project.path)
    }

    /// A top-level thread's row (and any expanded delegated children), its
    /// title at rest in `ink`. With `detail`, a quiet second line names its
    /// project.
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
        let kids = self.children.get(&id).map_or(&[][..], Vec::as_slice);
        let auto_open = !self.query.is_empty() || self.has_active_or_selected_child(id);
        let show_kids =
            !kids.is_empty() && (auto_open || !s.navigation.collapsed_parents.contains(&id));
        let rest = self.thread_rest_slot(thread, selected, kids.len(), show_kids, t);
        let menu = actions::thread_menu(s, thread, &key, t, layer);
        let place = detail.then(|| {
            thread
                .project
                .as_deref()
                .and_then(|id| s.projects.iter().find(|p| p.path == id))
                .map_or("No project", |project| project.name.as_str())
        });
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
                .group_color(if thread.archived { t.muted } else { t.text })
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
        if kids.is_empty() {
            return node;
        }
        let mut branch = col()
            .key(format!("thread-branch-{id}"))
            .w_full()
            .child(node);
        if show_kids {
            let active_tree = self.has_active_or_selected_child(id);
            branch = branch.children(self.child_rows(kids, 1, active_tree, t));
        }
        branch
    }

    fn thread_rest_slot(
        &self,
        thread: &ThreadSummary,
        selected: bool,
        kid_count: usize,
        show_kids: bool,
        t: &Theme,
    ) -> Option<El> {
        let id = thread.id;
        ui::status(thread.status, selected, t)
            .or_else(|| {
                thread.unread.then(|| {
                    ui::dot(t.accent)
                        .role("status")
                        .label("Unread")
                        .help("Unread")
                })
            })
            .or_else(|| {
                (kid_count > 0 && !show_kids).then(|| {
                    row()
                        .key("child-count")
                        .role("button")
                        .label(format!("Show {kid_count} delegated tasks"))
                        .help("Expand delegated tasks")
                        .px(S4)
                        .h(18.0)
                        .center()
                        .radius(RADIUS_INNER)
                        .on_press(move |s: &mut Desktop| s.navigation.toggle_parent(id))
                        .child(
                            text(kid_count.to_string())
                                .font(t.mono_micro)
                                .color(t.delegated),
                        )
                })
            })
            .or_else(|| {
                theme::relative_age(thread.updated_at, self.now)
                    .map(|age| text(age).font(t.mono_micro).color(t.faint).a11y_hidden())
            })
    }

    fn child_rows(
        &self,
        kids: &[&'a ThreadSummary],
        depth: u16,
        active_tree: bool,
        t: &Theme,
    ) -> Vec<El> {
        let mut out = Vec::new();
        let last_index = kids.len().saturating_sub(1);
        for (index, child) in kids.iter().enumerate() {
            let last = index == last_index;
            out.push(self.child_row(child, depth, last, active_tree, t));
            if let Some(grandchildren) = self.children.get(&child.id) {
                let grand_active = self.has_active_or_selected_child(child.id);
                let show_grand = !self.query.is_empty()
                    || grand_active
                    || !self.state.navigation.collapsed_parents.contains(&child.id);
                if show_grand {
                    out.extend(self.child_rows(grandchildren, depth + 1, grand_active, t));
                }
            }
        }
        out
    }

    fn child_row(
        &self,
        child: &ThreadSummary,
        depth: u16,
        last: bool,
        active_tree: bool,
        t: &Theme,
    ) -> El {
        let id = child.id;
        let selected = self.open == Some(id);
        let elbow = child_guide(last, active_tree || selected, t);
        let task_status = self
            .state
            .tasks
            .iter()
            .find(|task| task.thread == Some(id))
            .map(|task| task.status);
        let status_mark = match (child.status, task_status) {
            (ThreadStatus::Working, _) | (_, Some(TaskStatus::Running)) => Some(ui::radar(
                11.0,
                if selected { t.accent } else { t.muted },
                t,
            )),
            (ThreadStatus::Failed, _) | (_, Some(TaskStatus::Failed)) => Some(ui::dot(t.danger)),
            (_, Some(TaskStatus::Completed)) => Some(ui::dot(t.success)),
            (_, Some(TaskStatus::Cancelled)) => Some(ui::dot(t.faint)),
            _ => None,
        };
        let title = thread_title(child);
        let indent = f32::from(depth.saturating_sub(1)).mul_add(TREE_STEP, ROW_INSET);
        row()
            .key(format!("thread-{id}"))
            .role("treeitem")
            .label(title)
            .level(u32::from(depth) + 1)
            .w_full()
            .h(TREE_ROW_CHILD)
            .pl(indent)
            .pr(S6)
            .gap(S6)
            .center_y()
            .radius(RADIUS_CONTROL)
            .fill(if selected {
                t.selected
            } else {
                Color::TRANSPARENT
            })
            .hover_group()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx| s.select(id, tx))
            .child(elbow)
            .child(
                icon(ui::harness_icon(&child.harness), ICON_SMALL + 1.0)
                    .color(if selected { t.text } else { t.faint })
                    .group_color(t.muted)
                    .a11y_hidden(),
            )
            .child(
                text(title)
                    .font(if selected { t.small_medium } else { t.small })
                    .color(if selected { t.text } else { t.muted })
                    .group_color(t.text)
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .truncate()
                    .help_when_truncated(title)
                    .a11y_hidden(),
            )
            .children(status_mark.map(|mark| el().size(ICON, ICON).center().child(mark)))
    }

    fn project(&self, project: &Project, t: &Theme, layer: &mut Vec<El>) -> El {
        let s = self.state;
        let open =
            !self.query.is_empty() || !s.navigation.collapsed_projects.contains(&project.path);
        let id = project.path.clone();
        let new_id = project.path.clone();
        let key = format!("project-{id}");
        let menu_open = actions::menu_open(s, &key);
        let leading = if open {
            let layer_of = |glyph: &'static rune::Icon, rest: f32, hovered: f32| {
                el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .center()
                    .group_opacity(rest, hovered)
                    .child(icon(glyph, ICON).color(t.muted).group_color(t.text))
            };
            el().size(ICON, ICON)
                .child(layer_of(&ui::FOLDER, 1.0, 0.0))
                .child(layer_of(&ui::CHEVRON_OPEN, 0.0, 1.0))
        } else {
            el().size(ICON, ICON)
                .center()
                .child(icon(&ui::CHEVRON, ICON).color(t.faint).group_color(t.text))
        };
        let add_thread = ui::icon_button(
            &ui::PLUS,
            &format!("New thread in {}", project.name),
            IconButtonVariant::GhostMuted,
            Size::Small,
        )
        .disabled(!s.connected() || s.new_thread.is_some())
        .on_press_tx(move |s: &mut Desktop, tx| s.create_in_project(&new_id, tx))
        .build(&t.k)
        .key("project-new-thread");
        let menu = actions::project_menu(s, project, &key, t, layer);
        let trailing_actions = row()
            .gap(S2)
            .center_y()
            .group_opacity(if menu_open { 1.0 } else { 0.0 }, 1.0)
            .child(add_thread)
            .child(menu);
        let mut node = TreeLine {
            key: &key,
            label: &project.name,
            detail: None,
            depth: 0,
            strong: true,
            dim: false,
            selected: false,
            ink: t.text,
        }
        .build(Some(leading), trailing_actions, t)
        .expanded(open)
        .help(&project.path)
        .on_press(move |s: &mut Desktop| s.navigation.toggle_project(&id));
        if s.connected() {
            node = node.on_secondary_press(move |s: &mut Desktop, at, _| {
                s.navigation.menu = Some(actions::Context::at(&key, at));
            });
        }
        let mut branch = col()
            .key(format!("project-branch-{}", project.path))
            .w_full()
            .child(node);
        if let Some(threads) = self.projects.get(project.path.as_str()).filter(|_| open) {
            branch = branch.children(
                threads
                    .iter()
                    .map(|thread| self.thread(thread, t.muted, false, t, layer).level(2)),
            );
        }
        branch
    }
}

/// Vector L-elbow (last child) or T-junction (middle child) tree guide.
fn child_guide(last: bool, highlighted: bool, t: &Theme) -> El {
    let guide = if highlighted {
        t.delegated.alpha(0.5)
    } else {
        t.hairline
    };
    let shape = if last {
        Shape::new().stroke(
            vec![
                [7.0, 0.0],
                [7.0, TREE_ROW_CHILD / 2.0],
                [TREE_STEP - S4, TREE_ROW_CHILD / 2.0],
            ],
            HAIRLINE,
            guide,
        )
    } else {
        Shape::new()
            .line(
                [7.0, 0.0],
                [7.0, TREE_ROW_CHILD],
                HAIRLINE,
                LineCap::Round,
                guide,
            )
            .line(
                [7.0, TREE_ROW_CHILD / 2.0],
                [TREE_STEP - S4, TREE_ROW_CHILD / 2.0],
                HAIRLINE,
                LineCap::Round,
                guide,
            )
    };
    canvas(shape).size(TREE_STEP, TREE_ROW_CHILD).a11y_hidden()
}

/// Where `id`'s top-level thread is listed in the sidebar, counting from the
/// top, for which way a switch between two threads moves; `None` while the
/// filter hides it.
pub fn position(s: &Desktop, id: ThreadId) -> Option<usize> {
    let top = top_level(&s.threads, id, s.environment.as_deref());
    let outline = Outline::new(s);
    let listed: Vec<&ThreadSummary> = match s.navigation.mode {
        Mode::Recent => outline.recent.clone(),
        Mode::Projects => s
            .projects
            .iter()
            .filter_map(|project| outline.projects.get(project.path.as_str()))
            .flatten()
            .chain(&outline.loose)
            .copied()
            .collect(),
    };
    outline
        .pinned
        .iter()
        .chain(&listed)
        .position(|thread| thread.id == top)
}

fn open_match(s: &mut Desktop, tx: &mut Tx) {
    let outline = Outline::new(s);
    let id = match s.navigation.mode {
        Mode::Recent => outline
            .pinned
            .iter()
            .chain(&outline.recent)
            .next()
            .map(|thread| thread.id),
        Mode::Projects => outline
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
            .map(|thread| thread.id),
    };
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
        .header_padding(0.0, 0.0)
        .header(filter(s, t))
        .child(tree);
    if let Some(footer) = footer(s, t, layer) {
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

/// Opens the environment picker menu in a preview scene.
pub const fn preview_environment_menu(s: &mut Desktop) {
    s.navigation.env_menu = true;
}

/// Lists threads under their projects in a preview scene.
pub fn preview_projects(s: &mut Desktop, collapsed: &[&str]) {
    s.navigation.mode = Mode::Projects;
    for path in collapsed {
        s.navigation.collapsed_projects.insert((*path).to_owned());
    }
}

/// The leading sidebar actions. Rune-kit puts the sidebar toggle at the other
/// end.
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
    row()
        .key("sidebar-header-buttons")
        .gap(theme::S4)
        .child(new_thread_button(s, t))
        .child(add)
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
    .shortcut(ui::shortcut("N"))
    .disabled(!s.connected() || s.new_thread.is_some())
    .on_press_tx(Desktop::create_here)
    .build(&t.k)
    .key("new-thread")
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
        if outline.pinned.is_empty() && outline.recent.is_empty() {
            return tree.children(empty_note(s, &outline.query, t));
        }
        return tree.children(
            outline
                .recent
                .iter()
                .map(|thread| outline.thread(thread, t.muted, true, t, layer)),
        );
    }
    let projects: Vec<_> = s
        .projects
        .iter()
        .filter(|p| outline.visible_project(p))
        .collect();
    if !projects.is_empty() {
        tree = tree.child(
            col()
                .key("project-groups")
                .w_full()
                .gap(S12)
                .children(projects.iter().map(|p| outline.project(p, t, layer))),
        );
    }
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
    if outline.pinned.is_empty() && projects.is_empty() && outline.loose.is_empty() {
        tree = tree.children(empty_note(s, &outline.query, t));
    }
    tree
}

fn empty_note(s: &Desktop, query: &str, t: &Theme) -> Option<El> {
    let label = if !query.is_empty() {
        "No matching threads"
    } else if s.connected() {
        "No threads yet"
    } else {
        return None;
    };
    Some(
        text(label)
            .key("sidebar-empty")
            .role("status")
            .font(t.small)
            .color(t.faint)
            .px(ROW_INSET)
            .py(S8),
    )
}

/// The section row that picks how threads are listed, with the archived toggle
/// in its trailing slot.
fn mode_switch(current: Mode, archived: El, t: &Theme) -> El {
    let tab = |mode: Mode, label: &'static str| {
        let active = mode == current;
        row()
            .key(mode.name())
            .role("radio")
            .label(label)
            .checked(active)
            .h(Size::Small.height())
            .px(S8)
            .center()
            .radius(Size::Small.radius())
            .fill(if active {
                t.selected
            } else {
                Color::TRANSPARENT
            })
            .hover_group()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press(move |s: &mut Desktop| s.navigation.set_mode(mode))
            .child(
                ui::section(label, t)
                    .color(if active { t.text } else { t.muted })
                    .group_color(t.text)
                    .a11y_hidden(),
            )
    };
    row()
        .key("sidebar-mode")
        .role("radiogroup")
        .label("List threads by")
        .w_full()
        .h(theme::TREE_ROW)
        .mt(theme::S12)
        .mb(theme::S4)
        .gap(theme::S4)
        .center_y()
        .child(tab(Mode::Recent, "Recent"))
        .child(tab(Mode::Projects, "Projects"))
        .child(el().grow(1.0).a11y_hidden())
        .child(ui::trailing(None, Some(archived), true))
}

/// The quiet footer row: environment picker when more than one environment is
/// configured, merged with connection status. A healthy single-environment
/// connection shows no footer; Retry is omitted when the main pane already
/// offers it.
fn footer(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    let multi = s.environments.len() > 1;
    let main_has_retry = s.thread.items.is_empty() && s.thread.before.is_none() && !s.busy();
    let show_retry = matches!(s.status, Status::Error(_)) && !main_has_retry;
    let content = if multi {
        environment_footer(s, t, layer)
    } else {
        status_footer(s, t)?
    };
    let mut row = row()
        .key("connection")
        .w_full()
        .h(Size::Default.height())
        .gap(S8)
        .center_y()
        .child(content);
    if show_retry {
        row = row.child(
            ui::button("Retry", ButtonVariant::GhostMuted, Size::Small)
                .on_press_tx(Desktop::reconnect)
                .build(&t.k)
                .key("retry"),
        );
    }
    Some(col().w_full().gap(S8).child(ui::rule(t)).child(row))
}

fn status_footer(s: &Desktop, t: &Theme) -> Option<El> {
    let (color, label) = match &s.status {
        Status::Connecting => (t.warning, "Connecting…"),
        Status::Connected { .. } => return None,
        Status::Error(_) => (t.danger, "Offline"),
    };
    let detail = match (&s.status, &s.endpoint) {
        (Status::Error(message), _) => message.clone(),
        (_, Some(endpoint)) => endpoint.clone(),
        _ => String::new(),
    };
    Some(
        row()
            .role("status")
            .label(label)
            .help(detail)
            .pl(ROW_INSET)
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
    )
}

fn environment_footer(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let open = s.navigation.env_menu;
    let current = s
        .environment
        .as_deref()
        .or_else(|| s.environments.first().map(|env| env.id.as_str()))
        .unwrap_or("local");
    let active_env = s.environments.iter().find(|env| env.id == current);
    let (color, note, help) = match &s.status {
        Status::Connecting => (
            t.warning,
            Some("Connecting…"),
            s.endpoint.as_deref().unwrap_or("Connecting…"),
        ),
        Status::Error(message) => (t.danger, Some("Offline"), message.as_str()),
        Status::Connected { .. } => match active_env.map(|env| env.status) {
            Some(EnvironmentStatus::Unreachable) => (
                t.faint,
                Some("Unreachable"),
                active_env
                    .and_then(|env| env.url.as_deref())
                    .unwrap_or("Unreachable"),
            ),
            Some(EnvironmentStatus::Reachable) => (
                t.success,
                None,
                active_env
                    .and_then(|env| env.url.as_deref())
                    .unwrap_or("Reachable"),
            ),
            Some(EnvironmentStatus::Local) | None => (t.success, None, "Local"),
        },
    };
    let machine = match active_env.map(|env| env.status) {
        Some(EnvironmentStatus::Local) | None => &ui::LAPTOP,
        Some(EnvironmentStatus::Reachable | EnvironmentStatus::Unreachable) => &ui::SERVER,
    };
    let trigger = row()
        .key("environment-picker-trigger")
        .role("combobox")
        .label("Environment")
        .a11y_value(current)
        .help(help)
        .expanded(open)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .w_full()
        .h(Size::Default.height())
        .px(ROW_INSET)
        .gap(S6)
        .center_y()
        .radius(RADIUS_CONTROL)
        .on_press(|s: &mut Desktop| s.navigation.env_menu = !s.navigation.env_menu)
        .child(ui::dot(color))
        .child(
            icon(machine, ICON_SMALL + 1.0)
                .color(if open { t.text } else { t.muted })
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text(current)
                .font(t.ui_medium)
                .color(t.text)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate()
                .a11y_hidden(),
        )
        .children(note.map(|note| text(note).font(t.small).color(t.faint).a11y_hidden()))
        .child(
            icon(&ui::CHEVRONS_UP_DOWN, ICON_SMALL)
                .color(if open { t.text } else { t.faint })
                .group_color(t.text)
                .a11y_hidden(),
        );
    let mut menu = Menu::new("environment-picker")
        .placement(Placement::TopStart)
        .min_width(ui::MENU_WIDTH)
        .open(open)
        .on_close(|s: &mut Desktop| s.navigation.env_menu = false)
        .trigger(trigger);
    if open {
        menu = ui::menu_items(menu, environment_items(s, t));
    }
    menu.build(&t.overlay, layer).grow(1.0).shrink().min_w(0.0)
}

fn environment_items<'a>(s: &'a Desktop, t: &'a Theme) -> impl Iterator<Item = MenuItem> + 'a {
    s.environments.iter().map(move |env| {
        let id = env.id.clone();
        let selected = s.environment.as_deref() == Some(env.id.as_str());
        let (status_label, dot_color) = match env.status {
            EnvironmentStatus::Local => ("Local", t.success),
            EnvironmentStatus::Reachable => ("Reachable", t.success),
            EnvironmentStatus::Unreachable => ("Unreachable", t.faint),
        };
        let mut item = MenuItem::new(&env.id)
            .lead(ui::dot(dot_color))
            .checked(selected)
            .on_select_tx(move |s: &mut Desktop, tx| {
                s.navigation.env_menu = false;
                s.open_in_environment(id.clone(), None, tx);
            });
        if let Some(url) = &env.url {
            item = item.description(url);
        } else {
            item = item.description(status_label);
        }
        item
    })
}
