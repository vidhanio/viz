//! Deterministic project colour hashing, squircle monograms, and custom project
//! icon tiles.

use rune::{Color, El, Icon, TextStyle, icon, row, text};

use super::icons::MESSAGE_SQUARE_DASHED;
use crate::theme::{HAIRLINE, Theme, squircle_radius};

/// Stable hash mapping a project name onto one of the nine `project_palette`
/// slots (`(h * 31 + cp) % 9`).
pub fn project_color_index(name: &str) -> usize {
    name.chars().fold(0usize, |acc, ch| {
        acc.wrapping_mul(31).wrapping_add(ch as usize)
    }) % 9
}

/// Resolves a project's accent colour from its optional manual override or
/// deterministic name hash.
pub fn project_color(name: &str, custom_index: Option<usize>, t: &Theme) -> Color {
    let idx = custom_index.map_or_else(|| project_color_index(name), |i| i % 9);
    t.project_palette[idx]
}

/// Extracts one or two uppercase monogram initials from a project name.
pub fn project_initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '/' | '.'))
        .filter(|part| !part.is_empty())
        .collect();
    match words.as_slice() {
        [] => "P".to_owned(),
        [only] => only.chars().take(2).flat_map(char::to_uppercase).collect(),
        [first, second, ..] => first
            .chars()
            .take(1)
            .chain(second.chars().take(1))
            .flat_map(char::to_uppercase)
            .collect(),
    }
}

/// Renders a flat `size × size` squircle monogram (`25%` radius, `14%` colour
/// tint, `24%` colour hairline, contrast-verified initials).
pub fn project_monogram(name: &str, size: f32, custom_index: Option<usize>, t: &Theme) -> El {
    let color = project_color(name, custom_index, t);
    let initials = project_initials(name);
    let font_size = (size * 0.48).clamp(8.5, 16.0);
    let font = TextStyle {
        size: font_size,
        weight: 600.0,
        line_height: font_size,
        ..t.mono_micro
    };
    row()
        .size(size, size)
        .center()
        .radius(squircle_radius(size))
        .fill(color.alpha(0.14))
        .border(HAIRLINE, color.alpha(0.24))
        .a11y_hidden()
        .child(text(initials).font(font).color(color))
}

/// Renders a project's custom Lucide icon tile when `glyph` is set, or falls
/// back to [`project_monogram`].
pub fn project_icon(
    name: &str,
    glyph: Option<&'static Icon>,
    size: f32,
    custom_index: Option<usize>,
    t: &Theme,
) -> El {
    let Some(glyph) = glyph else {
        return project_monogram(name, size, custom_index, t);
    };
    let color = project_color(name, custom_index, t);
    let glyph_size = (size * 0.62).clamp(10.0, 20.0);
    row()
        .size(size, size)
        .center()
        .radius(squircle_radius(size))
        .fill(color.alpha(0.14))
        .border(HAIRLINE, color.alpha(0.24))
        .a11y_hidden()
        .child(icon(glyph, glyph_size).color(color))
}

/// Neutral squircle mark used for threads and drafts without an assigned
/// project.
pub fn no_project_mark(size: f32, t: &Theme) -> El {
    let glyph_size = (size * 0.62).clamp(10.0, 18.0);
    row()
        .size(size, size)
        .center()
        .radius(squircle_radius(size))
        .fill(t.chip_fill)
        .border(HAIRLINE, t.hairline)
        .a11y_hidden()
        .child(icon(&MESSAGE_SQUARE_DASHED, glyph_size).color(t.muted))
}
