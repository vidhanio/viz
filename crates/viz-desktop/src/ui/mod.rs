//! The desktop's components on Rune and rune-kit. Each takes a [`Size`] or a
//! variant and owns its insets; views compose them and add no padding of
//! their own beyond the layout tokens in `theme`.

use std::borrow::Cow;

use rune::{Color, Edge, El, Icon, TextEdit, Transition, Tx, col, el, icon, row, text};
use rune_kit::{
    ButtonVariant, IconButtonVariant, Keybind, Kit, Menu, MenuItem, Placement, PopupList, Spinner,
    TextInput,
};
use viz_client::protocol::ThreadStatus;

use crate::theme::{
    CARD_INSET, CARD_WIDTH, DIALOG_INSET, DIALOG_WIDTH, DOT, EMPTY_WIDTH, HAIRLINE, HORIZON_HEIGHT,
    ICON, ICON_SMALL, RADIUS_CARD, RADIUS_COMPOSER, RADIUS_INNER, READING_INSET, READING_WIDTH,
    ROW_INSET, S2, S4, S6, S8, S12, S16, S20, Size, TRAILING, TREE_ROW, TREE_ROW_TALL, TREE_STEP,
    Theme,
};

pub const COMMAND_KEY: &str = if cfg!(target_os = "macos") {
    "⌘"
} else {
    "Ctrl"
};
/// The sidebar toggle's shortcut, as its tooltip shows it.
pub const SIDEBAR_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘B"
} else {
    "Ctrl+B"
};

/// Formats a command-key shortcut for tooltips (`⌘N` on macOS, `Ctrl+N`
/// elsewhere).
pub fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("{COMMAND_KEY}{key}")
    } else {
        format!("{COMMAND_KEY}+{key}")
    }
}

type PressHandler = Box<dyn FnOnce(El) -> El>;

/// A text button whose hover feedback is ink only, with neutral disabled
/// submit styling.
pub struct Button {
    label: String,
    variant: ButtonVariant,
    size: Size,
    icon: Option<&'static Icon>,
    disabled: bool,
    press: Option<PressHandler>,
}

struct ButtonStyle {
    fill: Color,
    border: Option<Color>,
    rest: Color,
    hover: Color,
    press: Color,
}

fn button_style(variant: ButtonVariant, disabled: bool, k: &Kit) -> ButtonStyle {
    let c = &k.c;
    match variant {
        ButtonVariant::Submit if disabled => ButtonStyle {
            fill: c.bg_button_neutral,
            border: None,
            rest: c.text_faint,
            hover: c.text_faint,
            press: Color::TRANSPARENT,
        },
        ButtonVariant::Submit => ButtonStyle {
            fill: c.bg_icon_button_contrast,
            border: None,
            rest: c.submit_fg,
            hover: c.submit_fg,
            press: k.on(c.contrast_pressed, c.bg_icon_button_contrast),
        },
        ButtonVariant::Outline => ButtonStyle {
            fill: Color::TRANSPARENT,
            border: Some(c.border_muted),
            rest: c.text_base,
            hover: c.text_strong,
            press: c.pressed,
        },
        ButtonVariant::Danger => ButtonStyle {
            fill: c.bg_button_neutral,
            border: None,
            rest: c.danger.fg,
            hover: c.danger.fg,
            press: c.pressed,
        },
        ButtonVariant::Warning => ButtonStyle {
            fill: c.bg_button_neutral,
            border: None,
            rest: c.warning.fg,
            hover: c.warning.fg,
            press: c.pressed,
        },
        ButtonVariant::GhostMuted => ButtonStyle {
            fill: Color::TRANSPARENT,
            border: None,
            rest: c.text_muted,
            hover: c.text_base,
            press: Color::TRANSPARENT,
        },
        ButtonVariant::GhostFaint => ButtonStyle {
            fill: Color::TRANSPARENT,
            border: None,
            rest: c.text_faint,
            hover: c.text_base,
            press: Color::TRANSPARENT,
        },
        ButtonVariant::Ghost => ButtonStyle {
            fill: Color::TRANSPARENT,
            border: None,
            rest: c.text_base,
            hover: c.text_strong,
            press: Color::TRANSPARENT,
        },
        ButtonVariant::Contrast => ButtonStyle {
            fill: c.bg_contrast,
            border: None,
            rest: c.text_contrast,
            hover: c.text_contrast,
            press: k.on(c.contrast_pressed, c.bg_contrast),
        },
        ButtonVariant::Neutral | ButtonVariant::Loading => ButtonStyle {
            fill: c.bg_button_neutral,
            border: None,
            rest: c.text_base,
            hover: c.text_strong,
            press: c.pressed,
        },
    }
}

fn icon_button_style(
    variant: IconButtonVariant,
    disabled: bool,
    active: bool,
    k: &Kit,
) -> ButtonStyle {
    let c = &k.c;
    match variant {
        IconButtonVariant::Submit | IconButtonVariant::Contrast if disabled => ButtonStyle {
            fill: c.bg_button_neutral,
            border: None,
            rest: c.icon_faint,
            hover: c.icon_faint,
            press: Color::TRANSPARENT,
        },
        IconButtonVariant::Submit | IconButtonVariant::Contrast => ButtonStyle {
            fill: c.bg_icon_button_contrast,
            border: None,
            rest: c.submit_icon,
            hover: c.submit_icon,
            press: k.on(c.contrast_pressed, c.bg_icon_button_contrast),
        },
        IconButtonVariant::Neutral => ButtonStyle {
            fill: c.bg_button_neutral,
            border: None,
            rest: c.icon_base,
            hover: c.std_icon_strong,
            press: c.pressed,
        },
        IconButtonVariant::Ghost if active => ButtonStyle {
            fill: c.pressed,
            border: Some(c.border_base),
            rest: c.icon_base,
            hover: c.std_icon_strong,
            press: Color::TRANSPARENT,
        },
        IconButtonVariant::GhostMuted if active => ButtonStyle {
            fill: c.pressed,
            border: Some(c.border_base),
            rest: c.icon_accent,
            hover: c.icon_accent,
            press: Color::TRANSPARENT,
        },
        IconButtonVariant::Ghost => ButtonStyle {
            fill: Color::TRANSPARENT,
            border: None,
            rest: c.icon_base,
            hover: c.std_icon_strong,
            press: Color::TRANSPARENT,
        },
        IconButtonVariant::GhostMuted => ButtonStyle {
            fill: Color::TRANSPARENT,
            border: None,
            rest: c.icon_muted,
            hover: c.icon_base,
            press: Color::TRANSPARENT,
        },
    }
}

impl Button {
    pub const fn icon(mut self, glyph: &'static Icon) -> Self {
        self.icon = Some(glyph);
        self
    }

    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.press = Some(Box::new(move |el| el.on_press(f)));
        self
    }

    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.press = Some(Box::new(move |el| el.on_press_tx(f)));
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let px = match self.size {
            Size::Small | Size::Default => S12,
            Size::Large => S16,
        };
        let font = if matches!(self.variant, ButtonVariant::GhostMuted) {
            k.ty.ui
        } else {
            k.ty.ui_medium
        };
        let style = button_style(self.variant, self.disabled, k);
        let mut el = row()
            .role("button")
            .label(&self.label)
            .h(self.size.height())
            .px(px)
            .gap(S6)
            .center()
            .radius(self.size.radius())
            .continuous_corners()
            .fill(style.fill)
            .disabled(self.disabled)
            .focusable(!self.disabled)
            .focus_ring(HAIRLINE, S2, k.c.border_focus);
        if let Some(border) = style.border {
            el = el.border(HAIRLINE, border);
        }
        if !self.disabled {
            el = el.hover_group().press_color(style.hover).press_scale(0.97);
            if style.press.a > 0.0 {
                el = el.press(style.press);
            }
        }
        if let Some(glyph) = self.icon {
            let mut mark = icon(glyph, ICON)
                .key("icon")
                .color(style.rest)
                .a11y_hidden();
            if !self.disabled {
                mark = mark.group_color(style.hover);
            }
            el = el.child(mark);
        }
        let mut label = text(self.label)
            .key("label")
            .font(font)
            .color(style.rest)
            .a11y_hidden();
        if !self.disabled {
            label = label.group_color(style.hover);
        }
        el = el.child(label);
        if self.disabled && !matches!(self.variant, ButtonVariant::Submit) {
            el = el.layer_opacity(0.45);
        }
        if !self.disabled
            && let Some(press) = self.press
        {
            el = press(el);
        }
        el
    }
}

/// A square icon button whose hover feedback is ink only, with neutral
/// disabled submit styling and a distinct active state when expanded.
pub struct IconButton {
    glyph: &'static Icon,
    label: String,
    variant: IconButtonVariant,
    size: Size,
    disabled: bool,
    expanded: Option<bool>,
    shortcut: Option<String>,
    press: Option<PressHandler>,
}

impl IconButton {
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub const fn expanded(mut self, on: bool) -> Self {
        self.expanded = Some(on);
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.press = Some(Box::new(move |el| el.on_press(f)));
        self
    }

    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.press = Some(Box::new(move |el| el.on_press_tx(f)));
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let side = self.size.height();
        let active = self.expanded == Some(true) && !self.disabled;
        let style = icon_button_style(self.variant, self.disabled, active, k);
        let tip = match &self.shortcut {
            Some(shortcut) if !shortcut.is_empty() => format!("{}  {shortcut}", self.label),
            _ => self.label.clone(),
        };
        let mut el = row()
            .role("button")
            .label(&self.label)
            .help(tip)
            .size(side, side)
            .center()
            .radius(self.size.radius())
            .continuous_corners()
            .fill(style.fill)
            .disabled(self.disabled)
            .focusable(!self.disabled)
            .focus_ring(HAIRLINE, -HAIRLINE, k.c.border_focus);
        if let Some(shortcut) = self.shortcut.filter(|s| !s.is_empty()) {
            el = el.shortcut_hint(shortcut);
        }
        if let Some(open) = self.expanded {
            el = el.expanded(open);
        }
        if let Some(border) = style.border {
            el = el.border(HAIRLINE, border);
        }
        if !self.disabled {
            el = el.hover_group().press_color(style.hover).press_scale(0.97);
            if style.press.a > 0.0 {
                el = el.press(style.press);
            }
        }
        let mut glyph = icon(self.glyph, ICON)
            .key("icon")
            .color(style.rest)
            .a11y_hidden();
        if !self.disabled {
            glyph = glyph.group_color(style.hover);
        }
        el = el.child(glyph);
        if self.disabled
            && !matches!(
                self.variant,
                IconButtonVariant::Submit | IconButtonVariant::Contrast
            )
        {
            el = el.layer_opacity(0.45);
        }
        if !self.disabled
            && let Some(press) = self.press
        {
            el = press(el);
        }
        el
    }
}

pub fn button(label: impl Into<String>, variant: ButtonVariant, size: Size) -> Button {
    Button {
        label: label.into(),
        variant,
        size,
        icon: None,
        disabled: false,
        press: None,
    }
}

pub fn icon_button(
    glyph: &'static Icon,
    label: &str,
    variant: IconButtonVariant,
    size: Size,
) -> IconButton {
    IconButton {
        glyph,
        label: label.into(),
        variant,
        size,
        disabled: false,
        expanded: None,
        shortcut: None,
        press: None,
    }
}

pub fn kbd(key: &str, t: &Theme) -> El {
    Keybind::new(&[COMMAND_KEY, key]).build(&t.k)
}

/// A horizontal hairline across its parent.
pub fn rule(t: &Theme) -> El {
    el().w_full().h(HAIRLINE).fill(t.hairline).a11y_hidden()
}

/// A section title: small, medium, muted.
pub fn section(label: &str, t: &Theme) -> El {
    text(label).font(t.small_medium).color(t.muted)
}

/// A glyph centered in an icon-wide box one line tall, so it sits on the
/// first line of the text beside it.
pub fn lead(mark: El, line: f32) -> El {
    el().size(ICON, line).center().child(mark).a11y_hidden()
}

/// A pointer-transparent bottom horizon scrim pinned beneath the floating
/// composer stack so scrolling transcript content fades gently into the canvas.
pub fn horizon_scrim(t: &Theme) -> El {
    el().key("horizon-scrim")
        .w_full()
        .h(HORIZON_HEIGHT)
        .linear_gradient(std::f32::consts::PI, t.horizon_stops)
        .a11y_hidden()
}

/// The centered reading column's frame; its parent owns the vertical spacing.
pub fn reading(child: El) -> El {
    col()
        .w_full()
        .px(READING_INSET)
        .items_center()
        .child(child.w_full().max_w(READING_WIDTH))
}

/// rune-kit's shell marks its title bands (the toolbar, the sidebar's header
/// row) `.window_drag()`; viz's window isn't moved from its content. Pin this
/// over a band, beneath its controls (they keep their presses), and a press
/// between them does nothing.
pub fn drag_guard(key: &str) -> El {
    el().key(key).no_window_drag().a11y_hidden()
}

#[derive(Clone, Copy, Debug)]
pub enum Card {
    /// A raised surface with a hairline: tool output, the composer.
    Surface,
    /// A hairline only: plans, subagents.
    Outline,
    /// The quiet wash behind my messages.
    Message,
}

impl Card {
    /// The surface alone, for compound cards whose children own their insets.
    pub fn frame(self, t: &Theme) -> El {
        match self {
            Self::Surface => col()
                .w_full()
                .radius(RADIUS_CARD)
                .continuous_corners()
                .fill(t.surface)
                .border(HAIRLINE, t.hairline),
            Self::Outline => col()
                .w_full()
                .radius(RADIUS_CARD)
                .continuous_corners()
                .border(HAIRLINE, t.hairline),
            Self::Message => col()
                .w_full()
                .radius(RADIUS_COMPOSER)
                .continuous_corners()
                .fill(t.mine),
        }
    }

    /// The surface with the card inset and gap.
    pub fn build(self, t: &Theme) -> El {
        self.frame(t).padding(CARD_INSET).gap(S8)
    }
}

/// An empty state: an optional mark, a title, a line of detail, an optional
/// action.
pub fn empty(mark: Option<El>, title: &str, detail: &str, action: Option<El>, t: &Theme) -> El {
    let mut body = col()
        .key("empty")
        .w_full()
        .max_w(EMPTY_WIDTH)
        .gap(S8)
        .items_center();
    if let Some(mark) = mark {
        body = body.child(mark.mb(S4));
    }
    body = body.child(text(title).font(t.title).color(t.text));
    if !detail.is_empty() {
        body = body.child(
            text(detail)
                .font(t.ui)
                .color(t.muted)
                .wrap()
                .align(rune::Align::Center),
        );
    }
    body.children(action.map(|action| action.mt(S8)))
}

pub fn dot(color: Color) -> El {
    el().size(DOT, DOT)
        .radius(DOT / 2.0)
        .fill(color)
        .a11y_hidden()
}

/// The 5x5 radar sweep indicator, sharing wall-clock phase across the window.
pub fn radar(size: f32, color: Color, t: &Theme) -> El {
    Spinner::new().size(size).color(color).build(&t.k)
}

pub fn spinner(t: &Theme) -> El {
    radar(ICON_SMALL, t.accent, t)
}

/// A harness brand mark framed in a continuous-corner squircle medallion; an
/// active thread or task adds a quiet accent halo.
pub fn medallion(harness: Option<(&str, &str)>, active: bool, t: &Theme) -> El {
    let glyph = icon(
        harness.map_or(&HARNESS, |(id, _)| harness_icon(id)),
        ICON_SMALL,
    )
    .color(if active { t.text } else { t.muted })
    .a11y_hidden();
    let mut box_el = el()
        .size(S20, S20)
        .center()
        .radius(RADIUS_INNER)
        .continuous_corners()
        .fill(t.chip_fill)
        .border(HAIRLINE, if active { t.accent_veil } else { t.hairline })
        .child(glyph);
    if active {
        box_el = box_el.radial_gradient(
            [0.5, 0.5],
            [S16, S16],
            [(0.0, t.accent_veil), (1.0, Color::TRANSPARENT)],
        );
    }
    match harness {
        Some((_, name)) => box_el.help(name),
        None => box_el.a11y_hidden(),
    }
}

/// A thread's run state as an icon-sized mark: a spinner while it works,
/// a danger dot after a failure, nothing while idle.
pub fn status(status: ThreadStatus, t: &Theme) -> Option<El> {
    let (mark, label) = match status {
        ThreadStatus::Idle => return None,
        ThreadStatus::Working => (spinner(t), "Working"),
        ThreadStatus::Failed => (dot(t.danger), "Failed"),
    };
    Some(
        el().size(ICON, ICON)
            .center()
            .child(mark)
            .role("status")
            .label(label)
            .help(label),
    )
}

pub fn text_field(edit: &TextEdit, label: &str, placeholder: &str, size: Size) -> TextInput {
    TextInput::new(edit)
        .label(label)
        .placeholder(placeholder)
        .height(size.height())
        .width(None)
}

pub fn code(body: &str, t: &Theme) -> El {
    text(body)
        .font(t.mono)
        .color(t.text)
        .wrap_anywhere()
        .w_full()
        .selectable(true)
        .selection_color(t.k.selection())
}

/// A sidebar row. Labels start at `SIDEBAR_INSET` plus one `TREE_STEP` per
/// depth (a leading icon takes one step), only the label shrinks, and the row
/// ends in its [`trailing`] slot.
pub struct TreeLine<'a> {
    pub key: &'a str,
    pub label: &'a str,
    /// A quiet second line; the row grows to `TREE_ROW_TALL`.
    pub detail: Option<&'a str>,
    pub depth: u16,
    /// Unread: full ink.
    pub strong: bool,
    /// Archived: faint.
    pub dim: bool,
    pub selected: bool,
    /// The label's ink at rest: `muted` for most rows, `text` for a project's
    /// header.
    pub ink: Color,
}

impl TreeLine<'_> {
    pub fn build(self, leading: Option<El>, slot: El, t: &Theme) -> El {
        let color = if self.dim {
            t.faint
        } else if self.selected || self.strong {
            t.text
        } else {
            self.ink
        };
        let hover_color = if self.dim { t.muted } else { t.text };
        let title = text(self.label)
            .font(if self.selected { t.ui_medium } else { t.ui })
            .color(color)
            .group_color(hover_color)
            .truncate()
            .help_when_truncated(self.label);
        let labels = col()
            .min_w(0.0)
            .shrink()
            .grow(1.0)
            .a11y_hidden()
            .child(title)
            .children(self.detail.map(|detail| {
                text(detail)
                    .font(t.small)
                    .color(t.faint)
                    .truncate()
                    .help_when_truncated(detail)
            }));
        let height = if self.detail.is_some() {
            TREE_ROW_TALL
        } else {
            TREE_ROW
        };
        let mut line = row()
            .key(self.key)
            .role("treeitem")
            .label(self.label)
            .level(u32::from(self.depth) + 1)
            .h(height)
            .w_full()
            .pl(f32::from(self.depth).mul_add(TREE_STEP, ROW_INSET))
            .gap(S8)
            .center_y()
            .radius(Size::Default.radius())
            .fill(if self.selected {
                t.selected
            } else {
                Color::TRANSPARENT
            })
            .hover_group()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .children(leading.map(|mark| lead(mark, TREE_ROW)))
            .child(labels)
            .child(slot);
        if let Some(detail) = self.detail {
            line = line.description(detail);
        }
        line
    }
}

/// A row's fixed trailing slot. `rest` shows until the row is hovered or
/// focused, then `action` takes its place; `reveal` keeps the action up (its
/// menu is open).
pub fn trailing(rest: Option<El>, action: Option<El>, reveal: bool) -> El {
    let layer = |child: El| {
        el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .center()
            .child(child)
    };
    let mut slot = el().key("trailing").size(TRAILING, TRAILING);
    if let Some(rest) = rest {
        let hovered = if action.is_some() { 0.0 } else { 1.0 };
        let shown = if reveal { 0.0 } else { 1.0 };
        slot = slot.child(layer(rest).group_opacity(shown, hovered));
    }
    if let Some(action) = action {
        slot = slot.child(layer(action).group_opacity(if reveal { 1.0 } else { 0.0 }, 1.0));
    }
    slot
}

/// A sidebar section's title row, with an optional action in the trailing
/// slot.
pub fn section_row(key: &str, label: &str, action: Option<El>, t: &Theme) -> El {
    row()
        .key(key)
        .w_full()
        .h(TREE_ROW)
        .mt(S12)
        .pl(ROW_INSET)
        .gap(S8)
        .center_y()
        .child(section(label, t).grow(1.0))
        .child(trailing(None, action, true))
}

/// An inline select's trigger: its name, or `current` once chosen, and a
/// chevron, at most `max_w` wide. The caller handles its press.
pub fn select_trigger(
    id: &str,
    name: &str,
    current: Option<&str>,
    open: bool,
    max_w: f32,
    t: &Theme,
) -> El {
    inline_trigger(id, name, current, None, open, Some(max_w), t)
}

/// An inline select led by a harness icon. Size to the display name and
/// truncate only if the toolbar runs out of room. The caller handles presses.
pub fn marked_trigger(
    id: &str,
    name: &str,
    current: &str,
    mark: &'static Icon,
    open: bool,
    t: &Theme,
) -> El {
    inline_trigger(id, name, Some(current), Some(mark), open, None, t)
}

fn inline_trigger(
    id: &str,
    name: &str,
    current: Option<&str>,
    mark: Option<&'static Icon>,
    open: bool,
    max_w: Option<f32>,
    t: &Theme,
) -> El {
    let label = current.unwrap_or(name);
    let rest_text = if current.is_some() || open {
        t.text
    } else {
        t.muted
    };
    let rest_icon = if open { t.text } else { t.muted };
    let mut trigger = row()
        .key(format!("{id}-trigger"))
        .role("combobox")
        .label(name)
        .expanded(open)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .h(Size::Default.height())
        .pl(if mark.is_some() { S6 } else { S8 })
        .pr(S4)
        .gap(S4)
        .center_y()
        .radius(Size::Default.radius())
        .continuous_corners()
        .children(mark.map(|glyph| {
            icon(glyph, ICON)
                .color(rest_icon)
                .group_color(t.text)
                .a11y_hidden()
        }))
        .child(
            text(label)
                .font(t.ui_medium)
                .color(rest_text)
                .group_color(t.text)
                .min_w(0.0)
                .shrink()
                .truncate()
                .a11y_hidden(),
        )
        .child(
            icon(&CHEVRON_OPEN, ICON_SMALL)
                .icon_rotation(if open { std::f32::consts::PI } else { 0.0 })
                .color(rest_icon)
                .group_color(t.text)
                .a11y_hidden(),
        );
    if let Some(current) = current {
        trigger = trigger.a11y_value(current);
    }
    if let Some(max_w) = max_w {
        trigger = trigger.max_w(max_w);
    }
    trigger
}

/// The combobox trigger is separate from its menu: server ids can differ even
/// when the labels match.
pub fn picker<S: 'static>(
    id: &str,
    name: &str,
    current: Option<&str>,
    open: bool,
    toggle: impl Fn(&mut S) + 'static,
    close: impl Fn(&mut S) + 'static,
    t: &Theme,
) -> Menu {
    let trigger = select_trigger(id, name, current, open, PICKER_TRIGGER_MAX, t).on_press(toggle);
    picker_menu(id, trigger, open, close)
}

/// A picker's menu around its `trigger` (which carries the toggle).
pub fn picker_menu<S: 'static>(
    id: &str,
    trigger: El,
    open: bool,
    close: impl Fn(&mut S) + 'static,
) -> Menu {
    Menu::new(format!("{id}-picker"))
        .placement(Placement::TopStart)
        .min_width(MENU_WIDTH)
        .open(open)
        .on_close(close)
        .trigger(trigger)
}

/// The widest an inline select's trigger gets; rune-kit truncates its label
/// there.
pub const PICKER_TRIGGER_MAX: f32 = 156.0;
/// The model picker's trigger: a model's short name and its thinking level.
pub const MODEL_TRIGGER_MAX: f32 = 240.0;
/// A menu's narrowest width.
pub const MENU_WIDTH: f32 = 196.0;
const COMPLETION_HEIGHT: f32 = 240.0;
pub const COMPLETION_WIDTH: f32 = 360.0;

pub fn menu_items(menu: Menu, items: impl IntoIterator<Item = MenuItem>) -> Menu {
    items.into_iter().fold(menu, Menu::item)
}

pub fn popup_list<S: 'static>(key: &str, anchor: &str) -> PopupList<S> {
    PopupList::new(key, anchor)
        .placement(Placement::TopStart)
        .offset(S4)
        .max_height(COMPLETION_HEIGHT)
        .animate_filtering(false)
        .open(true)
}

/// The most lines a dialog's or toast's title wraps to before "…".
const TITLE_LINES: usize = 2;
/// The most lines a toast's detail (an error, a list of paths) shows.
const TOAST_DETAIL_LINES: usize = 4;

/// A dialog: a title of up to [`TITLE_LINES`], what it's about (`subject`,
/// one quiet line cut in the middle so a long path or thread title keeps both
/// ends) and a description, beside the close button's own column; then the
/// body, which insets itself like [`dialog_body`].
pub struct Dialog<'a> {
    key: &'a str,
    title: &'a str,
    subject: Option<&'a str>,
    detail: Option<&'a str>,
}

pub const fn dialog<'a>(key: &'a str, title: &'a str, detail: Option<&'a str>) -> Dialog<'a> {
    Dialog {
        key,
        title,
        subject: None,
        detail,
    }
}

impl<'a> Dialog<'a> {
    pub const fn subject(mut self, subject: &'a str) -> Self {
        self.subject = Some(subject);
        self
    }

    pub fn build<S: 'static>(
        self,
        close: impl Fn(&mut S) + Clone + 'static,
        body: El,
        t: &Theme,
        layer: &mut Vec<El>,
    ) {
        let mut head = col().grow(1.0).shrink().min_w(0.0).gap(S4).child(
            text(self.title)
                .font(t.title)
                .color(t.text)
                .wrap_anywhere()
                .max_lines(TITLE_LINES)
                .w_full(),
        );
        if let Some(subject) = self.subject {
            head = head.child(
                text(subject)
                    .font(t.ui_medium)
                    .color(t.text)
                    .truncate_middle()
                    .w_full(),
            );
        }
        if let Some(detail) = self.detail {
            let mut detail_el = text(detail)
                .font(t.ui)
                .color(t.muted)
                .wrap_anywhere()
                .w_full();
            if self.subject.is_some() {
                detail_el = detail_el.mt(S4);
            }
            head = head.child(detail_el);
        }
        let dismiss = icon_button(&CLOSE, "Close", IconButtonVariant::Ghost, Size::Small)
            .on_press(close.clone())
            .build(&t.k)
            .key("close")
            .mt(-S2)
            .mr(-S4);
        let header = row()
            .key("header")
            .w_full()
            .padding(DIALOG_INSET)
            .gap(S8)
            .items_start()
            .child(head)
            .child(dismiss);
        let panel = col()
            .label(self.title)
            .description(self.detail.unwrap_or_default())
            .child(header)
            .child(col().key("body").w_full().child(body));
        modal(self.key, panel, DIALOG_WIDTH, close, t, layer);
    }
}

/// A dialog without a title row, for an overlay whose first control says
/// what it is (the palette's search field).
pub fn sheet<S: 'static>(
    key: &str,
    label: &str,
    width: f32,
    close: impl Fn(&mut S) + 'static,
    body: impl IntoIterator<Item = El>,
    t: &Theme,
    layer: &mut Vec<El>,
) {
    modal(
        key,
        col().label(label).children(body),
        width,
        close,
        t,
        layer,
    );
}

/// rune-kit's `Dialog` scrim, panel, outline and dismissal (Escape, a click
/// outside) around `panel`.
fn modal<S: 'static>(
    key: &str,
    panel: El,
    width: f32,
    close: impl Fn(&mut S) + 'static,
    t: &Theme,
    layer: &mut Vec<El>,
) {
    let k = &t.overlay;
    let panel = k.shadow_layers(&k.elevation.overlay).fold(
        panel
            .key(format!("{key}-dialog"))
            .role("dialog")
            .w_full()
            .max_w(width)
            .radius(Size::Default.radius())
            .fill(k.c.layer_01)
            .token_motion(k.motion.overlay)
            .modal(true, close),
        El::box_shadow,
    );
    layer.push(
        el().key(format!("{key}-scrim"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(k.c.scrim)
            .token_motion(k.motion.overlay)
            .a11y_hidden(),
    );
    layer.push(
        el().key(format!("{key}-layer"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(DIALOG_INSET)
            .center()
            .child(panel),
    );
}

/// A toast: `mark`, a title of up to [`TITLE_LINES`] and a detail of up to
/// [`TOAST_DETAIL_LINES`], beside the close button's own column. Long words
/// (paths, URLs) break anywhere rather than overflow.
pub fn toast<S: 'static>(
    key: String,
    title: &str,
    detail: Option<&str>,
    mark: El,
    close: impl Fn(&mut S) + 'static,
    t: &Theme,
) -> El {
    let k = &t.overlay;
    let mut content = col()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(S4)
        .a11y_hidden()
        .child(
            text(title)
                .font(t.ui_medium)
                .color(t.text)
                .wrap_anywhere()
                .max_lines(TITLE_LINES)
                .w_full(),
        );
    if let Some(detail) = detail {
        content = content.child(
            text(detail)
                .font(t.ui)
                .color(t.muted)
                .wrap_anywhere()
                .max_lines(TOAST_DETAIL_LINES)
                .w_full(),
        );
    }
    let dismiss = icon_button(&CLOSE, "Close", IconButtonVariant::GhostMuted, Size::Small)
        .on_press(close)
        .build(k)
        .key("close")
        .mt(-S2)
        .mr(-S4);
    let panel = col()
        .key(key)
        .role("status")
        .label(title)
        .description(detail.unwrap_or_default())
        .w(CARD_WIDTH)
        .padding(CARD_INSET)
        .radius(RADIUS_CARD)
        .fill(k.c.layer_01)
        .child(
            row()
                .w_full()
                .gap(S8)
                .items_start()
                .child(mark.a11y_hidden())
                .child(content)
                .child(dismiss),
        );
    k.shadow_layers(&k.elevation.floating)
        .fold(panel, El::box_shadow)
        .transition(Transition::move_edge(Edge::Bottom))
        .token_motion(k.motion.overlay)
}

/// A dialog's body: its fields, then its actions, at the dialog inset.
pub fn dialog_body(fields: impl IntoIterator<Item = El>, actions: El) -> El {
    col()
        .w_full()
        .px(DIALOG_INSET)
        .pb(DIALOG_INSET)
        .gap(DIALOG_INSET)
        .children(fields)
        .child(actions)
}

/// A dialog's buttons, right-aligned, the primary last.
pub fn dialog_actions(buttons: impl IntoIterator<Item = El>) -> El {
    row().w_full().justify_end().gap(S8).children(buttons)
}

/// A one-field (or no-field) confirmation dialog.
pub struct DialogForm<'a> {
    pub title: &'a str,
    /// What the action is about, as [`Dialog::subject`].
    pub subject: Option<&'a str>,
    pub detail: Option<&'a str>,
    pub field: Option<(&'a TextEdit, &'a str)>,
    pub action: &'a str,
    pub variant: ButtonVariant,
}

impl DialogForm<'_> {
    pub fn build<S: 'static>(
        self,
        edit: impl Fn(&mut S) -> &mut TextEdit + 'static,
        close: impl Fn(&mut S) + Clone + 'static,
        submit: impl Fn(&mut S, &mut Tx) + Clone + 'static,
        t: &Theme,
        layer: &mut Vec<El>,
    ) {
        let input = self.field.map(|(value, placeholder)| {
            text_field(value, placeholder, placeholder, Size::Default)
                .on_edit(edit)
                .on_submit(submit.clone())
                .build(&t.k)
                .key("edit-value")
        });
        let disabled = self
            .field
            .is_some_and(|(value, _)| value.text.trim().is_empty());
        let actions = dialog_actions([
            button("Cancel", ButtonVariant::Ghost, Size::Default)
                .on_press(close.clone())
                .build(&t.k),
            button(self.action, self.variant, Size::Default)
                .disabled(disabled)
                .on_press_tx(submit)
                .build(&t.k),
        ]);
        let dialog = dialog("edit", self.title, self.detail);
        match self.subject {
            Some(subject) => dialog.subject(subject),
            None => dialog,
        }
        .build(close, dialog_body(input, actions), t, layer);
    }
}

/// Declares Lucide icons as Rune icons: Lucide's own SVG on its 24-point
/// grid, one stroke width throughout, drawn at the `ICON*` token sizes.
macro_rules! lucide {
    ($($(#[$doc:meta])* $name:ident = $icon:ident;)*) => {$(
        $(#[$doc])*
        pub static $name: Icon = Icon {
            name: concat!("lucide-", stringify!($icon)),
            view: 24.0,
            body: lucide_static_svg::Icon::$icon.svg_str(),
        };
    )*};
}

lucide! {
    /// New thread.
    COMPOSE = SquarePen;
    /// Send.
    ARROW_UP = ArrowUp;
    /// Queue a follow-up input.
    LIST_PLUS = ListPlus;
    /// Steer or send immediately into an active turn.
    CORNER_UP_RIGHT = CornerUpRight;
    /// Delegated work.
    AGENT = Workflow;
    BRAIN = Brain;
    CHEVRON = ChevronRight;
    CHEVRON_OPEN = ChevronDown;
    CHEVRON_UP = ChevronUp;
    MORE = Ellipsis;
    FOLDER = Folder;
    /// Add project.
    FOLDER_PLUS = FolderPlus;
    THREAD = MessageSquare;
    STOP = Square;
    SIDEBAR = PanelLeft;
    PIN = Pin;
    ARCHIVE = Archive;
    TRASH = Trash;
    PAPERCLIP = Paperclip;
    CHECK = Check;
    CLOSE = X;
    CIRCLE_X = CircleX;
    COPY = Copy;
    SEARCH = Search;
    LOCK = Lock;
    EYE = Eye;
    EYE_OFF = EyeOff;
    MINUS = Minus;
    TERMINAL = Terminal;
    FILE = File;
    FILE_READ = FileText;
    FILE_WRITE = FilePlus;
    FILE_EDIT = FilePenLine;
    WRENCH = Wrench;
    TIMER = Timer;
    /// A plan step not started yet.
    CIRCLE = Circle;
    /// A plan.
    PLAN = ListChecks;
    /// A run that ended early.
    CIRCLE_STOP = CircleStop;
    /// A delegated task's result arriving.
    INBOX = Inbox;
    /// Fetching a page.
    GLOBE = Globe;
    /// A harness without a known mark.
    HARNESS = SquareTerminal;
}

/// Declares a harness's monochrome brand mark from `icons/`, drawn in its
/// element's colour (`currentColor`) at `view` units square.
macro_rules! mark {
    ($($(#[$doc:meta])* $name:ident = $file:literal, $view:literal;)*) => {$(
        $(#[$doc])*
        pub static $name: Icon = Icon {
            name: concat!("harness-", $file),
            view: $view,
            body: include_str!(concat!("../../icons/", $file, ".svg")),
        };
    )*};
}

mark! {
    /// Anthropic's Claude starburst (`@lobehub/icons-static-svg`).
    CLAUDE = "claude", 24.0;
    /// The Codex mark (`@lobehub/icons-static-svg`).
    CODEX = "codex", 24.0;
    /// pi's block mark (`pi.dev/favicon.svg`), inset to Lucide's margin.
    PI = "pi", 700.0;
    /// oh-my-pi's pi mark (`can1357/oh-my-pi` `packages/collab-web/public/favicon.svg`).
    OMP = "omp", 52.0;
    /// Google's monochrome product icon (`antigravity.google/press/`).
    ANTIGRAVITY = "antigravity", 180.0;
}

/// A harness's mark, recognized from the first word of its config key.
/// Unknown keys use the terminal icon.
pub fn harness_icon(id: &str) -> &'static Icon {
    let word = id
        .split(|c: char| !c.is_ascii_alphanumeric())
        .next()
        .unwrap_or_default();
    let is = |names: &[&str]| names.iter().any(|known| word.eq_ignore_ascii_case(known));
    if is(&["claude", "anthropic"]) {
        &CLAUDE
    } else if is(&["codex", "openai"]) {
        &CODEX
    } else if is(&["omp"]) {
        &OMP
    } else if is(&["pi"]) {
        &PI
    } else if is(&["agy", "antigravity"]) {
        &ANTIGRAVITY
    } else {
        &HARNESS
    }
}

/// rune-kit's own glyphs, as Lucide.
pub fn kit_glyphs(glyph: &mut rune_kit::Glyphs) {
    glyph.check = &CHECK;
    glyph.check_small = &CHECK;
    glyph.chevron_down = &CHEVRON_OPEN;
    glyph.chevron_up = &CHEVRON_UP;
    glyph.caret_right = &CHEVRON;
    glyph.triangle_down = &CHEVRON_OPEN;
    glyph.triangle_right = &CHEVRON;
    glyph.close = &CLOSE;
    glyph.clear = &CIRCLE_X;
    glyph.copy = &COPY;
    glyph.search = &SEARCH;
    glyph.lock = &LOCK;
    glyph.eye = &EYE;
    glyph.eye_off = &EYE_OFF;
    glyph.circle_x = &CIRCLE_X;
    glyph.dash = &MINUS;
}

/// For `rune::set_icon_swap`: the icons rune-kit draws outside its glyph set
/// (the shell's sidebar toggle), as Lucide.
pub fn swap_kit_icon(icon: &Icon) -> Option<&'static Icon> {
    (icon.name == rune_kit::shell::SIDEBAR.name).then_some(&SIDEBAR)
}

/// Display external labels in sentence case without changing names with
/// established casing (such as `xAI` or `Use workspace context`).
pub fn sentence_label(label: &str) -> Cow<'_, str> {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) if first.is_lowercase() && !chars.clone().any(char::is_uppercase) => {
            Cow::Owned(first.to_uppercase().chain(chars).collect())
        }
        _ => Cow::Borrowed(label),
    }
}

/// Spell out the extra-high tier even when a runner only reports its ID.
pub fn thinking_label(label: &str) -> Cow<'_, str> {
    if ["xhigh", "x-high", "extra-high"]
        .iter()
        .any(|id| label.eq_ignore_ascii_case(id))
    {
        Cow::Borrowed("Extra high")
    } else {
        sentence_label(label)
    }
}
