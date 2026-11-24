//! Expanded tool detail inspectors (command ANSI terminal output, file diffs,
//! search matches, MCP structured I/O, and reasoning traces).

use rune::{Color, El, ScrollSpec, TextStyle, Tx, col, el, rich, row, text};
use rune_kit::{IconButtonVariant, Markdown, ScrollBar, ScrollBarStyle};
use viz_client::protocol::EntrySource;

use super::{ansi::parse_ansi, diff_card::file_diffs_inspector, search_card::search_inspector};
use crate::{
    app::Desktop,
    model::{Tool, ToolAction, ToolStatus, is_command, relative_path},
    theme::{HAIRLINE, RADIUS_CONTROL, S2, S4, S6, S10, S12, S28, Size, Theme},
    ui,
    update::{Msg, TimelineMsg},
};

/// Maximum command output viewport height (`max-h-80` = `320pt`).
const CMD_OUTPUT_MAX_H: f32 = 320.0;
/// Maximum reasoning trace height (`max-h-96` = `384pt`).
const REASONING_MAX_H: f32 = 384.0;

/// Returns whether `tool` has inspectable detail content.
#[must_use]
pub fn has_inspector(tool: &Tool) -> bool {
    let action = ToolAction::of(tool);
    !tool.output.trim().is_empty()
        || !tool.diffs.is_empty()
        || !tool.search_matches.is_empty()
        || (action == ToolAction::Other && tool.raw_input_pretty.is_some())
        || tool.more
        || tool.exit_code.is_some_and(|code| code != 0)
        || tool.exit_signal.is_some()
}

/// Renders the indented inspector panel for an expanded `tool` row.
pub fn tool_inspector(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let action = ToolAction::of(tool);
    let content = if !tool.diffs.is_empty() {
        file_diffs_inspector(s, tool, t)
    } else if is_command(&tool.name) {
        command_inspector(s, tool, t)
    } else if action == ToolAction::Search && !tool.search_matches.is_empty() {
        search_inspector(s, tool, t)
    } else if action == ToolAction::Other && tool.raw_input_pretty.is_some() {
        dynamic_tool_inspector(s, tool, t)
    } else {
        plain_output_inspector(s, tool, action == ToolAction::Read, t)
    };
    col()
        .key(format!("{}-inspector", tool.key))
        .w_full()
        .pl(S28)
        .pt(S2)
        .pb(S6)
        .child(content)
}

/// Renders an expanded reasoning trace beside a `2pt` hairline rule.
pub fn thought_inspector(key: &str, body: &str, streaming: bool, t: &Theme) -> El {
    let scrolled = col()
        .key(format!("{key}-thought-scroll"))
        .w_full()
        .max_h(REASONING_MAX_H)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: streaming,
        })
        .child(activity_markdown(body, t.ui, t.muted, streaming, true, t))
        .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k));
    row()
        .key(format!("{key}-thought-detail"))
        .w_full()
        .pl(S28)
        .pt(S2)
        .pb(S6)
        .gap(S12)
        .items_stretch()
        .child(el().w(S2).radius(HAIRLINE).fill(t.hairline).a11y_hidden())
        .child(scrolled.grow(1.0).shrink().min_w(0.0))
}

/// Shows a compact 4-line preview of a streaming thought that breaks cleanly
/// on line boundaries without slicing glyphs mid-line.
pub(super) fn thought_preview(body: &str, t: &Theme) -> El {
    let preview = compact_thought_preview(body);
    text(preview)
        .font(t.small)
        .color(t.muted)
        .w_full()
        .wrap()
        .max_lines(4)
}

fn compact_thought_preview(body: &str) -> String {
    let mut lines = Vec::new();
    for raw in body.lines() {
        let cleaned = raw
            .replace("**", "")
            .replace("__", "")
            .replace('`', "")
            .trim_start_matches(['#', '>', ' '])
            .trim()
            .to_owned();
        if !cleaned.is_empty() {
            lines.push(cleaned);
            if lines.len() >= 8 {
                break;
            }
        }
    }
    lines.join("\n")
}

fn activity_markdown(
    body: &str,
    font: TextStyle,
    color: Color,
    streaming: bool,
    interactive: bool,
    t: &Theme,
) -> El {
    let mut markdown = Markdown::new(body)
        .style(t.markdown().body_font(font).text(color))
        .streaming(streaming)
        .selectable(interactive)
        .scroll_wide(interactive)
        .copy_source(interactive);
    if interactive {
        markdown = markdown.on_link_tx(|s: &mut Desktop, url: &str, tx: &mut Tx| {
            s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.to_owned())), tx);
        });
    }
    markdown.build(&t.k).w_full()
}

fn command_inspector(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let mut card = inspector_frame("cmd-card", t)
        .child(cmd_header(s, tool, t))
        .child(ui::rule(t));

    if tool.terminal_truncated {
        card = card.child(
            text("Earlier output not shown")
                .font(t.micro)
                .color(t.faint)
                .px(S10)
                .pt(S6),
        );
    }

    let trimmed = tool.output.trim_end();
    if !trimmed.is_empty() {
        let (plain, spans) = parse_ansi(trimmed, t);
        let output_el = if spans.is_empty() {
            ui::code(&plain, t).font(t.mono_small)
        } else {
            rich(plain, spans)
                .font(t.mono_small)
                .color(t.text)
                .wrap_anywhere()
                .w_full()
                .selectable(true)
                .selection_color(t.k.selection())
        };
        card = card.child(
            col()
                .key("cmd-scroll")
                .w_full()
                .max_h(CMD_OUTPUT_MAX_H)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: tool.status == ToolStatus::Running,
                })
                .child(col().w_full().padding(S10).child(output_el))
                .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k)),
        );
    }

    if let Some(chunk_bar) = load_more_bar(s, tool, t) {
        card = card.child(ui::rule(t)).child(chunk_bar);
    }

    if let Some(exit_text) = exit_footer_text(tool) {
        card = card.child(ui::rule(t)).child(
            row()
                .key("cmd-exit")
                .w_full()
                .px(S10)
                .py(S4)
                .child(text(exit_text).font(t.mono_small).color(t.danger)),
        );
    }

    card
}

fn cmd_header(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let cmd = tool
        .input
        .as_deref()
        .unwrap_or(&tool.summary)
        .trim_start_matches("$ ");
    let cmd_copy = cmd.to_owned();
    let copy_id = format!("copy-cmd-{}", tool.key);
    let copied = s.model.timeline.copied_key.as_deref() == Some(copy_id.as_str());

    let header = row()
        .key("cmd-header")
        .w_full()
        .px(S10)
        .py(S6)
        .gap(S6)
        .center_y()
        .fill(t.surface_recessed)
        .child(
            text(format!("$ {cmd}"))
                .font(t.mono_small)
                .color(t.text_secondary)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate()
                .help_when_truncated(cmd),
        );

    header.child(
        rune_kit::IconButton::new(if copied { &ui::CHECK } else { &ui::COPY })
            .help("Copy command")
            .variant(IconButtonVariant::GhostMuted)
            .control_size(Size::Micro.kit())
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::CopyWithFeedback {
                        key: copy_id.clone(),
                        text: cmd_copy.clone(),
                    }),
                    tx,
                );
            })
            .build(&t.k)
            .key("copy-cmd"),
    )
}

fn dynamic_tool_inspector(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let mut card = inspector_frame("mcp-card", t);

    if let Some(args) = &tool.raw_input_pretty {
        card = card
            .child(section_head("Arguments", t))
            .child(ui::rule(t))
            .child(output_scroll(
                "mcp-args",
                literal_output(args, t),
                180.0,
                false,
                t,
            ));
    }

    let trimmed = tool.output.trim_end();
    if !trimmed.is_empty() {
        if tool.raw_input_pretty.is_some() {
            card = card.child(ui::rule(t));
        }
        let streaming = tool.status == ToolStatus::Running;
        card = card
            .child(section_head("Output", t))
            .child(ui::rule(t))
            .child(output_scroll(
                "mcp-out",
                tool_output(trimmed, false, streaming, t),
                240.0,
                streaming,
                t,
            ));
    }

    if let Some(chunk_bar) = load_more_bar(s, tool, t) {
        card = card.child(ui::rule(t)).child(chunk_bar);
    }
    card
}

fn inspector_frame(key: &str, t: &Theme) -> El {
    col()
        .key(key)
        .w_full()
        .radius(RADIUS_CONTROL)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .clip_rounded(RADIUS_CONTROL)
}

fn plain_output_inspector(s: &Desktop, tool: &Tool, mono: bool, t: &Theme) -> El {
    let copied = tool.output.clone();
    let header_title = if mono && !tool.summary.is_empty() {
        relative_path(&s.model, &tool.summary).to_owned()
    } else {
        "Output".to_owned()
    };
    let head = row()
        .w_full()
        .px(S10)
        .py(S6)
        .gap(S6)
        .center_y()
        .fill(t.surface_recessed)
        .child(
            text(header_title)
                .font(t.mono_small)
                .color(t.muted)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate(),
        )
        .child(
            rune_kit::IconButton::new(&ui::COPY)
                .help("Copy output")
                .variant(IconButtonVariant::GhostMuted)
                .control_size(Size::Micro.kit())
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(Msg::Timeline(TimelineMsg::CopyText(copied.clone())), tx);
                })
                .build(&t.k)
                .key("copy"),
        );
    let streaming = tool.status == ToolStatus::Running;
    let mut card = inspector_frame("plain-card", t)
        .child(head)
        .child(ui::rule(t))
        .child(output_scroll(
            "plain-scroll",
            tool_output(tool.output.trim_end(), mono, streaming, t),
            CMD_OUTPUT_MAX_H,
            streaming,
            t,
        ));
    if let Some(chunk_bar) = load_more_bar(s, tool, t) {
        card = card.child(ui::rule(t)).child(chunk_bar);
    }
    card
}

fn section_head(label: &str, t: &Theme) -> El {
    row()
        .w_full()
        .px(S10)
        .py(S6)
        .fill(t.surface_recessed)
        .child(text(label).font(t.micro_medium).color(t.muted))
}

/// Keep source reads and structured JSON literal. Text results from other
/// tools use the same Markdown renderer as reasoning.
fn tool_output(body: &str, literal: bool, streaming: bool, t: &Theme) -> El {
    if literal
        || (body.trim_start().starts_with(['{', '['])
            && serde_json::from_str::<serde_json::Value>(body).is_ok())
    {
        literal_output(body, t)
    } else {
        activity_markdown(body, t.ui, t.text, streaming, true, t)
    }
}

fn literal_output(body: &str, t: &Theme) -> El {
    text(body)
        .font(t.mono_small)
        .color(t.text)
        .wrap_anywhere()
        .selectable(true)
        .selection_color(t.k.selection())
}

fn output_scroll(key: &str, output: El, max_h: f32, stick_bottom: bool, t: &Theme) -> El {
    col()
        .key(key)
        .w_full()
        .max_h(max_h)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom })
        .scroll_fade(rune::ScrollFade::strength(crate::theme::S16).linear())
        .child(col().w_full().padding(S10).child(output))
        .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k))
}

fn load_more_bar(s: &Desktop, tool: &Tool, t: &Theme) -> Option<El> {
    if !tool.more {
        return None;
    }
    let source = tool
        .entry
        .map(|entry| EntrySource::Stored { entry })
        .or_else(|| tool.live_key.clone().map(|key| EntrySource::Live { key }))?;
    let loading = s.model.timeline.loading_chunks.contains(&tool.key);
    let key = tool.key.clone();
    Some(
        row()
            .key("load-more-bar")
            .w_full()
            .px(S10)
            .py(S6)
            .center_y()
            .child(
                rune_kit::Button::new(if loading {
                    "Loading full output…"
                } else {
                    "Load full output"
                })
                .variant(rune_kit::ButtonVariant::GhostMuted)
                .control_size(crate::theme::Size::Micro.kit())
                .icon(&ui::DOWNLOAD)
                .disabled(loading || !s.model.connected())
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(
                        Msg::Timeline(TimelineMsg::LoadEntryChunk {
                            key: key.clone(),
                            source: source.clone(),
                        }),
                        tx,
                    );
                })
                .build(&t.k)
                .key("load-chunk"),
            ),
    )
}

fn exit_footer_text(tool: &Tool) -> Option<String> {
    tool.exit_code.filter(|&code| code != 0).map_or_else(
        || {
            tool.exit_signal
                .as_deref()
                .map(|signal| format!("killed by {signal}"))
        },
        |code| Some(format!("exit {code}")),
    )
}
