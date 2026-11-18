//! The main pane's toolbar, on rune-kit's `Toolbar`: the conversation's title
//! on the left; its model and a pulse while it works on the right, with what
//! it has cost in the model's tooltip.

use std::fmt::Write as _;

use rune::text;
use rune_kit::shell::Toolbar;

use crate::{
    app::Desktop,
    theme::{self, Theme},
};

/// The toolbar over the timeline.
pub fn toolbar(s: &Desktop, t: &Theme) -> Toolbar {
    let thread = &s.thread;
    let title = s
        .selected
        .map_or_else(|| "viz".to_owned(), |id| s.title(id));
    let mut toolbar = Toolbar::new().leading(
        text(title)
            .key("title")
            .font(t.title)
            .color(t.text)
            .truncate()
            .shrink(),
    );
    if thread.live.busy {
        toolbar = toolbar.trailing(
            theme::pulse(t.accent, 7.0)
                .key("busy")
                .role("status")
                .label("Working"),
        );
    }
    if let Some(model) = &thread.agent.model {
        let mut help = thread
            .agent
            .provider
            .as_ref()
            .map_or_else(|| model.clone(), |provider| format!("{provider}/{model}"));
        if let Some(level) = &thread.agent.thinking {
            _ = write!(help, " · thinking {level}");
        }
        if let Some(usage) = thread.usage.filter(|usage| usage.tokens() > 0) {
            _ = write!(
                help,
                "\n{} in · {} out · {}",
                theme::compact_count(usage.input),
                theme::compact_count(usage.output),
                theme::cost(usage.cost)
            );
        }
        toolbar = toolbar.trailing(
            text(model.clone())
                .key("model")
                .font(t.mono_small)
                .color(t.muted)
                .truncate()
                .help(help),
        );
    }
    toolbar
}
