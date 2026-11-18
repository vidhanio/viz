//! The look: `OpenCode`'s theme through rune-kit, following the system's light
//! or dark appearance, plus the few app-specific marks (icons per tool, status
//! colours) the views share.

use rune::{Color, El, Icon, el};
use rune_kit::Kit;
use rune_theme_opencode::icons;

/// Inter and IBM Plex Mono, for `AppSpec::fonts`.
pub const FONTS: &[&[u8]] = rune_theme_opencode::FONTS;
/// `AppSpec::families`: (sans, mono).
pub const FAMILIES: (&str, &str) = rune_theme_opencode::FAMILIES;

/// The timeline's and composer's reading width.
pub const READING_WIDTH: f32 = 760.0;

/// The kit for the window's current appearance.
pub fn kit(dark: bool) -> &'static Kit {
    rune_theme_opencode::kit(dark)
}

/// A provider's mark (`anthropic`, `openai`, ...), or a generic one.
pub fn provider_icon(provider: &str) -> &'static Icon {
    rune_theme_opencode::provider_icons::named(provider)
        .unwrap_or(rune_theme_opencode::provider_icons::FALLBACK)
}

/// The icon a tool's work-log row shows, by the tool's name.
pub fn tool_icon(name: &str) -> &'static Icon {
    let name = name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| name.contains(word));
    if name == "subagent" || has(&["agent", "delegate"]) {
        &icons::FORK
    } else if has(&["bash", "shell", "exec", "command", "terminal"]) {
        &icons::CONSOLE
    } else if has(&["edit", "write", "patch"]) {
        &icons::PENCIL_LINE
    } else if has(&["read", "file", "open"]) {
        &icons::OPEN_FILE
    } else if has(&["search", "grep", "find", "glob"]) {
        &icons::MAGNIFYING_GLASS
    } else if has(&["web", "fetch", "http", "url", "browse"]) {
        &icons::GLOBE
    } else if has(&["memory", "zoom", "recall", "remember"]) {
        &icons::OUTLINE_BRAIN
    } else if has(&["date", "time", "clock"]) {
        &icons::STATUS
    } else {
        &icons::TASK
    }
}

/// A small round status dot.
pub fn dot(color: Color, size: f32) -> El {
    el().size(size, size)
        .radius(size / 2.0)
        .fill(color)
        .a11y_hidden()
}

/// Thousands as `12.3k`, millions as `1.2M`.
#[expect(
    clippy::cast_precision_loss,
    reason = "token counts are shown to one decimal"
)]
pub fn compact_count(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// A duration as `320ms`, `4.2s` or `2m 5s`.
#[expect(
    clippy::cast_precision_loss,
    reason = "durations are shown to one decimal"
)]
pub fn duration(ms: u64) -> String {
    match ms {
        0..1_000 => format!("{ms}ms"),
        1_000..60_000 => format!("{:.1}s", ms as f64 / 1e3),
        _ => format!("{}m {}s", ms / 60_000, ms % 60_000 / 1_000),
    }
}

/// Dollars, with more precision for small amounts.
pub fn cost(dollars: f64) -> String {
    if dollars < 0.995 {
        format!("${dollars:.3}")
    } else {
        format!("${dollars:.2}")
    }
}
