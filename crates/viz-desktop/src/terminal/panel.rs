//! Rune state and event routing for desktop terminal attachments.

mod chrome;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

pub use chrome::view;
use rune::{El, Ime, Key, KeyEvent, TextEdit, Tx, text};
use viz_client::protocol::{
    InteractiveScope, Sequence, Snapshot, TerminalId, TerminalScope, Update,
};
use viz_terminal::Viewer;

use super::{Action, TerminalView, search::Search};
use crate::{
    app::Desktop,
    model::terminal::{Place, Split, scope},
    theme::Theme,
    ui,
    update::{
        GitMsg, Msg,
        terminal::{Command, TerminalMsg, rpc},
    },
};

#[derive(Default)]
pub struct Widgets {
    views: BTreeMap<TerminalId, Rc<RefCell<TerminalView>>>,
    sequences: BTreeMap<TerminalId, Sequence>,
    pub errors: BTreeMap<TerminalId, String>,
    pub leases: BTreeSet<TerminalId>,
    query: TextEdit,
    search: Search,
    find_pane: Option<TerminalId>,
    drag_height: f32,
}

pub fn sync(app: &mut Desktop) {
    let dock = &app.model.terminal;
    let widgets = &mut app.widgets.terminal;
    let keep = |id: &TerminalId| {
        app.model.panels.terminal_open
            && (dock.attached.contains(id)
                || (!app.model.connected() && dock.summary(*id).is_some()))
    };
    widgets.views.retain(|id, _| keep(id));
    widgets.sequences.retain(|id, _| keep(id));
    widgets.errors.retain(|id, _| keep(id));
    widgets.leases.retain(keep);
    if widgets.find_pane != dock.active {
        widgets.find_pane = dock.active;
        widgets.search = Search::default();
        widgets.query = TextEdit::new("");
    }
}

fn key(id: TerminalId) -> String {
    format!("terminal-pane-{}", id.0)
}

fn dispatch(app: &mut Desktop, msg: TerminalMsg, tx: &mut Tx) {
    app.dispatch(Msg::Terminal(msg), tx);
}

pub fn snapshot(app: &mut Desktop, id: TerminalId, snapshot: &Snapshot, tx: &mut Tx) {
    if !app.model.terminal.attached.contains(&id) {
        return;
    }
    let widgets = &mut app.widgets.terminal;
    match Viewer::attach(snapshot).and_then(|viewer| Ok(TerminalView::new(viewer)?)) {
        Ok(mut view) => {
            view.ime_enabled = app.model.terminal.active == Some(id);
            widgets.views.insert(id, Rc::new(RefCell::new(view)));
            widgets.sequences.insert(id, snapshot.sequence);
            widgets.errors.remove(&id);
            if app.model.terminal.active == Some(id)
                && (tx.focused().is_none() || tx.focused() == Some("terminal-attach"))
            {
                tx.focus_when_shown(key(id), false);
            }
        }
        Err(error) => {
            widgets.errors.insert(id, error.to_string());
        }
    }
}

pub fn stream(app: &mut Desktop, id: TerminalId, update: &Update, tx: &mut Tx) {
    if !app.model.terminal.attached.contains(&id) {
        return;
    }
    let widgets = &mut app.widgets.terminal;
    let Some(previous) = widgets.sequences.get_mut(&id) else {
        return;
    };
    if *previous >= update.sequence {
        return;
    }
    if widgets
        .views
        .get(&id)
        .is_some_and(|view| view.borrow_mut().viewer.apply(update).is_ok())
    {
        *previous = update.sequence;
    } else {
        widgets.sequences.remove(&id);
        app.apply_effects(vec![rpc(Command::Attach(id))], Some(tx));
    }
}

fn route(app: &mut Desktop, id: TerminalId, action: Action, tx: &mut Tx) {
    if matches!(action, Action::Focus) {
        dispatch(app, TerminalMsg::Select(id), tx);
        return;
    }
    let dock = &app.model.terminal;
    if !dock.attached.contains(&id)
        || dock
            .summary(id)
            .is_some_and(|summary| summary.exit.is_some())
        || app.widgets.terminal.errors.contains_key(&id)
    {
        return;
    }
    let command = match action {
        Action::Input(bytes) => Command::Input(id, bytes),
        Action::Resize(size, cell) => Command::Resize(id, size, cell),
        Action::Focus => return,
    };
    app.apply_effects(vec![rpc(command)], Some(tx));
}

fn create(app: &mut Desktop, project: bool, place: Place, tx: &mut Tx) {
    let target = match place {
        Place::Split(anchor, _) => {
            app.model
                .terminal
                .summary(anchor)
                .and_then(|summary| match &summary.scope {
                    TerminalScope::Thread { thread } => {
                        Some(InteractiveScope::Thread { thread: *thread })
                    }
                    TerminalScope::Project { project } => Some(InteractiveScope::Project {
                        project: project.clone(),
                    }),
                    TerminalScope::Agent { .. } => None,
                })
        }
        Place::Tab => scope(&app.model, project),
    };
    if let Some(scope) = target {
        dispatch(app, TerminalMsg::Create(scope, place), tx);
    }
}

pub fn focus_active(app: &Desktop, tx: &mut Tx) {
    if let Some(id) = app.model.terminal.active {
        if let Some(view) = app.widgets.terminal.views.get(&id) {
            view.borrow_mut().ime_enabled = true;
        }
        tx.focus_when_shown(key(id), false);
    } else {
        tx.focus_when_shown("terminal-attach", false);
    }
}

pub fn toggle(app: &mut Desktop, tx: &mut Tx) {
    app.dispatch(Msg::Git(GitMsg::ToggleTerminal), tx);
    if app.model.panels.terminal_open {
        focus_active(app, tx);
    }
}

pub fn shortcut(app: &mut Desktop, event: &KeyEvent, tx: &mut Tx, focused: bool) -> bool {
    let mac = cfg!(target_os = "macos");
    if event.is_command('j')
        && !event.mods.alt
        && event.mods.shift == (focused && !mac)
        && (app.model.panels.terminal_open || scope(&app.model, false).is_some())
    {
        toggle(app, tx);
        return true;
    }
    if !app.model.panels.terminal_open {
        return false;
    }
    if event.is_command('t') && event.mods.shift && !event.mods.alt {
        create(app, false, Place::Tab, tx);
        return true;
    }
    if !focused || (!mac && !event.mods.shift) {
        return false;
    }
    let Some(id) = app.model.terminal.active else {
        return false;
    };
    if event.is_command('d') {
        let direction = if (mac && event.mods.shift) || event.mods.alt {
            Split::Down
        } else {
            Split::Right
        };
        create(app, false, Place::Split(id, direction), tx);
    } else if event.is_command('w') && !event.mods.alt {
        dispatch(app, TerminalMsg::Close(id), tx);
    } else if event.is_command('f') && !event.mods.alt {
        open_find(app, tx);
    } else {
        return false;
    }
    true
}

fn open_find(app: &mut Desktop, tx: &mut Tx) {
    dispatch(app, TerminalMsg::Find(true), tx);
    tx.focus_when_shown("terminal-find", true);
}

fn search(app: &mut Desktop, step: isize) {
    let widgets = &mut app.widgets.terminal;
    if let Some(view) = app
        .model
        .terminal
        .active
        .and_then(|id| widgets.views.get(&id))
    {
        view.borrow_mut()
            .find(&widgets.query.text, step, &mut widgets.search);
    }
}

fn search_bar(app: &Desktop, t: &Theme) -> El {
    let widgets = &app.widgets.terminal;
    let status = &widgets.search;
    let label = if status.count == 0 {
        "No matches".to_owned()
    } else {
        format!("{}/{}", status.index + 1, status.count)
    };
    ui::search_input(&widgets.query, "Find in scrollback")
        .input_key("terminal-find")
        .on_edit(|app: &mut Desktop| &mut app.widgets.terminal.query)
        .on_change(|app, _| search(app, 0))
        .on_key(|app, event, tx| match event.key {
            Key::Escape => {
                dispatch(app, TerminalMsg::Find(false), tx);
                focus_active(app, tx);
                true
            }
            Key::Enter => {
                search(app, if event.mods.shift { -1 } else { 1 });
                true
            }
            _ => false,
        })
        .trailing(
            text(if widgets.query.text.is_empty() {
                String::new()
            } else {
                label
            })
            .font(t.small)
            .color(t.muted),
        )
        .build(t)
}

/// Mounts a local snapshot for a deterministic headless preview.
pub fn preview(
    app: &mut Desktop,
    id: TerminalId,
    size: viz_terminal::Size,
    output: &str,
    exit: Option<viz_terminal::Exit>,
    query: &str,
) {
    let view = || -> Result<TerminalView, viz_terminal::EmulationError> {
        let mut screen = viz_terminal::LiveScreen::new(size, super::CELL)?;
        screen.output(output.as_bytes().to_vec(), |_| {});
        if let Some(exit) = exit {
            screen.exit(exit);
        }
        let viewer = Viewer::attach(&screen.snapshot()?)?;
        Ok(TerminalView::new(viewer)?)
    };
    match view() {
        Ok(view) => {
            app.widgets
                .terminal
                .views
                .insert(id, Rc::new(RefCell::new(view)));
            if !query.is_empty() {
                app.widgets.terminal.query = TextEdit::new(query);
                search(app, 0);
            }
        }
        Err(error) => tracing::error!(%error, "could not initialize dock preview"),
    }
}

pub fn ime(app: &mut Desktop, event: &Ime, tx: &mut Tx) -> bool {
    let Some(id) = app.model.terminal.active else {
        return false;
    };
    let Some(view) = app.widgets.terminal.views.get(&id) else {
        return false;
    };
    match event {
        Ime::Disabled => view.borrow_mut().preedit.clear(),
        _ if tx.focused() != Some(key(id).as_str()) => return false,
        Ime::Preedit(text, ..) => view.borrow_mut().preedit.clone_from(text),
        Ime::Commit(text) => {
            let mut view = view.borrow_mut();
            view.preedit.clear();
            view.scroll(ghostty_vt::terminal::ScrollViewport::Bottom);
            drop(view);
            route(app, id, Action::Input(text.as_bytes().to_vec()), tx);
        }
        _ => {}
    }
    true
}
