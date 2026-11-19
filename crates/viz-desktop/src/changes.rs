//! The selected conversation's Git inspector and its RPC requests.

mod dialog;

pub use dialog::{
    branch_dialog, commit_dialog, commit_result, confirm_revert, overlays, pr_dialog, pr_result,
};
use rune::{Color, El, ScrollSpec, Tx, col, el, icon, row, text};
use rune_kit::{
    ButtonVariant, IconButtonVariant, SegmentedControl,
    diff::{self, DiffState, DiffView, FoldRow},
    shell::PaneState,
};
use viz_client::protocol::{
    Branches, ConversationId, DiffFile, DiffFileStatus, DiffScope, EntryId,
};

use crate::{
    app::Desktop,
    backend::Command,
    theme::{
        HAIRLINE, ICON_SMALL, PANEL_INSET, S4, S8, S16, S32, S40, Size, TOOLBAR_HEIGHT, TREE_ROW,
        Theme,
    },
    ui,
};

const SCOPES: [DiffScope; 3] = [DiffScope::Worktree, DiffScope::Turn, DiffScope::Branch];
const DIFF_KEY: &str = "changes-diff";
/// The changed-file list shows this many rows before it scrolls.
const FILE_ROWS: f32 = 5.0;
/// Where a pressable row's fill starts inside the panel; its text starts at
/// `PANEL_INSET`.
const ROW_EDGE: f32 = PANEL_INSET - S8;

/// Git commands sent to the backend for the selected conversation.
#[derive(Debug)]
pub enum Request {
    Diff {
        conversation: ConversationId,
        scope: DiffScope,
        turn: Option<EntryId>,
        request: u64,
    },
    Branches(ConversationId),
    Checkout {
        conversation: ConversationId,
        branch: String,
        create: bool,
    },
    Commit {
        conversation: ConversationId,
        message: Option<String>,
    },
    Push(ConversationId),
    Pr {
        conversation: ConversationId,
        title: Option<String>,
        body: Option<String>,
        draft: bool,
    },
    Revert {
        conversation: ConversationId,
        turn: EntryId,
    },
}

/// Window-local review state. The split pane stays where the user left it
/// across chats.
#[derive(Debug)]
pub struct Changes {
    pub pane: PaneState,
    scope: DiffScope,
    turn: Option<EntryId>,
    next_request: u64,
    pending: Option<u64>,
    files: Result<Vec<DiffFile>, String>,
    active: usize,
    unified: DiffState,
    dialog: dialog::State,
}

impl Default for Changes {
    fn default() -> Self {
        let mut unified = DiffState::default();
        unified.set_wrap(true);
        Self {
            pane: PaneState::new(432.0).range(280.0, 760.0).collapsed(),
            scope: DiffScope::Worktree,
            turn: None,
            next_request: 0,
            pending: None,
            files: Ok(Vec::new()),
            active: 0,
            unified,
            dialog: dialog::State::default(),
        }
    }
}

impl Changes {
    /// Drop the old conversation's data without disturbing the split or reusing
    /// a request id.
    pub fn reset(&mut self) {
        *self = Self {
            pane: self.pane,
            next_request: self.next_request,
            ..Self::default()
        };
    }

    /// Apply the latest diff response and keep the selected file if it still
    /// exists.
    pub fn set_diff(&mut self, request: u64, result: Result<Vec<DiffFile>, String>) {
        if self.pending != Some(request) {
            return;
        }
        self.pending = None;
        let previous = self.active_file();
        let active = previous
            .and_then(|previous| {
                result
                    .as_ref()
                    .ok()?
                    .iter()
                    .position(|file| file.path == previous.path)
            })
            .unwrap_or_default();
        let replacement = result.as_ref().ok().and_then(|files| files.get(active));
        let same_patch =
            replacement.map(|file| file.patch.as_str()) == previous.map(|file| file.patch.as_str());
        self.active = active;
        self.files = result;
        if !same_patch {
            self.update_patch();
        }
    }

    /// Supply the branch picker with the repository's local branches, or its
    /// load error.
    pub fn set_branches(&mut self, result: Result<Branches, String>) {
        self.dialog.set_branches(result);
    }

    fn active_file(&self) -> Option<&DiffFile> {
        self.files.as_ref().ok()?.get(self.active)
    }

    fn clear_diff(&mut self) {
        self.pending = None;
        self.files = Ok(Vec::new());
        self.active = 0;
        if !self.unified.files().is_empty() {
            self.unified.set_files(Vec::new());
        }
    }

    fn update_patch(&mut self) {
        // Parse only the file being read; large repositories can return many
        // patches.
        let files = self
            .active_file()
            .map_or_else(Vec::new, |file| diff::parse(&file.patch));
        self.unified.set_files(files);
    }
}

fn target(s: &Desktop) -> Option<ConversationId> {
    if s.connected()
        && s.conversation
            .as_ref()
            .is_some_and(|state| state.git.is_some())
    {
        s.selected
    } else {
        None
    }
}

/// Reload the active tab when Git state changes or a Git mutation succeeds.
pub fn refresh(s: &mut Desktop, tx: &mut Tx) {
    if !s.changes.pane.is_open() {
        return;
    }
    let Some(conversation) = target(s) else {
        s.changes.clear_diff();
        return;
    };
    let request = s.changes.next_request;
    s.changes.next_request += 1;
    let scope = s.changes.scope;
    let turn = if scope == DiffScope::Turn {
        s.changes.turn
    } else {
        None
    };
    let command = Command::Git(Request::Diff {
        conversation,
        scope,
        turn,
        request,
    });
    s.changes.pending = Some(request);
    if !s.send(command, tx) {
        s.changes
            .set_diff(request, Err("The backend is offline.".into()));
    }
}

/// Toggle the right pane; request its current scope when it opens.
pub fn toggle(s: &mut Desktop, tx: &mut Tx) {
    s.changes.pane.toggle();
    if s.changes.pane.is_open() {
        refresh(s, tx);
    }
}

/// Show the changes since the start of the given timeline turn.
pub fn turn(s: &mut Desktop, tx: &mut Tx, entry: EntryId) {
    s.changes.pane.set_open(true);
    select_scope(s, tx, DiffScope::Turn, Some(entry));
}

/// Push the current branch through the backend.
pub fn push(s: &mut Desktop, tx: &mut Tx) {
    if let Some(conversation) = target(s) {
        s.send(Command::Git(Request::Push(conversation)), tx);
    }
}

fn select_scope(s: &mut Desktop, tx: &mut Tx, scope: DiffScope, turn: Option<EntryId>) {
    if s.changes.scope != scope || s.changes.turn != turn {
        s.changes.scope = scope;
        s.changes.turn = turn;
        s.changes.clear_diff();
    }
    refresh(s, tx);
}

/// Build the right-hand pane for `SplitView`; its dialogs are mounted
/// separately.
pub fn view(s: &Desktop, t: &Theme, _layer: &mut Vec<El>) -> El {
    let scope = s.changes.scope;
    let tabs = SegmentedControl::new(
        &["Changes", "Turn", "Branch"],
        SCOPES.iter().position(|&tab| tab == scope),
    )
    .label("Diff scope")
    .fit()
    .on_pick(|s: &mut Desktop, tab, tx| {
        let turn = s.changes.turn;
        select_scope(s, tx, SCOPES[tab], turn);
    })
    .build(&t.k)
    .key("scope");
    let close = ui::icon_button(
        &ui::CLOSE,
        "Close changes",
        IconButtonVariant::GhostMuted,
        Size::Default,
    )
    .on_press_tx(|s: &mut Desktop, tx| {
        s.changes.pane.set_open(false);
        tx.focus("draft", false);
    })
    .build(&t.k)
    .key("close-changes");
    col()
        .key("changes")
        .w_full()
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.bg)
        .child(
            row()
                .key("changes-tabs")
                .w_full()
                .h(TOOLBAR_HEIGHT)
                .px(PANEL_INSET)
                .center_y()
                .child(tabs)
                .child(el().grow(1.0))
                .child(close),
        )
        .child(ui::rule(t))
        .children(git_bar(s, t).map(|bar| col().w_full().child(bar).child(ui::rule(t))))
        .children(turn_label(scope, s.changes.turn, t))
        .child(content(s, t))
}

/// Branch, sync state and the repository actions, above the diff.
#[expect(
    clippy::too_many_lines,
    reason = "one row: the branch picker, sync counts and three actions"
)]
fn git_bar(s: &Desktop, t: &Theme) -> Option<El> {
    let git = s.conversation.as_ref()?.git.as_ref()?;
    let live = target(s).is_some();
    let branch = git.branch.as_deref().unwrap_or("Detached");
    let mut picker = row()
        .key("git-branch")
        .role("button")
        .label("Switch branch")
        .description(branch)
        .h(Size::Small.height())
        .px(S8)
        .gap(S4)
        .center_y()
        .radius(Size::Small.radius())
        .min_w(0.0)
        .shrink()
        .child(
            icon(&ui::GIT_BRANCH, ICON_SMALL)
                .color(t.muted)
                .a11y_hidden(),
        )
        .child(
            text(branch)
                .font(t.ui)
                .color(t.text)
                .truncate()
                .shrink()
                .min_w(0.0)
                .help_when_truncated(branch)
                .a11y_hidden(),
        )
        .child(
            icon(&ui::CHEVRON_OPEN, ICON_SMALL)
                .color(t.faint)
                .a11y_hidden(),
        );
    if live {
        picker = picker
            .focusable(true)
            .focus_ring(HAIRLINE, HAIRLINE, t.accent)
            .hover(t.hover)
            .on_press_tx(branch_dialog);
    }
    let sync = if git.upstream.is_some() {
        let ink = |count: u64| if count == 0 { t.faint } else { t.muted };
        row()
            .key("git-sync")
            .gap(S4)
            .center_y()
            .help(git.upstream.clone().unwrap_or_default())
            .child(
                text(format!("↑{}", git.ahead))
                    .font(t.mono_small)
                    .color(ink(git.ahead)),
            )
            .child(
                text(format!("↓{}", git.behind))
                    .font(t.mono_small)
                    .color(ink(git.behind)),
            )
    } else {
        row()
            .key("git-sync")
            .child(text("Unpublished").font(t.small).color(t.faint))
    };
    let action = |label: &str, key: &str, enabled: bool, press: fn(&mut Desktop, &mut Tx)| {
        ui::button(label, ButtonVariant::Ghost, Size::Small)
            .disabled(!live || !enabled)
            .on_press_tx(press)
            .build(&t.k)
            .key(key)
    };
    let unpushed = git.upstream.is_none() || git.ahead > 0;
    Some(
        row()
            .key("git-bar")
            .w_full()
            .h(S40)
            .px(ROW_EDGE)
            .gap(S4)
            .center_y()
            .child(picker)
            .child(sync)
            .child(el().grow(1.0))
            .child(action(
                "Commit",
                "git-commit",
                !git.files.is_empty(),
                commit_dialog,
            ))
            .child(action(
                "Push",
                "git-push",
                unpushed && git.branch.is_some(),
                push,
            ))
            .child(action(
                "Create PR",
                "git-pr",
                git.branch.is_some(),
                pr_dialog,
            )),
    )
}

fn turn_label(scope: DiffScope, turn: Option<EntryId>, t: &Theme) -> Option<El> {
    if scope != DiffScope::Turn {
        return None;
    }
    let mut label = row()
        .w_full()
        .h(S32)
        .pl(PANEL_INSET)
        .pr(ROW_EDGE)
        .gap(S4)
        .center_y();
    if let Some(turn) = turn {
        label = label
            .child(
                text(format!("Turn #{}", turn.0))
                    .font(t.small)
                    .color(t.muted),
            )
            .child(el().grow(1.0))
            .child(
                ui::button("Show latest", ButtonVariant::Ghost, Size::Small)
                    .on_press_tx(|s: &mut Desktop, tx| select_scope(s, tx, DiffScope::Turn, None))
                    .build(&t.k),
            );
    } else {
        label = label.child(text("Latest turn").font(t.small).color(t.muted));
    }
    Some(label)
}

fn content(s: &Desktop, t: &Theme) -> El {
    let c = &s.changes;
    match &c.files {
        Ok(files) if !files.is_empty() => file_view(s, files, t),
        _ if c.pending.is_some() && s.connected() => centered(
            row()
                .gap(S8)
                .center_y()
                .child(ui::spinner(t))
                .child(text("Loading changes").font(t.ui).color(t.muted)),
        ),
        Err(error) => centered(ui::empty(
            None,
            "Couldn’t load changes",
            error,
            Some(
                ui::button("Retry", ButtonVariant::Outline, Size::Default)
                    .disabled(!s.connected())
                    .on_press_tx(refresh)
                    .build(&t.k),
            ),
            t,
        )),
        Ok(_) => {
            let (title, detail) = if !s.connected() {
                ("Offline", "Connect to the backend to review changes.")
            } else if s.selected.is_none() {
                ("No chat", "Open a chat to review its changes.")
            } else if s.conversation.is_none() {
                ("Loading", "Waiting for the repository’s state.")
            } else if let Some(state) = &s.conversation
                && state.git.is_none()
            {
                (
                    "No Git repository",
                    "This chat isn’t in a Git working tree.",
                )
            } else {
                match c.scope {
                    DiffScope::Worktree => ("No changes", "The working tree is clean."),
                    DiffScope::Turn => ("No changes", "This turn changed no files."),
                    DiffScope::Branch => ("No changes", "This branch matches its base."),
                }
            };
            centered(ui::empty(None, title, detail, None, t))
        }
    }
}

fn centered(body: El) -> El {
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(S16)
        .center()
        .child(body)
}

fn file_view(s: &Desktop, files: &[DiffFile], t: &Theme) -> El {
    let c = &s.changes;
    let count = files.len();
    let noun = if count == 1 { "file" } else { "files" };
    let (added, deleted) = files.iter().fold((0, 0), |(added, deleted), file| {
        (added + file.additions, deleted + file.deletions)
    });
    let summary = row()
        .key("change-summary")
        .w_full()
        .h(S32)
        .px(PANEL_INSET)
        .center_y()
        .gap(S8)
        .child(
            text(format!("{count} {noun} changed"))
                .font(t.small_medium)
                .color(t.muted),
        )
        .child(el().grow(1.0))
        .children(
            c.pending
                .map(|_| ui::spinner(t).label("Refreshing changes")),
        )
        .child(counts(added, deleted, t));
    let list = col()
        .key("changed-files")
        .role("listbox")
        .label("Changed files")
        .focus_group()
        .w_full()
        .max_h(FILE_ROWS.mul_add(TREE_ROW, S8))
        .px(ROW_EDGE)
        .pb(S8)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .children(
            files
                .iter()
                .enumerate()
                .map(|(i, file)| file_row(file, i, count, i == c.active, t)),
        );
    let diff = if c.unified.files().is_empty() {
        centered(ui::empty(
            None,
            "No text diff",
            "There’s no patch to show for this file.",
            None,
            t,
        ))
    } else {
        DiffView::new(
            |s: &Desktop| &s.changes.unified,
            |s: &mut Desktop| &mut s.changes.unified,
        )
        .key(DIFF_KEY)
        .fold_row(FoldRow::LineInfo)
        .build(s, &t.k)
        .focusable(true)
        .role("group")
        .label("Unified diff")
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_key(|s: &mut Desktop, ev, tx| s.changes.unified.key(ev, tx))
    };
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(summary)
        .child(list)
        .child(ui::rule(t))
        .child(diff)
}

fn file_row(file: &DiffFile, index: usize, count: usize, selected: bool, t: &Theme) -> El {
    let (mark, status, color) = match file.status {
        DiffFileStatus::Modified => ("M", "Modified", t.warning),
        DiffFileStatus::Added => ("A", "Added", t.success),
        DiffFileStatus::Deleted => ("D", "Deleted", t.danger),
        DiffFileStatus::Renamed => ("R", "Renamed", t.accent),
    };
    let path = file.path.as_str();
    let help = file
        .old_path
        .as_deref()
        .map_or_else(|| path.to_owned(), |old| format!("{old} → {path}"));
    row()
        .key(format!("file-{path}"))
        .role("option")
        .label(&help)
        .description(status)
        .position_in_set(index + 1, count)
        .focusable(true)
        .focus_selected(selected)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .w_full()
        .h(TREE_ROW)
        .px(PANEL_INSET - ROW_EDGE)
        .center_y()
        .gap(S8)
        .radius(Size::Default.radius())
        .fill(if selected {
            t.selected
        } else {
            Color::TRANSPARENT
        })
        .hover(if selected { t.selected } else { t.hover })
        .help(help)
        .child(
            text(mark)
                .font(t.mono_small)
                .color(color)
                .w(ICON_SMALL)
                .a11y_hidden(),
        )
        .child(
            text(path)
                .font(t.ui)
                .color(t.text)
                .grow(1.0)
                .shrink()
                .truncate()
                .a11y_hidden(),
        )
        .child(counts(file.additions, file.deletions, t))
        .on_press_tx(move |s: &mut Desktop, tx| {
            if s.changes.active != index {
                s.changes.active = index;
                s.changes.update_patch();
                tx.scroll_to_y("changes-diff/scroll", 0.0);
            }
        })
}

fn counts(added: i64, deleted: i64, t: &Theme) -> El {
    let ink = |value, color| if value == 0 { t.faint } else { color };
    row()
        .gap(S4)
        .center_y()
        .child(
            text(format!("+{added}"))
                .font(t.mono_small)
                .color(ink(added, t.success)),
        )
        .child(
            text(format!("−{deleted}"))
                .font(t.mono_small)
                .color(ink(deleted, t.danger)),
        )
}

/// Populate the inspector with representative Git changes for the desktop
/// scene.
pub fn preview(s: &mut Desktop) {
    use DiffFileStatus::{Added, Modified};

    const ROUTE: &str = "diff --git a/backend/src/routes.ts b/backend/src/routes.ts
--- a/backend/src/routes.ts
+++ b/backend/src/routes.ts
@@ -28,5 +28,7 @@ export function routes(app: App) {
   app.get('/v1/health', health);
-  app.get('/v1/projects', list);
+  app.get('/v1/projects', listProjects);
+  app.get('/v1/projects/:id', getProject);
+  app.post('/v1/projects', createProject);
   app.get('/v1/rpc', upgrade);
   return app;
 }
";
    const GLOSSARY: &str = "diff --git a/docs/glossary.md b/docs/glossary.md
new file mode 100644
--- /dev/null
+++ b/docs/glossary.md
@@ -0,0 +1,4 @@
+# Glossary
+
+- A project groups conversations in one directory.
+- A worktree gives a conversation its own checkout.
";
    let files = [
        ("backend/src/routes.ts", Modified, 3, 1, ROUTE),
        ("docs/glossary.md", Added, 4, 0, GLOSSARY),
    ]
    .into_iter()
    .map(|(path, status, additions, deletions, patch)| DiffFile {
        path: path.into(),
        old_path: None,
        status,
        additions,
        deletions,
        patch: patch.into(),
    })
    .collect();
    s.changes.pane.set_open(true);
    s.changes.files = Ok(files);
    s.changes.active = 0;
    s.changes.update_patch();
}

/// Populate the same review scene with its commit form open, without sending a
/// command.
pub fn preview_commit(s: &mut Desktop) {
    preview(s);
    s.changes.dialog.preview_commit();
}
