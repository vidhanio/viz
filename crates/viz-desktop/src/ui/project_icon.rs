//! Deterministic project colour hashing, squircle monograms, and custom project
//! icon tiles.

use rune::{Color, El, Icon, SurfaceFit, TextStyle, el, icon, row, text};
use viz_client::protocol::{Project, ProjectIcon};

use super::icons::MESSAGE_SQUARE_DASHED;
use crate::{
    app::Desktop,
    theme::{Theme, squircle_radius},
};

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
/// tint and contrast-verified initials).
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
        .a11y_hidden()
        .child(icon(&MESSAGE_SQUARE_DASHED, glyph_size).color(t.muted))
}

/// Renders a project's `size × size` squircle icon tile, or
/// [`no_project_mark`] when `project` is `None`.
pub fn project_mark(s: &Desktop, project: Option<&Project>, size: f32, t: &Theme) -> El {
    let Some(project) = project else {
        return no_project_mark(size, t);
    };
    match &project.settings.icon {
        ProjectIcon::Automatic | ProjectIcon::OwnerAvatar | ProjectIcon::Uploaded { .. } => {
            if let Some(image) = s
                .project_icon_image(&project.path)
                .filter(|image| image.is_ready() && image.error().is_none())
            {
                return el()
                    .size(size, size)
                    .radius(squircle_radius(size))
                    .clip_rounded(squircle_radius(size))
                    .pixels_fit(image.pixels(), SurfaceFit::Fill)
                    .a11y_hidden();
            }
            project_monogram(&project.name, size, None, t)
        }
        ProjectIcon::Lucide { name, color } => {
            let glyph = lucide_by_name(name);
            let color_idx = palette_index(color);
            project_icon(&project.name, glyph, size, color_idx, t)
        }
        ProjectIcon::Emoji { emoji } => {
            let font_size = (size * 0.65).clamp(9.0, 16.0);
            let style = TextStyle {
                size: font_size,
                line_height: font_size,
                ..t.small
            };
            row()
                .size(size, size)
                .center()
                .radius(squircle_radius(size))
                .fill(t.chip_fill)
                .a11y_hidden()
                .child(text(emoji).font(style).color(t.text))
        }
    }
}

fn lucide_by_name(name: &str) -> Option<&'static Icon> {
    match name.to_ascii_lowercase().as_str() {
        "folder" => Some(&super::icons::FOLDER),
        "folder-code" | "foldercode" => Some(&super::icons::FOLDER_CODE),
        "folder-git-2" | "foldergit2" | "git" => Some(&super::icons::FOLDER_GIT2),
        "folder-tree" | "foldertree" => Some(&super::icons::FOLDER_TREE),
        "terminal" => Some(&super::icons::TERMINAL),
        "globe" => Some(&super::icons::GLOBE),
        "cpu" => Some(&super::icons::CPU),
        "layers" => Some(&super::icons::LAYERS),
        "sparkles" => Some(&super::icons::SPARKLES),
        "wrench" => Some(&super::icons::WRENCH),
        "bot" => Some(&super::icons::BOT),
        "shield" => Some(&super::icons::SHIELD),
        "star" => Some(&super::icons::STAR),
        "flame" => Some(&super::icons::FLAME),
        "zap" => Some(&super::icons::ZAP),
        "git-branch" | "gitbranch" => Some(&super::icons::GIT_BRANCH),
        _ => None,
    }
}

fn palette_index(color: &str) -> Option<usize> {
    if let Ok(index) = color.parse::<usize>() {
        return Some(index % 9);
    }
    match color.to_ascii_lowercase().as_str() {
        "blue" | "accent" => Some(0),
        "green" | "success" => Some(1),
        "purple" | "violet" | "delegated" => Some(2),
        "orange" | "amber" | "steer" => Some(3),
        "cyan" | "teal" | "info" => Some(4),
        "red" | "rose" | "danger" => Some(5),
        "yellow" | "warning" => Some(6),
        "brown" | "copper" => Some(7),
        "gray" | "grey" | "neutral" | "muted" => Some(8),
        _ => None,
    }
}
