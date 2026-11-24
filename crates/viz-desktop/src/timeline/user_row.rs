//! User message bubbles: right-aligned fit-content surface, full Markdown
//! rendering with clickable `file://` / `@file` pills and inline embedded
//! attachments (`![name][img-1]`, `[name][file-1]`), corner grouping for
//! stacked prompts, steer header, auto-collapse with gradient fade, and hover
//! action strip (Copy, Edit from here, Fork, Restore hook).

use std::{collections::HashMap, fmt::Write as _, path::Path};

use rune::{Color, El, Image, SurfaceFit, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::{IconButtonVariant, Markdown, MarkdownStyle};
use viz_client::protocol::{
    PromptAttachment, PromptContext, ThreadId, attachment_ref_labels, text_references_label,
};

use super::{file_preview, markdown as timeline_md};
use crate::{
    app::Desktop,
    model::{User, split_path},
    motion,
    theme::{
        CARD_INSET, HAIRLINE, ICON, ICON_SMALL, RADIUS_CARD, RADIUS_CONTROL, RADIUS_SM,
        RADIUS_SURFACE, READING_WIDTH, S2, S4, S6, S8, S12, Size, Theme,
    },
    ui,
    update::{Msg, TimelineMsg},
};

/// Maximum bubble width as a fraction of the reading lane (`~588pt`).
const BUBBLE_FRAC: f32 = 0.80;
/// Maximum user bubble width in points.
const BUBBLE_MAX_W: f32 = READING_WIDTH * BUBBLE_FRAC;
/// Height cap for auto-collapsed long user messages (`max-h-44` = `176pt`).
const COLLAPSE_HEIGHT: f32 = 176.0;
/// Bottom linear gradient fade height over clipped long messages (`28pt`).
const FADE_HEIGHT: f32 = 28.0;
/// Approximate line threshold before a user message offers auto-collapse.
const COLLAPSE_LINE_LIMIT: usize = 8;
/// Approximate wrapped characters per bubble line for collapse estimation.
const APPROX_LINE_CHARS: usize = 68;
/// Fixed width of a non-image file attachment card (`256pt`).
const FILE_CARD_WIDTH: f32 = 256.0;
/// Maximum display height of an inline embedded attachment image in a bubble.
const INLINE_IMAGE_MAX_H: f32 = 220.0;
/// Maximum display width of an inline embedded attachment image in a bubble.
const INLINE_IMAGE_MAX_W: f32 = 420.0;

/// Position of a user bubble within a stacked group of consecutive user
/// prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackPos {
    /// Standalone user prompt.
    Single,
    /// First prompt of a stacked group (tight bottom-right corner).
    First,
    /// Middle prompt of a stacked group (tight top-right & bottom-right
    /// corners).
    Middle,
    /// Last prompt of a stacked group (tight top-right corner).
    Last,
}

impl StackPos {
    const fn corners(self) -> [f32; 4] {
        match self {
            Self::Single => [RADIUS_SURFACE; 4],
            Self::First => [RADIUS_SURFACE, RADIUS_SURFACE, RADIUS_SM, RADIUS_SURFACE],
            Self::Middle => [RADIUS_SURFACE, RADIUS_SM, RADIUS_SM, RADIUS_SURFACE],
            Self::Last => [RADIUS_SURFACE, RADIUS_SM, RADIUS_SURFACE, RADIUS_SURFACE],
        }
    }
}

/// Renders one right-aligned fit-content user prompt bubble with full Markdown,
/// inline `@file` and attachment rendering, long-message collapse, and hover
/// actions.
pub fn user_row(s: &Desktop, user: &User, pos: StackPos, t: &Theme) -> El {
    let m = &s.model;
    let thread = m.selection.selected;
    let fold_key = format!("{}-full", user.key);
    let collapsible = should_collapse(&user.text);
    let expanded = !collapsible || m.timeline.is_open(thread, &fold_key, false);

    let labels = attachment_ref_labels(user.attachments.iter().map(|a| a.mime.as_str()));
    let unreferenced_attachments: Vec<&PromptAttachment> = user
        .attachments
        .iter()
        .zip(&labels)
        .filter(|(_, label)| !text_references_label(&user.text, label))
        .map(|(att, _)| att)
        .collect();

    let side_actions = matches!(pos, StackPos::First | StackPos::Middle);
    let actions = hover_actions(s, user, t);

    let mut column = col().w_full().min_w(0.0).items_end().gap(S4);

    if user.steered {
        column = column.child(steer_header(t));
    }

    if !unreferenced_attachments.is_empty() {
        column = column.child(attachment_strip(s, &unreferenced_attachments, t));
    }

    if !user.text.is_empty() || !user.context.is_empty() || user.origin.is_some() {
        let card = bubble(s, user, &labels, pos, collapsible, expanded, &fold_key, t);
        if side_actions && let Some(strip) = actions {
            column = column.child(
                row()
                    .w_full()
                    .justify_end()
                    .min_w(0.0)
                    .gap(S4)
                    .center_y()
                    .child(strip)
                    .child(card),
            );
        } else {
            column = column.child(card);
            if let Some(strip) = actions {
                column = column.child(strip);
            }
        }
    } else if let Some(strip) = actions {
        column = column.child(strip);
    }

    row()
        .key(format!("{}-row", user.key))
        .w_full()
        .justify_end()
        .hover_group()
        .child(column)
}

fn steer_header(t: &Theme) -> El {
    row()
        .key("steer-header")
        .gap(S4)
        .px(S4)
        .center_y()
        .child(icon(&ui::REDO2, ICON_SMALL).color(t.steer).a11y_hidden())
        .child(text("Steer").font(t.micro_medium).color(t.muted))
}

#[expect(
    clippy::too_many_arguments,
    reason = "renders fit-content Markdown user bubble with context chips, inline attachments, and collapse toggle"
)]
fn bubble(
    s: &Desktop,
    user: &User,
    attachment_labels: &[String],
    pos: StackPos,
    collapsible: bool,
    expanded: bool,
    fold_key: &str,
    t: &Theme,
) -> El {
    let thread = s.model.selection.selected.unwrap_or(ThreadId(0));
    let cwd = s.model.thread_state().map(|st| st.cwd.as_str());
    let (prepared_md, inlined_files) =
        prepare_user_markdown(s, user, thread, cwd, attachment_labels);
    let remaining_context: Vec<&PromptContext> = user
        .context
        .iter()
        .filter(|ctx| match ctx {
            PromptContext::File { path } => !inlined_files.contains(&path.as_str()),
            PromptContext::Link { .. } => true,
        })
        .collect();

    let corners = pos.corners();
    let mut card = col()
        .key(&user.key)
        .role("article")
        .label("You")
        .max_w(BUBBLE_MAX_W)
        .min_w(0.0)
        .padding(CARD_INSET)
        .gap(S8)
        .corners(corners)
        .fill(t.mine);
    if !t.k.dark {
        card = card.border(HAIRLINE, t.hairline);
    }

    if !remaining_context.is_empty() || user.origin.is_some() {
        card = card.child(context_chips(&remaining_context, user.origin.as_ref(), t));
    }

    if !prepared_md.is_empty() {
        let body = user_markdown_body(s, user, thread, cwd, prepared_md, t);

        if collapsible && !expanded {
            let clipped = col()
                .key("clipped")
                .min_w(0.0)
                .max_h(COLLAPSE_HEIGHT)
                .clip_fade_linear(0.0, 0.0, 0.0, FADE_HEIGHT)
                .child(body);
            card = card.child(clipped);
        } else {
            card = card.child(body);
        }
    }

    if collapsible {
        let toggle_key = fold_key.to_owned();
        let anchor_key = user.key.clone();
        let label = if expanded {
            "Show less"
        } else {
            "Show full message"
        };
        card = card.child(
            row().justify_start().child(
                rune_kit::Button::new(label)
                    .variant(rune_kit::ButtonVariant::GhostMuted)
                    .control_size(Size::Micro.kit())
                    .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                        s.dispatch(
                            Msg::Timeline(TimelineMsg::ToggleFold {
                                key: toggle_key.clone(),
                                default_open: false,
                                anchor: Some(anchor_key.clone()),
                            }),
                            tx,
                        );
                    })
                    .build(&t.k)
                    .key("toggle-full"),
            ),
        );
    }

    card
}

fn user_markdown_body(
    s: &Desktop,
    user: &User,
    thread: ThreadId,
    cwd: Option<&str>,
    source: String,
    t: &Theme,
) -> El {
    let mut image_map: HashMap<String, Image> = HashMap::new();
    for att in &user.attachments {
        if att.mime.starts_with("image/")
            && let Some(img) = s.thread_attachment_image(thread, &att.id)
            && img.is_ready()
            && img.error().is_none()
        {
            image_map.insert(
                format!("viz-attachment://{}/{}", thread.0, att.id),
                img.clone(),
            );
        }
    }

    let border = t.hairline;
    let mut md = timeline_md::with_file_pills(
        Markdown::new(source)
            .style(user_markdown_style(t))
            .selectable(true)
            .copy_source(true)
            .scroll_wide(true),
        cwd,
        t,
    );

    if !image_map.is_empty() {
        md = md
            .images(move |src| image_map.get(src).cloned())
            .image_frame(move |el, frame| {
                let cap_w = frame.image.size().map_or(INLINE_IMAGE_MAX_W, |[w, h]| {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "image dimensions fit comfortably in f32 layout bounds"
                    )]
                    let (wf, hf) = (w as f32, (h.max(1)) as f32);
                    let aspect = wf / hf;
                    wf.min(INLINE_IMAGE_MAX_W).min(INLINE_IMAGE_MAX_H * aspect)
                });
                let open_src = frame.src.to_owned();
                el.max_w(cap_w)
                    .radius(RADIUS_CONTROL)
                    .clip_rounded(RADIUS_CONTROL)
                    .border(HAIRLINE, border)
                    .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                        s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(open_src.clone())), tx);
                    })
            });
    }

    md.build(&t.k).key("text").min_w(0.0)
}

fn user_markdown_style(t: &Theme) -> MarkdownStyle {
    let table_font = TextStyle {
        size: 13.0,
        line_height: 20.0,
        ..t.body
    };
    let code_bg = if t.k.dark {
        t.canvas.alpha(0.55)
    } else {
        t.text.alpha(0.06)
    };
    let code_border = if t.k.dark {
        t.hairline
    } else {
        t.border_control
    };
    t.markdown()
        .inline_code_bg(code_bg)
        .inline_code_pad(4.0)
        .code_block(code_bg, code_border)
        .code_block_padding(S12, 8.0)
        .code_margin(6.0, 8.0)
        .text_width(2.0_f32.mul_add(-CARD_INSET, BUBBLE_MAX_W))
        .table_outline(None)
        .table_header_fill(Color::TRANSPARENT)
        .table_header_text(t.text_strong)
        .table_header_weight(600.0)
        .table_rules(t.hairline)
        .table_flush_left(true)
        .table_font(table_font)
        .table_cell_padding(10.0, 6.0)
        .table_header_pad(10.0, 6.0)
        .table_margin(6.0, 8.0)
        .table_scroll_bar(t.faint.alpha(0.35), t.muted.alpha(0.55))
}

/// Transforms a user prompt's raw text into render-ready Markdown:
/// 1. Converts `@file` mentions matching `PromptContext::File` (outside code)
///    into clickable `file://` / `viz-file://` pill links.
/// 2. Downgrades image attachment embeds (`![alt][img-N]`) whose pixels are not
///    yet decoded into clickable `[alt][img-N]` attachment pills so no
///    broken-image icon flashes while loading.
/// 3. Appends CommonMark link reference definitions for every stored attachment
///    pointing to `viz-attachment://<thread>/<id>`.
fn prepare_user_markdown<'a>(
    s: &Desktop,
    user: &'a User,
    thread: ThreadId,
    cwd: Option<&str>,
    labels: &[String],
) -> (String, Vec<&'a str>) {
    if user.text.is_empty() {
        return (String::new(), Vec::new());
    }

    let mut md = user.text.clone();
    let mut inlined_files = Vec::new();
    for ctx in &user.context {
        let PromptContext::File { path } = ctx else {
            continue;
        };
        let target_url = context_file_url(path, cwd);
        let (_, base) = split_path(path);
        let mut matched = false;
        let full_token = format!("@{path}");
        if replace_outside_code(
            &mut md,
            &full_token,
            &format!("[{full_token}](<{target_url}>)"),
        ) {
            matched = true;
        }
        if !base.is_empty() && base != path {
            let base_token = format!("@{base}");
            if replace_outside_code(
                &mut md,
                &base_token,
                &format!("[{base_token}](<{target_url}>)"),
            ) {
                matched = true;
            }
        }
        if matched {
            inlined_files.push(path.as_str());
        }
    }

    if !user.attachments.is_empty() {
        for (att, label) in user.attachments.iter().zip(labels) {
            if att.mime.starts_with("image/") {
                let ready = s
                    .thread_attachment_image(thread, &att.id)
                    .is_some_and(|img| img.is_ready() && img.error().is_none());
                if !ready {
                    downgrade_image_ref_to_pill(&mut md, label);
                }
            }
        }

        md.push_str("\n\n");
        for (idx, (att, label)) in user.attachments.iter().zip(labels).enumerate() {
            if idx > 0 {
                md.push('\n');
            }
            let escaped = att.name.replace('\\', "\\\\").replace('"', "\\\"");
            let _ = write!(
                md,
                "[{label}]: <viz-attachment://{}/{}> \"{escaped}\"",
                thread.0, att.id
            );
        }
    }

    (md, inlined_files)
}

fn context_file_url(path: &str, cwd: Option<&str>) -> String {
    if Path::new(path).is_absolute()
        && let Some(url) = file_preview::path_to_file_url(path)
    {
        return url;
    }
    if let Some(root) = cwd {
        let joined = Path::new(root).join(path);
        if let Some(url) = file_preview::path_to_file_url(&joined.to_string_lossy()) {
            return url;
        }
    }
    let encoded = path.replace(' ', "%20");
    format!("viz-file://{encoded}")
}

/// Replaces standalone occurrences of `token` (not preceded by `[` or
/// identifier chars and not inside inline code or fenced code blocks) with
/// `replacement`.
fn replace_outside_code(text: &mut String, token: &str, replacement: &str) -> bool {
    if token.is_empty() || !text.contains(token) {
        return false;
    }
    let bytes = text.as_bytes();
    let tbytes = token.as_bytes();
    let mut out = String::with_capacity(text.len() + 32);
    let mut i = 0usize;
    let mut in_fence = false;
    let mut inline_ticks = 0usize;
    let mut at_line_start = true;
    let mut changed = false;

    while i < bytes.len() {
        if at_line_start && inline_ticks == 0 {
            let trimmed_off = bytes[i..]
                .iter()
                .take(3)
                .take_while(|&&b| b == b' ')
                .count();
            if bytes[i + trimmed_off..].starts_with(b"```")
                || bytes[i + trimmed_off..].starts_with(b"~~~")
            {
                in_fence = !in_fence;
                let line_end = text[i..].find('\n').map_or(bytes.len(), |off| i + off + 1);
                out.push_str(&text[i..line_end]);
                i = line_end;
                at_line_start = true;
                continue;
            }
        }
        if in_fence {
            let line_end = text[i..].find('\n').map_or(bytes.len(), |off| i + off + 1);
            out.push_str(&text[i..line_end]);
            i = line_end;
            at_line_start = true;
            continue;
        }
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if inline_ticks == 0 {
                inline_ticks = run;
            } else if inline_ticks == run {
                inline_ticks = 0;
            }
            out.push_str(&text[i..i + run]);
            i += run;
            at_line_start = false;
            continue;
        }
        if inline_ticks == 0 && bytes[i..].starts_with(tbytes) {
            let prev = if i == 0 { None } else { Some(bytes[i - 1]) };
            let next = bytes.get(i + tbytes.len()).copied();
            let valid_prev = prev
                .is_none_or(|b| !b.is_ascii_alphanumeric() && b != b'[' && b != b'_' && b != b'/');
            let valid_next = next.is_none_or(|b| {
                !b.is_ascii_alphanumeric() && b != b'_' && b != b'.' && b != b'/' && b != b'-'
            });
            if valid_prev && valid_next {
                out.push_str(replacement);
                i += tbytes.len();
                at_line_start = false;
                changed = true;
                continue;
            }
        }
        let ch = text[i..].chars().next().expect("valid utf-8 boundary");
        out.push(ch);
        i += ch.len_utf8();
        at_line_start = ch == '\n';
    }

    if changed {
        *text = out;
    }
    changed
}

/// Converts `![alt][label]` into `[alt][label]` so an unloaded image renders
/// as a clickable pill link instead of a broken-image glyph.
fn downgrade_image_ref_to_pill(text: &mut String, label: &str) {
    let needle = format!("][{label}]");
    let mut search_from = 0usize;
    while let Some(rel) = text[search_from..].find(&needle) {
        let pos = search_from + rel;
        if let Some(open) = text[..pos].rfind('[')
            && open > 0
            && text.as_bytes()[open - 1] == b'!'
        {
            text.remove(open - 1);
            search_from = pos + needle.len() - 1;
        } else {
            search_from = pos + needle.len();
        }
    }
}

fn context_chips(
    context: &[&PromptContext],
    origin: Option<&viz_client::protocol::Origin>,
    t: &Theme,
) -> El {
    let mut strip = row().key("context-chips").gap(S4).flex_wrap().center_y();
    if let Some(origin) = origin {
        strip = strip.child(
            rune_kit::Badge::new(format!("via {}", origin.connector))
                .build(&t.k)
                .key("origin"),
        );
    }
    let chip_bg = t.text.alpha(if t.k.dark { 0.07 } else { 0.05 });
    for (idx, ctx) in context.iter().enumerate() {
        let chip = match ctx {
            PromptContext::File { path } => {
                let open_path = path.clone();
                let (_, name) = split_path(path);
                let display = if name.is_empty() { path.as_str() } else { name };
                row()
                    .role("button")
                    .label(format!("Preview {path}"))
                    .help(format!("Preview {path}"))
                    .px(S6)
                    .py(S2)
                    .gap(S4)
                    .center_y()
                    .radius(RADIUS_SM)
                    .fill(chip_bg)
                    .border(HAIRLINE, t.hairline)
                    .hover(t.hover_wash)
                    .focusable(true)
                    .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                    .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                        s.dispatch(
                            Msg::Timeline(TimelineMsg::OpenFilePreview {
                                path: open_path.clone(),
                                lines: None,
                            }),
                            tx,
                        );
                    })
                    .child(
                        icon(rune_kit::file_icon(path), ICON_SMALL)
                            .color(t.muted)
                            .a11y_hidden(),
                    )
                    .child(
                        text(format!("@{display}"))
                            .font(t.small_medium)
                            .color(t.text),
                    )
            }
            PromptContext::Link { uri, name, title } => {
                let link_uri = uri.clone();
                let mut chip = row()
                    .role("link")
                    .label(title.as_deref().unwrap_or(name))
                    .help(title.clone().unwrap_or_else(|| uri.clone()))
                    .px(S6)
                    .py(S2)
                    .gap(S4)
                    .center_y()
                    .radius(RADIUS_SM)
                    .fill(chip_bg)
                    .border(HAIRLINE, t.hairline)
                    .hover(t.hover_wash)
                    .focusable(true)
                    .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                    .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                        s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(link_uri.clone())), tx);
                    })
                    .child(text(name).font(t.small_medium).color(t.text_secondary));
                if let Some(title) = title {
                    chip = chip.help(title.clone());
                }
                chip
            }
        };
        strip = strip.child(chip.key(format!("ctx-{idx}")));
    }
    strip
}

fn attachment_strip(s: &Desktop, attachments: &[&PromptAttachment], t: &Theme) -> El {
    let thread = s.model.selection.selected;
    let mut strip = row()
        .key("message-attachments")
        .gap(S6)
        .flex_wrap()
        .justify_end();
    for (idx, file) in attachments.iter().enumerate() {
        let is_image = file.mime.starts_with("image/");
        let glyph = if is_image {
            &ui::EYE
        } else {
            rune_kit::file_icon(&file.name)
        };
        let thumb = thread
            .filter(|_| is_image)
            .and_then(|thread_id| s.thread_attachment_image(thread_id, &file.id))
            .filter(|img| img.is_ready() && img.error().is_none())
            .map_or_else(
                || {
                    row()
                        .size(32.0, 32.0)
                        .center()
                        .radius(RADIUS_SM)
                        .fill(t.chip_fill)
                        .a11y_hidden()
                        .child(icon(glyph, ICON).color(t.muted))
                },
                |image| {
                    el().size(32.0, 32.0)
                        .radius(RADIUS_SM)
                        .clip_rounded(RADIUS_SM)
                        .pixels_fit(image.pixels(), SurfaceFit::Fill)
                        .a11y_hidden()
                },
            );
        let mut card = row()
            .key(format!("attachment-{idx}"))
            .w(FILE_CARD_WIDTH)
            .py(S8)
            .pl(S8)
            .pr(S12)
            .gap(S8)
            .center_y()
            .radius(RADIUS_CARD)
            .fill(t.mine)
            .border(HAIRLINE, t.hairline)
            .child(thumb)
            .child(
                col()
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .gap(S2)
                    .child(
                        text(&file.name)
                            .font(t.small_medium)
                            .color(t.text)
                            .truncate()
                            .help_when_truncated(&file.name),
                    )
                    .child(text(&file.mime).font(t.micro).color(t.faint).truncate()),
            );
        if let Some(thread) = thread {
            let id = file.id.clone();
            let name = file.name.clone();
            let mime = file.mime.clone();
            card = card
                .role("button")
                .label(format!("Preview {}", file.name))
                .hover(t.hover_wash)
                .focusable(true)
                .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(
                        Msg::Timeline(TimelineMsg::OpenAttachmentPreview {
                            thread,
                            id: id.clone(),
                            name: name.clone(),
                            mime: mime.clone(),
                        }),
                        tx,
                    );
                });
        }
        strip = strip.child(card);
    }
    strip
}

fn hover_actions(s: &Desktop, user: &User, t: &Theme) -> Option<El> {
    if user.text.is_empty() && user.entry_id.is_none() {
        return None;
    }
    let connected = s.model.connected();
    let has_git = s.model.project().is_some_and(|project| project.git);
    let copy_key = format!("copy-{}", user.key);
    let copied = s.model.timeline.copied_key.as_deref() == Some(copy_key.as_str());

    let mut actions = row()
        .key(format!("{}-actions", user.key))
        .h(Size::Small.height())
        .gap(S2)
        .px(S4)
        .center_y()
        .group_opacity(0.0, 1.0);

    if let Some(entry) = user.entry_id {
        if has_git {
            actions = actions.child(
                rune_kit::IconButton::new(&ui::ROTATE_CCW)
                    .help("Restore checkpoint")
                    .variant(IconButtonVariant::GhostMuted)
                    .control_size(Size::Small.kit())
                    .disabled(!connected)
                    .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                        s.dispatch(Msg::Timeline(TimelineMsg::RestoreCheckpoint(entry)), tx);
                    })
                    .build(&t.k)
                    .key("restore"),
            );
        }
        actions = actions.child(
            rune_kit::IconButton::new(&ui::GIT_FORK)
                .help("Fork from here")
                .variant(IconButtonVariant::GhostMuted)
                .control_size(Size::Small.kit())
                .disabled(!connected)
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(
                        Msg::Timeline(TimelineMsg::ForkBefore {
                            entry,
                            load_prompt: false,
                        }),
                        tx,
                    );
                })
                .build(&t.k)
                .key("fork-before"),
        );
        actions = actions.child(
            rune_kit::IconButton::new(&ui::PENCIL)
                .help("Edit from here")
                .variant(IconButtonVariant::GhostMuted)
                .control_size(Size::Small.kit())
                .disabled(!connected)
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(
                        Msg::Timeline(TimelineMsg::ForkBefore {
                            entry,
                            load_prompt: true,
                        }),
                        tx,
                    );
                })
                .build(&t.k)
                .key("edit-from-here"),
        );
    }

    if !user.text.is_empty() {
        let text_copy = user.text.clone();
        let copy_id = copy_key;
        let copy_btn = copy_morph_button(
            "copy-user",
            "Copy message",
            copied,
            move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::CopyWithFeedback {
                        key: copy_id.clone(),
                        text: text_copy.clone(),
                    }),
                    tx,
                );
            },
            t,
        );
        actions = actions.child(copy_btn);
    }

    Some(actions)
}

/// Compact `24×24` Copy button with in-place `Copy <-> Check` icon morph.
pub fn copy_morph_button(
    key: &'static str,
    label: &'static str,
    copied: bool,
    on_press: impl Fn(&mut Desktop, &mut Tx) + 'static,
    t: &Theme,
) -> El {
    let glyph = if copied { &ui::CHECK } else { &ui::COPY };
    let color = if copied { t.success } else { t.muted };
    let state_key = if copied { "done" } else { "copy" };
    let side = Size::Small.height();
    row()
        .key(key)
        .role("button")
        .label(label)
        .help(label)
        .size(side, side)
        .center()
        .radius(Size::Small.radius())
        .hover(t.hover_wash)
        .press(t.pressed_wash)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .press_scale(motion::PRESS_SCALE)
        .on_press_tx(on_press)
        .child(motion::icon_morph(
            "morph",
            state_key,
            glyph,
            Size::Small.icon_size(),
            Size::Small.icon_size(),
            color,
        ))
}

fn should_collapse(text: &str) -> bool {
    let estimated_lines: usize = text
        .lines()
        .map(|line| (line.chars().count() / APPROX_LINE_CHARS).max(1))
        .sum();
    estimated_lines > COLLAPSE_LINE_LIMIT
}
