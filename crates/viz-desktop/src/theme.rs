//! viz's look, "Tidepool": calm water at night (and in daylight).
//!
//! - **Surfaces** are ink with a little blue in it rather than neutral grey:
//!   the sidebar is the deepest pool, the main pane a shade up, cards (the
//!   composer, expanded tool output, code) a step above that. Light is the same
//!   scheme on cool paper.
//! - **One accent**, sea-glass teal: mint on ink, deep teal on paper. It marks
//!   only what's "mine" or "now": my messages, the open conversation, the send
//!   button, the caret, the working pulse.
//! - **The tide line** is the signature: a 2-point accent edge with a soft
//!   glow, on the open conversation's sidebar row and on my messages, and as
//!   the composer's glow while it has focus.
//! - **Two voices**: Inter for people (interface 13, messages 14.5 on a 23
//!   line), IBM Plex Mono for the machine (tool calls, code, model ids, 12).
//!
//! Everything is a rune-kit [`Kit`] from [`Kit::from_palette`] with the tokens
//! the kit's components read overridden, plus the named colours the app's own
//! rows use. Only the font files come from rune-theme-opencode.

use std::sync::LazyLock;

use rune::{Color, El, Icon, LoopSpec, Shadow, TextStyle, el};
use rune_kit::{Kit, MarkdownStyle, PaletteFocus, PaletteSeed};

/// Inter and IBM Plex Mono, for `AppSpec::fonts`.
pub const FONTS: &[&[u8]] = rune_theme_opencode::FONTS;
/// `AppSpec::families`: (sans, mono).
pub const FAMILIES: (&str, &str) = rune_theme_opencode::FAMILIES;

/// The timeline's and composer's reading width.
pub const READING_WIDTH: f32 = 760.0;

/// The palette, type and kits for one appearance.
#[derive(Debug)]
pub struct Theme {
    /// rune-kit's tokens, derived from the palette below.
    pub k: Kit,
    /// `k` with square main-pane corners, for the shell: the main pane sits
    /// flush against the sidebar and the window's edges, not in a card.
    pub shell: Kit,
    /// Cards on the main pane (composer, expanded tool output, code).
    pub surface: Color,
    /// One-pixel rules and outlines.
    pub hairline: Color,
    /// Hover wash on rows.
    pub hover: Color,
    /// The quiet wash behind my messages.
    pub mine: Color,
    /// Body text.
    pub text: Color,
    /// Secondary text.
    pub muted: Color,
    /// Tertiary text: hints, durations, argument summaries.
    pub faint: Color,
    /// The one accent.
    pub accent: Color,
    /// The accent's glow around the tide line and the focused composer.
    pub glow: Color,
    /// The open conversation's row.
    pub selected: Color,
    /// Errors.
    pub danger: Color,
    /// Connected.
    pub success: Color,
    /// Connecting.
    pub warning: Color,
    /// Interface text, 13 on 20.
    pub ui: TextStyle,
    /// `ui` at the medium weight.
    pub ui_medium: TextStyle,
    /// Small labels, 11.5 on 16.
    pub small: TextStyle,
    /// Message text, 14.5 on 23.
    pub body: TextStyle,
    /// The header's title, 14 semibold.
    pub title: TextStyle,
    /// Tool rows and code, 12 on 18.
    pub mono: TextStyle,
    /// `mono` at the medium weight, for tool names.
    pub mono_medium: TextStyle,
    /// Small mono for durations and the model id, 11.5 on 16.
    pub mono_small: TextStyle,
}

/// One appearance's palette, as hex.
struct Tones {
    bg: &'static str,
    deep: &'static str,
    surface: &'static str,
    raised: &'static str,
    text: &'static str,
    muted: &'static str,
    faint: &'static str,
    accent: &'static str,
    on_accent: &'static str,
    danger: &'static str,
    success: &'static str,
    warning: &'static str,
}

/// Night: blue-black ink, mint sea glass.
const NIGHT: Tones = Tones {
    bg: "#0c0f14",
    deep: "#090b0f",
    surface: "#131821",
    raised: "#1a202b",
    text: "#e4e8ef",
    muted: "#8a94a6",
    faint: "#596274",
    accent: "#5fd4b8",
    on_accent: "#05221b",
    danger: "#f2777f",
    success: "#5fd4b8",
    warning: "#e9b45c",
};

/// Day: cool paper, deep teal.
const DAY: Tones = Tones {
    bg: "#f7f8f6",
    deep: "#eef0ed",
    surface: "#ffffff",
    raised: "#eef1ef",
    text: "#161a20",
    muted: "#5f6776",
    faint: "#9aa1ad",
    accent: "#0d8f7a",
    on_accent: "#ffffff",
    danger: "#d6434f",
    success: "#0d8f7a",
    warning: "#c98a1c",
};

impl Theme {
    /// The theme for the window's appearance.
    pub fn get(dark: bool) -> &'static Self {
        static DARK: LazyLock<Theme> = LazyLock::new(|| Theme::new(true));
        static LIGHT: LazyLock<Theme> = LazyLock::new(|| Theme::new(false));
        if dark { &DARK } else { &LIGHT }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the palette, then each kit token it sets"
    )]
    fn new(dark: bool) -> Self {
        let hex = |s: &str| rune::color::hex(s).unwrap_or(Color::BLACK);
        let t = if dark { NIGHT } else { DAY };
        let (bg, deep, surface, raised) = (hex(t.bg), hex(t.deep), hex(t.surface), hex(t.raised));
        let (text, muted, faint) = (hex(t.text), hex(t.muted), hex(t.faint));
        let (accent, on_accent, danger) = (hex(t.accent), hex(t.on_accent), hex(t.danger));
        let mut k = Kit::from_palette_focus(
            PaletteSeed {
                bg,
                fg: text,
                accent,
            },
            PaletteFocus::Accent,
        );
        let ink = |a: f32| k.alpha(text, a);
        let hairline = ink(if dark { 0.07 } else { 0.09 });
        let hover = ink(if dark { 0.045 } else { 0.04 });
        let pressed = ink(if dark { 0.08 } else { 0.07 });
        let selected = k.alpha(accent, if dark { 0.10 } else { 0.09 });
        let mine = k.alpha(accent, if dark { 0.055 } else { 0.06 });
        let glow = accent.alpha(if dark { 0.45 } else { 0.30 });
        let strong = ink(if dark { 0.14 } else { 0.16 });
        let c = &mut k.c;
        c.page = bg;
        c.bg_base = bg;
        // The shell's backdrop: the sidebar.
        c.bg_deep = deep;
        c.layer_01 = surface;
        c.layer_02 = raised;
        c.surface_raised = surface;
        c.surface_raised_base = surface;
        c.text_muted = muted;
        c.text_faint = faint;
        c.text_weak = muted;
        c.icon_muted = muted;
        c.icon_faint = faint;
        c.icon_weaker = faint;
        c.std_icon = muted;
        c.std_text = muted;
        c.text_accent = accent;
        c.icon_accent = accent;
        c.bg_accent = accent;
        c.border_muted = hairline;
        c.border_base = hairline;
        c.border_weak = hairline;
        c.border_strong = strong;
        c.hover = hover;
        c.pressed = pressed;
        c.surface_hover = hover;
        c.danger.fg = danger;
        c.danger.icon = danger;
        // The send button: an accent disc.
        c.bg_icon_button_contrast = accent;
        c.submit_icon = on_accent;
        c.submit_fg = on_accent;
        c.submit_highlight = Color::WHITE.alpha(0.0);
        c.submit_hover = Color::WHITE.alpha(if dark { 0.14 } else { 0.12 });
        let ui = TextStyle::new(13.0, 400.0).lh(20.0);
        let mono = TextStyle::new(12.0, 400.0).mono().lh(18.0);
        k.ty.ui = ui;
        k.ty.ui_medium = TextStyle {
            weight: 500.0,
            ..ui
        };
        k.ty.mono = mono;
        k.radius.md = 7.0;
        k.radius.lg = 10.0;
        k.radius.xl = 16.0;
        // Softer, wider drop shadows than the kit's, for the floating composer
        // and toasts.
        let drop = |y: f32, blur: f32, a: f32| Shadow {
            y,
            blur,
            color: Color::BLACK.alpha(a),
            ..Shadow::default()
        };
        k.elevation.floating = vec![
            drop(8.0, 28.0, if dark { 0.45 } else { 0.10 }),
            drop(1.0, 3.0, if dark { 0.30 } else { 0.06 }),
        ];
        let mut shell = k.clone();
        shell.radius.lg = 0.0;
        Self {
            shell,
            surface,
            hairline,
            hover,
            mine,
            text,
            muted,
            faint,
            accent,
            glow,
            selected,
            danger,
            success: hex(t.success),
            warning: hex(t.warning),
            ui,
            ui_medium: TextStyle {
                weight: 500.0,
                ..ui
            },
            small: TextStyle::new(11.5, 400.0).lh(16.0),
            body: TextStyle::new(14.5, 400.0).lh(23.0),
            title: TextStyle::new(14.0, 600.0).lh(20.0),
            mono,
            mono_medium: TextStyle {
                weight: 500.0,
                ..mono
            },
            mono_small: TextStyle::new(11.5, 400.0).mono().lh(16.0),
            k,
        }
    }

    /// Assistant prose: the message type, code blocks on `surface`.
    pub fn markdown(&self) -> MarkdownStyle {
        let heading = |size: f32, lh: f32| TextStyle {
            size,
            weight: 620.0,
            line_height: lh,
            ..self.body
        };
        MarkdownStyle::from_kit(&self.k)
            .body_font(self.body)
            .heading_font(1, heading(19.0, 28.0))
            .heading_font(2, heading(16.5, 26.0))
            .heading_font(3, heading(14.5, 23.0))
            .code_block_font(TextStyle {
                line_height: 19.0,
                ..self.mono
            })
            .code_block(self.surface, self.hairline)
            .code_block_padding(14.0, 12.0)
            .inline_code_font(0.86, 400.0)
            .code_margin(12.0, 16.0)
            .quote_rule(self.accent)
            .rule(self.hairline)
            .link(self.accent)
            .strong_weight(620.0)
            .list(22.0, 6.0)
    }

    /// The tide line: a 2-point accent edge with a soft glow, `height` tall.
    pub fn tide(&self, height: f32) -> El {
        el().w(2.0)
            .h(height)
            .radius(1.0)
            .fill(self.accent)
            .shadow(0.0, 8.0, self.glow)
            .a11y_hidden()
    }
}

/// A small round dot.
pub fn dot(color: Color, size: f32) -> El {
    el().size(size, size)
        .radius(size / 2.0)
        .fill(color)
        .a11y_hidden()
}

/// A dot that breathes, for "working".
pub fn pulse(color: Color, size: f32) -> El {
    dot(color, size).looping(LoopSpec::opacity(1.4, 0.0, 0.25, 1.0))
}

/// The orchestrator's mark: a small four-point star.
pub static STAR: Icon = Icon {
    name: "viz-star",
    view: 16.0,
    body: r#"<path d="M8 2C8.35 5.3 10.7 7.65 14 8C10.7 8.35 8.35 10.7 8 14C7.65 10.7 5.3 8.35 2 8C5.3 7.65 7.65 5.3 8 2Z" fill="currentColor"/>"#,
};

/// New chat.
pub static PLUS: Icon = Icon {
    name: "viz-plus",
    view: 16.0,
    body: r#"<path d="M8 3.5V12.5M3.5 8H12.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>"#,
};

/// Send.
pub static ARROW_UP: Icon = Icon {
    name: "viz-arrow-up",
    view: 16.0,
    body: r#"<path d="M8 13V3.5M3.75 7.5L8 3.25L12.25 7.5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// A subagent: a hooked arrow down and right.
pub static BRANCH: Icon = Icon {
    name: "viz-branch",
    view: 16.0,
    body: r#"<path d="M4.5 3V7.5C4.5 9 5.5 10 7 10H12M9.5 7.5L12 10L9.5 12.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// A collapsed row's disclosure.
pub static CHEVRON: Icon = Icon {
    name: "viz-chevron",
    view: 16.0,
    body: r#"<path d="M6.5 4.5L10 8L6.5 11.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// An open row's disclosure.
pub static CHEVRON_OPEN: Icon = Icon {
    name: "viz-chevron-open",
    view: 16.0,
    body: r#"<path d="M4.5 6.5L8 10L11.5 6.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// Thousands as `12.3k`, millions as `1.2M`.
#[expect(
    clippy::cast_precision_loss,
    reason = "token counts are shown to one decimal"
)]
pub fn compact_count(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// A duration as `320ms`, `4.2s` or `2m 5s`.
#[expect(
    clippy::cast_precision_loss,
    reason = "durations are shown to one decimal"
)]
pub fn duration(ms: u64) -> String {
    match ms {
        0..1_000 => format!("{ms}ms"),
        1_000..60_000 => format!("{:.1}s", ms as f64 / 1e3),
        _ => format!("{}m {}s", ms / 60_000, ms % 60_000 / 1_000),
    }
}

/// Dollars, with more precision for small amounts.
pub fn cost(dollars: f64) -> String {
    if dollars < 0.995 {
        format!("${dollars:.3}")
    } else {
        format!("${dollars:.2}")
    }
}
