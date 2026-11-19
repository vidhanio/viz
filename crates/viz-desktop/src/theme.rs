//! Shared neutral surfaces, type scale, and one blue accent for light and dark.

use std::sync::LazyLock;

use rune::{Color, Icon, Shadow, TextStyle};
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

/// Dark appearance.
const NIGHT: Tones = Tones {
    bg: "#151618",
    deep: "#101113",
    surface: "#1c1e21",
    raised: "#232529",
    text: "#ebedf0",
    muted: "#acb0ba",
    faint: "#858a95",
    accent: "#96baff",
    on_accent: "#121f37",
    danger: "#ee9198",
    success: "#8ac5a0",
    warning: "#e8ba72",
};

/// Light appearance.
const DAY: Tones = Tones {
    bg: "#fafafb",
    deep: "#f1f2f4",
    surface: "#ffffff",
    raised: "#e9ebef",
    text: "#202126",
    muted: "#555b65",
    faint: "#686f7a",
    accent: "#275bc2",
    on_accent: "#ffffff",
    danger: "#b62d40",
    success: "#287443",
    warning: "#8c5b0a",
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
        let hairline = ink(if dark { 0.12 } else { 0.13 });
        let hover = ink(if dark { 0.07 } else { 0.055 });
        let pressed = ink(if dark { 0.08 } else { 0.07 });
        let selected = k.alpha(accent, if dark { 0.15 } else { 0.10 });
        let mine = raised;
        let strong = ink(if dark { 0.14 } else { 0.16 });
        let c = &mut k.c;
        c.page = bg;
        c.bg_base = bg;
        // The shell's backdrop: the sidebar.
        c.bg_deep = deep;
        c.layer_01 = surface;
        c.layer_02 = raised;
        c.layer_03 = raised;
        c.surface_raised = surface;
        c.surface_raised_base = surface;
        c.bg_button_neutral = raised;
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
        c.std_border = strong;
        c.hover = hover;
        c.pressed = pressed;
        c.surface_hover = hover;
        c.danger.fg = danger;
        c.danger.icon = danger;
        c.success.fg = hex(t.success);
        c.success.icon = hex(t.success);
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
        k.radius.sm = 5.0;
        k.radius.md = 7.0;
        k.radius.lg = 9.0;
        k.radius.xl = 12.0;
        let outline = Shadow {
            color: strong,
            spread: 1.0,
            ..Shadow::default()
        };
        k.elevation = rune_kit::Elevation::default();
        k.elevation.floating = vec![outline];
        k.elevation.overlay = vec![outline];
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
            .quote_rule(self.hairline)
            .rule(self.hairline)
            .link(self.text)
            .strong_weight(620.0)
            .list(22.0, 6.0)
    }
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
