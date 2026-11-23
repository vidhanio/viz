//! Composer footer controls: the draft's status, the model picker trigger,
//! and the context window meter.

use rune::{El, row, text};
use rune_kit::ProgressCircle;
use viz_client::protocol::ContextUsage;

use super::draft;
use crate::{
    app::Desktop,
    model_picker,
    theme::{self, ICON_SMALL, READING_INSET, READING_WIDTH, S2, S4, S8, Size, Theme},
};

/// Drop the numeric context label below this composer width, keeping the ring
/// and tooltip.
const CONTEXT_TEXT_MIN_WIDTH: f32 = 520.0;

/// The draft status or model trigger, then the context meter.
pub(super) fn toolbar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let bar = row()
        .key("settings")
        .h(Size::Default.height())
        .gap(S2)
        .center_y();
    if !m.connected() {
        return bar;
    }
    let bar = match &m.selection.new_thread {
        Some(thread) => draft::status(s, thread, bar, t),
        None => bar,
    };
    let usage = m
        .selection
        .new_thread
        .is_none()
        .then(|| m.thread_state().and_then(|state| state.usage.as_ref()))
        .flatten()
        .and_then(|usage| context_usage(s, usage, t));
    let bar = bar.children(model_picker::trigger_button(s, t, layer));
    match usage {
        Some(usage) => bar.child(rune::spacer()).child(usage),
        None => bar,
    }
}

fn context_usage(s: &Desktop, usage: &ContextUsage, t: &Theme) -> Option<El> {
    if usage.size == 0 {
        return None;
    }
    let percent = used_percent(usage.used, usage.size);
    let label = format!(
        "Context: {percent} used, {} of {} tokens",
        usage.used, usage.size
    );
    let used = u128::from(usage.used.min(usage.size));
    let size = u128::from(usage.size.max(1));
    let ink = if used * 100 >= size * 85 {
        t.danger
    } else if used * 100 >= size * 65 {
        t.warning
    } else {
        t.muted
    };
    let mut ring_kit = t.k.clone();
    ring_kit.c.icon_muted = ink;
    let reading_width = s.model.panels.window_width - s.widgets.sidebar_pane.shown_width();
    let composer_width = READING_INSET
        .mul_add(-2.0, reading_width)
        .min(READING_WIDTH);
    let show_text = composer_width >= CONTEXT_TEXT_MIN_WIDTH;
    let total = compact_tokens(usage.size);
    Some(
        row()
            .key("context-usage")
            .center_y()
            .gap(S4)
            .ml(S8)
            .mr(S4)
            .role("group")
            .label(&label)
            .help(label)
            .child(
                ProgressCircle::new(theme::fraction(usage.used, usage.size))
                    .size(ICON_SMALL + S2)
                    .build(&ring_kit)
                    .a11y_hidden(),
            )
            .children(show_text.then(|| {
                row()
                    .center_y()
                    .a11y_hidden()
                    .child(text(percent).font(t.mono_small).color(ink))
                    .child(text(format!("/{total}")).font(t.mono_small).color(t.faint))
            })),
    )
}

/// Whole percent of the window used, rounded half up; any use below half a
/// percent reads `<1%` rather than an empty-looking `0%`.
fn used_percent(used: u64, size: u64) -> String {
    let used = u128::from(used.min(size));
    let size = u128::from(size.max(1));
    match (used * 200 + size) / (size * 2) {
        0 if used > 0 => "<1%".to_owned(),
        whole => format!("{whole}%"),
    }
}

/// Round to one decimal, promoting a rounded 1000k to 1m.
fn compact_tokens(count: u64) -> String {
    let (unit, suffix) = match count {
        0..1_000 => return count.to_string(),
        1_000..999_950 => (1_000, 'k'),
        _ => (1_000_000, 'm'),
    };
    let step = unit / 10;
    let tenths = count / step + u64::from(count % step >= step / 2);
    let whole = tenths / 10;
    match tenths % 10 {
        0 => format!("{whole}{suffix}"),
        digit => format!("{whole}.{digit}{suffix}"),
    }
}
