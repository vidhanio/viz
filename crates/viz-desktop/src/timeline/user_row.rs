//! User message bubbles: right-aligned surface, corner grouping for stacked
//! prompts, steer header, attachment cards, `176pt` auto-collapse with gradient
//! fade, and hover action strip (Copy, Edit from here, Fork, Restore hook).

use rune::{Color, El, Tx, col, el, icon, row, text};
use rune_kit::IconButtonVariant;
use viz_client::protocol::{PromptAttachment, PromptContext};

use crate::{
    app::Desktop,
    model::User,
    motion,
    theme::{
        CARD_INSET, HAIRLINE, ICON, ICON_SMALL, RADIUS_CARD, RADIUS_SM, RADIUS_SURFACE, S2, S4, S6,
        S8, S12, Size, Theme,
    },
    ui::{self, BadgeSize, InlineStyle, Tone},
    update::{Msg, TimelineMsg},
};

/// Maximum bubble width as a fraction of the `736pt` reading lane (`~588pt`).
const BUBBLE_FRAC: f32 = 0.80;
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

/// Renders one right-aligned user prompt bubble with optional steer header,
/// context/attachment cards, long-message collapse, and hover actions.
pub fn user_row(s: &Desktop, user: &User, pos: StackPos, t: &Theme) -> El {
    let m = &s.model;
    let fold_key = format!("{}-full", user.key);
    let collapsible = should_collapse(&user.text);
    let expanded = !collapsible
        || m.timeline.is_open(
            m.selection.selected,
            &m.disclosure.expanded,
            &fold_key,
            None,
            false,
        );

    let mut column = col().min_w(0.0).shrink().items_end().gap(S4);

    if user.steered {
        column = column.child(steer_header(t));
    }

    if !user.attachments.is_empty() {
        column = column.child(attachment_strip(&user.attachments, t));
    }

    if !user.text.is_empty() || !user.context.is_empty() {
        column = column.child(bubble(user, pos, collapsible, expanded, &fold_key, t));
    }

    if let Some(actions) = hover_actions(s, user, t) {
        column = column.child(actions);
    }

    row()
        .key(format!("{}-row", user.key))
        .w_full()
        .justify_end()
        .hover_group()
        .child(el().w_frac(1.0 - BUBBLE_FRAC).a11y_hidden())
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
    user: &User,
    pos: StackPos,
    collapsible: bool,
    expanded: bool,
    fold_key: &str,
    t: &Theme,
) -> El {
    let corners = pos.corners();
    let mut card = col()
        .key(&user.key)
        .role("article")
        .label("You")
        .w_full()
        .padding(CARD_INSET)
        .gap(S8)
        .corners(corners)
        .fill(t.mine);
    if !t.k.dark {
        card = card.border(HAIRLINE, t.hairline);
    }

    if !user.context.is_empty() || user.origin.is_some() {
        card = card.child(context_chips(&user.context, user.origin.as_ref(), t));
    }

    if !user.text.is_empty() {
        let body = text(&user.text)
            .key("text")
            .font(t.body)
            .color(t.text)
            .wrap()
            .selectable(true)
            .selection_color(t.k.selection());

        if collapsible && !expanded {
            let fade = el()
                .key("fade")
                .pin(Some(0.0), None, Some(0.0), Some(0.0))
                .h(FADE_HEIGHT)
                .linear_gradient(0.0, [(0.0, Color::TRANSPARENT), (1.0, t.mine)])
                .a11y_hidden();
            let clipped = col()
                .key("clipped")
                .w_full()
                .max_h(COLLAPSE_HEIGHT)
                .clip_rounded(0.0)
                .child(body)
                .child(fade);
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
            row().w_full().justify_start().child(
                ui::inline_button(label)
                    .style(InlineStyle::InkOnly)
                    .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                        s.dispatch(
                            Msg::Timeline(TimelineMsg::ToggleFold {
                                key: toggle_key.clone(),
                                legacy_key: None,
                                default_open: false,
                                anchor: Some(anchor_key.clone()),
                            }),
                            tx,
                        );
                    })
                    .build(t)
                    .key("toggle-full"),
            ),
        );
    }

    card
}

fn context_chips(
    context: &[PromptContext],
    origin: Option<&viz_client::protocol::Origin>,
    t: &Theme,
) -> El {
    let mut strip = row().key("context-chips").gap(S4).flex_wrap().center_y();
    if let Some(origin) = origin {
        strip = strip.child(
            ui::badge(format!("via {}", origin.connector))
                .tone(Tone::Neutral)
                .size(BadgeSize::Nano)
                .build(t)
                .key("origin"),
        );
    }
    for (idx, ctx) in context.iter().enumerate() {
        let chip = match ctx {
            PromptContext::File { path } => ui::badge(format!("@{path}"))
                .tone(Tone::Info)
                .size(BadgeSize::Nano)
                .mono()
                .build(t),
            PromptContext::Link { name, title, .. } => {
                let mut badge = ui::badge(name)
                    .tone(Tone::Delegated)
                    .size(BadgeSize::Nano)
                    .build(t);
                if let Some(title) = title {
                    badge = badge.help(title.clone());
                }
                badge
            }
        };
        strip = strip.child(chip.key(format!("ctx-{idx}")));
    }
    strip
}

fn attachment_strip(attachments: &[PromptAttachment], t: &Theme) -> El {
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
        let card = row()
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
            .child(
                row()
                    .size(32.0, 32.0)
                    .center()
                    .radius(RADIUS_SM)
                    .fill(t.chip_fill)
                    .a11y_hidden()
                    .child(icon(glyph, ICON).color(t.muted)),
            )
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
                ui::icon_button(
                    &ui::ROTATE_CCW,
                    "Restore checkpoint",
                    IconButtonVariant::GhostMuted,
                    Size::Small,
                )
                .disabled(!connected)
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(Msg::Timeline(TimelineMsg::RestoreCheckpoint(entry)), tx);
                })
                .build(&t.k)
                .key("restore"),
            );
        }
        actions = actions.child(
            ui::icon_button(
                &ui::GIT_FORK,
                "Fork from here",
                IconButtonVariant::GhostMuted,
                Size::Small,
            )
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
            ui::icon_button(
                &ui::PENCIL,
                "Edit from here",
                IconButtonVariant::GhostMuted,
                Size::Small,
            )
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
