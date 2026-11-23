//! Settings and icon choices backed by project patches.

use rune::{El, Icon, row, text};
use viz_client::protocol::{
    DefaultCheckout, Project, ProjectAutoSettle, ProjectIcon, ProjectIconChange,
    ProjectSettingsPatch as Patch, Setting,
};

use super::{OptionRow, push};
use crate::{
    model::projects::{Destination, Field, ProjectAction as Action, ProjectView as View},
    theme::{S8, S16, S24, Theme},
    ui,
};

const GLYPHS: &[(&str, &Icon)] = &[
    ("folder", &ui::FOLDER),
    ("folder-code", &ui::FOLDER_CODE),
    ("folder-git-2", &ui::FOLDER_GIT2),
    ("terminal", &ui::TERMINAL),
    ("zap", &ui::ZAP),
    ("flame", &ui::FLAME),
    ("globe", &ui::GLOBE),
    ("git-branch", &ui::GIT_BRANCH),
];

fn save(project: &str, patch: Patch) -> Action {
    Action::Save(project.into(), Box::new(patch))
}

fn edit(project: &str, field: Field, value: String) -> Action {
    Action::Push(
        View::Edit {
            project: project.into(),
            field,
        },
        value,
    )
}

fn icon(project: &str, icon: ProjectIconChange) -> Action {
    save(
        project,
        Patch {
            icon: Some(icon),
            ..Patch::default()
        },
    )
}

pub(super) fn options(project: &Project) -> Vec<OptionRow> {
    let id = &project.path;
    let s = &project.settings;
    let mut rows = vec![
        OptionRow::new(
            "Display name",
            &ui::COMPOSE,
            edit(id, Field::Name, project.name.clone()),
        )
        .detail(&project.name),
        OptionRow::new("Project icon", &ui::FOLDER, push(View::Icon(id.clone()))),
    ];
    for (field, value) in [
        (Field::Harness, s.default_harness.as_deref()),
        (Field::Model, s.default_model.as_deref()),
        (Field::Base, s.base_branch.as_deref()),
        (Field::Prefix, s.branch_prefix.as_deref()),
    ] {
        rows.push(
            OptionRow::new(
                field.label(),
                &ui::COMPOSE,
                edit(id, field, value.unwrap_or_default().into()),
            )
            .detail(value.unwrap_or("Inherited")),
        );
    }
    let options = if s.default_options.is_empty() {
        String::new()
    } else {
        serde_json::to_string(&s.default_options).unwrap_or_default()
    };
    rows.push(
        OptionRow::new(
            "Default options",
            &ui::COMPOSE,
            edit(id, Field::Options, options),
        )
        .detail(format!("{} overrides", s.default_options.len())),
    );
    checkout_options(project, &mut rows);
    lifecycle_options(project, &mut rows);
    rows
}

fn checkout_options(project: &Project, rows: &mut Vec<OptionRow>) {
    let id = &project.path;
    let s = &project.settings;
    for (label, mode) in [
        ("Local checkout", DefaultCheckout::Local),
        ("New worktree", DefaultCheckout::NewWorktree),
    ] {
        rows.push(
            OptionRow::new(
                label,
                &ui::GIT_BRANCH,
                save(
                    id,
                    Patch {
                        default_checkout: Some(mode),
                        ..Patch::default()
                    },
                ),
            )
            .badges(if s.default_checkout == mode {
                vec!["Default"]
            } else {
                Vec::new()
            })
            .disabled(mode == DefaultCheckout::NewWorktree && !project.git),
        );
    }
    rows.push(OptionRow::new(
        "Add additional directory",
        &ui::FOLDER_PLUS,
        push(View::Folders(Destination::Additional(id.clone()))),
    ));
    for (index, path) in s.additional_directories.iter().enumerate() {
        let mut paths = s.additional_directories.clone();
        paths.remove(index);
        rows.push(
            OptionRow::new(
                "Remove additional directory",
                &ui::TRASH,
                save(
                    id,
                    Patch {
                        additional_directories: Some(paths),
                        ..Patch::default()
                    },
                ),
            )
            .detail(path),
        );
    }
}

fn lifecycle_options(project: &Project, rows: &mut Vec<OptionRow>) {
    let id = &project.path;
    let s = &project.settings;
    let days = match s.auto_settle {
        Some(ProjectAutoSettle::AfterDays { days }) => days.to_string(),
        _ => String::new(),
    };
    rows.extend([
        OptionRow::new(
            "Auto-settle · Default",
            &ui::ARCHIVE,
            save(
                id,
                Patch {
                    auto_settle: Some(Setting::Clear),
                    ..Patch::default()
                },
            ),
        )
        .badges(if s.auto_settle.is_none() {
            vec!["Selected"]
        } else {
            Vec::new()
        }),
        OptionRow::new(
            "Auto-settle · After days",
            &ui::ARCHIVE,
            edit(id, Field::Settle, days.clone()),
        )
        .detail(if days.is_empty() {
            "Set a delay".into()
        } else {
            format!("After {days} days")
        }),
        OptionRow::new(
            "Auto-settle · Never",
            &ui::ARCHIVE,
            save(
                id,
                Patch {
                    auto_settle: Some(Setting::Set(ProjectAutoSettle::Never)),
                    ..Patch::default()
                },
            ),
        )
        .badges(if s.auto_settle == Some(ProjectAutoSettle::Never) {
            vec!["Selected"]
        } else {
            Vec::new()
        }),
        OptionRow::new(
            if s.pinned {
                "Unpin project"
            } else {
                "Pin project"
            },
            &ui::PIN,
            save(
                id,
                Patch {
                    pinned: Some(!s.pinned),
                    ..Patch::default()
                },
            ),
        ),
        OptionRow::new(
            if s.archived {
                "Unarchive project"
            } else {
                "Archive project"
            },
            &ui::ARCHIVE,
            save(
                id,
                Patch {
                    archived: Some(!s.archived),
                    ..Patch::default()
                },
            ),
        ),
        OptionRow::new(
            "Remove project…",
            &ui::TRASH,
            push(View::Remove(id.clone())),
        ),
    ]);
}

pub(super) fn icons(project: &str) -> Vec<OptionRow> {
    let mut rows = vec![
        OptionRow::new(
            "Automatic",
            &ui::FOLDER,
            icon(project, ProjectIconChange::Automatic),
        )
        .detail("Repository icon when available"),
        OptionRow::new(
            "Owner avatar",
            &ui::AGENT,
            icon(project, ProjectIconChange::OwnerAvatar),
        ),
        OptionRow::new(
            "Emoji",
            &ui::FLAME,
            edit(project, Field::Emoji, String::new()),
        ),
        OptionRow::new(
            "Upload image",
            &ui::FOLDER_PLUS,
            Action::UploadIcon(project.into()),
        ),
    ];
    rows.extend(GLYPHS.iter().map(|&(name, glyph)| {
        OptionRow::new(
            name,
            glyph,
            push(View::Color {
                project: project.into(),
                glyph: name.into(),
            }),
        )
    }));
    rows
}

pub(super) fn colors(project: &str, glyph: &str) -> Vec<OptionRow> {
    let mark = GLYPHS
        .iter()
        .find(|(name, _)| *name == glyph)
        .map_or(&ui::FOLDER, |(_, icon)| *icon);
    [
        "Blue", "Green", "Violet", "Orange", "Cyan", "Red", "Yellow", "Brown", "Gray",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, name)| {
        OptionRow::new(
            name,
            mark,
            icon(
                project,
                ProjectIconChange::Lucide {
                    name: glyph.into(),
                    color: i.to_string(),
                },
            ),
        )
        .color(i)
    })
    .collect()
}

pub fn mark(project: &Project, size: f32, t: &Theme) -> El {
    match &project.settings.icon {
        ProjectIcon::Emoji { emoji } => row()
            .size(size, size)
            .center()
            .child(text(emoji).font(t.ui)),
        ProjectIcon::Lucide { name, color } => ui::project_icon(
            &project.name,
            GLYPHS
                .iter()
                .find(|(id, _)| id == name)
                .map(|(_, glyph)| *glyph),
            size,
            color.parse().ok(),
            t,
        ),
        _ => ui::project_monogram(&project.name, size, None, t),
    }
}

pub(super) fn preview(project: &Project, image: Option<&rune::Image>, t: &Theme) -> El {
    let icon = image
        .filter(|image| image.is_ready() && image.error().is_none())
        .map_or_else(
            || mark(project, S24, t),
            |image| {
                rune::el()
                    .size(S24, S24)
                    .pixels_fit(image.pixels(), rune::SurfaceFit::Fit)
            },
        );
    row()
        .center_y()
        .gap(S8)
        .child(icon)
        .child(
            rune::col()
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(&project.name)
                        .font(t.ui_medium)
                        .color(t.text)
                        .truncate(),
                )
                .child(
                    text(&project.path)
                        .font(t.mono_small)
                        .color(t.muted)
                        .truncate(),
                ),
        )
        .min_h(S16)
}
