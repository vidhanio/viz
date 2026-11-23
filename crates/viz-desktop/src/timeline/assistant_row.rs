//! Canvas assistant prose, clean horizontal-border Markdown tables,
//! syntax-highlighted code blocks with wrap/copy header, and turn metadata
//! footer.

use std::collections::HashSet;

use rune::{Color, El, TextStyle, Tx, col, icon, row, text};
use rune_kit::{IconButtonVariant, Markdown, MarkdownStyle, file_icon};
use viz_client::protocol::EntrySource;

use super::user_row::copy_morph_button;
use crate::{
    app::Desktop,
    model::{TurnView, format_clock_hm, format_turn_duration, format_usage_summary},
    theme::{
        CHAT_CONTENT_MAX_WIDTH, CHAT_LANE_PADDING_X, HAIRLINE, ICON_MD, RADIUS_CONTROL, S2, S4, S6,
        S12, Size, Theme,
    },
    ui::{self, BadgeSize, InlineStyle, Tone},
    update::{Msg, TimelineMsg},
};

/// Height of the code block header bar (`30pt`).
const CODE_HEADER_HEIGHT: f32 = 30.0;

/// Renders an assistant Markdown prose block directly on the canvas (no
/// bubble), with T3's borderless horizontal-rule tables and framed code-block
/// headers.
pub fn prose(
    s: &Desktop,
    key: &str,
    entry_id: Option<viz_client::protocol::EntryId>,
    source: &str,
    streaming: bool,
    more: bool,
    t: &Theme,
) -> El {
    let wrapped_set: HashSet<String> = s.model.timeline.wrapped_code.clone();
    let copied_key = s.model.timeline.copied_key.clone();
    let msg_key = key.to_owned();
    let msg_key_wrap = msg_key.clone();
    let t_frame = Theme::get(t.k.dark);

    let style = assistant_markdown_style(t);
    let mut md = Markdown::new(source.to_owned())
        .style(style)
        .streaming(streaming)
        .selectable(true)
        .copy_source(true)
        .scroll_wide(true)
        .on_link_tx(|s: &mut Desktop, url: &str, tx: &mut Tx| {
            s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.to_owned())), tx);
        })
        .code_wrap(move |index, _| wrapped_set.contains(&format!("{msg_key_wrap}-code-{index}")))
        .code_frame(move |card, frame| {
            code_frame_card(card, frame, &msg_key, copied_key.as_deref(), t_frame)
        })
        .build(&t.k)
        .key(key.to_owned())
        .w_full()
        .px(S4)
        .py(S2);
    if streaming {
        md = md.streaming(true);
    }

    if more && let Some(entry) = entry_id {
        let loading = s.model.timeline.loading_chunks.contains(key);
        let load_key = key.to_owned();
        return col()
            .key(format!("{key}-wrap"))
            .w_full()
            .gap(S6)
            .child(md)
            .child(
                row().px(S4).child(
                    ui::inline_button(if loading {
                        "Loading full message…"
                    } else {
                        "Load full message"
                    })
                    .style(InlineStyle::Wash)
                    .disabled(loading || !s.model.connected())
                    .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                        s.dispatch(
                            Msg::Timeline(TimelineMsg::LoadEntryChunk {
                                key: load_key.clone(),
                                source: EntrySource::Stored { entry },
                            }),
                            tx,
                        );
                    })
                    .build(t)
                    .key("load-more"),
                ),
            );
    }

    md
}

/// Renders the subtle metadata & action footer beneath a completed assistant
/// turn: always visible on the trailing turn, hover-revealed on earlier turns.
pub fn turn_footer(s: &Desktop, turn: &TurnView<'_>, is_latest: bool, t: &Theme) -> Option<El> {
    let end = turn.end?;
    let answer = &end.answer;
    if answer.is_empty() && end.usage.is_none() && !turn.interrupted {
        return None;
    }

    let copy_id = format!("copy-{}", end.key);
    let copied = s.model.timeline.copied_key.as_deref() == Some(copy_id.as_str());
    let connected = s.model.connected();

    let mut bar = row()
        .key(format!("{}-meta", end.key))
        .w_full()
        .pt(S6)
        .px(S4)
        .gap(S4)
        .center_y();
    if !is_latest {
        bar = bar.group_opacity(0.0, 1.0);
    }

    if turn.interrupted {
        bar = bar.child(
            ui::badge("Interrupted")
                .tone(Tone::Warning)
                .size(BadgeSize::Nano)
                .build(t)
                .key("interrupted")
                .mr(S4),
        );
    }

    if !answer.is_empty() {
        let answer_copy = answer.clone();
        let copy_key = copy_id;
        bar = bar.child(copy_morph_button(
            "copy-answer",
            "Copy response",
            copied,
            move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::CopyWithFeedback {
                        key: copy_key.clone(),
                        text: answer_copy.clone(),
                    }),
                    tx,
                );
            },
            t,
        ));
    }

    if let Some(entry) = end.entry_id {
        bar = bar.child(
            ui::icon_button(
                &ui::GIT_FORK,
                "Fork from this response",
                IconButtonVariant::GhostMuted,
                Size::Small,
            )
            .disabled(!connected)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(Msg::Timeline(TimelineMsg::ForkThrough(entry)), tx);
            })
            .build(&t.k)
            .key("fork-through"),
        );
    }

    if !answer.is_empty() {
        let reply_source = answer.clone();
        bar = bar.child(
            ui::icon_button(
                &ui::REPLY,
                "Reply to this response",
                IconButtonVariant::GhostMuted,
                Size::Small,
            )
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::ReplyTo(reply_source.clone())),
                    tx,
                );
            })
            .build(&t.k)
            .key("reply"),
        );
    }

    let meta_parts = build_meta_parts(s, turn);
    if !meta_parts.is_empty() {
        bar = bar.child(
            text(meta_parts.join(" · "))
                .font(t.micro)
                .color(t.faint)
                .ml(S4)
                .truncate()
                .shrink()
                .min_w(0.0),
        );
    }

    Some(bar)
}

fn build_meta_parts(s: &Desktop, turn: &TurnView<'_>) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(summary) = s
        .model
        .selection
        .selected
        .and_then(|id| s.model.summary(id))
    {
        let label = summary
            .model
            .as_deref()
            .filter(|m| !m.is_empty())
            .unwrap_or(summary.harness_name.as_str());
        if !label.is_empty() {
            parts.push(label.to_owned());
        }
    }
    if let Some(ms) = turn.duration_ms().filter(|&ms| ms > 0) {
        parts.push(format_turn_duration(ms));
    }
    if let Some(usage) = turn
        .end
        .and_then(|end| end.usage.as_ref())
        .and_then(format_usage_summary)
    {
        parts.push(usage);
    }
    if let Some(clock) = turn.ended_at.and_then(format_clock_hm) {
        parts.push(clock);
    }
    parts
}

/// T3-calibrated Markdown styling: borderless horizontal-rule tables, crisp
/// inline code chips, and clean code bodies ready for `code_frame`.
fn assistant_markdown_style(t: &Theme) -> MarkdownStyle {
    let table_font = TextStyle {
        size: 13.0,
        line_height: 20.0,
        ..t.body
    };
    t.markdown()
        .inline_code_bg(t.chip_fill)
        .inline_code_pad(4.0)
        .code_block(t.surface, Color::TRANSPARENT)
        .code_block_padding(S12, 10.0)
        .code_margin(10.0, 12.0)
        .text_width(2.0_f32.mul_add(-CHAT_LANE_PADDING_X, CHAT_CONTENT_MAX_WIDTH))
        .table_outline(None)
        .table_header_fill(Color::TRANSPARENT)
        .table_header_text(t.text_strong)
        .table_header_weight(600.0)
        .table_rules(t.hairline)
        .table_flush_left(true)
        .table_full_width(true)
        .table_font(table_font)
        .table_cell_padding(12.0, 8.0)
        .table_header_pad(12.0, 8.0)
        .table_margin(10.0, 12.0)
        .table_scroll_bar(t.faint.alpha(0.35), t.muted.alpha(0.55))
}

fn code_frame_card(
    card: El,
    frame: &rune_kit::CodeFrame<'_>,
    msg_key: &str,
    copied_key: Option<&str>,
    t: &Theme,
) -> El {
    let block_key = format!("{msg_key}-code-{}", frame.index);
    let copy_id = format!("copy-{block_key}");
    let is_copied = copied_key == Some(copy_id.as_str());
    let lang = frame.language.unwrap_or("text");
    let code_source = frame.source.to_owned();
    let wrap_key = block_key.clone();
    let border = if t.k.dark {
        t.hairline
    } else {
        t.border_control
    };
    let header = row()
        .key("code-header")
        .w_full()
        .h(CODE_HEADER_HEIGHT)
        .pl(S12)
        .pr(S6)
        .gap(S6)
        .center_y()
        .child(
            icon(language_icon(frame.language), ICON_MD)
                .color(t.muted)
                .a11y_hidden(),
        )
        .child(
            text(lang.to_owned())
                .font(t.mono_small)
                .color(t.muted)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate(),
        )
        .child(
            ui::icon_button(
                &ui::TEXT_WRAP,
                if frame.wrapped {
                    "Disable word wrap"
                } else {
                    "Enable word wrap"
                },
                IconButtonVariant::GhostMuted,
                Size::Micro,
            )
            .active(frame.wrapped)
            .on_press(move |s: &mut Desktop| {
                s.dispatch_local(Msg::Timeline(TimelineMsg::ToggleCodeWrap(wrap_key.clone())));
            })
            .build(&t.k)
            .key("wrap"),
        )
        .child(
            ui::icon_button(
                if is_copied { &ui::CHECK } else { &ui::COPY },
                "Copy code",
                IconButtonVariant::GhostMuted,
                Size::Micro,
            )
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::CopyWithFeedback {
                        key: copy_id.clone(),
                        text: code_source.clone(),
                    }),
                    tx,
                );
            })
            .build(&t.k)
            .key("copy"),
        );
    col()
        .key(block_key)
        .w_full()
        .radius(RADIUS_CONTROL)
        .fill(t.surface)
        .border(HAIRLINE, border)
        .clip_rounded(RADIUS_CONTROL)
        .child(header)
        .child(ui::rule(t))
        .child(card)
}

fn language_icon(language: Option<&str>) -> &'static rune::Icon {
    match language.map(str::to_ascii_lowercase).as_deref() {
        Some("sh" | "bash" | "zsh" | "fish" | "shell" | "console" | "terminal") => &ui::TERMINAL,
        Some(lang) => file_icon(&format!("file.{lang}")),
        None => &ui::FILE_READ,
    }
}
