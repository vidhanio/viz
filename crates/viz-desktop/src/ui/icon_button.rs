//! Square/circular icon buttons and compact bare inline actions.

use rune::{Color, El, Icon, Tx, icon, row, text};
use rune_kit::{IconButtonVariant, Kit};

use super::badge::{Tone, tooltip_text};
use crate::{
    motion,
    theme::{HAIRLINE, ICON_SMALL, RADIUS_PILL, RADIUS_SM, S4, Size, Theme},
};

type PressHandler = Box<dyn FnOnce(El) -> El>;

struct IconStyle {
    fill: Color,
    hover_fill: Color,
    border: Option<Color>,
    rest: Color,
    hover: Color,
    press: Color,
}

fn icon_button_style(
    variant: IconButtonVariant,
    active: bool,
    disabled: bool,
    k: &Kit,
) -> IconStyle {
    let c = &k.c;
    match variant {
        IconButtonVariant::Submit | IconButtonVariant::Contrast if disabled => IconStyle {
            fill: c.bg_button_neutral,
            hover_fill: Color::TRANSPARENT,
            border: None,
            rest: c.icon_faint,
            hover: c.icon_faint,
            press: Color::TRANSPARENT,
        },
        IconButtonVariant::Submit | IconButtonVariant::Contrast => IconStyle {
            fill: c.bg_icon_button_contrast,
            hover_fill: c.submit_hover,
            border: None,
            rest: c.submit_icon,
            hover: c.submit_icon,
            press: k.on(c.contrast_pressed, c.bg_icon_button_contrast),
        },
        IconButtonVariant::Neutral => IconStyle {
            fill: if active {
                c.pressed
            } else {
                c.bg_button_neutral
            },
            hover_fill: c.hover,
            border: Some(if active {
                c.border_strong
            } else {
                c.border_muted
            }),
            rest: if active {
                c.std_icon_strong
            } else {
                c.icon_base
            },
            hover: c.std_icon_strong,
            press: c.pressed,
        },
        IconButtonVariant::Ghost => IconStyle {
            fill: if active { c.hover } else { Color::TRANSPARENT },
            hover_fill: c.hover,
            border: None,
            rest: if active {
                c.std_icon_strong
            } else {
                c.icon_base
            },
            hover: c.std_icon_strong,
            press: c.pressed,
        },
        IconButtonVariant::GhostMuted => IconStyle {
            fill: if active { c.hover } else { Color::TRANSPARENT },
            hover_fill: c.hover,
            border: None,
            rest: if active { c.icon_base } else { c.icon_muted },
            hover: c.icon_base,
            press: c.pressed,
        },
    }
}

/// A square (or circular) icon button with hover wash, active toggle state,
/// tooltip + shortcut hint, focus ring, and [`motion::PRESS_SCALE`].
pub struct IconButton {
    glyph: &'static Icon,
    label: String,
    variant: IconButtonVariant,
    size: Size,
    circle: bool,
    active: bool,
    disabled: bool,
    shortcut: Option<String>,
    press: Option<PressHandler>,
}

/// Starts an [`IconButton`] with `glyph`, accessible `label`, `variant`, and
/// `size`.
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
        circle: false,
        active: false,
        disabled: false,
        shortcut: None,
        press: None,
    }
}

impl IconButton {
    /// Renders as a full circle (`RADIUS_PILL`), used for composer send/stop
    /// actions.
    pub const fn circle(mut self) -> Self {
        self.circle = true;
        self
    }

    /// Marks the toggle icon button as active (tinted hover fill and stronger
    /// ink).
    pub const fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Disables activation and applies flat disabled styling.
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Appends a shortcut to the button's tooltip and accessibility hint.
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Registers a click/activation callback.
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.press = Some(Box::new(move |el| el.on_press(f)));
        self
    }

    /// Registers a transaction-aware click/activation callback.
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.press = Some(Box::new(move |el| el.on_press_tx(f)));
        self
    }

    /// Builds the icon button element against `k`.
    pub fn build(self, k: &Kit) -> El {
        let side = self.size.height();
        let radius = if self.circle {
            RADIUS_PILL
        } else {
            self.size.radius()
        };
        let style = icon_button_style(self.variant, self.active, self.disabled, k);
        let tip = tooltip_text(&self.label, self.shortcut.as_deref());
        let mut el = row()
            .role("button")
            .label(&self.label)
            .help(tip)
            .size(side, side)
            .center()
            .radius(radius)
            .fill(style.fill)
            .disabled(self.disabled)
            .focusable(!self.disabled)
            .focus_ring(HAIRLINE, -HAIRLINE, k.c.border_focus);
        if let Some(shortcut) = self.shortcut.filter(|s| !s.is_empty()) {
            el = el.shortcut_hint(shortcut);
        }
        if let Some(border) = style.border {
            el = el.border(HAIRLINE, border);
        }
        if !self.disabled {
            el = el
                .hover_group()
                .press_color(style.hover)
                .press_scale(motion::PRESS_SCALE);
            if style.hover_fill.a > 0.0 {
                el = el.hover(style.hover_fill);
            }
            if style.press.a > 0.0 {
                el = el.press(style.press);
            }
        }
        let mut glyph = icon(self.glyph, self.size.icon_size())
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

/// Visual hover treatment for [`InlineButton`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InlineStyle {
    /// Quiet hover wash behind the inline control.
    #[default]
    Wash,
    /// Ink-only hover shift with no background fill (timeline footers, work-log
    /// toggles).
    InkOnly,
}

/// Compact bare action button for timeline footers, diff headers, and inline
/// disclosure controls.
pub struct InlineButton {
    label: String,
    icon: Option<&'static Icon>,
    trailing_icon: Option<&'static Icon>,
    tone: Tone,
    style: InlineStyle,
    disabled: bool,
    press: Option<PressHandler>,
}

/// Starts a compact [`InlineButton`].
pub fn inline_button(label: impl Into<String>) -> InlineButton {
    InlineButton {
        label: label.into(),
        icon: None,
        trailing_icon: None,
        tone: Tone::Neutral,
        style: InlineStyle::Wash,
        disabled: false,
        press: None,
    }
}

impl InlineButton {
    /// Adds a leading `12pt` Lucide icon.
    pub const fn icon(mut self, glyph: &'static Icon) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// Adds a trailing `12pt` Lucide icon.
    pub const fn trailing_icon(mut self, glyph: &'static Icon) -> Self {
        self.trailing_icon = Some(glyph);
        self
    }

    /// Tints the inline label and icons using a semantic [`Tone`].
    pub const fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Switches between subtle hover wash and ink-only hover feedback.
    pub const fn style(mut self, style: InlineStyle) -> Self {
        self.style = style;
        self
    }

    /// Disables activation and dims the control.
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Registers a click/activation callback.
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.press = Some(Box::new(move |el| el.on_press(f)));
        self
    }

    /// Registers a transaction-aware click/activation callback.
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.press = Some(Box::new(move |el| el.on_press_tx(f)));
        self
    }

    /// Builds the inline button element against `t`.
    pub fn build(self, t: &Theme) -> El {
        let (rest, hover) = match self.tone {
            Tone::Neutral => (t.muted, t.text),
            other => {
                let fg = other.colors(t).fg;
                (fg, fg)
            }
        };
        let mut el = row()
            .role("button")
            .label(&self.label)
            .h(Size::Micro.height())
            .px(S4)
            .gap(S4)
            .center_y()
            .radius(RADIUS_SM)
            .disabled(self.disabled)
            .focusable(!self.disabled)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent);
        if !self.disabled {
            el = el.hover_group().press_scale(motion::PRESS_SCALE);
            if self.style == InlineStyle::Wash {
                el = el.hover(t.hover_wash).press(t.pressed_wash);
            }
        }
        if let Some(glyph) = self.icon {
            let mut mark = icon(glyph, ICON_SMALL).color(rest).a11y_hidden();
            if !self.disabled {
                mark = mark.group_color(hover);
            }
            el = el.child(mark);
        }
        let mut label = text(self.label)
            .font(t.small_medium)
            .color(rest)
            .truncate()
            .a11y_hidden();
        if !self.disabled {
            label = label.group_color(hover);
        }
        el = el.child(label);
        if let Some(glyph) = self.trailing_icon {
            let mut mark = icon(glyph, ICON_SMALL).color(rest).a11y_hidden();
            if !self.disabled {
                mark = mark.group_color(hover);
            }
            el = el.child(mark);
        }
        if self.disabled {
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
