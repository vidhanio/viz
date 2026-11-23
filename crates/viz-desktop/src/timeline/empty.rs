//! Empty, loading, offline, onboarding, and new-thread draft project picker
//! states shown when a timeline has no conversation items yet.

use rune::{El, col, icon, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant, Menu, MenuItem, Placement};

use crate::{
    app::Desktop,
    checkout,
    model::{NewThread, Status},
    motion,
    theme::{
        EMPTY_TOP, EMPTY_WIDTH, HAIRLINE, ICON_SMALL, RADIUS_CONTROL, S4, S6, S8, S12, Size, Theme,
    },
    ui,
    update::{ComposerMsg, Msg, SelectionMsg, SidebarMsg},
};

/// The `open_picker` value of a draft's project picker.
pub const DRAFT_PROJECT: &str = "draft-project";

/// A thread whose transcript hasn't arrived: nothing, then a quiet radar
/// spinner and caption if it takes a while.
pub fn loading(t: &Theme) -> El {
    col()
        .key("loading")
        .w_full()
        .pt(EMPTY_TOP)
        .items_center()
        .gap(S8)
        .role("status")
        .label("Opening transcript")
        .child(ui::radar(crate::theme::ICON, t.accent, t))
        .child(text("Opening transcript…").font(t.small).color(t.faint))
        .transition(motion::fade(motion::QUICK_PAIR))
        .enter_delay(motion::LOADING_DELAY)
}

/// What to show when the timeline has no messages.
pub fn empty_state(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    match &m.connection.status {
        Status::Error(message) => {
            let detail = if message.trim().is_empty() {
                "Start viz-server to reconnect."
            } else {
                message.as_str()
            };
            let retry = rune_kit::Button::new("Retry")
                .variant(ButtonVariant::Neutral)
                .control_size(Size::Default.kit())
                .on_press_tx(|s: &mut Desktop, tx| {
                    s.dispatch(Msg::Selection(SelectionMsg::Reconnect), tx);
                })
                .build(&t.k)
                .key("retry");
            let mut body = col()
                .key("empty")
                .w_full()
                .max_w(EMPTY_WIDTH)
                .gap(S8)
                .items_center()
                .child(text("Offline").font(t.title).color(t.text))
                .child(
                    text(detail)
                        .font(t.ui)
                        .color(t.muted)
                        .wrap_anywhere()
                        .max_lines(3)
                        .align(rune::Align::Center)
                        .w_full(),
                );
            if let Some(endpoint) = &m.connection.endpoint {
                body = body.child(
                    text(endpoint)
                        .font(t.mono_small)
                        .color(t.faint)
                        .truncate_middle()
                        .w_full()
                        .align(rune::Align::Center),
                );
            }
            return body.child(retry.mt(S8));
        }
        Status::Connecting => return ui::empty(None, "Connecting…", "", None, t),
        Status::Connected { .. } => {}
    }
    if m.selection.selected.is_none() && m.selection.new_thread.is_none() {
        let new_thread = rune_kit::Button::new("New thread")
            .variant(ButtonVariant::Submit)
            .control_size(Size::Large.kit())
            .icon(&ui::COMPOSE)
            .on_press_tx(|s: &mut Desktop, tx| {
                s.dispatch(Msg::Selection(SelectionMsg::CreateHere), tx);
            })
            .build(&t.k);
        let (detail, actions) = if m.cache().has_projects() {
            ("Select a thread or start a new one.", new_thread)
        } else {
            let add_project = rune_kit::Button::new("Add project")
                .variant(ButtonVariant::Neutral)
                .control_size(Size::Large.kit())
                .icon(&ui::FOLDER_PLUS)
                .on_press_tx(|s: &mut Desktop, tx| {
                    s.dispatch(Msg::Sidebar(SidebarMsg::OpenAddProject), tx);
                })
                .build(&t.k);
            (
                "Start a thread or add a project folder.",
                row().gap(S8).child(new_thread).child(add_project),
            )
        };
        return ui::empty(None, "No thread selected", detail, Some(actions), t);
    }
    thread_hint(s, t, layer)
}

fn thread_hint(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let Some(thread) = &m.selection.new_thread else {
        let title = m
            .project()
            .map_or("New thread", |project| project.name.as_str());
        return ui::empty(None, title, "", None, t);
    };
    let harnesses = m.harnesses();
    let choose_harness = !thread.sending
        && thread.harness.is_none()
        && harnesses.len() > 1
        && harnesses.iter().all(|harness| !harness.default);
    col()
        .key("empty")
        .w_full()
        .max_w(EMPTY_WIDTH)
        .gap(S12)
        .items_center()
        .child(draft_project(s, thread, t, layer))
        .children(choose_harness.then(|| {
            text("Choose a harness below.")
                .font(t.ui)
                .color(t.muted)
                .wrap()
                .align(rune::Align::Center)
        }))
}

fn draft_project(s: &Desktop, thread: &NewThread, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let open = m.composer.open_picker.as_deref() == Some(DRAFT_PROJECT) && !thread.sending;
    let current = m
        .project()
        .map_or("No project", |project| project.name.as_str());
    let trigger = draft_project_trigger(current, thread.project.is_some(), open, thread.sending, t)
        .on_press(|s: &mut Desktop| {
            s.dispatch_local(Msg::Composer(ComposerMsg::TogglePicker(
                DRAFT_PROJECT.to_owned(),
            )));
        });
    let mut menu = Menu::new(format!("{DRAFT_PROJECT}-picker"))
        .placement(Placement::Bottom)
        .min_width(ui::MENU_WIDTH)
        .open(open)
        .on_close(|s: &mut Desktop| s.dispatch_local(Msg::Composer(ComposerMsg::ClosePicker)))
        .trigger(trigger);
    if open {
        let choice = |label: &str, glyph: &'static rune::Icon, id: Option<&str>| {
            let picked = thread.project.as_deref() == id;
            let id = id.map(str::to_owned);
            MenuItem::new(label)
                .icon(glyph)
                .checked(picked)
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Selection(SelectionMsg::ChooseDraftProject(id.clone())),
                        tx,
                    );
                })
        };
        menu = menu
            .item(choice("No project", &ui::THREAD, None))
            .separator();
        menu = ui::menu_items(
            menu,
            m.cache()
                .projects()
                .map(|project| choice(&project.name, &ui::FOLDER, Some(&project.path))),
        );
    }
    let add = rune_kit::IconButton::new(&ui::FOLDER_PLUS)
        .help("Add project")
        .variant(IconButtonVariant::GhostMuted)
        .control_size(Size::Default.kit())
        .disabled(thread.sending)
        .on_press_tx(|s: &mut Desktop, tx| s.dispatch(Msg::Sidebar(SidebarMsg::OpenAddProject), tx))
        .build(&t.k)
        .key("draft-add-project");
    let place = menu.build(&t.overlay, layer).min_w(0.0).shrink();
    let checkout = checkout::draft(s, thread, t, layer);
    row()
        .key("draft-place")
        .gap(S4)
        .center_y()
        .min_w(0.0)
        .child(place)
        .child(add)
        .children(checkout)
}

fn draft_project_trigger(
    current: &str,
    has_project: bool,
    open: bool,
    disabled: bool,
    t: &Theme,
) -> El {
    row()
        .key(format!("{DRAFT_PROJECT}-trigger"))
        .role("combobox")
        .label(format!("Project: {current}"))
        .expanded(open)
        .focusable(!disabled)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .h(Size::Default.height())
        .px(S8)
        .gap(S6)
        .center_y()
        .radius(RADIUS_CONTROL)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .min_w(0.0)
        .max_w(ui::MODEL_TRIGGER_MAX)
        .child(
            icon(&ui::FOLDER, ICON_SMALL)
                .color(if has_project { t.text } else { t.muted })
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text(current)
                .font(t.ui_medium)
                .color(if has_project { t.text } else { t.muted })
                .group_color(t.text)
                .min_w(0.0)
                .shrink()
                .truncate()
                .a11y_hidden(),
        )
        .child(
            icon(&ui::CHEVRON_OPEN, ICON_SMALL)
                .key("chevron")
                .icon_rotation(if open { std::f32::consts::PI } else { 0.0 })
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        )
}
