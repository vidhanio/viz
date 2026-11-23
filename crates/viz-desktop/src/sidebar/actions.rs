//! Sidebar modal rename/delete dialogs and hover quick-action snooze popover.

use rune::El;
use rune_kit::{ButtonVariant, Menu, MenuItem, Placement};
use viz_client::protocol::ThreadSummary;

use crate::{
    app::Desktop,
    model::{SidebarEdit, SnoozePreset},
    theme::Theme,
    ui::{self, DialogForm},
    update::{Msg, SidebarMsg},
};

/// Mounts the active rename or delete confirmation dialog independently of
/// whether the sidebar is open.
pub(super) fn dialog(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let m = &s.model;
    let Some(edit) = &m.sidebar.edit else {
        return;
    };
    let form = match edit {
        SidebarEdit::DeleteThread(id) => DialogForm {
            title: "Delete thread?",
            subject: Some(m.title(*id)),
            detail: Some(
                if m.summary(*id)
                    .is_some_and(|thread| thread.worktree.is_some())
                {
                    "This can’t be undone. Its worktree stays."
                } else {
                    "This can’t be undone."
                },
            ),
            field: None,
            action: "Delete",
            variant: ButtonVariant::Danger,
        },
        SidebarEdit::DeleteWithWorktree(id) => DialogForm {
            title: "Delete thread and worktree?",
            subject: m
                .summary(*id)
                .and_then(|thread| thread.worktree.as_ref())
                .map(|worktree| worktree.path.as_str()),
            detail: Some(
                "The worktree must have no uncommitted changes. Its branch is kept. This can’t be undone.",
            ),
            field: None,
            action: "Delete",
            variant: ButtonVariant::Danger,
        },
    };
    form.build(
        |s: &mut Desktop| &mut s.widgets.sidebar_dialog,
        |s| s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseEdit)),
        |s, tx| {
            let value = s.widgets.sidebar_dialog.text.clone();
            s.dispatch(Msg::Sidebar(SidebarMsg::EditValueChanged(value)), tx);
            s.dispatch(Msg::Sidebar(SidebarMsg::SubmitEdit), tx);
        },
        t,
        layer,
    );
}

/// Mounts the hover quick-action snooze preset popover anchored to `trigger`
/// when open for `thread`.
pub(super) fn snooze_popover(
    s: &Desktop,
    thread: &ThreadSummary,
    trigger: El,
    now: i64,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let id = thread.id;
    let open = s.model.connected() && s.model.sidebar.snooze_popover == Some(id);
    let mut menu = Menu::new(format!("snooze-{id}"))
        .trigger(trigger)
        .placement(Placement::BottomEnd)
        .min_width(172.0)
        .open(open)
        .on_close(|s: &mut Desktop| s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseSnoozePopover)));
    if open {
        for preset in SnoozePreset::ALL {
            menu = menu.item(MenuItem::new(preset.label()).icon(&ui::CLOCK).on_select_tx(
                move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::SnoozePreset {
                            thread: id,
                            preset,
                            now,
                        }),
                        tx,
                    );
                },
            ));
        }
    }
    menu.build(&t.overlay, layer)
}
