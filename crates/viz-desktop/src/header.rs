//! Conversation title and workspace context.

use rune::{El, el, icon, row, text};
use rune_kit::shell::Toolbar;

use crate::{app::Desktop, theme::Theme, ui};

pub fn toolbar(s: &Desktop, t: &Theme) -> Toolbar {
    let title = s.selected.map_or_else(
        || {
            if s.new_chat.is_some() {
                "New chat"
            } else {
                "viz"
            }
        },
        |id| s.title(id),
    );
    let toolbar = Toolbar::new().leading(
        text(title)
            .key("title")
            .font(t.title)
            .color(t.text)
            .truncate()
            .shrink()
            .help_when_truncated(title),
    );
    let project = s.project();
    let mut metadata = row().key("header-metadata").center_y().gap(7.0).shrink();
    if let Some(project) = project {
        metadata = metadata.child(quiet("header-project", &project.name, t).help(&project.path));
        if let Some(branch) = project.branch.as_deref() {
            metadata = metadata.child(
                row()
                    .key("header-branch")
                    .center_y()
                    .gap(5.0)
                    .shrink()
                    .child(icon(&ui::GIT_BRANCH, 13.0).color(t.muted))
                    .child(quiet("branch", branch, t)),
            );
        }
    }
    if let Some(harness) = s
        .conversation
        .as_ref()
        .and_then(|state| state.harness.as_deref())
        .or_else(|| {
            s.selected
                .and_then(|id| s.summary(id))
                .and_then(|summary| summary.harness.as_deref())
        })
    {
        if project.is_some() {
            metadata = metadata.child(el().w(1.0).h(12.0).fill(t.hairline).a11y_hidden());
        }
        metadata = metadata.child(quiet("header-harness", harness, t));
    }
    if let Some(mark) = s
        .conversation
        .as_ref()
        .and_then(|state| ui::status(state.status, t))
    {
        metadata = metadata.child(mark.key("header-status"));
    }
    toolbar.trailing(metadata)
}

fn quiet(key: &str, label: &str, t: &Theme) -> El {
    text(label)
        .key(key)
        .font(t.small)
        .color(t.muted)
        .max_w(136.0)
        .min_w(0.0)
        .shrink()
        .truncate()
        .help_when_truncated(label)
}
