//! Text buttons built on `Theme` tokens and `motion::PRESS_SCALE`.

use rune::{Color, El, Icon, TextStyle, Tx, icon, row, text};
use rune_kit::{ButtonVariant, Kit, Spinner};

use super::badge::tooltip_text;
use crate::{
    motion,
    theme::{HAIRLINE, S2, S6, Size},
};

type PressHandler = Box<dyn FnOnce(El) -> El>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ButtonActivity {
    #[default]
    Idle,
    Active,
    Loading,
    Disabled,
}

struct ButtonStyle {
    fill: Color,
    hover_fill: Color,
    border: Option<Color>,
    rest: Color,
    hover: Color,
    press: Color,
}

fn button_style(variant: ButtonVariant, activity: ButtonActivity, k: &Kit) -> ButtonStyle {
    let c = &k.c;
    let active = activity == ButtonActivity::Active;
    let disabled = activity == ButtonActivity::Disabled;
    match variant {
        ButtonVariant::Submit if disabled => ButtonStyle {
            fill: c.bg_button_neutral,
            hover_fill: Color::TRANSPARENT,
            border: None,
            rest: c.text_faint,
            hover: c.text_faint,
            press: Color::TRANSPARENT,
        },
        ButtonVariant::Submit => ButtonStyle {
            fill: c.bg_icon_button_contrast,
            hover_fill: c.submit_hover,
            border: None,
            rest: c.submit_fg,
            hover: c.submit_fg,
            press: k.on(c.contrast_pressed, c.bg_icon_button_contrast),
        },
        ButtonVariant::Outline => ButtonStyle {
            fill: if active { c.hover } else { Color::TRANSPARENT },
            hover_fill: c.hover,
            border: Some(if active {
                c.border_strong
            } else {
                c.border_base
            }),
            rest: if active { c.text_strong } else { c.text_base },
            hover: c.text_strong,
            press: c.pressed,
        },
        ButtonVariant::Danger => ButtonStyle {
            fill: c.danger.bg,
            hover_fill: c.danger.fg.alpha(0.10),
            border: Some(c.danger.border),
            rest: c.danger.fg,
            hover: c.danger.fg,
            press: c.pressed,
        },
        ButtonVariant::Warning => ButtonStyle {
            fill: c.warning.bg,
            hover_fill: c.warning.fg.alpha(0.10),
            border: Some(c.warning.border),
            rest: c.warning.fg,
            hover: c.warning.fg,
            press: c.pressed,
        },
        ButtonVariant::GhostMuted => ButtonStyle {
            fill: if active { c.hover } else { Color::TRANSPARENT },
            hover_fill: c.hover,
            border: None,
            rest: if active { c.text_base } else { c.text_muted },
            hover: c.text_base,
            press: c.pressed,
        },
        ButtonVariant::GhostFaint => ButtonStyle {
            fill: if active { c.hover } else { Color::TRANSPARENT },
            hover_fill: c.hover,
            border: None,
            rest: if active { c.text_base } else { c.text_faint },
            hover: c.text_base,
            press: c.pressed,
        },
        ButtonVariant::Ghost => ButtonStyle {
            fill: if active { c.hover } else { Color::TRANSPARENT },
            hover_fill: c.hover,
            border: None,
            rest: if active { c.text_strong } else { c.text_base },
            hover: c.text_strong,
            press: c.pressed,
        },
        ButtonVariant::Contrast => ButtonStyle {
            fill: c.bg_contrast,
            hover_fill: c.contrast_hover,
            border: None,
            rest: c.text_contrast,
            hover: c.text_contrast,
            press: k.on(c.contrast_pressed, c.bg_contrast),
        },
        ButtonVariant::Neutral | ButtonVariant::Loading => ButtonStyle {
            fill: if active {
                c.pressed
            } else {
                c.bg_button_neutral
            },
            hover_fill: c.hover,
            border: Some(c.border_muted),
            rest: if active { c.text_strong } else { c.text_base },
            hover: c.text_strong,
            press: c.pressed,
        },
    }
}

const fn button_font(size: Size, variant: ButtonVariant, k: &Kit) -> TextStyle {
    let quiet = matches!(
        variant,
        ButtonVariant::GhostMuted | ButtonVariant::GhostFaint
    );
    match size {
        Size::Tiny | Size::Micro | Size::Small => {
            if quiet {
                k.ty.small
            } else {
                k.ty.small_medium
            }
        }
        Size::Compact | Size::Default | Size::Large => {
            if quiet {
                k.ty.ui
            } else {
                k.ty.ui_medium
            }
        }
    }
}

fn tinted_icon(
    key: &'static str,
    glyph: &'static Icon,
    size: f32,
    style: &ButtonStyle,
    interactive: bool,
) -> El {
    let mut mark = icon(glyph, size).key(key).color(style.rest).a11y_hidden();
    if interactive {
        mark = mark.group_color(style.hover);
    }
    mark
}

/// A flat base16 text button supporting all [`Size`] steps, active/loading/
/// disabled states, keyboard focus rings, and [`motion::PRESS_SCALE`].
pub struct Button {
    label: String,
    variant: ButtonVariant,
    size: Size,
    icon: Option<&'static Icon>,
    trailing_icon: Option<&'static Icon>,
    activity: ButtonActivity,
    full_width: bool,
    shortcut: Option<String>,
    press: Option<PressHandler>,
}

/// Starts a [`Button`] with `label`, `variant`, and `size`.
pub fn button(label: impl Into<String>, variant: ButtonVariant, size: Size) -> Button {
    Button {
        label: label.into(),
        variant,
        size,
        icon: None,
        trailing_icon: None,
        activity: ButtonActivity::Idle,
        full_width: false,
        shortcut: None,
        press: None,
    }
}

impl Button {
    /// Adds a leading Lucide icon.
    pub const fn icon(mut self, glyph: &'static Icon) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// Adds a trailing Lucide icon (such as a dropdown chevron or external
    /// arrow).
    pub const fn trailing_icon(mut self, glyph: &'static Icon) -> Self {
        self.trailing_icon = Some(glyph);
        self
    }

    /// Marks the button as toggled/pressed-on (e.g., an open filter or panel
    /// toggle).
    pub const fn active(mut self, active: bool) -> Self {
        if active && matches!(self.activity, ButtonActivity::Idle) {
            self.activity = ButtonActivity::Active;
        }
        self
    }

    /// Shows an inline radar spinner and disables clicks while an operation is
    /// in flight.
    pub const fn loading(mut self, loading: bool) -> Self {
        if loading {
            self.activity = ButtonActivity::Loading;
        }
        self
    }

    /// Disables pointer and keyboard activation and dims non-submit variants.
    pub const fn disabled(mut self, disabled: bool) -> Self {
        if disabled {
            self.activity = ButtonActivity::Disabled;
        }
        self
    }

    /// Stretches the button to fill its parent's width.
    pub const fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    /// Attaches a shortcut tooltip and accessibility hint.
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

    /// Builds the button element against `k`.
    pub fn build(self, k: &Kit) -> El {
        let disabled = self.activity == ButtonActivity::Disabled;
        let loading = self.activity == ButtonActivity::Loading;
        let interactive = !disabled && !loading;
        let icon_size = self.size.icon_size();
        let font = button_font(self.size, self.variant, k);
        let style = button_style(self.variant, self.activity, k);

        let mut el = row()
            .role("button")
            .label(&self.label)
            .h(self.size.height())
            .px(self.size.px())
            .gap(S6)
            .center()
            .radius(self.size.radius())
            .fill(style.fill)
            .disabled(!interactive)
            .focusable(interactive)
            .focus_ring(HAIRLINE, S2, k.c.border_focus);
        if self.full_width {
            el = el.w_full();
        }
        if let Some(shortcut) = self.shortcut.as_deref().filter(|s| !s.is_empty()) {
            el = el
                .help(tooltip_text(&self.label, Some(shortcut)))
                .shortcut_hint(shortcut);
        }
        if let Some(border) = style.border {
            el = el.border(HAIRLINE, border);
        }
        if interactive {
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
        if loading {
            el = el.child(
                Spinner::new()
                    .size(icon_size)
                    .color(style.rest)
                    .build(k)
                    .key("spinner")
                    .a11y_hidden(),
            );
        } else if let Some(glyph) = self.icon {
            el = el.child(tinted_icon("icon", glyph, icon_size, &style, interactive));
        }
        let mut label = text(self.label)
            .key("label")
            .font(font)
            .color(style.rest)
            .truncate()
            .a11y_hidden();
        if interactive {
            label = label.group_color(style.hover);
        }
        el = el.child(label);
        if let Some(glyph) = self.trailing_icon {
            el = el.child(tinted_icon(
                "trailing-icon",
                glyph,
                icon_size,
                &style,
                interactive,
            ));
        }
        if disabled && !matches!(self.variant, ButtonVariant::Submit) {
            el = el.layer_opacity(0.45);
        }
        if interactive && let Some(press) = self.press {
            el = press(el);
        }
        el
    }
}
