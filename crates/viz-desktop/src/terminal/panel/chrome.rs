//! Flat dock chrome and pane layout.

use std::path::Path;

use rune::{El, Phase, Tx, col, el, row, text};
use rune_kit::ButtonVariant;
use viz_client::protocol::{TerminalId, TerminalScope, TerminalSummary};

use super::{create, dispatch, focus_active, key, open_find, route, search_bar, shortcut, toggle};
use crate::{
    app::Desktop,
    model::terminal::{Accessory, Dock, Place, Split, scope},
    theme::{HAIRLINE, S2, S4, S8, Size, Theme},
    ui,
    update::terminal::{Command, TerminalMsg, rpc},
};

fn control(
    label: &str,
    disabled: bool,
    t: &Theme,
    action: impl Fn(&mut Desktop, &mut Tx) + 'static,
) -> El {
    ui::button(label, ButtonVariant::Ghost, Size::Compact)
        .disabled(disabled)
        .on_press_tx(action)
        .build(&t.k)
}

fn tab(app: &Desktop, terminal: &TerminalSummary, t: &Theme) -> El {
    let id = terminal.id;
    let active = app
        .model
        .terminal
        .active
        .is_some_and(|active| active == id || app.model.terminal.panes(id).1.contains(&active));
    let scope = match terminal.scope {
        TerminalScope::Thread { .. } => "Thread",
        TerminalScope::Project { .. } => "Project",
        TerminalScope::Agent { .. } => "ACP",
    };
    let live_title = app.widgets.terminal.views.get(&id).and_then(|view| {
        view.borrow()
            .viewer
            .terminal()
            .title()
            .ok()
            .filter(|title| !title.is_empty())
            .map(str::to_owned)
    });
    let title = live_title.as_deref().unwrap_or_else(|| {
        if terminal.title.is_empty() {
            Path::new(&terminal.cwd)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&terminal.cwd)
        } else {
            &terminal.title
        }
    });
    let label = format!("{title} · {scope}");
    let short_title = title.char_indices().nth(32).map_or_else(
        || label.clone(),
        |(end, _)| format!("{}… · {scope}", &title[..end]),
    );
    ui::button(short_title, ButtonVariant::Ghost, Size::Compact)
        .active(active)
        .on_press_tx(move |app: &mut Desktop, tx| {
            dispatch(app, TerminalMsg::Select(id), tx);
            focus_active(app, tx);
        })
        .build(&t.k)
        .label(&label)
        .help(&label)
        .key(format!("terminal-tab-{}", id.0))
}

fn pane(app: &Desktop, id: TerminalId, t: &Theme) -> El {
    let widgets = &app.widgets.terminal;
    let active = app.model.terminal.active == Some(id);
    let content = if let Some(view) = widgets.views.get(&id) {
        crate::terminal::view(
            &key(id),
            view,
            t,
            false,
            move |app, action, tx| route(app, id, action, tx),
            |app, event, tx| shortcut(app, event, tx, true),
        )
    } else if let Some(error) = widgets.errors.get(&id) {
        col()
            .padding(S8)
            .child(text(error).font(t.small).color(t.muted))
            .child(control(
                "Retry attachment",
                !app.model.connected(),
                t,
                move |app, tx| {
                    app.apply_effects(vec![rpc(Command::Attach(id))], Some(tx));
                },
            ))
    } else {
        text(if app.model.connected() {
            "Attaching terminal…"
        } else {
            "Terminal offline"
        })
        .font(t.small)
        .color(t.muted)
        .padding(S8)
    };
    col()
        .key(format!("terminal-cell-{}", id.0))
        .grow(1.0)
        .min_h(0.0)
        .min_w(0.0)
        .clip_soft(0.0)
        .padding(HAIRLINE)
        .fill(if active { t.accent } else { t.surface_recessed })
        .children(widgets.leases.contains(&id).then(|| {
            text("Another viewer controls the terminal size")
                .font(t.small)
                .color(t.muted)
                .px(S8)
        }))
        .child(content.grow(1.0).min_h(0.0))
        .children(
            app.model
                .terminal
                .summary(id)
                .and_then(|summary| summary.exit)
                .map(|exit| {
                    let label = match exit {
                        viz_terminal::Exit::Code(code) => format!("Process exited ({code})"),
                        viz_terminal::Exit::Signal(signal) => {
                            format!("Process ended by signal {signal}")
                        }
                        viz_terminal::Exit::Lost => "Process status unavailable".to_owned(),
                    };
                    row()
                        .fill(t.surface)
                        .px(S8)
                        .child(text(label).font(t.small).color(t.muted))
                }),
        )
}

pub fn view(app: &Desktop, t: &Theme) -> El {
    let dock = &app.model.terminal;
    let online = app.model.connected();
    let active = dock.active;
    let candidates = dock
        .sessions
        .iter()
        .filter(|summary| Dock::visible(&app.model, summary));
    let tabs = candidates.clone().filter(|summary| {
        let (_, panes) = dock.panes(summary.id);
        (!matches!(summary.scope, TerminalScope::Agent { .. }) || active == Some(summary.id))
            && panes.first().is_none_or(|first| *first == summary.id)
    });
    let commands =
        candidates.filter(|summary| matches!(summary.scope, TerminalScope::Agent { .. }));
    let has_commands = commands.clone().next().is_some();
    let mut body = col()
        .key("terminal-content")
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .instant_changes();
    if let Some(id) = active {
        let (split, panes) = dock.panes(id);
        let layout = if split == Split::Right { row() } else { col() };
        body = body.child(
            layout
                .key("terminal-split")
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .gap(S2)
                .children(
                    if panes.is_empty() {
                        std::slice::from_ref(&id)
                    } else {
                        panes
                    }
                    .iter()
                    .copied()
                    .filter(|id| {
                        dock.summary(*id)
                            .is_some_and(|summary| Dock::visible(&app.model, summary))
                    })
                    .map(|id| pane(app, id, t)),
                ),
        );
    } else {
        let message = if dock.creating.is_some() {
            "Starting shell…"
        } else {
            "No terminal in this checkout"
        };
        body = body.child(text(message).font(t.small).color(t.muted).padding(S8));
    }
    col()
        .key("terminal-dock")
        .w_full()
        .min_h(0.0)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .transition(crate::motion::slide_panel(rune::Edge::Bottom))
        .child(resize_handle())
        .child(
            row()
                .key("terminal-tabs")
                .center_y()
                .gap(S4)
                .px(S8)
                .w_full()
                .wrap()
                .children(tabs.map(|session| tab(app, session, t))),
        )
        .child(actions(app, t, has_commands))
        .children((dock.accessory == Accessory::Commands).then(|| {
            row()
                .key("terminal-commands")
                .wrap()
                .px(S8)
                .children(commands.map(|session| tab(app, session, t)))
        }))
        .children((!online).then(|| {
            text("Environment disconnected")
                .font(t.small)
                .color(t.warning)
                .padding(S8)
        }))
        .children(
            (dock.accessory == Accessory::Find).then(|| col().px(S8).child(search_bar(app, t))),
        )
        .child(body)
}

fn actions(app: &Desktop, t: &Theme, has_commands: bool) -> El {
    let dock = &app.model.terminal;
    let online = app.model.connected();
    let active = dock.active;
    let can_create = online && dock.creating.is_none();
    let can_split = can_create
        && active
            .and_then(|id| dock.summary(id))
            .is_some_and(|summary| !matches!(summary.scope, TerminalScope::Agent { .. }));
    row()
        .key("terminal-actions")
        .center_y()
        .gap(S4)
        .px(S8)
        .w_full()
        .wrap()
        .child(
            control(
                "New terminal",
                !can_create || scope(&app.model, false).is_none(),
                t,
                |app, tx| create(app, false, Place::Tab, tx),
            )
            .key("terminal-attach"),
        )
        .child(control(
            "Project shell",
            !can_create || scope(&app.model, true).is_none(),
            t,
            |app, tx| create(app, true, Place::Tab, tx),
        ))
        .child(control("Split right", !can_split, t, |app, tx| {
            if let Some(id) = app.model.terminal.active {
                create(app, false, Place::Split(id, Split::Right), tx);
            }
        }))
        .child(control("Split down", !can_split, t, |app, tx| {
            if let Some(id) = app.model.terminal.active {
                create(app, false, Place::Split(id, Split::Down), tx);
            }
        }))
        .child(control("Find", active.is_none(), t, open_find))
        .child(control(
            "Close terminal",
            !online || active.is_none(),
            t,
            |app, tx| {
                if let Some(id) = app.model.terminal.active {
                    dispatch(app, TerminalMsg::Close(id), tx);
                }
            },
        ))
        .child(control("ACP commands", !has_commands, t, |app, tx| {
            dispatch(app, TerminalMsg::Commands, tx);
        }))
        .child(control("Hide", false, t, toggle))
}

fn resize_handle() -> El {
    el().key("terminal-resize")
        .h(S4)
        .w_full()
        .cursor(rune::Cursor::ResizeRow)
        .on_drag(|app: &mut Desktop, drag, tx| {
            if drag.phase == Phase::Down {
                app.widgets.terminal.drag_height = crate::model::terminal::height(&app.model);
            } else {
                let height = app.widgets.terminal.drag_height - drag.dy;
                dispatch(
                    app,
                    TerminalMsg::Height(height, drag.phase == Phase::Up),
                    tx,
                );
                if drag.phase == Phase::Up {
                    focus_active(app, tx);
                }
            }
        })
}
