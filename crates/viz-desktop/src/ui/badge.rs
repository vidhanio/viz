//! Semantic tones, diff statistics, and tooltip
//! helpers.

use rune::{Color, El, el, row, text};

use crate::{
    motion,
    theme::{DOT, S6, Theme},
};

/// Semantic accent role for badges, banners, and inline status elements.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tone {
    /// Quiet neutral chip fill.
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
}

/// Resolved flat fill and ink for a [`Tone`].
#[derive(Debug, Clone, Copy)]
pub struct ToneColors {
    /// Primary text and icon ink (`>= 4.5:1`).
    pub fg: Color,
    /// Subtle tinted background fill.
    pub bg: Color,
}

impl Tone {
    /// Resolves foreground and subtle tinted fill from `t`.
    pub fn colors(self, t: &Theme) -> ToneColors {
        match self {
            Self::Neutral => ToneColors {
                fg: t.muted,
                bg: t.chip_fill,
            },
            Self::Accent => tinted(t.accent_text, t.accent),
            Self::Success => tinted(t.success, t.success),
            Self::Danger => tinted(t.danger, t.danger),
            Self::Warning => tinted(t.warning, t.warning),
            Self::Info => tinted(t.info, t.info),
        }
    }
}

fn tinted(fg: Color, tint: Color) -> ToneColors {
    ToneColors {
        fg,
        bg: tint.alpha(0.12),
    }
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
