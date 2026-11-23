//! Active draft thread card and empty/loading primary shelf states.

use rune::{Align, El, col, icon, row, text};
use rune_kit::IconButtonVariant;

use crate::{
    app::Desktop,
    model::{DraftPreview, ProjectFilter, Status},
    theme::{HAIRLINE, ICON_MD, ICON_SMALL, RADIUS_CARD, S4, S6, S8, S10, S16, Size, Theme},
    ui,
    update::{Msg, SidebarMsg},
};

/// Dashed-border card at the top of the triage shelves representing an active
/// unsent new-thread draft.
pub(super) fn draft_card(s: &Desktop, draft: &DraftPreview<'_>, t: &Theme) -> El {
    let project_name = draft.project.map_or("No project", |p| p.name.as_str());
    let mut card = col()
        .key(format!("draft-{}", draft.id))
        .role("treeitem")
        .label(format!("Draft: {}", draft.title))
        .w_full()
        .px(S10)
        .py(S8)
        .gap(3.0)
        .radius(RADIUS_CARD)
        .fill(t.selected)
        .border(HAIRLINE, t.border_dashed)
        .border_dash(4.0, 3.0)
        .child(
            row()
                .w_full()
                .h(18.0)
                .gap(S6)
                .center_y()
                .child(ui::project_mark(s, draft.project, ICON_MD, t))
                .child(
                    text(project_name)
                        .font(t.small)
                        .color(t.muted)
                        .truncate()
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0),
                )
                .child(
                    text("Draft")
                        .font(t.mono_micro)
                        .color(t.accent_text)
                        .a11y_hidden(),
                )
                .child(
                    ui::icon_button(
                        &ui::CLOSE,
                        "Discard draft",
                        IconButtonVariant::GhostMuted,
                        Size::Tiny,
                    )
                    .on_press_tx(|s: &mut Desktop, tx| {
                        s.dispatch(Msg::Sidebar(SidebarMsg::DiscardDraft), tx);
                    })
                    .build(&t.k)
                    .key("discard-draft"),
                ),
        )
        .child(
            row()
                .w_full()
                .min_h(19.0)
                .gap(S6)
                .center_y()
                .a11y_hidden()
                .child(icon(&ui::COMPOSE, ICON_SMALL).color(t.accent_text))
                .child(
                    text(&draft.title)
                        .font(t.panel_row)
                        .color(t.text_strong)
                        .truncate()
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0),
                ),
        );
    if draft.branch.is_some() || draft.harness_id.is_some() {
        let mut meta = row().w_full().h(16.0).gap(S6).center_y().a11y_hidden();
        if let Some(branch) = &draft.branch {
            meta = meta.child(
                text(branch)
                    .font(t.small)
                    .color(t.faint)
                    .truncate()
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0),
            );
        } else {
            meta = meta.child(rune::spacer());
        }
        if let (Some(id), Some(name)) = (draft.harness_id, draft.harness_name) {
            meta = meta.child(icon(ui::harness_icon(id), ICON_MD).color(t.text).help(name));
        }
        card = card.child(meta);
    }
    card
}

/// Loading skeleton or quiet empty state shown when no primary shelves contain
/// threads.
pub(super) fn empty_primary_state(s: &Desktop, t: &Theme) -> El {
    let m = &s.model;
    if matches!(m.connection.status, Status::Connecting) && m.cache().thread_count() == 0 {
        return col()
            .key("sidebar-loading")
            .w_full()
            .px(S4)
            .py(S8)
            .child(ui::skeleton_rows("sidebar-skeleton", 4, 56.0, true, t));
    }
    if matches!(m.connection.status, Status::Error(_)) && m.cache().thread_count() == 0 {
        return col()
            .key("sidebar-offline")
            .role("status")
            .w_full()
            .px(S16)
            .py(32.0)
            .items_center()
            .child(
                text("Offline")
                    .font(t.small)
                    .color(t.faint)
                    .align(Align::Center),
            );
    }
    let (message, action) = match &m.sidebar.project_filter {
        ProjectFilter::All => (
            "No active threads".to_owned(),
            m.connected().then(|| {
                ui::inline_button("Start a new thread")
                    .tone(ui::Tone::Accent)
                    .on_press_tx(|s: &mut Desktop, tx| {
                        s.dispatch(Msg::Sidebar(SidebarMsg::NewThread), tx);
                    })
                    .build(t)
            }),
        ),
        ProjectFilter::NoProject => (
            "No threads without a project".to_owned(),
            Some(clear_filter_button(t)),
        ),
        ProjectFilter::Project(path) => {
            let name = m
                .cache()
                .project(path)
                .map_or(path.as_str(), |p| p.name.as_str());
            (
                format!("No threads in {name}"),
                Some(clear_filter_button(t)),
            )
        }
    };
    col()
        .key("sidebar-empty")
        .role("status")
        .w_full()
        .px(S16)
        .py(32.0)
        .gap(S6)
        .items_center()
        .child(
            text(message)
                .font(t.small)
                .color(t.faint)
                .align(Align::Center)
                .wrap(),
        )
        .children(action)
}

fn clear_filter_button(t: &Theme) -> El {
    ui::inline_button("Clear filter")
        .tone(ui::Tone::Accent)
        .on_press_tx(|s: &mut Desktop, tx| {
            tx.instant();
            s.dispatch(
                Msg::Sidebar(SidebarMsg::SetProjectFilter(ProjectFilter::All)),
                tx,
            );
        })
        .build(t)
}
