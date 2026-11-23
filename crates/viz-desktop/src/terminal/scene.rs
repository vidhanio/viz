//! Headless terminal scenes generated through the same VT attachment API as
//! live viewers.

use std::{cell::RefCell, fmt::Write as _, rc::Rc};

use ghostty_vt::{
    selection::Selection,
    terminal::{Point, PointCoordinate},
};
use rune::El;
use viz_terminal::{Exit, LiveScreen, Size, Viewer};

use super::{Action, CELL, TerminalView};
use crate::{app::Desktop, theme::Theme};

thread_local! {
    static SCENE: RefCell<Option<Scene>> = const { RefCell::new(None) };
}

struct Scene {
    screen: LiveScreen,
    view: Rc<RefCell<TerminalView>>,
    actions: Rc<RefCell<Vec<Action>>>,
    alternate: bool,
    revision: u64,
}

pub fn shell(_: &mut Desktop) {
    install(Preset::Shell);
}

pub fn alternate(_: &mut Desktop) {
    install(Preset::Alternate);
}

pub fn selection(_: &mut Desktop) {
    install(Preset::Selection);
}

pub fn exited(_: &mut Desktop) {
    install(Preset::Exited);
}

#[derive(Clone, Copy)]
enum Preset {
    Shell,
    Alternate,
    Selection,
    Exited,
}

fn install(preset: Preset) {
    match Scene::new(preset) {
        Ok(scene) => SCENE.set(Some(scene)),
        Err(error) => tracing::error!(%error, "could not initialize terminal scene"),
    }
}

pub fn view(theme: &Theme) -> Option<El> {
    SCENE.with_borrow_mut(|scene| {
        let scene = scene.as_mut()?;
        scene.advance();
        let actions = Rc::clone(&scene.actions);
        Some(super::view::<Desktop>(
            "terminal-scene",
            &scene.view,
            theme,
            move |_, action| {
                actions.borrow_mut().push(action);
            },
        ))
    })
}

/// Routes composition only while the headless scene owns the keyboard.
pub fn ime(_: &mut Desktop, event: &rune::Ime, tx: &mut rune::Tx) -> bool {
    SCENE.with_borrow_mut(|scene| {
        let Some(scene) = scene.as_mut() else {
            return false;
        };
        let mut view = scene.view.borrow_mut();
        match event {
            rune::Ime::Disabled => view.preedit.clear(),
            _ if tx.focused() != Some("terminal-scene") => return false,
            rune::Ime::Preedit(text, ..) => view.preedit.clone_from(text),
            rune::Ime::Commit(text) => {
                view.preedit.clear();
                scene
                    .actions
                    .borrow_mut()
                    .push(Action::Input(text.as_bytes().to_vec()));
            }
            _ => {}
        }
        true
    })
}

/// Keeps window focus reports ordered with the terminal's other input.
pub fn focus(app: &mut Desktop, focused: bool, tx: &mut rune::Tx) {
    app.set_focus(focused, tx);
    SCENE.with_borrow_mut(|scene| {
        if let Some(scene) = scene {
            let view = scene.view.borrow();
            if !view.ime_enabled {
                return;
            }
            let event = if focused {
                ghostty_vt::focus::Event::Gained
            } else {
                ghostty_vt::focus::Event::Lost
            };
            if let Ok(Some(bytes)) = super::input::focus(view.viewer.terminal(), event) {
                scene.actions.borrow_mut().push(Action::Input(bytes));
            }
        }
    });
}

impl Scene {
    fn new(preset: Preset) -> Result<Self, viz_terminal::EmulationError> {
        let mut screen = LiveScreen::new(Size::new(80, 18), CELL)?;
        let alternate = matches!(preset, Preset::Alternate);
        let code = if alternate {
            fullscreen(0, 18)
        } else {
            shell_output()
        };
        let _ = screen.output(code.into_bytes(), |_| {});
        if matches!(preset, Preset::Exited) {
            let _ = screen.exit(Exit::Code(0));
        }
        let viewer = Viewer::attach(&screen.snapshot()?)?;
        if matches!(preset, Preset::Selection) {
            let terminal = viewer.terminal();
            let a = terminal.grid_ref(Point::Viewport(PointCoordinate { x: 0, y: 3 }))?;
            let b = terminal.grid_ref(Point::Viewport(PointCoordinate { x: 36, y: 5 }))?;
            terminal.set_selection(Some(&Selection::new(a, b, false)))?;
        }
        Ok(Self {
            screen,
            view: Rc::new(RefCell::new(TerminalView::new(viewer)?)),
            actions: Rc::default(),
            alternate,
            revision: 0,
        })
    }

    fn advance(&mut self) {
        for action in self.actions.take() {
            let mut state = self.view.borrow_mut();
            let update = match action {
                Action::Input(bytes) if self.alternate && bytes == b" " => {
                    self.revision += 1;
                    let rows = state.viewer.terminal().rows().unwrap_or(18);
                    self.screen
                        .output(fullscreen(self.revision, rows).into_bytes(), |_| {})
                }
                Action::Resize(size, cell) if self.alternate => {
                    if let Ok(update) = self.screen.resize(size, cell, |_| {}) {
                        let _ = state.viewer.apply(&update);
                    }
                    self.screen
                        .output(fullscreen(self.revision, size.rows()).into_bytes(), |_| {})
                }
                Action::Input(_) | Action::Resize(_, _) => continue,
            };
            let _ = state.viewer.apply(&update);
        }
    }
}

fn shell_output() -> String {
    concat!(
        "\x1b]0;viz — bash\x07\x1b]7;file://localhost/work/viz\x07",
        "\x1b[1;32m~/work/viz\x1b[0m \x1b[36moverhaul/terminal\x1b[0m $ cargo clippy\r\n",
        "    \x1b[1;32mChecking\x1b[0m viz-terminal v0.1.0\r\n",
        "    \x1b[1;32mFinished\x1b[0m dev profile in 0.56s\r\n",
        "\x1b[1;32m~/work/viz\x1b[0m $ ls crates\r\n",
        "\x1b[34mviz-client      viz-desktop      viz-protocol\x1b[0m\r\n",
        "\x1b[34mviz-config      viz-server       viz-terminal\x1b[0m\r\n",
        "\x1b[1;32m~/work/viz\x1b[0m $ \x1b[5 q",
    )
    .to_owned()
}

fn fullscreen(revision: u64, rows: u16) -> String {
    let mut data = String::from("\x1b[?1049h\x1b[?25l\x1b[H\x1b[2J\x1b[1;36mProcess list\x1b[0m");
    let _ = write!(data, "    frame {revision}    Space to redraw\r\n\r\n");
    data.push_str("\x1b[1;7m   PID USER       CPU% MEM% COMMAND                   \x1b[0m\r\n");
    for index in 0..rows.saturating_sub(3) {
        let name = ["cargo", "rustc", "viz-server", "bash"][usize::from(index % 4)];
        let load = (3 + u64::from(index) * 5 + revision) % 87;
        let _ = write!(
            data,
            "\x1b[3{}m{:>6} viz       {:>3}%   1% {name}\x1b[0m",
            2 + (u64::from(index) + revision) % 5,
            1601 + index,
            load
        );
        if index + 1 < rows.saturating_sub(3) {
            data.push_str("\r\n");
        }
    }
    data
}
