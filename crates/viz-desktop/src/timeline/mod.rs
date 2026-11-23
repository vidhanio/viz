//! Transcript, activity, plan checklists, expandable tool records and
//! subagent cards, in the reading column.

mod activity;
mod message;
mod output;
mod subagents;

use rune::{El, ScrollFade, ScrollSpec, col, icon, row, text};
use rune_kit::{
    ButtonVariant, IconButtonVariant, Menu, MenuItem, Placement, ScrollBar, ScrollBarStyle,
};

use crate::{
    app::{Desktop, motion_switch},
    checkout,
    model::{Item, NewThread, Status, ToolStatus, is_activity},
    motion,
    theme::{
        EMPTY_TOP, EMPTY_WIDTH, HAIRLINE, ICON_SMALL, RADIUS_CONTROL, S4, S6, S8, S12, S16, S24,
        S32, Size, Theme,
    },
    ui,
    update::{ComposerMsg, Msg, SelectionMsg, SidebarMsg, TimelineMsg},
};

/// The `open_picker` value of a draft's project picker.
pub const DRAFT_PROJECT: &str = "draft-project";

/// What a row is, for the space above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Mine,
    Prose,
    Activity,
    TurnEnd,
    Notice,
}

/// Points above a row of kind `next` that follows one of kind `prev`.
const fn space(prev: Kind, next: Kind) -> f32 {
    match (prev, next) {
        (_, Kind::TurnEnd) => 0.0,
        (Kind::Activity, Kind::Activity) => S4,
        (Kind::Mine, _) => S24,
        (_, Kind::Mine) => S32,
        _ => S16,
    }
}

/// Rows in order, each spaced from the one before by its kind.
struct Rows {
    rows: Vec<El>,
    last: Option<Kind>,
}

impl Rows {
    fn push(&mut self, kind: Kind, el: El) {
        let gap = self.last.map_or(0.0, |prev| space(prev, kind));
        self.rows.push(el.mt(gap));
        self.last = Some(kind);
    }
}

/// The scrolling timeline, or what to show in its place. Each thread (and
/// draft) is its own keyed page with its own scroll, so changing threads
/// slides the old page out and the new one in along the thread switch vector.
pub fn timeline(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let thread = m.transcript();
    let empty = thread.items.is_empty() && thread.before.is_none() && !m.busy();
    let body = if m.is_loading() && m.connected() {
        loading(t)
    } else if empty {
        empty_state(s, t, layer).pt(EMPTY_TOP)
    } else {
        let mut rows = Rows {
            rows: Vec::new(),
            last: None,
        };
        if thread.before.is_some() {
            rows.push(Kind::Notice, load_earlier(s, t));
        }
        items(
            s,
            thread
                .displayed_history()
                .iter()
                .map(|item| (item, false))
                .chain(thread.acp_partial.iter().map(|item| (item, true))),
            &mut rows,
            t,
        );
        live(s, &mut rows, t);
        col().key("items").children(rows.rows)
    };
    let body = ui::reading(body)
        .key("timeline-body")
        .min_h(0.0)
        .pt(S24)
        .pb(S24);
    let page = match (m.selection.selected, &m.selection.new_thread) {
        (Some(id), _) => format!("page-{id}"),
        (None, Some(draft)) => format!("page-draft-{}", draft.id),
        (None, None) => "page-none".to_owned(),
    };
    let switch = motion_switch(m.selection.switch);
    col()
        .key("timeline")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .clip_rounded(0.0)
        .child_exit(switch.exit(motion::TRAVEL_PANE))
        .child(
            col()
                .key(page)
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .transition(switch.enter(motion::TRAVEL_PANE))
                .scroll(ScrollSpec { stick_bottom: true })
                .scroll_fade(ScrollFade::strength(S24).linear())
                .scroll_anchor()
                .child(body)
                .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k)),
        )
}

/// A thread whose transcript hasn't arrived: nothing, then a quiet radar
/// spinner and caption if it takes a while.
fn loading(t: &Theme) -> El {
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

/// Older history, between two rules.
fn load_earlier(s: &Desktop, t: &Theme) -> El {
    let m = &s.model;
    let label = if m.is_history_loading() {
        "Loading earlier messages…"
    } else {
        "Load earlier messages"
    };
    row()
        .key("load-earlier")
        .w_full()
        .gap(S8)
        .center_y()
        .child(ui::rule(t).grow(1.0).w(0.0))
        .child(
            ui::button(label, ButtonVariant::GhostMuted, Size::Small)
                .disabled(m.is_history_loading() || !m.connected())
                .on_press_tx(|s: &mut Desktop, tx| {
                    s.dispatch(Msg::Timeline(TimelineMsg::LoadEarlier), tx);
                })
                .build(&t.k),
        )
        .child(ui::rule(t).grow(1.0).w(0.0))
}

fn items<'a>(
    s: &Desktop,
    items: impl Iterator<Item = (&'a Item, bool)> + Clone,
    rows: &mut Rows,
    t: &Theme,
) {
    let mut items = items;
    while let Some((item, streaming)) = items.next() {
        match item {
            Item::User(user) => rows.push(Kind::Mine, message::mine(user, t)),
            Item::Text { key, text } => {
                let mut content = message::prose(key, text, streaming, t);
                if let Some(end) = items
                    .clone()
                    .take_while(|(item, _)| !matches!(item, Item::User(_)))
                    .find_map(|(item, _)| match item {
                        Item::TurnEnd(end) => Some(end),
                        _ => None,
                    })
                {
                    content = content.hover_group_shared(end.key.as_str());
                }
                rows.push(Kind::Prose, content);
            }
            Item::Thinking { .. } | Item::Tool(_) => {
                let count = items
                    .clone()
                    .take_while(|(item, _)| is_activity(item))
                    .count();
                let block = match item {
                    Item::Tool(tool) if count == 0 => activity::single_tool(s, tool, t),
                    _ => {
                        let burst =
                            std::iter::once((item, streaming)).chain(items.clone().take(count));
                        activity::activity_group(s, item.key(), &burst, t)
                    }
                };
                rows.push(Kind::Activity, block);
                for _ in 0..count {
                    items.next();
                }
            }
            Item::Plan { key, steps } => rows.push(Kind::Prose, message::plan(key, steps, t)),
            Item::Continued { key } => rows.push(
                Kind::Activity,
                activity::line(key, activity::mark(&ui::AGENT, t))
                    .child(text("Continued on its own").font(t.ui).color(t.muted)),
            ),
            Item::TurnStatus { key, text: words } => rows.push(
                Kind::Activity,
                activity::line(key, activity::mark(&ui::CIRCLE_STOP, t))
                    .role("status")
                    .child(text(words).font(t.ui).color(t.muted).truncate().shrink()),
            ),
            Item::TaskResult(result) => {
                rows.push(Kind::Activity, subagents::task_result(s, result, t));
            }
            Item::Notice { key, text } => rows.push(Kind::Notice, message::notice(key, text, t)),
            Item::TurnEnd(end) => {
                if let Some(el) = message::turn_end(end, t) {
                    rows.push(Kind::TurnEnd, el);
                }
            }
        }
    }
}

/// A quiet line while the run is working but shows nothing else that moves.
fn live(s: &Desktop, rows: &mut Rows, t: &Theme) {
    let m = &s.model;
    if !m.busy() {
        return;
    }
    let transcript = m.transcript();
    let running = transcript
        .items
        .iter()
        .rev()
        .take_while(|item| !matches!(item, Item::User(_)))
        .any(|item| matches!(item, Item::Tool(tool) if tool.status == ToolStatus::Running));
    if transcript.acp_partial.is_empty() && !running {
        rows.push(
            Kind::Activity,
            activity::line("working", ui::spinner(t))
                .role("status")
                .label("Working")
                .child(text("Working").font(t.ui).color(t.muted)),
        );
    }
}

/// What to show when the timeline has no messages.
fn empty_state(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    match &m.connection.status {
        Status::Error(message) => {
            let detail = if message.trim().is_empty() {
                "Start viz-server to reconnect."
            } else {
                message.as_str()
            };
            let retry = ui::button("Retry", ButtonVariant::Neutral, Size::Default)
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
        let new_thread = ui::button("New thread", ButtonVariant::Submit, Size::Large)
            .icon(&ui::COMPOSE)
            .on_press_tx(|s: &mut Desktop, tx| {
                s.dispatch(Msg::Selection(SelectionMsg::CreateHere), tx);
            })
            .build(&t.k);
        let (detail, actions) = if m.cache().has_projects() {
            ("Select a thread or start a new one.", new_thread)
        } else {
            let add_project = ui::button("Add project", ButtonVariant::Neutral, Size::Large)
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

/// An empty thread or fresh draft: on a draft, just project selection; on an
/// existing empty thread, only the project name.
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
    let add = ui::icon_button(
        &ui::FOLDER_PLUS,
        "Add project",
        IconButtonVariant::GhostMuted,
        Size::Default,
    )
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
