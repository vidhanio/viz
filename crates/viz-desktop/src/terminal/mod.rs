//! Rune viewport for a Ghostty terminal attachment.

mod input;
mod paint;
pub mod panel;
pub mod scene;
mod search;

use std::{cell::RefCell, rc::Rc};

use libghostty_vt::{
    Terminal,
    fmt::{Format, Formatter},
    grid_ref::TrackedGridRef,
    point::{Coordinate, Point},
    render::{CellIterator, RenderState, RowIterator},
    selection::Selection,
    terminal::ScrollViewport,
};
use rune::{Drag, El, Phase, el, text};
use viz_terminal::{CellSize, Size, Viewer};

use crate::theme::Theme;

pub const CELL: CellSize = CellSize {
    width: 8,
    height: 18,
};
const PAD: f32 = 12.0;

/// Actions for the surrounding desktop session to route.
pub enum Action {
    Input(Vec<u8>),
    Resize(Size, CellSize),
    Focus,
}

pub struct TerminalView {
    viewer: Viewer,
    render: RenderState<'static>,
    rows: RowIterator<'static>,
    cells: CellIterator<'static>,
    encoder: input::Encoder,
    cache: paint::Frame,
    anchor: Option<TrackedGridRef>,
    requested: Option<Size>,
    preedit: String,
    ime_enabled: bool,
}

impl TerminalView {
    pub(super) fn new(viewer: Viewer) -> Result<Self, libghostty_vt::Error> {
        Ok(Self {
            viewer,
            render: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
            encoder: input::Encoder::new()?,
            cache: paint::Frame::default(),
            anchor: None,
            requested: None,
            preedit: String::new(),
            ime_enabled: false,
        })
    }

    fn refresh(&mut self) -> Result<(), libghostty_vt::Error> {
        let snapshot = self.render.update(self.viewer.terminal())?;
        self.cache
            .update(&snapshot, &mut self.rows, &mut self.cells)
    }

    fn scroll(&mut self, scroll: ScrollViewport) {
        self.viewer.terminal_mut().scroll_viewport(scroll);
    }

    fn select(&mut self, drag: &Drag) -> Result<(), libghostty_vt::Error> {
        let terminal = self.viewer.terminal();
        let point = coordinate(terminal, drag.x, drag.y)?;
        if drag.phase == Phase::Down {
            self.anchor = Some(terminal.track_grid_ref(Point::Viewport(point))?);
            terminal.set_selection(None)?;
        } else if let Some(anchor) = &self.anchor {
            if !drag.cancelled()
                && let Some(start) = anchor.snapshot(terminal)?
            {
                let end = terminal.grid_ref(Point::Viewport(point))?;
                terminal.set_selection(Some(&Selection::new(start, end, false)))?;
            }
            if drag.phase == Phase::Up {
                self.anchor = None;
            }
        }
        Ok(())
    }

    fn link(&self, x: f32, y: f32) -> Result<Option<String>, libghostty_vt::Error> {
        let terminal = self.viewer.terminal();
        let point = coordinate(terminal, x, y)?;
        let cell = terminal.grid_ref(Point::Viewport(point))?;
        let mut bytes = [0; 4096];
        let len = cell.hyperlink_uri(&mut bytes)?;
        Ok(str::from_utf8(&bytes[..len])
            .ok()
            .filter(|link| link.starts_with("https://") || link.starts_with("http://"))
            .map(str::to_owned))
    }
}

/// Builds the terminal element. State and routing stay with the caller so the
/// view can be dropped when its panel closes and recreated on attachment.
#[expect(
    clippy::too_many_lines,
    reason = "each Rune event binding stays with its element"
)]
pub fn view<S: 'static>(
    key: &str,
    model: &Rc<RefCell<TerminalView>>,
    theme: &Theme,
    show_exit: bool,
    action: impl Fn(&mut S, Action, &mut rune::Tx) + 'static,
    shortcut: impl Fn(&mut S, &rune::KeyEvent, &mut rune::Tx) -> bool + 'static,
) -> El {
    let action = Rc::new(action);
    let mut state = model.borrow_mut();
    let rendered = if state.refresh().is_ok() {
        state.cache.view(
            key,
            theme,
            state.viewer.exit(),
            show_exit,
            state.ime_enabled,
            &state.preedit,
        )
    } else {
        text("Terminal display unavailable")
            .font(theme.mono)
            .color(theme.muted)
    };
    drop(state);

    let key_state = Rc::clone(model);
    let key_action = Rc::clone(&action);
    let wheel_state = Rc::clone(model);
    let wheel_action = Rc::clone(&action);
    let drag_state = Rc::clone(model);
    let drag_action = Rc::clone(&action);
    let resize_state = Rc::clone(model);
    let focus_key = key.to_owned();
    el().key(key.to_owned())
        .label("Terminal viewport")
        .w_full()
        .h_full()
        .clip_soft(0.0)
        .fill(theme.k.c.bg_base)
        .focusable(true)
        .child(rendered)
        .on_key(move |app: &mut S, event, tx| {
            if shortcut(app, event, tx) {
                return true;
            }
            let mut state = key_state.borrow_mut();
            if event.mods.cmd && event.mods.shift {
                match event.key {
                    rune::Key::Home => {
                        state.scroll(ScrollViewport::Top);
                        return true;
                    }
                    rune::Key::End => {
                        state.scroll(ScrollViewport::Bottom);
                        return true;
                    }
                    _ => {}
                }
            }
            if (cfg!(target_os = "macos") || event.mods.shift) && event.is_command('c') {
                if let Ok(Some(text)) = copy_selection(state.viewer.terminal()) {
                    tx.copy(text);
                }
                return true;
            }
            if (cfg!(target_os = "macos") || event.mods.shift) && event.is_command('v') {
                let state = Rc::clone(&key_state);
                let action = Rc::clone(&key_action);
                tx.read_clipboard(move |app: &mut S, clipboard, tx| {
                    if let Ok(bytes) =
                        input::paste(state.borrow().viewer.terminal(), clipboard.text())
                    {
                        action(app, Action::Input(bytes), tx);
                    }
                });
                return true;
            }
            if event.mods.shift && matches!(event.key, rune::Key::PageUp | rune::Key::PageDown) {
                let rows = state
                    .viewer
                    .terminal()
                    .rows()
                    .map_or(0, |rows| isize::try_from(rows).unwrap_or_default());
                state.scroll(ScrollViewport::Delta(if event.key == rune::Key::PageUp {
                    -rows
                } else {
                    rows
                }));
                return true;
            }
            let TerminalView {
                viewer, encoder, ..
            } = &mut *state;
            if let Ok(Some(bytes)) = encoder.key(viewer.terminal(), event, tx.key_base()) {
                if viewer.exit().is_none() {
                    viewer
                        .terminal_mut()
                        .scroll_viewport(ScrollViewport::Bottom);
                    key_action(app, Action::Input(bytes), tx);
                }
                return true;
            }
            false
        })
        .on_wheel(move |app: &mut S, event: rune::WheelEvent, tx| {
            let mut state = wheel_state.borrow_mut();
            if event.dy == 0.0 {
                return false;
            }
            let TerminalView {
                viewer, encoder, ..
            } = &mut *state;
            if !event.mods.shift && matches!(viewer.terminal().is_mouse_tracking(), Ok(true)) {
                let button = if event.dy > 0.0 {
                    libghostty_vt::mouse::Button::Four
                } else {
                    libghostty_vt::mouse::Button::Five
                };
                if let Ok(bytes) = encoder.mouse(
                    viewer.terminal(),
                    libghostty_vt::mouse::Action::Press,
                    button,
                    [event.x, event.y],
                    event.mods,
                ) {
                    wheel_action(app, Action::Input(bytes), tx);
                }
            } else {
                state.scroll(ScrollViewport::Delta(if event.dy > 0.0 { -3 } else { 3 }));
            }
            true
        })
        .on_drag(move |app: &mut S, drag, tx| {
            if drag.phase == Phase::Down {
                drag_action(app, Action::Focus, tx);
                tx.focus(&focus_key, false);
            }
            let mut state = drag_state.borrow_mut();
            if drag.phase == Phase::Down && !state.ime_enabled {
                state.ime_enabled = true;
                if let Ok(Some(bytes)) =
                    input::focus(state.viewer.terminal(), libghostty_vt::focus::Event::Gained)
                {
                    drag_action(app, Action::Input(bytes), tx);
                }
            }
            let mods = tx.mods();
            let TerminalView {
                viewer, encoder, ..
            } = &mut *state;
            if !mods.shift && matches!(viewer.terminal().is_mouse_tracking(), Ok(true)) {
                let mouse = match drag.phase {
                    Phase::Down => libghostty_vt::mouse::Action::Press,
                    Phase::Move => libghostty_vt::mouse::Action::Motion,
                    Phase::Up => libghostty_vt::mouse::Action::Release,
                };
                if let Ok(bytes) = encoder.mouse(
                    viewer.terminal(),
                    mouse,
                    libghostty_vt::mouse::Button::Left,
                    [drag.x, drag.y],
                    mods,
                ) {
                    drag_action(app, Action::Input(bytes), tx);
                }
            } else if mods.cmd && drag.phase == Phase::Down {
                if let Ok(Some(link)) = state.link(drag.x, drag.y) {
                    let _ = tx.open_url(&link);
                }
            } else {
                let _ = state.select(drag);
            }
        })
        .on_layout_tx(move |app: &mut S, rect, tx| {
            let columns = cells(2.0_f32.mul_add(-PAD, rect[2]), CELL.width);
            let rows = cells(2.0_f32.mul_add(-PAD, rect[3]), CELL.height);
            let size = Size::new(columns, rows);
            let mut state = resize_state.borrow_mut();
            if state.requested != Some(size) {
                state.requested = Some(size);
                action(app, Action::Resize(size, CELL), tx);
            }
        })
}

fn copy_selection(terminal: &Terminal) -> Result<Option<String>, libghostty_vt::Error> {
    let Some(mut formatter) = Formatter::new_selection(terminal)? else {
        return Ok(None);
    };
    formatter
        .set_format(Format::Plain)?
        .set_unwrap(false)?
        .set_trim(false)?;
    let bytes = formatter.format_alloc(None)?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

fn coordinate(terminal: &Terminal, x: f32, y: f32) -> Result<Coordinate, libghostty_vt::Error> {
    Ok(Coordinate {
        x: cells(x - PAD, CELL.width).min(terminal.cols()?.saturating_sub(1)),
        y: u32::from(cells(y - PAD, CELL.height).min(terminal.rows()?.saturating_sub(1))),
    })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "nonnegative clamped point coordinates become cell indices"
)]
fn cells(points: f32, metric: u16) -> u16 {
    (points / f32::from(metric)).clamp(0.0, f32::from(u16::MAX)) as u16
}
