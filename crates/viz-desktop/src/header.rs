//! The thread header in the main pane's toolbar: the conversation's title,
//! model and thinking level, whether it's working, and what it has cost.

use rune::{El, row, text};
use rune_kit::{Badge, BadgeVariant, Kit, Spinner, shell::Toolbar};

use crate::{app::Desktop, theme};

/// The toolbar over the timeline.
pub fn toolbar(s: &Desktop, k: &Kit) -> Toolbar {
    let c = &k.c;
    let thread = &s.thread;
    let title = s
        .selected
        .map_or_else(|| "Not connected".to_owned(), |id| s.title(id));
    let mut leading = row()
        .key("thread-title")
        .gap(8.0)
        .center_y()
        .min_w(0.0)
        .shrink()
        .child(
            text(title)
                .key("title")
                .font(k.ty.ui_medium)
                .color(c.text_base)
                .truncate()
                .shrink(),
        );
    if let Some(model) = &thread.agent.model {
        leading = leading.child(model_chip(thread.agent.provider.as_deref(), model, k));
    }
    if let Some(level) = &thread.agent.thinking {
        leading = leading.child(
            Badge::new(format!("thinking {level}"))
                .variant(BadgeVariant::Subtle)
                .build(k)
                .key("thinking"),
        );
    }
    let mut trailing = row().key("thread-meta").gap(12.0).center_y();
    if thread.live.busy {
        trailing = trailing.child(
            row()
                .key("busy")
                .role("status")
                .label("Working")
                .gap(6.0)
                .center_y()
                .child(Spinner::new().size(12.0).build(k))
                .child(text("Working").font(k.ty.small_medium).color(c.text_muted)),
        );
    }
    if let Some(usage) = thread.usage.filter(|usage| usage.tokens() > 0) {
        let summary = format!(
            "{} tokens · {}",
            theme::compact_count(usage.tokens()),
            theme::cost(usage.cost)
        );
        let detail = format!(
            "{} in · {} out · {}",
            theme::compact_count(usage.input),
            theme::compact_count(usage.output),
            theme::cost(usage.cost)
        );
        trailing = trailing.child(
            text(summary)
                .key("usage")
                .font(k.ty.small)
                .color(c.text_faint)
                .help(detail),
        );
    }
    Toolbar::new().leading(leading).trailing(trailing)
}

/// The provider's mark and the model id, in a quiet pill.
pub fn model_chip(provider: Option<&str>, model: &str, k: &Kit) -> El {
    let c = &k.c;
    let mut chip = row()
        .key("model")
        .h(20.0)
        .px(6.0)
        .gap(4.0)
        .center_y()
        .radius(k.radius.sm)
        .fill(c.layer_02)
        .help(provider.map_or_else(|| model.to_owned(), |p| format!("{p}/{model}")));
    if let Some(provider) = provider {
        chip = chip.child(rune::icon(theme::provider_icon(provider), 12.0).color(c.icon_muted));
    }
    chip.child(
        text(model.to_owned())
            .font(k.ty.small_medium)
            .color(c.text_muted)
            .truncate(),
    )
}
