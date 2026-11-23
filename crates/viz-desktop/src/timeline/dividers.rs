//! System dividers (compaction, continuation, turn stop status, forked-from,
//! omitted entry chunk loader), non-text media cards, and inline turn error
//! banners.

use std::f32::consts::FRAC_PI_2;

use rune::{El, Tx, col, icon, row, text};
use rune_kit::ButtonVariant;
use viz_client::protocol::{CompactionStatus, ContentKind, EntrySource, ForkSource};

use crate::{
    app::Desktop,
    model::{CompactionNotice, ErrorKind, MediaItem},
    motion,
    theme::{
        CARD_INSET, HAIRLINE, ICON_MD, ICON_SMALL, RADIUS_CARD, RADIUS_SM, S4, S6, S8, S12, Size,
        Theme,
    },
    ui::{self, Tone},
    update::{Msg, TimelineMsg},
};

/// Horizontal-rule centered divider for a context compaction event, with an
/// expandable summary panel once compacted.
pub fn compaction_divider(s: &Desktop, notice: &CompactionNotice, t: &Theme) -> El {
    let fold_key = format!("{}-summary", notice.key);
    let has_summary = !notice.summary.trim().is_empty();
    let open = has_summary
        && s.model.timeline.is_open(
            s.model.selection.selected,
            &s.model.disclosure.expanded,
            &fold_key,
            None,
            false,
        );
    let pill = compaction_pill(notice, fold_key, has_summary, open, t);
    let line = row()
        .key(format!("{}-line", notice.key))
        .w_full()
        .py(S6)
        .gap(S8)
        .center_y()
        .child(ui::rule(t).grow(1.0).w(0.0))
        .child(pill)
        .child(ui::rule(t).grow(1.0).w(0.0));

    let mut block = motion::fold_container(col().key(notice.key.clone()).w_full()).child(line);
    if let Some(error) = &notice.error {
        block = block.child(
            text(error)
                .font(t.small)
                .color(t.danger)
                .wrap()
                .px(CARD_INSET)
                .pb(S4),
        );
    }
    if open {
        block = block.child(motion::fold_row(
            col()
                .key("summary")
                .w_full()
                .mb(S6)
                .padding(CARD_INSET)
                .radius(RADIUS_CARD)
                .fill(t.surface_recessed)
                .border(HAIRLINE, t.hairline)
                .child(
                    text(notice.summary.trim())
                        .font(t.small)
                        .color(t.muted)
                        .wrap()
                        .selectable(true)
                        .selection_color(t.k.selection()),
                ),
        ));
    }
    block
}

fn compaction_pill(
    notice: &CompactionNotice,
    fold_key: String,
    has_summary: bool,
    open: bool,
    t: &Theme,
) -> El {
    let (lead, label, color) = match notice.status {
        CompactionStatus::InProgress => (Some(ui::spinner(t)), "Compacting context…", t.muted),
        CompactionStatus::Completed => (
            Some(icon(&ui::MINIMIZE2, ICON_SMALL).color(t.faint)),
            "Context compacted",
            t.muted,
        ),
        CompactionStatus::Failed => (
            Some(icon(&ui::CIRCLE_ALERT, ICON_SMALL).color(t.danger)),
            "Context compaction failed",
            t.danger,
        ),
        CompactionStatus::Cancelled => (
            Some(icon(&ui::CIRCLE_STOP, ICON_SMALL).color(t.faint)),
            "Context compaction stopped",
            t.muted,
        ),
    };
    let mut pill = row()
        .key(format!("{}-pill", notice.key))
        .px(S8)
        .py(S4)
        .gap(S6)
        .center_y()
        .radius(RADIUS_SM)
        .children(lead.map(El::a11y_hidden))
        .child(text(label).font(t.micro_medium).color(color));
    if has_summary {
        pill = pill
            .role("button")
            .label(label)
            .expanded(open)
            .hover(t.hover_wash)
            .hover_group()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::ToggleFold {
                        key: fold_key.clone(),
                        legacy_key: None,
                        default_open: false,
                        anchor: None,
                    }),
                    tx,
                );
            })
            .child(
                motion::chevron(&ui::CHEVRON, ICON_SMALL, open, FRAC_PI_2)
                    .key("chevron")
                    .color(t.faint)
                    .group_color(t.text)
                    .a11y_hidden(),
            );
    }
    pill
}

/// Hairline-flanked system divider indicating the agent continued on its own.
pub fn continued_divider(key: &str, t: &Theme) -> El {
    hairline_divider(key, &ui::AGENT, "Continued on its own", t.muted, t)
}

/// Hairline-flanked divider for a non-default ACP stop reason (`max_tokens`,
/// `refusal`, etc.).
pub fn status_divider(key: &str, stop_reason: &str, words: &str, t: &Theme) -> El {
    let glyph = if stop_reason == "cancelled" {
        &ui::CIRCLE_STOP
    } else {
        &ui::INFO
    };
    hairline_divider(key, glyph, words, t.muted, t).role("status")
}

/// Hairline-flanked clickable banner at the top of a forked thread linking back
/// to its source thread.
pub fn forked_from_divider(s: &Desktop, source: ForkSource, t: &Theme) -> El {
    let title = s.model.summary(source.thread).map_or_else(
        || format!("thread {}", source.thread),
        |sum| sum.title.clone(),
    );
    let link_label = format!("Forked from {title}");
    let target = source.thread;
    let pill = row()
        .key("forked-from-link")
        .role("link")
        .label(link_label.clone())
        .px(S8)
        .py(S4)
        .gap(S6)
        .center_y()
        .radius(RADIUS_SM)
        .hover(t.hover_wash)
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
            s.dispatch(
                Msg::Timeline(TimelineMsg::OpenThreadLink {
                    thread: target,
                    environment: None,
                }),
                tx,
            );
        })
        .child(
            icon(&ui::GIT_FORK, ICON_SMALL)
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text(link_label)
                .font(t.micro_medium)
                .color(t.muted)
                .group_color(t.text)
                .truncate()
                .shrink(),
        );

    row()
        .key("forked-from")
        .w_full()
        .pb(S12)
        .gap(S8)
        .center_y()
        .child(ui::rule(t).grow(1.0).w(0.0))
        .child(pill)
        .child(ui::rule(t).grow(1.0).w(0.0))
}

/// Hairline-flanked lazy-load row for an omitted wire entry recoverable via
/// `thread.entry_chunk`.
pub fn omitted_divider(s: &Desktop, key: &str, source: &EntrySource, t: &Theme) -> El {
    let loading = s.model.timeline.loading_chunks.contains(key);
    let load_key = key.to_owned();
    let source = source.clone();
    row()
        .key(key.to_owned())
        .w_full()
        .py(S6)
        .gap(S8)
        .center_y()
        .child(ui::rule(t).grow(1.0).w(0.0))
        .child(
            rune_kit::Button::new(if loading {
                "Loading omitted entry…"
            } else {
                "Load omitted entry"
            })
            .variant(rune_kit::ButtonVariant::GhostMuted)
            .control_size(crate::theme::Size::Micro.kit())
            .icon(&ui::DOWNLOAD)
            .disabled(loading || !s.model.connected())
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::LoadEntryChunk {
                        key: load_key.clone(),
                        source: source.clone(),
                    }),
                    tx,
                );
            })
            .build(&t.k)
            .key("load"),
        )
        .child(ui::rule(t).grow(1.0).w(0.0))
}

/// Compact surface card for non-text ACP media blocks (`Image`, `Audio`,
/// `ResourceLink`, `Resource`).
pub fn media_card(media: &MediaItem, t: &Theme) -> El {
    let (label, glyph) = match media.block.kind {
        ContentKind::Image => ("Image", &ui::FILE),
        ContentKind::Audio => ("Audio", &ui::FILE),
        ContentKind::ResourceLink => ("Resource link", &ui::LINK),
        ContentKind::Resource => ("Resource", &ui::PAPERCLIP),
        ContentKind::Text | ContentKind::Other => ("Attachment", &ui::PAPERCLIP),
    };
    let name = media
        .block
        .payload
        .get("name")
        .or_else(|| media.block.payload.get("title"))
        .or_else(|| media.block.payload.get("uri"))
        .or_else(|| media.block.payload.get("mimeType"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(label);
    let uri = media
        .block
        .payload
        .get("uri")
        .and_then(serde_json::Value::as_str)
        .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
        .map(str::to_owned);

    let mut card = row()
        .key(media.key.clone())
        .w_full()
        .my(S4)
        .px(CARD_INSET)
        .py(S8)
        .gap(S8)
        .center_y()
        .radius(RADIUS_CARD)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .child(icon(glyph, ICON_MD).color(t.muted).a11y_hidden())
        .child(rune_kit::Badge::new(label).build(&t.k).key("kind"))
        .child(
            text(name)
                .font(t.small)
                .color(t.text)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate(),
        );

    if let Some(url) = uri {
        card = card.child(
            rune_kit::Button::new("Open")
                .variant(rune_kit::ButtonVariant::GhostMuted)
                .control_size(crate::theme::Size::Micro.kit())
                .icon(&ui::EXTERNAL_LINK)
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.clone())), tx);
                })
                .build(&t.k)
                .key("open"),
        );
    }

    card
}

/// Inline turn error banner distinguishing usage-limit warnings from auth and
/// general failures, with optional Retry and Dismiss actions.
pub fn error_banner(
    s: &Desktop,
    key: &str,
    kind: ErrorKind,
    message: &str,
    t: &Theme,
) -> Option<El> {
    if s.model.timeline.dismissed_errors.contains(key) {
        return None;
    }
    let tone = match kind {
        ErrorKind::UsageLimit => Tone::Warning,
        ErrorKind::Auth | ErrorKind::General => Tone::Danger,
    };
    let dismiss_key = key.to_owned();
    let mut card = rune_kit::Card::new()
        .accent(tone.color(t))
        .title(kind.heading(), Some(tone.glyph()))
        .description(message);

    if s.model.connected() && !s.model.busy() && kind != ErrorKind::Auth {
        card = card.action(
            rune_kit::Button::new("Retry")
                .variant(ButtonVariant::Neutral)
                .control_size(Size::Small.kit())
                .icon(&ui::REFRESH_CW)
                .on_press_tx(|s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(Msg::Timeline(TimelineMsg::RetryLastPrompt), tx);
                })
                .build(&t.k)
                .key("retry"),
        );
    }

    card = card.action(
        rune_kit::IconButton::new(&ui::CLOSE)
            .help("Dismiss")
            .variant(rune_kit::IconButtonVariant::GhostMuted)
            .control_size(Size::Micro.kit())
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::DismissError(dismiss_key.clone())),
                    tx,
                );
            })
            .build(&t.k)
            .key("dismiss"),
    );

    Some(card.build(&t.k).key(key.to_owned()).my(S6))
}

fn hairline_divider(
    key: &str,
    glyph: &'static rune::Icon,
    label: &str,
    color: rune::Color,
    t: &Theme,
) -> El {
    row()
        .key(key.to_owned())
        .w_full()
        .py(S6)
        .gap(S8)
        .center_y()
        .child(ui::rule(t).grow(1.0).w(0.0))
        .child(
            row()
                .gap(S6)
                .center_y()
                .child(icon(glyph, ICON_SMALL).color(t.faint).a11y_hidden())
                .child(text(label).font(t.micro).color(color)),
        )
        .child(ui::rule(t).grow(1.0).w(0.0))
}
