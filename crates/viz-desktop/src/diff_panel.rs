//! Docked right-side diff review panel and per-turn diff state.
//!
//! Renders unified or split diffs from `thread.diff` and `run.diff` using
//! `rune_kit::diff::DiffView`. On wide windows (`>= NARROW_BREAKPOINT`), it
//! docks beside the reading pane with a draggable left divider; on narrow
//! windows it floats as a right-aligned overlay sheet so the reading column
//! stays usable.

use std::collections::{HashMap, HashSet};

use rune::{Color, Cursor, El, Phase, ScrollSpec, Shadow, Tx, col, el, icon, row, text};
use rune_kit::{
    ButtonVariant, IconButtonVariant,
    diff::{DiffMode, DiffState, DiffView, parse},
};
use viz_client::protocol::{GitChange, RunId};

use crate::{
    app::Desktop,
    theme::{
        HAIRLINE, ICON_SMALL, OVERLAY_BLUR, RADIUS_INNER, S2, S4, S6, S8, S12, Size,
        TOOLBAR_HEIGHT, Theme,
    },
    ui,
};

/// Minimum window width where the diff panel docks side-by-side instead of
/// floating as a right-aligned sheet.
pub const NARROW_BREAKPOINT: f32 = 1040.0;
/// Default docked diff panel width, in points.
const DEFAULT_WIDTH: f32 = 480.0;
/// Narrowest docked diff panel width, in points.
const MIN_WIDTH: f32 = 360.0;
/// Keep enough reading-pane space for the composer controls.
const MIN_READING_WIDTH: f32 = 420.0;
/// Widest docked diff panel width, in points.
const MAX_WIDTH: f32 = 640.0;
/// Width of the draggable left resize hit target.
const RESIZE_HIT: f32 = 5.0;

/// Which diff snapshot the panel is displaying.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiffScope {
    /// Whole-thread diff (`thread.diff`): first to latest completed checkpoint.
    #[default]
    Thread,
    /// Uncommitted changes in the thread's checkout.
    WorkingTree,
    /// Single settled run diff (`run.diff`).
    Run {
        /// Wire run identifier.
        run: RunId,
        /// 1-based settled turn number for display.
        turn: usize,
    },
}

impl DiffScope {
    pub const fn checkout(working_tree: bool) -> Self {
        if working_tree {
            Self::WorkingTree
        } else {
            Self::Thread
        }
    }
}

/// One changed path in a summary or diff response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    /// Path relative to the repository root.
    pub path: String,
    /// Kind of change reported by Git.
    pub change: GitChange,
}

/// Loading state of the diff panel's active scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DiffStatus {
    /// Nothing requested yet.
    #[default]
    Idle,
    /// Waiting on `thread.diff` or `run.diff`.
    Loading,
    /// Diff arrived (may contain zero files).
    Ready,
    /// Request failed.
    Error(String),
}

/// State for the right-side diff review panel and cached per-turn summaries.
#[derive(Debug)]
pub struct DiffPanel {
    /// Whether the panel is open.
    pub open: bool,
    /// Scope currently loaded or loading in the panel.
    pub scope: DiffScope,
    turn_scope: Option<(RunId, usize)>,
    /// Docked panel width in points.
    pub width: f32,
    /// Current load state.
    pub status: DiffStatus,
    /// File summary for the active scope.
    pub files: Vec<DiffFile>,
    /// Virtualized diff view state.
    pub view: DiffState,
    /// Cached per-run file summaries for inline `"Changed N files"` chips.
    pub run_summaries: HashMap<RunId, Vec<DiffFile>>,
    /// Runs whose file summary is currently in flight.
    pub loading_summaries: HashSet<RunId>,
    /// File path to scroll to once the active scope finishes loading.
    pending_file: Option<String>,
    /// Width at the start of an active resize drag.
    drag_start: f32,
}

impl Default for DiffPanel {
    fn default() -> Self {
        Self {
            open: false,
            scope: DiffScope::Thread,
            turn_scope: None,
            width: DEFAULT_WIDTH,
            status: DiffStatus::Idle,
            files: Vec::new(),
            view: DiffState::default(),
            run_summaries: HashMap::new(),
            loading_summaries: HashSet::new(),
            pending_file: None,
            drag_start: DEFAULT_WIDTH,
        }
    }
}

impl DiffPanel {
    /// Clears thread-specific diff state when switching threads.
    pub fn reset(&mut self) {
        self.open = false;
        self.scope = DiffScope::Thread;
        self.turn_scope = None;
        self.status = DiffStatus::Idle;
        self.files.clear();
        self.view.set_files(Vec::new());
        self.run_summaries.clear();
        self.loading_summaries.clear();
        self.pending_file = None;
    }

    /// Marks `scope` as loading and optionally remembers a file path to reveal.
    pub fn begin_load(&mut self, scope: DiffScope, jump_file: Option<String>) {
        self.open = true;
        self.scope = scope;
        if let DiffScope::Run { run, turn } = scope {
            self.turn_scope = Some((run, turn));
        }
        self.files.clear();
        self.view.set_files(Vec::new());
        self.status = DiffStatus::Loading;
        self.pending_file = jump_file;
    }

    /// Applies a loaded diff response for `scope`.
    pub fn apply_diff(
        &mut self,
        scope: DiffScope,
        files: Vec<DiffFile>,
        patch: Option<&str>,
        tx: &mut Tx,
    ) {
        if self.scope != scope {
            return;
        }
        let parsed = patch.map_or_default(parse);
        self.view.set_files(parsed);
        self.files = files;
        self.status = DiffStatus::Ready;
        if let Some(target) = self.pending_file.take()
            && let Some(index) = self.view.file_index(&target)
        {
            self.view.scroll_to_file(index, tx);
        }
    }

    /// Applies a failed diff request for `scope`.
    pub fn apply_error(&mut self, scope: DiffScope, message: String) {
        if self.scope == scope {
            self.status = DiffStatus::Error(message);
            self.pending_file = None;
        }
    }

    /// Stores a loaded per-run summary for inline `"Changed N files"` chips.
    pub fn apply_run_summary(&mut self, run: RunId, files: Vec<DiffFile>) {
        self.loading_summaries.remove(&run);
        self.run_summaries.insert(run, files);
    }

    /// Clears a failed per-run summary load so the user can retry.
    pub fn clear_run_summary_loading(&mut self, run: RunId) {
        self.loading_summaries.remove(&run);
    }

    /// Populates the panel with preview data for headless scenes.
    pub fn set_preview(&mut self, scope: DiffScope, files: Vec<DiffFile>, patch: &str) {
        self.begin_load(scope, None);
        self.files = files;
        self.view.set_files(parse(patch));
        self.status = DiffStatus::Ready;
    }

    /// Total additions and deletions across all parsed files.
    pub fn stats(&self) -> (usize, usize) {
        self.view.files().iter().fold((0, 0), |(add, del), file| {
            (add + file.additions(), del + file.deletions())
        })
    }
}

/// Short single-letter badge and semantic color for a Git change kind.
pub const fn change_badge(change: GitChange, t: &Theme) -> (&'static str, Color) {
    match change {
        GitChange::Added => ("A", t.success),
        GitChange::Deleted => ("D", t.danger),
        GitChange::Modified => ("M", t.warning),
        GitChange::TypeChanged => ("T", t.worktree),
    }
}

fn max_docked_width(s: &Desktop) -> f32 {
    (s.window_width - s.sidebar.shown_width() - MIN_READING_WIDTH).min(MAX_WIDTH)
}

/// Width that a docked diff panel takes out of the reading pane.
pub fn docked_width(s: &Desktop) -> Option<f32> {
    let max = max_docked_width(s);
    (s.diff.open && s.window_width >= NARROW_BREAKPOINT && max >= MIN_WIDTH)
        .then(|| s.diff.width.clamp(MIN_WIDTH, max))
}

/// Docked right-side diff panel when the window is wide enough.
pub fn docked(s: &Desktop, t: &Theme) -> Option<El> {
    let width = docked_width(s)?;
    let handle = el()
        .key("diff-panel-resize")
        .w(RESIZE_HIT)
        .h_full()
        .absolute(0.0, 0.0)
        .cursor(Cursor::ResizeColumn)
        .on_drag(|s: &mut Desktop, drag, _| match drag.phase {
            Phase::Down => s.diff.drag_start = docked_width(s).unwrap_or(s.diff.width),
            Phase::Move => {
                s.diff.width = (s.diff.drag_start - drag.dx)
                    .clamp(MIN_WIDTH, max_docked_width(s).max(MIN_WIDTH));
            }
            Phase::Up => {}
        });
    Some(
        el().key("diff-panel-docked")
            .w(width)
            .h_full()
            .min_h(0.0)
            .shrink()
            .fill(t.k.c.bg_base)
            .child(panel_content(s, t).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
            .child(
                el().key("diff-panel-divider")
                    .w(HAIRLINE)
                    .h_full()
                    .absolute(0.0, 0.0)
                    .fill(t.hairline)
                    .a11y_hidden(),
            )
            .child(handle),
    )
}

/// Floating right-aligned overlay sheet for narrow windows.
pub fn sheet(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    if !s.diff.open || docked_width(s).is_some() {
        return;
    }
    let width = (s.window_width - 48.0).clamp(MIN_WIDTH, DEFAULT_WIDTH);
    let panel = el()
        .key("diff-panel-sheet")
        .role("dialog")
        .label("Changes")
        .w(width)
        .h_full()
        .min_h(0.0)
        .fill(t.overlay_glass)
        .backdrop_blur(OVERLAY_BLUR)
        .border(HAIRLINE, t.hairline)
        .box_shadow(Shadow {
            x: -6.0,
            y: 0.0,
            blur: 24.0,
            spread: 0.0,
            color: Color::BLACK.alpha(if t.k.dark { 0.36 } else { 0.14 }),
            inset: false,
        })
        .modal(true, |s: &mut Desktop| s.diff.open = false)
        .child(panel_content(s, t).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)));
    layer.push(
        el().key("diff-panel-scrim")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(t.overlay.c.scrim)
            .a11y_hidden(),
    );
    layer.push(
        row()
            .key("diff-panel-sheet-layer")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .justify_end()
            .child(panel),
    );
}

fn panel_content(s: &Desktop, t: &Theme) -> El {
    let mut column = col()
        .key("diff-panel-content")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(header_bar(s, t))
        .child(ui::rule(t).key("diff-panel-header-rule"));
    if let Some(strip) = file_strip(s, t) {
        column = column
            .child(strip)
            .child(ui::rule(t).key("diff-panel-files-rule"));
    }
    column.child(body(s, t))
}

fn header_bar(s: &Desktop, t: &Theme) -> El {
    let (add, del) = s.diff.stats();
    let mode = s.diff.view.mode();
    let scope = s.diff.scope;
    let mut tabs = row().key("diff-scope-tabs").gap(S8).center_y().min_w(0.0);
    if let Some((run, turn)) = s.diff.turn_scope {
        tabs = tabs.child(scope_tab(
            "scope-run",
            &format!("Turn #{turn}"),
            scope == DiffScope::Run { run, turn },
            move |s: &mut Desktop, tx| s.open_run_diff(run, turn, None, tx),
            t,
        ));
    }
    tabs = tabs
        .child(scope_tab(
            "scope-thread",
            "Thread",
            scope == DiffScope::Thread,
            Desktop::open_thread_diff,
            t,
        ))
        .child(scope_tab(
            "scope-working-tree",
            "Working tree",
            scope == DiffScope::WorkingTree,
            Desktop::open_working_tree_diff,
            t,
        ));
    let stats = (add > 0 || del > 0).then(|| {
        row()
            .key("diff-stats")
            .gap(S4)
            .center_y()
            .children(
                (add > 0).then(|| text(format!("+{add}")).font(t.mono_micro).color(t.success)),
            )
            .children((del > 0).then(|| text(format!("-{del}")).font(t.mono_micro).color(t.danger)))
    });
    let mode_toggle = row()
        .key("diff-mode-toggle")
        .gap(S2)
        .center_y()
        .child(
            ui::icon_button(
                &ui::ROWS_2,
                "Unified diff",
                IconButtonVariant::GhostMuted,
                Size::Small,
            )
            .expanded(mode == DiffMode::Unified)
            .on_press(|s: &mut Desktop| s.diff.view.set_mode(DiffMode::Unified))
            .build(&t.k)
            .key("diff-unified"),
        )
        .child(
            ui::icon_button(
                &ui::COLUMNS_2,
                "Split diff",
                IconButtonVariant::GhostMuted,
                Size::Small,
            )
            .expanded(mode == DiffMode::Split)
            .on_press(|s: &mut Desktop| s.diff.view.set_mode(DiffMode::Split))
            .build(&t.k)
            .key("diff-split"),
        );
    let close = ui::icon_button(
        &ui::CLOSE,
        "Close diff panel",
        IconButtonVariant::GhostMuted,
        Size::Small,
    )
    .on_press(|s: &mut Desktop| s.diff.open = false)
    .build(&t.k)
    .key("diff-close");
    row()
        .key("diff-panel-header")
        .w_full()
        .h(TOOLBAR_HEIGHT)
        .px(S12)
        .gap(S8)
        .center_y()
        .child(tabs)
        .child(el().grow(1.0).min_w(0.0).a11y_hidden())
        .children(stats)
        .child(mode_toggle)
        .child(close)
}

fn scope_tab(
    key: &str,
    label: &str,
    active: bool,
    on_press: impl Fn(&mut Desktop, &mut Tx) + 'static,
    t: &Theme,
) -> El {
    let mut tab = col()
        .key(key.to_owned())
        .role("tab")
        .label(label.to_owned())
        .checked(active)
        .h(TOOLBAR_HEIGHT)
        .justify_center()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .on_press_tx(on_press)
        .child(
            text(label.to_owned())
                .font(if active { t.ui_medium } else { t.ui })
                .color(if active { t.text } else { t.muted })
                .group_color(t.text)
                .a11y_hidden(),
        );
    if active {
        tab = tab.child(
            el().key("diff-tab-rail")
                .pin(Some(0.0), None, Some(0.0), Some(0.0))
                .h(S2)
                .radius(HAIRLINE)
                .fill(t.accent)
                .a11y_hidden(),
        );
    }
    tab
}

fn file_strip(s: &Desktop, t: &Theme) -> Option<El> {
    let parsed = s.diff.view.files();
    if parsed.len() <= 1 {
        return None;
    }
    let visible = s.diff.view.visible_file();
    let chips = parsed.iter().enumerate().map(|(index, file)| {
        let path = file.path();
        let name = path.rsplit('/').next().unwrap_or(path);
        let active = index == visible;
        let change = s
            .diff
            .files
            .iter()
            .find(|entry| entry.path == path)
            .map_or(GitChange::Modified, |entry| entry.change);
        let (letter, color) = change_badge(change, t);
        row()
            .key(format!("diff-jump-{index}"))
            .role("button")
            .label(format!("Jump to {path}"))
            .help(path.to_owned())
            .h(Size::Small.height())
            .px(S6)
            .gap(S4)
            .center_y()
            .radius(RADIUS_INNER)
            .continuous_corners()
            .fill(if active { t.accent_veil } else { t.chip_fill })
            .border(
                HAIRLINE,
                if active {
                    t.accent.alpha(0.35)
                } else {
                    t.hairline
                },
            )
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .hover_group()
            .press_scale(0.97)
            .on_press_tx(move |s: &mut Desktop, tx| s.diff.view.scroll_to_file(index, tx))
            .child(text(letter).font(t.mono_micro).color(color).a11y_hidden())
            .child(
                text(name.to_owned())
                    .font(t.mono_small)
                    .color(if active { t.text } else { t.muted })
                    .group_color(t.text)
                    .a11y_hidden(),
            )
    });
    Some(
        row()
            .key("diff-file-strip")
            .w_full()
            .max_h(96.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(
                row()
                    .w_full()
                    .px(S12)
                    .py(S6)
                    .gap(S6)
                    .flex_wrap()
                    .children(chips),
            ),
    )
}

fn body(s: &Desktop, t: &Theme) -> El {
    match &s.diff.status {
        DiffStatus::Idle | DiffStatus::Loading => col()
            .key("diff-loading")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .center()
            .gap(S8)
            .child(ui::radar(ICON_SMALL, t.accent, t))
            .child(text("Loading changes…").font(t.small).color(t.muted)),
        DiffStatus::Error(message) => {
            let scope = s.diff.scope;
            col()
                .key("diff-error")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .center()
                .px(S12)
                .gap(S8)
                .child(text("Couldn’t load diff").font(t.ui_medium).color(t.text))
                .child(
                    text(message.clone())
                        .font(t.small)
                        .color(t.muted)
                        .wrap_anywhere()
                        .align(rune::Align::Center),
                )
                .child(
                    ui::button("Retry", ButtonVariant::Outline, Size::Small)
                        .on_press_tx(move |s: &mut Desktop, tx| match scope {
                            DiffScope::Thread => s.open_thread_diff(tx),
                            DiffScope::WorkingTree => s.open_working_tree_diff(tx),
                            DiffScope::Run { run, turn } => s.open_run_diff(run, turn, None, tx),
                        })
                        .build(&t.k),
                )
        }
        DiffStatus::Ready if s.diff.view.files().is_empty() => col()
            .key("diff-empty")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .center()
            .gap(S6)
            .child(
                icon(&ui::FILE_DIFF, ICON_SMALL)
                    .color(t.faint)
                    .a11y_hidden(),
            )
            .child(
                text(if s.diff.scope == DiffScope::WorkingTree {
                    "Working tree is clean"
                } else {
                    "No recorded file changes"
                })
                .font(t.ui)
                .color(t.muted),
            ),
        DiffStatus::Ready => DiffView::new(
            |s: &Desktop| &s.diff.view,
            |s: &mut Desktop| &mut s.diff.view,
        )
        .key("thread-diff-view")
        .build(s, &t.k),
    }
}
