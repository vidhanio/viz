//! Lucide UI icons, monochrome harness brand marks, and thinking-effort
//! glyphs.
//!
//! Every icon draws from Lucide's 24pt grid or a bundled monochrome brand mark
//! in `icons/`, tinted exclusively via base16 semantic tokens.

use rune::{El, Icon, icon};

use crate::theme::{ICON, Theme};

/// Declares Lucide icons as Rune icons on Lucide's 24-point grid.
macro_rules! lucide {
    ($($(#[$doc:meta])* $name:ident = $icon:ident;)*) => {$(
        $(#[$doc])*
        pub static $name: Icon = Icon {
            name: concat!("lucide-", stringify!($icon)),
            view: 24.0,
            body: lucide_static_svg::Icon::$icon.svg_str(),
        };
    )*};
}

lucide! {
    /// New thread.
    COMPOSE = SquarePen;
    /// Send message.
    ARROW_UP = ArrowUp;
    /// Scroll or move down.
    ARROW_DOWN = ArrowDown;
    /// Navigate back.
    ARROW_LEFT = ArrowLeft;
    /// Navigate forward.
    /// Steer or send immediately into an active turn.
    CORNER_UP_RIGHT = CornerUpRight;
    /// Fast mode.
    ZAP = Zap;
    /// Flame project icon.
    FLAME = Flame;
    /// Delegated work or workflow graph.
    AGENT = Workflow;
    /// Thinking / reasoning indicator.
    BRAIN = Brain;
    /// Collapsed disclosure chevron.
    CHEVRON = ChevronRight;
    /// Back disclosure chevron.
    /// Open / dropdown chevron.
    CHEVRON_OPEN = ChevronDown;
    /// Upward disclosure chevron.
    CHEVRON_UP = ChevronUp;
    /// Overflow actions menu.
    MORE = Ellipsis;
    /// Project directory.
    FOLDER = Folder;
    /// Open directory.
    FOLDER_OPEN = FolderOpen;
    /// Code project icon option.
    FOLDER_CODE = FolderCode;
    /// Git repository project icon option.
    FOLDER_GIT2 = FolderGit2;
    /// Directory tree view.
    FOLDER_TREE = FolderTree;
    /// Add project.
    FOLDER_PLUS = FolderPlus;
    /// Git checkout or branch.
    GIT_BRANCH = GitBranch;
    /// New Git worktree.
    GIT_BRANCH_PLUS = GitBranchPlus;
    /// Git commit.
    GIT_COMMIT = GitCommitHorizontal;
    /// Git merge / merged pull request.
    GIT_MERGE = GitMerge;
    /// Fork / delegated branch.
    GIT_FORK = GitFork;
    /// Open pull request.
    PR_OPEN = GitPullRequest;
    /// Draft pull request.
    PR_DRAFT = GitPullRequestDraft;
    /// Closed pull request.
    PR_CLOSED = GitPullRequestClosed;
    /// Add item or thread.
    PLUS = Plus;
    /// Local environment indicator.
    LAPTOP = Laptop;
    /// Remote environment indicator.
    SERVER = Server;
    /// Selector / switcher double chevron.
    CHEVRONS_UP_DOWN = ChevronsUpDown;
    /// Collapse all disclosure chevron pair.
    /// Thread / conversation icon.
    THREAD = MessageSquare;
    /// No-project / unassigned conversation icon.
    MESSAGE_SQUARE_DASHED = MessageSquareDashed;
    /// Stop active run.
    STOP = Square;
    /// Toggle left sidebar.
    SIDEBAR = PanelLeft;
    /// Open thread details popover.
    SQUARE_MENU = SquareMenu;
    /// Pin thread.
    PIN = Pin;
    /// Unpin thread.
    PIN_OFF = PinOff;
    /// Archive / settle thread.
    ARCHIVE = Archive;
    /// Delete item.
    TRASH = Trash;
    /// Attach file.
    PAPERCLIP = Paperclip;
    /// Confirmation checkmark.
    CHECK = Check;
    /// Completed status check circle.
    CIRCLE_CHECK = CircleCheck;
    /// Dismiss / clear.
    CLOSE = X;
    /// Failed status circle X.
    CIRCLE_X = CircleX;
    /// Error / alert circle.
    CIRCLE_ALERT = CircleExclamationPoint;
    /// Warning triangle.
    TRIANGLE_ALERT = TriangleExclamationPoint;
    /// Informational badge or notice.
    INFO = Info;
    /// Copy to clipboard.
    COPY = Copy;
    /// Search input leading icon.
    SEARCH = Search;
    /// Filter menu trigger.
    LIST_FILTER = ListFilter;
    /// Lock / restricted permissions.
    LOCK = Lock;
    /// Unlocked / open permissions.
    UNLOCK = LockOpen;
    /// Reveal value.
    EYE = Eye;
    /// Hide value.
    EYE_OFF = EyeOff;
    /// Dash / minus.
    MINUS = Minus;
    /// Shell / terminal command.
    TERMINAL = Terminal;
    /// Generic file.
    FILE = File;
    /// File read operation.
    FILE_READ = FileText;
    /// File creation operation.
    /// File edit operation.
    FILE_EDIT = FilePenLine;
    /// Diff file summary.
    FILE_DIFF = FileDiff;
    /// File search operation.
    /// Content grep search operation.
    /// Generic tool call.
    WRENCH = Wrench;
    /// Elapsed timer.
    TIMER = Timer;
    /// Snooze / timestamp clock.
    CLOCK = Clock;
    /// Rename / edit title.
    PENCIL = Pencil;
    /// Refresh / resync.
    REFRESH_CW = RefreshCw;
    /// Unread indicator.
    MAIL = Mail;
    /// Read indicator.
    MAIL_OPEN = MailOpen;
    /// Settings dialog.
    SETTINGS = Settings;
    /// Options / harness sliders.
    /// Unified diff view.
    /// Split diff view.
    COLUMNS2 = Columns2;
    /// Queue follow-up message.
    LIST_PLUS = ListPlus;
    /// Run / execute.
    /// Retry / rerun clockwise.
    ROTATE_CW = RotateCw;
    /// Undo / revert counter-clockwise.
    ROTATE_CCW = RotateCcw;
    /// Compact / minimize view.
    MINIMIZE2 = Minimize2;
    /// Permissions shield.
    SHIELD = Shield;
    /// Supervised / verified shield.
    SHIELD_CHECK = ShieldCheck;
    /// Full access / elevated shield.
    /// Skill / model highlight sparkles.
    SPARKLES = Sparkles;
    /// Favourite model star.
    STAR = Star;
    /// Subagent / assistant bot.
    BOT = Bot;
    /// Task list / checklist.
    /// Redo / retry step.
    REDO2 = Redo2;
    /// Reply / quote.
    REPLY = Reply;
    /// Download / pull.
    DOWNLOAD = Download;
    /// Open external link.
    EXTERNAL_LINK = ExternalLink;
    /// Push branch to remote.
    /// Upload / stage changes.
    UPLOAD = Upload;
    /// Soft wrap toggle.
    TEXT_WRAP = TextWrap;
    /// Whitespace / formatting markers.
    /// Empty / no tests mark.
    /// Plan step not started yet.
    /// Active plan step.
    CIRCLE_DOT = CircleDot;
    /// Skipped / cancelled step.
    /// Pending / draft circle.
    CIRCLE_DASHED = CircleDashed;
    /// Hard failure octagon.
    /// Approved by user.
    /// User avatar / author.
    /// Rejected by user.
    /// Link / reference.
    LINK = Link;
    /// Linked worktree / issue.
    /// Unlinked / detached.
    /// Stacked layers / worktrees.
    LAYERS = Layers;
    /// Appearance / display monitor.
    MONITOR = Monitor;
    /// Theme palette.
    /// Model / compute processor.
    CPU = Cpu;
    /// Run that ended early.
    CIRCLE_STOP = CircleStop;
    /// Delegated task result arriving.
    INBOX = Inbox;
    /// Fetching a page or remote URL.
    GLOBE = Globe;
    /// Harness without a known brand mark.
    HARNESS = SquareTerminal;
}

/// Filled favourite star: Lucide's `Star` outline filled with `currentColor`.
pub static STAR_FILLED: Icon = Icon {
    name: "viz-star-filled",
    view: 24.0,
    body: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 0 0 1.597-1.16z"/></svg>"#,
};

/// Declares a harness's monochrome brand mark from `icons/`, drawn in its
/// element's colour (`currentColor`) at `view` units square.
macro_rules! mark {
    ($($(#[$doc:meta])* $name:ident = $file:literal, $view:literal;)*) => {$(
        $(#[$doc])*
        pub static $name: Icon = Icon {
            name: concat!("harness-", $file),
            view: $view,
            body: include_str!(concat!("../../icons/", $file, ".svg")),
        };
    )*};
}

mark! {
    /// Anthropic's Claude starburst (`@lobehub/icons-static-svg`).
    CLAUDE = "claude", 24.0;
    /// The Codex mark (`@lobehub/icons-static-svg`).
    CODEX = "codex", 24.0;
    /// pi's block mark (`pi.dev/favicon.svg`), inset to Lucide's margin.
    PI = "pi", 700.0;
    /// oh-my-pi's pi mark (`can1357/oh-my-pi`).
    OMP = "omp", 52.0;
    /// Google's monochrome product icon (`antigravity.google/press/`).
    ANTIGRAVITY = "antigravity", 180.0;
}

/// A harness's mark, recognized from the first word of its config key.
/// Unknown keys fall back to [`HARNESS`].
pub fn harness_icon(id: &str) -> &'static Icon {
    let word = id
        .split(|c: char| !c.is_ascii_alphanumeric())
        .next()
        .unwrap_or_default();
    let is = |names: &[&str]| names.iter().any(|known| word.eq_ignore_ascii_case(known));
    if is(&["claude", "anthropic"]) {
        &CLAUDE
    } else if is(&["codex", "openai"]) {
        &CODEX
    } else if is(&["omp"]) {
        &OMP
    } else if is(&["pi"]) {
        &PI
    } else if is(&["agy", "antigravity"]) {
        &ANTIGRAVITY
    } else {
        &HARNESS
    }
}

/// A flat monochrome harness mark at [`ICON`] size with an optional tooltip.
pub fn harness_mark(harness: Option<(&str, &str)>, active: bool, t: &Theme) -> El {
    let glyph = icon(harness.map_or(&HARNESS, |(id, _)| harness_icon(id)), ICON).color(if active {
        t.text
    } else {
        t.muted
    });
    match harness {
        Some((_, name)) => glyph.help(name),
        None => glyph.a11y_hidden(),
    }
}

/// A flat monochrome harness mark at an explicit `size` with a tooltip.
pub fn harness_glyph(id: &str, name: &str, size: f32, active: bool, t: &Theme) -> El {
    icon(harness_icon(id), size)
        .color(if active { t.text } else { t.muted })
        .help(name)
}

/// Maps `rune-kit`'s internal glyphs onto Lucide icons.
pub fn kit_glyphs(glyph: &mut rune_kit::Glyphs) {
    glyph.check = &CHECK;
    glyph.check_small = &CHECK;
    glyph.chevron_down = &CHEVRON_OPEN;
    glyph.chevron_up = &CHEVRON_UP;
    glyph.caret_right = &CHEVRON;
    glyph.triangle_down = &CHEVRON_OPEN;
    glyph.triangle_right = &CHEVRON;
    glyph.close = &CLOSE;
    glyph.clear = &CIRCLE_X;
    glyph.copy = &COPY;
    glyph.search = &SEARCH;
    glyph.lock = &LOCK;
    glyph.eye = &EYE;
    glyph.eye_off = &EYE_OFF;
    glyph.circle_x = &CIRCLE_X;
    glyph.dash = &MINUS;
}

/// Swaps `rune-kit`'s shell sidebar icon for Lucide's [`SIDEBAR`] glyph.
pub fn swap_kit_icon(icon: &Icon) -> Option<&'static Icon> {
    (icon.name == rune_kit::shell::SIDEBAR.name).then_some(&SIDEBAR)
}
