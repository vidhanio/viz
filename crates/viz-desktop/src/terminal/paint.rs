//! Convert dirty Ghostty rows into cached Rune text runs.

use std::sync::atomic::{AtomicU64, Ordering};

use ghostty_vt::{
    render::{CellIteration, CellIterator, Cursor, CursorVisualStyle, RowIterator, Snapshot},
    screen::CellWide,
    style::{RgbColor, Style, StyleColor, Underline},
};
use rune::{Color, El, col, el, memo, text};
use viz_terminal::Exit;

use super::{CELL, PAD};
use crate::theme::Theme;

pub(super) struct Frame {
    id: u64,
    rows: Vec<Line>,
    cursor: Option<Cursor>,
    palette: Option<[RgbColor; 256]>,
}

impl Default for Frame {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            rows: Vec::new(),
            cursor: None,
            palette: None,
        }
    }
}

#[derive(Default)]
struct Line {
    runs: Vec<Run>,
    version: u64,
}

struct Run {
    column: u16,
    width: u16,
    value: String,
    style: Style,
    selected: bool,
    wide: bool,
}

impl Frame {
    pub(super) fn update(
        &mut self,

        snapshot: &Snapshot<'_>,
        rows: &mut RowIterator,
        cells: &mut CellIterator,
    ) -> Result<(), ghostty_vt::Error> {
        self.cursor = Some(snapshot.cursor()?);
        let palette = snapshot.colors()?.palette;
        let size = usize::from(snapshot.rows()?);
        if self.rows.len() != size || self.palette != Some(palette) {
            self.rows.resize_with(size, Line::default);
            self.palette = Some(palette);
            snapshot.set_dirty(ghostty_vt::render::Dirty::Full)?;
        }
        let mut rows = rows.update(snapshot)?;
        while let Some((y, row)) = rows.next_dirty() {
            let line = &mut self.rows[usize::from(y)];
            line.runs.clear();
            let mut cells = cells.update(row)?;
            let mut x = 0;
            let mut grapheme = String::new();
            while let Some(cell) = cells.next() {
                line.push(cell, x, &mut grapheme)?;
                x += 1;
            }
            line.version = line.version.wrapping_add(1);
            row.set_dirty(false)?;
        }
        snapshot.clean()?;
        Ok(())
    }

    pub(super) fn view(
        &self,
        key: &str,
        theme: &Theme,
        exit: Option<Exit>,
        ime_enabled: bool,
        preedit: &str,
    ) -> El {
        let palette = self.palette.as_ref();
        let colors = ansi(theme);
        let mut grid = col().pin(Some(PAD), Some(PAD), Some(PAD), None);
        for (index, line) in self.rows.iter().enumerate() {
            grid = grid.child(memo(
                format!("{key}-row-{index}"),
                (self.id, line.version, colors, theme.selected),
                || line.view(theme, palette),
            ));
        }

        if let Some(exit) = exit {
            let label = match exit {
                Exit::Code(code) => format!("Process exited ({code})"),
                Exit::Signal(signal) => format!("Process ended by signal {signal}"),
                Exit::Lost => "Process status unavailable".to_owned(),
            };
            grid = grid.child(text(label).font(theme.mono).color(theme.muted));
        }

        let mut layer = el().w_full().h_full().child(grid);
        if exit.is_none()
            && let Some(cursor) = self.cursor
            && cursor.visible
            && let Some(position) = cursor.viewport
        {
            layer = layer.child(cursor_el(cursor, position, theme));
            if ime_enabled {
                let x = f32::from(position.x).mul_add(f32::from(CELL.width), PAD);
                let y = f32::from(position.y).mul_add(f32::from(CELL.height), PAD);
                layer = layer.child(
                    el().pin(Some(x), Some(y), None, None)
                        .min_w(f32::from(CELL.width))
                        .h(f32::from(CELL.height))
                        .ime_cursor()
                        .ime_option_as_alt(true)
                        .child(
                            text(preedit)
                                .font(theme.mono)
                                .color(theme.text)
                                .fill(theme.selected),
                        ),
                );
            }
        }
        layer
    }
}

impl Line {
    fn push(
        &mut self,
        cell: &CellIteration<'_>,
        x: u16,
        value: &mut String,
    ) -> Result<(), ghostty_vt::Error> {
        let width = match cell.raw_cell()?.wide()? {
            CellWide::SpacerTail => return Ok(()),
            CellWide::Wide => 2,
            _ => 1,
        };
        let style = cell.style()?;
        let selected = cell.is_selected()?;
        value.clear();
        if !style.invisible {
            cell.graphemes_utf8(value)?;
        }
        if width == 1
            && let Some(run) = self.runs.last_mut()
            && !run.wide
            && run.column + run.width == x
            && run.style == style
            && run.selected == selected
        {
            if value.is_empty() {
                run.value.push(' ');
            } else {
                run.value.push_str(value);
            }
            run.width += 1;
            return Ok(());
        }
        if value.is_empty() {
            value.push(' ');
        }
        self.runs.push(Run {
            column: x,
            width,
            value: value.clone(),
            style,
            selected,
            wide: width > 1,
        });
        Ok(())
    }

    fn view(&self, theme: &Theme, palette: Option<&[RgbColor; 256]>) -> El {
        let mut line = el().h(f32::from(CELL.height)).w_full();
        for run in &self.runs {
            line = line.child(run.view(theme, palette));
        }
        line
    }
}

impl Run {
    fn view(&self, theme: &Theme, palette: Option<&[RgbColor; 256]>) -> El {
        let colors = ansi(theme);
        let mut fg = color(self.style.fg_color, theme.text, &colors, palette);
        let mut bg = color(self.style.bg_color, theme.k.c.bg_base, &colors, palette);
        if self.style.inverse {
            std::mem::swap(&mut fg, &mut bg);
        }
        if self.selected {
            bg = theme.selected;
        }
        if self.style.faint {
            fg = fg.mix(bg, 0.45);
        }
        let font = rune::TextStyle {
            size: f32::from(CELL.width) / 0.6,
            line_height: f32::from(CELL.height),
            weight: if self.style.bold { 700.0 } else { 400.0 },
            italic: self.style.italic,
            features: "'liga' 0, 'calt' 0",
            ..theme.mono
        };
        let start = f32::from(self.column) * f32::from(CELL.width);
        let width = f32::from(self.width) * f32::from(CELL.width);
        let height = f32::from(CELL.height);
        let mut run = el()
            .pin(Some(start), Some(0.0), None, None)
            .size(width, height)
            .fill(bg);
        if self.style.underline != Underline::None {
            let underline = color(self.style.underline_color, fg, &colors, palette);
            run = run.child(
                el().pin(Some(0.0), None, Some(0.0), Some(2.0))
                    .h(1.0)
                    .fill(underline),
            );
        }
        if self.style.strikethrough {
            run = run.child(
                el().pin(Some(0.0), Some(height / 2.0), Some(0.0), None)
                    .h(1.0)
                    .fill(fg),
            );
        }
        if !self.value.trim().is_empty() {
            run = run.child(text(&self.value).font(font).color(fg).w(width));
        }
        run
    }
}

fn cursor_el(cursor: Cursor, position: ghostty_vt::render::CursorViewport, theme: &Theme) -> El {
    let x = f32::from(position.x).mul_add(f32::from(CELL.width), PAD);
    let y = f32::from(position.y).mul_add(f32::from(CELL.height), PAD);
    let width = f32::from(CELL.width);
    let height = f32::from(CELL.height);
    let element = el().pin(Some(x), Some(y), None, None);
    match cursor.visual_style {
        CursorVisualStyle::Bar => element.size(2.0, height).fill(theme.text),
        CursorVisualStyle::Underline => el()
            .pin(Some(x), Some(y + height - 2.0), None, None)
            .size(width, 2.0)
            .fill(theme.text),
        CursorVisualStyle::Block => element.size(width, height).fill(theme.text.alpha(0.45)),
        _ => element.size(width, height).border(1.0, theme.text),
    }
}

const fn ansi(t: &Theme) -> [Color; 16] {
    let bg = t.k.c.bg_base;
    [
        bg,
        t.danger,
        t.success,
        t.warning,
        t.accent,
        t.delegated,
        t.remote,
        t.text,
        t.faint,
        t.danger,
        t.success,
        t.warning,
        t.accent_text,
        t.delegated,
        t.remote,
        t.text,
    ]
}

fn color(
    value: StyleColor,
    fallback: Color,
    ansi: &[Color; 16],
    palette: Option<&[RgbColor; 256]>,
) -> Color {
    match value {
        StyleColor::None => fallback,
        StyleColor::Rgb(value) => rgb(value),
        StyleColor::Palette(index) => {
            let index = usize::from(index.0);
            ansi.get(index)
                .copied()
                .or_else(|| palette.map(|p| rgb(p[index])))
                .unwrap_or(fallback)
        }
    }
}

fn rgb(value: RgbColor) -> Color {
    Color {
        r: f32::from(value.r) / 255.0,
        g: f32::from(value.g) / 255.0,
        b: f32::from(value.b) / 255.0,
        a: 1.0,
    }
}
