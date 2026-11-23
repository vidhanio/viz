//! Home screen bottom Context Strip: environment selector, Git worktree mode
//! selector (reusing Track G checkouts), branch / base-ref selector, and right
//! Access Mode trigger (`home.md` §4).

use std::path::Path;

use rune::{Color, El, Icon, col, icon, row, text};
use rune_kit::{Placement, scrim};
use viz_client::protocol::{EnvironmentStatus, Project, WorktreeTarget};

use super::branch_picker;
use crate::{
    app::Desktop,
    checkout::DRAFT_CHECKOUT,
    model::{Checkouts, NewThread},
    motion,
    theme::{HAIRLINE, ICON_SMALL, RADIUS_SM, S2, S4, S6, S8, S24, S28, Theme},
    ui::{self, RowDensity},
    update::{ComposerMsg, HOME_ENV_PICKER, Msg, SelectionMsg},
};

const ENV_POPOVER_WIDTH: f32 = 224.0;
const MODE_POPOVER_WIDTH: f32 = 256.0;
const COMPACT_STRIP_WIDTH: f32 = 460.0;

/// Single-line metadata strip rendered directly underneath the composer card on
/// the Home / draft screen (`home.md` §4).
pub fn home_strip(s: &Desktop, compact_width: f32, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let compact = compact_width < COMPACT_STRIP_WIDTH;
    let mut left = row().key("context-left").gap(S4).center_y().min_w(0.0);

    if should_show_env(s) {
        left = left.child(env_selector(s, compact, t, layer));
    }

    if let Some(project) = m.project().filter(|project| project.git) {
        let draft = m.selection.new_thread.as_ref();
        left = left
            .child(worktree_mode_selector(s, draft, project, compact, t, layer))
            .child(branch_picker::selector(
                s, draft, project, compact, t, layer,
            ));
    }

    let right = crate::model_picker::access::access_trigger(s, t, layer);

    row()
        .key("home-context-strip")
        .w_full()
        .min_h(S28)
        .px(S8)
        .pt(S8)
        .gap(S4)
        .center_y()
        .child(left)
        .child(rune::spacer())
        .children(right)
}

fn should_show_env(s: &Desktop) -> bool {
    let envs = &s.model.connection.environments;
    envs.len() >= 2
        || s.model
            .connection
            .environment
            .as_deref()
            .is_some_and(|env| env != "local")
}

/// Shared low-chroma context-strip pill trigger (`24px` height, `12px` icon,
/// subtle hover wash).
#[expect(
    clippy::too_many_arguments,
    reason = "context strip pill trigger configures icon, optional label, state, and chevron"
)]
pub(super) fn strip_pill(
    key: &'static str,
    a11y_label: &str,
    glyph: &'static Icon,
    label: Option<&str>,
    open: bool,
    disabled: bool,
    show_chevron: bool,
    t: &Theme,
) -> El {
    let ink = if open { t.text } else { t.muted };
    let fill = if open {
        t.hover_wash
    } else {
        Color::TRANSPARENT
    };
    let mut trigger = row()
        .key(key)
        .role("combobox")
        .label(a11y_label)
        .help(a11y_label)
        .expanded(open)
        .h(S24)
        .px(S6)
        .gap(5.0)
        .center_y()
        .radius(RADIUS_SM)
        .fill(fill)
        .disabled(disabled)
        .focusable(!disabled)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent);
    if !disabled {
        trigger = trigger
            .hover_group()
            .hover(t.hover_wash)
            .press(t.pressed_wash)
            .press_scale(motion::PRESS_SCALE);
    }
    trigger = trigger.child(
        icon(glyph, ICON_SMALL)
            .color(ink)
            .group_color(t.text)
            .a11y_hidden(),
    );
    if let Some(label) = label {
        trigger = trigger.child(
            text(label)
                .font(t.small)
                .color(ink)
                .group_color(t.text)
                .max_w(168.0)
                .shrink()
                .truncate()
                .a11y_hidden(),
        );
    }
    if show_chevron {
        trigger = trigger.child(
            motion::chevron(&ui::CHEVRON_OPEN, ICON_SMALL, open, std::f32::consts::PI)
                .key("chevron")
                .color(ink)
                .group_color(t.text)
                .a11y_hidden(),
        );
    }
    trigger
}

fn env_selector(s: &Desktop, compact: bool, t: &Theme, layer: &mut Vec<El>) -> El {
    const TRIGGER: &str = "home-env-trigger";
    let m = &s.model;
    let open = m.composer.open_picker.as_deref() == Some(HOME_ENV_PICKER);
    let current = m.connection.environment.as_deref().unwrap_or("local");
    let display = if current == "local" {
        "This device"
    } else {
        current
    };
    let glyph = if current == "local" {
        &ui::LAPTOP
    } else {
        &ui::SERVER
    };
    let trigger = strip_pill(
        TRIGGER,
        &format!("Environment: {display}"),
        glyph,
        (!compact).then_some(display),
        open,
        false,
        true,
        t,
    )
    .on_press(|s: &mut Desktop| {
        s.dispatch_local(Msg::Composer(ComposerMsg::TogglePicker(
            HOME_ENV_PICKER.to_owned(),
        )));
    });
    if open {
        let rows = m.connection.environments.iter().map(|env| {
            let active = current == env.id;
            let offline = env.status == EnvironmentStatus::Unreachable;
            let name = if env.id == "local" {
                "This device"
            } else {
                env.id.as_str()
            };
            let glyph = if env.status == EnvironmentStatus::Local {
                &ui::LAPTOP
            } else {
                &ui::SERVER
            };
            let target = env.id.clone();
            let mut item = ui::list_row(format!("home-env-{}", env.id), name)
                .density(RowDensity::Compact)
                .leading_icon(glyph)
                .selected(active)
                .disabled(offline)
                .on_press_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(Msg::Composer(ComposerMsg::ClosePicker), tx);
                    s.dispatch(
                        Msg::Selection(SelectionMsg::OpenInEnvironment {
                            environment: target.clone(),
                            thread: None,
                        }),
                        tx,
                    );
                });
            if offline {
                item = item.detail("Offline");
            } else if active {
                item = item.trailing(
                    icon(&ui::CHECK, ICON_SMALL)
                        .color(t.accent_text)
                        .a11y_hidden(),
                );
            }
            item.build(t)
        });
        let content = col()
            .w_full()
            .padding(ui::MENU_INSET)
            .gap(S2)
            .children(rows);
        layer.push(scrim("home-env-scrim", close_picker));
        layer.push(
            ui::anchored_popover(
                "home-env-menu",
                TRIGGER,
                Placement::TopStart,
                ENV_POPOVER_WIDTH,
                content,
                t,
            )
            .role("dialog")
            .label("Environment")
            .focus_return(TRIGGER)
            .modal(true, close_picker),
        );
    }
    trigger
}

fn worktree_mode_selector(
    s: &Desktop,
    draft: Option<&NewThread>,
    project: &Project,
    compact: bool,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    const TRIGGER: &str = "draft-checkout-trigger";
    let m = &s.model;
    let sending = draft.is_some_and(|d| d.sending);
    let open = m.composer.open_picker.as_deref() == Some(DRAFT_CHECKOUT) && !sending;
    let checkouts = m.checkouts(&project.path);
    let current = draft.and_then(|d| d.worktree.as_ref());
    let (label, glyph) = match current {
        None | Some(WorktreeTarget::Local) => ("Current checkout".to_owned(), &ui::FOLDER),
        Some(WorktreeTarget::New { .. }) => ("New worktree".to_owned(), &ui::GIT_BRANCH),
        Some(WorktreeTarget::Existing { path }) => {
            let branch = worktree_branch_name(checkouts, path);
            (format!("Current worktree ({branch})"), &ui::GIT_BRANCH)
        }
    };
    let path = project.path.clone();
    let trigger = strip_pill(
        TRIGGER,
        &format!("Checkout mode: {label}"),
        glyph,
        (!compact).then_some(label.as_str()),
        open,
        sending,
        true,
        t,
    )
    .on_press_tx(move |s: &mut Desktop, tx| {
        s.dispatch(
            Msg::Composer(ComposerMsg::ToggleCheckoutPicker {
                key: DRAFT_CHECKOUT.to_owned(),
                project: path.clone(),
            }),
            tx,
        );
    });
    if open {
        let body = col()
            .w_full()
            .padding(ui::MENU_INSET)
            .gap(S2)
            .children(mode_rows(project, checkouts, current, t));
        layer.push(scrim("draft-checkout-scrim", close_picker));
        layer.push(
            ui::anchored_popover(
                "draft-checkout-menu",
                TRIGGER,
                Placement::TopStart,
                MODE_POPOVER_WIDTH,
                body,
                t,
            )
            .role("dialog")
            .label("Checkout mode")
            .focus_return(TRIGGER)
            .modal(true, close_picker),
        );
    }
    trigger
}

/// The checkout menu's rows: current checkout, new worktree, then each
/// existing non-main worktree.
fn mode_rows(
    project: &Project,
    checkouts: Option<&Checkouts>,
    current: Option<&WorktreeTarget>,
    t: &Theme,
) -> Vec<El> {
    let is_local = matches!(current, None | Some(WorktreeTarget::Local));
    let is_new = matches!(current, Some(WorktreeTarget::New { .. }));
    let mut items = vec![
        mode_row(
            "mode-local",
            "Current checkout",
            "Work directly in the project folder",
            &ui::FOLDER,
            is_local,
            WorktreeTarget::Local,
            t,
        ),
        mode_row(
            "mode-new",
            "New worktree",
            "Isolate changes on a new branch",
            &ui::GIT_BRANCH,
            is_new,
            WorktreeTarget::New {
                base: project.branch.clone(),
                branch: None,
            },
            t,
        ),
    ];
    if let Some(checkouts) = checkouts {
        for worktree in checkouts.worktrees.iter().filter(|w| !w.main) {
            let name = worktree_branch_name(Some(checkouts), &worktree.path);
            let active = matches!(
                current,
                Some(WorktreeTarget::Existing { path }) if path == &worktree.path
            );
            items.push(mode_row(
                &format!("mode-prev-{}", worktree.path),
                &format!("Previous worktree ({name})"),
                "Reuse existing worktree",
                &ui::GIT_BRANCH,
                active,
                WorktreeTarget::Existing {
                    path: worktree.path.clone(),
                },
                t,
            ));
        }
    }
    items
}

fn mode_row(
    key: &str,
    title: &str,
    subtitle: &str,
    glyph: &'static Icon,
    selected: bool,
    target: WorktreeTarget,
    t: &Theme,
) -> El {
    let mut item = ui::list_row(key, title)
        .subtitle(subtitle)
        .leading_icon(glyph)
        .selected(selected)
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(
                Msg::Selection(SelectionMsg::ChooseDraftWorktree(target.clone())),
                tx,
            );
        });
    if selected {
        item = item.trailing(
            icon(&ui::CHECK, ICON_SMALL)
                .color(t.accent_text)
                .a11y_hidden(),
        );
    }
    item.build(t)
}

/// Resolves the branch label for a worktree path from cached checkouts or the
/// folder basename.
pub(super) fn worktree_branch_name(checkouts: Option<&Checkouts>, path: &str) -> String {
    checkouts
        .and_then(|c| c.worktrees.iter().find(|w| w.path == path))
        .and_then(|w| w.branch.clone())
        .unwrap_or_else(|| {
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path)
                .to_owned()
        })
}

/// Closes whichever context strip popover is open.
pub(super) fn close_picker(s: &mut Desktop) {
    s.dispatch_local(Msg::Composer(ComposerMsg::ClosePicker));
}
