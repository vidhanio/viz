//! User message bubbles: right-aligned fit-content surface, full Markdown
//! rendering with clickable `file://` / `@file` pills and inline embedded
//! attachments (`![name][img-1]`, `[name][file-1]`), corner grouping for
//! stacked prompts, steer header, auto-collapse with gradient fade, and hover
//! action strip (Copy, Edit from here, Fork, Restore hook).

use std::{collections::HashMap, fmt::Write as _, path::Path};

use rune::{Color, El, Image, TextStyle, Tx, col, icon, row, text};
use rune_kit::{IconButtonVariant, Markdown, MarkdownStyle};
use viz_client::protocol::{PromptRef, ThreadId};

use super::{file_preview, markdown as timeline_md};
use crate::{
    app::Desktop,
    model::User,
    motion,
    theme::{
        CARD_INSET, HAIRLINE, ICON_SMALL, RADIUS_CONTROL, RADIUS_SM, RADIUS_SURFACE, READING_WIDTH,
        S2, S4, S8, S12, Size, Theme,
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
    let fold_key = format!("{}-full", user.key);
    let collapsible = should_collapse(&user.prompt.text) || user.full.is_some();
    let expanded = !collapsible || m.is_fold_open(&fold_key, false);

    let side_actions = matches!(pos, StackPos::First | StackPos::Middle);
    let actions = hover_actions(s, user, t);

    let mut column = col().w_full().min_w(0.0).items_end().gap(S4);

    if user.steered {
        column = column.child(steer_header(t));
    }

    if !user.prompt.text.is_empty() || user.origin.is_some() {
        let card = bubble(s, user, pos, collapsible, expanded, &fold_key, t);
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

fn bubble(
    s: &Desktop,
    user: &User,
    pos: StackPos,
    collapsible: bool,
    expanded: bool,
    fold_key: &str,
    t: &Theme,
) -> El {
    let thread = s.model.selection.selected.unwrap_or(ThreadId(0));
    let cwd = s.model.thread_state().map(|st| st.cwd.as_str());
    let prepared_md = prepare_user_markdown(s, user, thread, cwd);
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

    if let Some(origin) = &user.origin {
        card = card.child(rune_kit::Badge::new(format!("via {}", origin.connector)).build(&t.k));
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
        let preview_entry = user.entry_id.filter(|_| {
            user.full
                .is_some_and(|size| size > viz_client::protocol::limits::AUTO_HYDRATE_ENTRY_BYTES)
        });
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
                        let msg = preview_entry.map_or_else(
                            || TimelineMsg::ToggleFold {
                                key: toggle_key.clone(),
                                default_open: false,
                                anchor: Some(anchor_key.clone()),
                            },
                            |entry| TimelineMsg::OpenEntryPreview {
                                key: anchor_key.clone(),
                                entry,
                                title: "Prompt".into(),
                            },
                        );
                        s.dispatch(Msg::Timeline(msg), tx);
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
    for att in user.prompt.attachments() {
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

/// Resolves canonical prompt anchors to timeline links and images.
fn prepare_user_markdown(s: &Desktop, user: &User, thread: ThreadId, cwd: Option<&str>) -> String {
    let mut md = String::new();
    let mut end = 0;
    for anchor in user.prompt.anchors() {
        md.push_str(&user.prompt.text[end..anchor.range.start]);
        end = anchor.range.end;
        let mut image = false;
        let target = match anchor.target {
            PromptRef::File { path, .. } => Some(context_file_url(path, cwd)),
            PromptRef::Link { uri, .. } => Some(uri.clone()),
            PromptRef::GitHub { item } => Some(item.url.clone()),
            PromptRef::Attachment { id, mime, .. } => {
                image = mime.starts_with("image/")
                    && s.thread_attachment_image(thread, id)
                        .is_some_and(|img| img.is_ready() && img.error().is_none());
                Some(format!("viz-attachment://{}/{id}", thread.0))
            }
            _ => None,
        };
        if let Some(url) = target {
            let prefix = if image { "!" } else { "" };
            let label = anchor
                .label
                .replace('\\', "\\\\")
                .replace('[', "\\[")
                .replace(']', "\\]");
            let _ = write!(md, "{prefix}[{label}](<{url}>)");
        } else {
            md.push_str(&anchor.label);
        }
    }
    md.push_str(&user.prompt.text[end..]);
    md
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

fn hover_actions(s: &Desktop, user: &User, t: &Theme) -> Option<El> {
    if user.prompt.text.is_empty() && user.entry_id.is_none() {
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

    Some(actions.children(copy_action(user, copied, t)))
}

fn copy_action(user: &User, copied: bool, t: &Theme) -> Option<El> {
    if let Some(entry) = user.entry_id.filter(|_| user.full.is_some()) {
        let key = user.key.clone();
        return Some(copy_morph_button(
            "copy-user",
            "Copy message",
            copied,
            move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::CopyPrompt {
                        key: key.clone(),
                        entry,
                    }),
                    tx,
                );
            },
            t,
        ));
    }
    if user.prompt.text.is_empty() {
        return None;
    }
    let text_copy = user.prompt.display_text();
    let copy_id = format!("copy-{}", user.key);
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
    Some(copy_btn)
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
