//! Compact bare inline actions for timeline footers, diff headers, and
//! disclosure controls.

use rune::{El, Icon, Tx, icon, row, text};

use super::badge::Tone;
use crate::{
    motion,
    theme::{HAIRLINE, ICON_SMALL, RADIUS_SM, S4, Size, Theme},
};

type PressHandler = Box<dyn FnOnce(El) -> El>;

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
            el = el.layer_opacity(0.50);
        }
        if !self.disabled
            && let Some(press) = self.press
        {
            el = press(el);
        }
        el
    }
}
