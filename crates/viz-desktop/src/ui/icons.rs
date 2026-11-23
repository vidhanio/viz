//! Lucide UI icons, monochrome harness brand marks, and thinking-effort
//! glyphs.
//!
//! Every icon draws from Lucide's 24pt grid or a bundled monochrome brand mark
//! in `icons/`, tinted exclusively via base16 semantic tokens.

use rune::{El, Icon, el, icon, row};

use crate::theme::{HAIRLINE, ICON, ICON_SMALL, S2, Theme};

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
    ARROW_RIGHT = ArrowRight;
    /// Steer or send immediately into an active turn.
    CORNER_UP_RIGHT = CornerUpRight;
    /// Return to parent directory or thread.
    CORNER_LEFT_UP = CornerLeftUp;
    /// Open delegated task thread or external action.
    ARROW_UP_RIGHT = ArrowUpRight;
    /// Extra-high thinking tier.
    ZAP = Zap;
    /// Maximum thinking tier.
    FLAME = Flame;
    /// Delegated work or workflow graph.
    AGENT = Workflow;
    /// Thinking / reasoning indicator.
    BRAIN = Brain;
    /// Collapsed disclosure chevron.
    CHEVRON = ChevronRight;
    /// Back disclosure chevron.
    CHEVRON_LEFT = ChevronLeft;
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
    CHEVRONS_DOWN_UP = ChevronsDownUp;
    /// Thread / conversation icon.
    THREAD = MessageSquare;
    /// No-project / unassigned conversation icon.
    MESSAGE_SQUARE_DASHED = MessageSquareDashed;
    /// Stop active run.
    STOP = Square;
    /// Toggle left sidebar.
    SIDEBAR = PanelLeft;
    /// Toggle right inspector / diff panel.
    PANEL_RIGHT = PanelRight;
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
    FILE_WRITE = FilePlus;
    /// File edit operation.
    FILE_EDIT = FilePenLine;
    /// Diff file summary.
    FILE_DIFF = FileDiff;
    /// File search operation.
    FILE_SEARCH = FileSearch;
    /// Content grep search operation.
    TEXT_SEARCH = TextSearch;
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
    SLIDERS_HORIZONTAL = SlidersHorizontal;
    /// Unified diff view.
    ROWS3 = Rows3;
    /// Split diff view.
    COLUMNS2 = Columns2;
    /// Queue follow-up message.
    LIST_PLUS = ListPlus;
    /// Run / execute.
    PLAY = Play;
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
    SHIELD_ALERT = ShieldExclamationPoint;
    /// Skill / model highlight sparkles.
    SPARKLES = Sparkles;
    /// Favourite model star.
    STAR = Star;
    /// Subagent / assistant bot.
    BOT = Bot;
    /// Task list / checklist.
    LIST_TODO = ListTodo;
    /// Redo / retry step.
    REDO2 = Redo2;
    /// Reply / quote.
    REPLY = Reply;
    /// Download / pull.
    DOWNLOAD = Download;
    /// Open external link.
    EXTERNAL_LINK = ExternalLink;
    /// Push branch to remote.
    CLOUD_UPLOAD = CloudUpload;
    /// Upload / stage changes.
    UPLOAD = Upload;
    /// Soft wrap toggle.
    TEXT_WRAP = TextWrap;
    /// Whitespace / formatting markers.
    PILCROW = Pilcrow;
    /// Empty / no tests mark.
    FLASK_OFF = FlaskConicalOff;
    /// Plan step not started yet.
    CIRCLE = Circle;
    /// Active plan step.
    CIRCLE_DOT = CircleDot;
    /// Skipped / cancelled step.
    CIRCLE_SLASH = CircleSlash;
    /// Pending / draft circle.
    CIRCLE_DASHED = CircleDashed;
    /// Hard failure octagon.
    OCTAGON_X = OctagonX;
    /// Approved by user.
    USER_CHECK = UserCheck;
    /// User avatar / author.
    USER_ROUND = UserRound;
    /// Rejected by user.
    USER_ROUND_X = UserRoundX;
    /// Link / reference.
    LINK = Link;
    /// Linked worktree / issue.
    LINK2 = Link2;
    /// Unlinked / detached.
    UNLINK2 = Unlink2;
    /// Stacked layers / worktrees.
    LAYERS = Layers;
    /// Appearance / display monitor.
    MONITOR = Monitor;
    /// Theme palette.
    PALETTE = Palette;
    /// Model / compute processor.
    CPU = Cpu;
    /// Multi-step plan list.
    PLAN = ListChecks;
    /// Run that ended early.
    CIRCLE_STOP = CircleStop;
    /// Delegated task result arriving.
    INBOX = Inbox;
    /// Fetching a page or remote URL.
    GLOBE = Globe;
    /// Harness without a known brand mark.
    HARNESS = SquareTerminal;
}

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

/// Visual tier for a thinking/reasoning effort choice, distinguishing `Xhigh`
/// (`ZAP`) and `Max` (`FLAME`) from the three-bar signal scale (`Off`..`High`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    /// Reasoning disabled (`0` lit bars).
    Off,
    /// Low reasoning (`1` lit bar).
    Low,
    /// Medium / balanced reasoning (`2` lit bars).
    Medium,
    /// High reasoning (`3` lit bars).
    High,
    /// Extra-high reasoning (`ZAP` in `t.steer`).
    Xhigh,
    /// Maximum reasoning (`FLAME` in `t.danger`).
    Max,
}

impl Effort {
    /// Classifies an arbitrary harness option value/label pair into an
    /// [`Effort`] tier.
    pub fn classify(value: &str, name: &str, index: usize, total: usize) -> Self {
        let lower_val = value.to_ascii_lowercase();
        let lower_name = name.to_ascii_lowercase();
        let has = |needle: &str| lower_val.contains(needle) || lower_name.contains(needle);
        let exact = |needle: &str| lower_val == needle || lower_name == needle;
        if exact("none") || exact("off") || exact("disabled") {
            Self::Off
        } else if has("xhigh") || has("x-high") || has("extra-high") || has("extra high") {
            Self::Xhigh
        } else if exact("max") || has("maximum") || has("ultra") || lower_val.ends_with("max") {
            Self::Max
        } else if has("min") || has("low") {
            Self::Low
        } else if has("high") {
            Self::High
        } else if has("med") || has("default") || has("auto") || total <= 1 {
            Self::Medium
        } else {
            match ((index * 3) / total.max(1) + 1).min(3) {
                0 => Self::Off,
                1 => Self::Low,
                2 => Self::Medium,
                _ => Self::High,
            }
        }
    }

    /// Canonical short label for display in triggers and galleries.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Xhigh => "Xhigh",
            Self::Max => "Max",
        }
    }
}

/// Signal bars for `Off`..`High`, lightning bolt for `Xhigh`, and flame for
/// `Max`.
pub fn effort_glyph(effort: Effort, on_accent: bool, t: &Theme) -> El {
    match effort {
        Effort::Xhigh => icon(&ZAP, ICON_SMALL)
            .color(if on_accent { t.on_accent } else { t.steer })
            .a11y_hidden(),
        Effort::Max => icon(&FLAME, ICON_SMALL)
            .color(if on_accent { t.on_accent } else { t.danger })
            .a11y_hidden(),
        Effort::Off | Effort::Low | Effort::Medium | Effort::High => {
            let bars = match effort {
                Effort::Off => 0,
                Effort::Low => 1,
                Effort::Medium => 2,
                Effort::High => 3,
                Effort::Xhigh | Effort::Max => unreachable!(),
            };
            let lit = if on_accent { t.on_accent } else { t.text };
            let dim = if on_accent {
                t.on_accent.alpha(0.35)
            } else {
                t.border_control
            };
            row().gap(1.5).items_end().h(10.0).a11y_hidden().children(
                [4.0, 7.0, 10.0].into_iter().enumerate().map(|(i, height)| {
                    el().w(S2)
                        .h(height)
                        .radius(HAIRLINE)
                        .fill(if i < bars { lit } else { dim })
                }),
            )
        }
    }
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
