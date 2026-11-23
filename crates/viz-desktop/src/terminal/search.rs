//! Search retained terminal cells and reveal the selected match.

use libghostty_vt::{
    point::{Coordinate, Point},
    selection::Selection,
    terminal::ScrollViewport,
};

use super::TerminalView;

#[derive(Default)]
pub(super) struct Search {
    pub(super) index: usize,
    pub(super) count: usize,
}

impl TerminalView {
    pub(super) fn find(&mut self, query: &str, step: isize, search: &mut Search) {
        if query.is_empty() {
            *search = Search::default();
            let _ = self.viewer.terminal().set_selection(None);
            return;
        }
        let terminal = self.viewer.terminal_mut();
        let columns = terminal.cols().unwrap_or_default();
        let history = terminal.scrollback_rows().unwrap_or_default();
        let rows = history.saturating_add(usize::from(terminal.rows().unwrap_or_default()));
        let query = query.to_lowercase();
        let mut matches = Vec::new();
        let mut line = String::new();
        let mut offsets = Vec::new();
        let mut graphemes = [char::default(); 64];
        for y in 0..u32::try_from(rows).unwrap_or(u32::MAX) {
            line.clear();
            offsets.clear();
            for x in 0..columns {
                let Ok(cell) = terminal.grid_ref(Point::Screen(Coordinate { x, y })) else {
                    break;
                };
                offsets.push((line.len(), x));
                match cell.graphemes(&mut graphemes) {
                    Ok(0) | Err(_) => line.push(' '),
                    Ok(len) => line.extend(graphemes[..len].iter().flat_map(|c| c.to_lowercase())),
                }
            }
            for (offset, _) in line.match_indices(&query) {
                let start = offsets
                    .partition_point(|(at, _)| *at <= offset)
                    .saturating_sub(1);
                let end = offsets
                    .partition_point(|(at, _)| *at < offset + query.len())
                    .saturating_sub(1);
                matches.push((y, offsets[start].1, offsets[end].1));
            }
        }

        let old_count = search.count;
        search.count = matches.len();
        if matches.is_empty() {
            search.index = 0;
            let _ = terminal.set_selection(None);
            return;
        }
        search.index = match step {
            0 => matches.len() - 1,
            -1 if old_count > 0 => {
                (search.index.min(matches.len() - 1) + matches.len() - 1) % matches.len()
            }
            _ => (search.index + 1) % matches.len(),
        };
        let (y, start, end) = matches[search.index];
        terminal.scroll_viewport(ScrollViewport::Row((y as usize).saturating_sub(2)));
        if let (Ok(start), Ok(end)) = (
            terminal.grid_ref(Point::Screen(Coordinate { x: start, y })),
            terminal.grid_ref(Point::Screen(Coordinate { x: end, y })),
        ) {
            let _ = terminal.set_selection(Some(&Selection::new(start, end, false)));
        }
    }
}
