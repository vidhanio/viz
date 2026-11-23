//! Shared flat base16 UI primitives for `viz-desktop`.
//!
//! Every component takes a [`crate::theme::Size`] or semantic variant, owns its
//! internal insets, implements every interactive state (rest, hover, active,
//! keyboard focus ring, selected, disabled, loading, empty, error), and rides
//! on [`crate::motion`]. Views compose these primitives without inventing
//! ad-hoc buttons, inputs, rows, or overlays.

mod badge;
mod dialog;
mod feedback;
pub mod gallery;
mod icon_button;

mod icons;
mod project_icon;
mod row;
mod surface;

use rune::{Color, El, el, text};
use rune_kit::Spinner;

#[expect(
    unused_imports,
    reason = "complete ui component API is exported for Phase 1 feature tracks and gallery"
)]
pub use self::{
    badge::{Tone, ToneColors, diff_stat, diff_stat_rolling, dot, tooltip_text, with_tooltip},
    dialog::{DialogForm, dialog_actions, dialog_body, dialog_subject, sheet},
    feedback::{
        Banner, EmptyState, LoadingDelay, empty, empty_state, error_banner, loading_state,
        skeleton_rows, status_banner,
    },
    icon_button::{InlineButton, InlineStyle, inline_button},
    icons::{
        AGENT, ANTIGRAVITY, ARCHIVE, ARROW_DOWN, ARROW_LEFT, ARROW_UP, BOT, BRAIN, CHECK, CHEVRON,
        CHEVRON_OPEN, CHEVRON_UP, CHEVRONS_UP_DOWN, CIRCLE_ALERT, CIRCLE_CHECK, CIRCLE_DASHED,
        CIRCLE_DOT, CIRCLE_STOP, CIRCLE_X, CLAUDE, CLOCK, CLOSE, CODEX, COLUMNS2, COMPOSE, COPY,
        CORNER_UP_RIGHT, CPU, DOWNLOAD, EXTERNAL_LINK, EYE, EYE_OFF, FILE, FILE_DIFF, FILE_EDIT,
        FILE_READ, FLAME, FOLDER, FOLDER_CODE, FOLDER_GIT2, FOLDER_OPEN, FOLDER_PLUS, FOLDER_TREE,
        GIT_BRANCH, GIT_BRANCH_PLUS, GIT_COMMIT, GIT_FORK, GIT_MERGE, GLOBE, HARNESS, INBOX, INFO,
        LAPTOP, LAYERS, LINK, LIST_FILTER, LIST_PLUS, LOCK, MAIL, MAIL_OPEN, MESSAGE_SQUARE_DASHED,
        MINIMIZE2, MINUS, MONITOR, MORE, OMP, PANEL_RIGHT, PAPERCLIP, PENCIL, PI, PIN, PIN_OFF,
        PLUS, PR_CLOSED, PR_DRAFT, PR_OPEN, REDO2, REFRESH_CW, REPLY, ROTATE_CCW, ROTATE_CW,
        SEARCH, SERVER, SETTINGS, SHIELD, SHIELD_CHECK, SIDEBAR, SPARKLES, STAR, STAR_FILLED, STOP,
        TERMINAL, TEXT_WRAP, THREAD, TIMER, TRASH, TRIANGLE_ALERT, UNLOCK, UPLOAD, WRENCH, ZAP,
        harness_glyph, harness_icon, harness_mark, kit_glyphs, swap_kit_icon,
    },
    project_icon::{
        no_project_mark, project_color, project_color_index, project_icon, project_initials,
        project_mark, project_monogram,
    },
    row::{ListRow, RowDensity, SectionHeader, list_row, section_header},
    surface::{
        Card, ComboboxShell, MENU_INSET, MENU_WIDTH, MODEL_TRIGGER_MAX, ScrollMode,
        anchored_popover, combobox_shell, menu_items, popover_frame, scroll_viewport,
        tucked_drawer,
    },
};
use crate::theme::{ICON, ICON_SMALL, READING_INSET, READING_WIDTH, Theme};

/// Platform command modifier display label (`⌘` on macOS, `Ctrl` elsewhere).
pub const COMMAND_KEY: &str = if cfg!(target_os = "macos") {
    "⌘"
} else {
    "Ctrl"
};

/// The sidebar toggle's shortcut, as its tooltip shows it.
pub const SIDEBAR_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘B"
} else {
    "Ctrl+B"
};

/// Formats a command-key shortcut for tooltips (`⌘N` on macOS, `Ctrl+N`
/// elsewhere).
pub fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("{COMMAND_KEY}{key}")
    } else {
        format!("{COMMAND_KEY}+{key}")
    }
}

/// A horizontal hairline across its parent, snapped to whole device pixels.
pub fn rule(t: &Theme) -> El {
    rune::hairline(t.hairline).w_full().a11y_hidden()
}

/// A section title: small, medium, muted.
pub fn section(label: &str, t: &Theme) -> El {
    text(label).font(t.small_medium).color(t.muted)
}

/// A glyph centered in an icon-wide box one line tall, so it sits on the first
/// line of the text beside it.
pub fn lead(mark: El, line: f32) -> El {
    el().size(ICON, line).center().child(mark).a11y_hidden()
}

/// The centered reading column's frame; its parent owns vertical spacing.
pub fn reading(child: El) -> El {
    rune::col()
        .w_full()
        .px(READING_INSET)
        .items_center()
        .child(child.w_full().max_w(READING_WIDTH))
}

/// Blocks window dragging over toolbar and header bands without eating control
/// clicks.
pub fn drag_guard(key: &str) -> El {
    el().key(key).no_window_drag().a11y_hidden()
}

/// The 5x5 radar sweep indicator, sharing wall-clock phase across the window.
pub fn radar(size: f32, color: Color, t: &Theme) -> El {
    Spinner::new().size(size).color(color).build(&t.k)
}

/// Accent radar spinner at [`ICON_SMALL`] size.
pub fn spinner(t: &Theme) -> El {
    radar(ICON_SMALL, t.accent, t)
}

/// Selectable wrapped monospace code block body.
pub fn code(body: &str, t: &Theme) -> El {
    text(body)
        .font(t.mono)
        .color(t.text)
        .wrap_anywhere()
        .w_full()
        .selectable(true)
        .selection_color(t.k.selection())
}
