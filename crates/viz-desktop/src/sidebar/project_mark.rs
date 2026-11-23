//! Project identity marks for sidebar cards, filter chips, combobox rows, and
//! hover cards: fetched image icons, custom Lucide glyphs, emoji tiles, and
//! deterministic base16 squircle monograms.

use rune::{El, Icon, SurfaceFit, TextStyle, el, row, text};
use viz_client::protocol::{Project, ProjectIcon};

use crate::{
    app::Desktop,
    theme::{HAIRLINE, Theme, squircle_radius},
    ui,
};

/// Renders a project's `size × size` squircle icon tile, or
/// [`ui::no_project_mark`] when `project` is `None`.
pub(super) fn mark(s: &Desktop, project: Option<&Project>, size: f32, t: &Theme) -> El {
    let Some(project) = project else {
        return ui::no_project_mark(size, t);
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
            ui::project_monogram(&project.name, size, None, t)
        }
        ProjectIcon::Lucide { name, color } => {
            let glyph = lucide_by_name(name);
            let color_idx = palette_index(color);
            ui::project_icon(&project.name, glyph, size, color_idx, t)
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
                .border(HAIRLINE, t.hairline)
                .a11y_hidden()
                .child(text(emoji).font(style).color(t.text))
        }
    }
}

fn lucide_by_name(name: &str) -> Option<&'static Icon> {
    match name.to_ascii_lowercase().as_str() {
        "folder" => Some(&ui::FOLDER),
        "folder-code" | "foldercode" => Some(&ui::FOLDER_CODE),
        "folder-git-2" | "foldergit2" | "git" => Some(&ui::FOLDER_GIT2),
        "folder-tree" | "foldertree" => Some(&ui::FOLDER_TREE),
        "terminal" => Some(&ui::TERMINAL),
        "globe" => Some(&ui::GLOBE),
        "cpu" => Some(&ui::CPU),
        "layers" => Some(&ui::LAYERS),
        "sparkles" => Some(&ui::SPARKLES),
        "wrench" => Some(&ui::WRENCH),
        "bot" => Some(&ui::BOT),
        "shield" => Some(&ui::SHIELD),
        "star" => Some(&ui::STAR),
        "flame" => Some(&ui::FLAME),
        "zap" => Some(&ui::ZAP),
        "git-branch" | "gitbranch" => Some(&ui::GIT_BRANCH),
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
