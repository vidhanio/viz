//! Flat semantic badges, keyboard keycaps, diff statistics, and tooltip
//! helpers.

use rune::{Color, El, Icon, el, icon, row, text};

use super::COMMAND_KEY;
use crate::{
    motion,
    theme::{DOT, HAIRLINE, RADIUS_PILL, RADIUS_SM, RADIUS_XS, S2, S4, S6, Theme},
};

/// Semantic accent role for badges, banners, and inline status elements.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tone {
    /// Quiet neutral chip fill and hairline.
    #[default]
    Neutral,
    /// Primary interactive accent (`base0D`).
    Accent,
    /// Positive / open / added status (`base0B`).
    Success,
    /// Destructive / error / removed status (`base08`).
    Danger,
    /// Attention / warning / approval status (`base0A`).
    Warning,
    /// Informational / remote / `@file` accent (`base0C`).
    Info,
    /// Steer mode / `$skill` / extra-high effort (`base09`).
    Steer,
    /// Delegated subagent / merged PR / `/command` (`base0E`).
    Delegated,
}

/// Resolved flat surface, ink, and `1px` hairline colors for a [`Tone`].
#[derive(Debug, Clone, Copy)]
pub struct ToneColors {
    /// Primary text and icon ink (`>= 4.5:1`).
    pub fg: Color,
    /// Subtle tinted background fill.
    pub bg: Color,
    /// Crisp `1px` border framing the tinted surface.
    pub border: Color,
}

impl Tone {
    /// Resolves foreground, subtle tinted fill, and hairline border from `t`.
    pub fn colors(self, t: &Theme) -> ToneColors {
        match self {
            Self::Neutral => ToneColors {
                fg: t.muted,
                bg: t.chip_fill,
                border: t.hairline,
            },
            Self::Accent => tinted(t.accent_text, t.accent),
            Self::Success => tinted(t.success, t.success),
            Self::Danger => tinted(t.danger, t.danger),
            Self::Warning => tinted(t.warning, t.warning),
            Self::Info => tinted(t.info, t.info),
            Self::Steer => tinted(t.steer, t.steer),
            Self::Delegated => tinted(t.delegated, t.delegated),
        }
    }
}

fn tinted(fg: Color, tint: Color) -> ToneColors {
    ToneColors {
        fg,
        bg: tint.alpha(0.12),
        border: tint.alpha(0.28),
    }
}

/// Height and typography density for [`Badge`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BadgeSize {
    /// Standard compact tag (`20pt` tall, `12/16` medium).
    #[default]
    Small,
    /// Micro inline tag (`16pt` tall, `10/12` nano) for PR state and refs.
    Nano,
}

/// Flat base16 badge with optional leading dot or Lucide icon.
#[derive(Debug)]
pub struct Badge {
    label: String,
    tone: Tone,
    size: BadgeSize,
    icon: Option<&'static Icon>,
    dot: bool,
    pill: bool,
    mono: bool,
}

/// Starts a flat semantic [`Badge`].
pub fn badge(label: impl Into<String>) -> Badge {
    Badge {
        label: label.into(),
        tone: Tone::Neutral,
        size: BadgeSize::Small,
        icon: None,
        dot: false,
        pill: false,
        mono: false,
    }
}

impl Badge {
    /// Sets the semantic [`Tone`].
    pub const fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Sets [`BadgeSize::Small`] or [`BadgeSize::Nano`].
    pub const fn size(mut self, size: BadgeSize) -> Self {
        self.size = size;
        self
    }

    /// Adds a leading Lucide icon tinted with the badge's tone.
    pub const fn icon(mut self, glyph: &'static Icon) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// Adds a leading status dot tinted with the badge's tone.
    pub const fn dot(mut self) -> Self {
        self.dot = true;
        self
    }

    /// Uses a full capsule radius (`RADIUS_PILL`) instead of `RADIUS_SM`.
    pub const fn pill(mut self) -> Self {
        self.pill = true;
        self
    }

    /// Renders the badge label in tabular monospace.
    pub const fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// Builds the flat hairline badge element.
    pub fn build(self, t: &Theme) -> El {
        let colors = self.tone.colors(t);
        let (height, px, icon_size, font) = match self.size {
            BadgeSize::Small => (
                20.0,
                S6,
                11.0,
                if self.mono {
                    t.mono_small
                } else {
                    t.small_medium
                },
            ),
            BadgeSize::Nano => (
                16.0,
                S4,
                10.0,
                if self.mono { t.mono_micro } else { t.nano },
            ),
        };
        let radius = if self.pill { RADIUS_PILL } else { RADIUS_SM };
        let mut el = row()
            .h(height)
            .px(px)
            .gap(S4)
            .center_y()
            .radius(radius)
            .fill(colors.bg)
            .border(HAIRLINE, colors.border);
        if self.dot {
            el = el.child(
                rune::el()
                    .size(DOT, DOT)
                    .radius(DOT / 2.0)
                    .fill(colors.fg)
                    .a11y_hidden(),
            );
        } else if let Some(glyph) = self.icon {
            el = el.child(icon(glyph, icon_size).color(colors.fg).a11y_hidden());
        }
        el.child(text(self.label).font(font).color(colors.fg).truncate())
    }
}

/// Single flat base16 keycap (`18pt` tall, `RADIUS_XS`, `chip_fill` + `1px`
/// hairline).
pub fn kbd_cap(label: &str, t: &Theme) -> El {
    row()
        .min_w(18.0)
        .h(18.0)
        .px(S4)
        .center()
        .radius(RADIUS_XS)
        .fill(t.chip_fill)
        .border(HAIRLINE, t.hairline)
        .a11y_hidden()
        .child(text(label).font(t.mono_micro).color(t.muted))
}

/// Row of flat keycaps separated by `2pt` for shortcuts (`⌘ K`, `Shift ↵`).
pub fn kbd_group(keys: &[&str], t: &Theme) -> El {
    row()
        .gap(S2)
        .center_y()
        .a11y_hidden()
        .children(keys.iter().map(|key| kbd_cap(key, t)))
}

/// Platform command shortcut keycap group (`⌘` + `key` on macOS, `Ctrl` + `key`
/// elsewhere).
pub fn kbd(key: &str, t: &Theme) -> El {
    kbd_group(&[COMMAND_KEY, key], t)
}

/// Static tabular `+added -removed` diff stat pair (`mono_small`, `success` /
/// `danger`).
pub fn diff_stat(added: u32, removed: u32, t: &Theme) -> El {
    let mut stat = row().gap(S6).center_y();
    if added == 0 && removed == 0 {
        return stat.child(text("+0").font(t.mono_small).color(t.faint));
    }
    if added > 0 {
        stat = stat.child(
            text(format!("+{added}"))
                .font(t.mono_small)
                .color(t.success),
        );
    }
    if removed > 0 {
        stat = stat.child(
            text(format!("-{removed}"))
                .font(t.mono_small)
                .color(t.danger),
        );
    }
    stat
}

/// Rolling tabular `+added -removed` diff stat pair powered by
/// [`motion::rolling_diff_stat`].
pub fn diff_stat_rolling(key_prefix: &str, added: u32, removed: u32, t: &Theme) -> El {
    motion::rolling_diff_stat(key_prefix, added, removed, t)
}

/// Formats tooltip copy with an optional trailing keyboard shortcut.
pub fn tooltip_text(label: &str, shortcut: Option<&str>) -> String {
    shortcut
        .filter(|s| !s.is_empty())
        .map_or_else(|| label.to_owned(), |key| format!("{label}  {key}"))
}

/// Attaches a consistent tooltip and accessibility shortcut hint to `target`.
pub fn with_tooltip(target: El, label: &str, shortcut: Option<&str>) -> El {
    let tip = tooltip_text(label, shortcut);
    let mut out = target.help(tip);
    if let Some(key) = shortcut.filter(|s| !s.is_empty()) {
        out = out.shortcut_hint(key);
    }
    out
}

/// A compact fixed-size dot indicator (`DOT × DOT`).
pub fn dot(color: Color) -> El {
    el().size(DOT, DOT)
        .radius(DOT / 2.0)
        .fill(color)
        .a11y_hidden()
}
