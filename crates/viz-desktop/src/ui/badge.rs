//! Semantic tones, diff statistics, and tooltip
//! helpers.

use rune::{Color, El, Icon, el, icon, row, text};

use crate::{
    motion,
    theme::{DOT, ICON, S6, Theme},
};

/// Semantic accent role for status cards, toasts, and inline status elements.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tone {
    /// Quiet neutral ink.
    #[default]
    Neutral,
    /// Positive / open / added status (`base0B`).
    Success,
    /// Destructive / error / removed status (`base08`).
    Danger,
    /// Attention / warning / approval status (`base0A`).
    Warning,
}

impl Tone {
    /// The status glyph cards and toasts lead with.
    pub const fn glyph(self) -> &'static Icon {
        match self {
            Self::Danger => &super::CIRCLE_ALERT,
            Self::Warning => &super::TRIANGLE_ALERT,
            Self::Success => &super::CIRCLE_CHECK,
            Self::Neutral => &super::INFO,
        }
    }

    /// The tone's [`Tone::glyph`] in its ink, as a toast's leading mark.
    pub fn mark(self, t: &Theme) -> El {
        icon(self.glyph(), ICON).color(self.color(t))
    }

    /// Resolves foreground ink from `t`.
    pub const fn color(self, t: &Theme) -> Color {
        match self {
            Self::Neutral => t.muted,
            Self::Success => t.success,
            Self::Danger => t.danger,
            Self::Warning => t.warning,
        }
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
