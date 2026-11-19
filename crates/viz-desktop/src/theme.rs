//! Design tokens. Views read these through `ui` components; a bare number in a
//! view is a bug.
//!
//! - Space: a four-point grid (`S4`…`S48`); `S2` and `S6` are half steps for
//!   optical centering only.
//! - Controls: [`Size`] — small 24, default 28, large 36 points tall, each with
//!   its radius (6, 6, 8). Cards round at [`RADIUS_CARD`], chips at
//!   [`RADIUS_INNER`].
//! - Icons: 12, 16 and 20 points; status dots are [`DOT`].
//! - Type: `small` 11.5/16, `ui` 13/20, `title` 14/20, `body` 14.5/23, `mono`
//!   12/18 (see [`Theme`]).
//! - Rules: one [`HAIRLINE`], `Theme::hairline` ink; overlays get a hairline
//!   outline and no shadow.
//! - Sidebar: rows fill from [`SIDEBAR_PAD`]; every label, icon and section
//!   title starts at [`SIDEBAR_INSET`] plus [`TREE_STEP`] per depth; every row
//!   ends in a [`TRAILING`]-wide slot centered under the sidebar toggle.
//! - Main pane: one [`READING_WIDTH`] column, [`READING_INSET`] from the pane's
//!   edges, shared by the toolbar title, the transcript and the composer.

use std::sync::LazyLock;

use rune::{Color, Shadow, TextStyle};
use rune_kit::{Kit, MarkdownStyle, PaletteFocus, PaletteSeed};

/// Inter and IBM Plex Mono, for `AppSpec::fonts`.
pub const FONTS: &[&[u8]] = rune_theme_opencode::FONTS;
/// `AppSpec::families`: (sans, mono).
pub const FAMILIES: (&str, &str) = rune_theme_opencode::FAMILIES;

pub const S2: f32 = 2.0;
pub const S4: f32 = 4.0;
pub const S6: f32 = 6.0;
pub const S8: f32 = 8.0;
pub const S12: f32 = 12.0;
pub const S16: f32 = 16.0;
pub const S20: f32 = 20.0;
pub const S24: f32 = 24.0;
pub const S32: f32 = 32.0;
pub const S40: f32 = 40.0;
pub const S48: f32 = 48.0;

pub const HAIRLINE: f32 = 1.0;
pub const ICON_SMALL: f32 = 12.0;
pub const ICON: f32 = 16.0;
pub const ICON_LARGE: f32 = 20.0;
pub const DOT: f32 = 6.0;

/// Chips, key caps and other marks inside a control.
pub const RADIUS_INNER: f32 = S4;
/// Cards, the composer, popovers.
pub const RADIUS_CARD: f32 = S8;

/// The shell's title band: the toolbar, the sidebar's header row, the
/// inspector's tab row.
pub const TOOLBAR_HEIGHT: f32 = rune_kit::shell::TOOLBAR_HEIGHT;
/// rune-kit's padding at the toolbar band's ends.
pub const TOOLBAR_PAD: f32 = S12;

/// The sidebar pane's padding: where row fills start and end.
pub const SIDEBAR_PAD: f32 = S8;
/// Where sidebar content starts: labels, icons, section titles, the footer.
pub const SIDEBAR_INSET: f32 = S16;
/// A tree row's inset inside its fill.
pub const ROW_INSET: f32 = SIDEBAR_INSET - SIDEBAR_PAD;
pub const TREE_ROW: f32 = Size::Default.height();
/// One depth: a leading icon and its gap, so a child's label lines up with
/// its parent's.
pub const TREE_STEP: f32 = ICON + S8;
/// The fixed slot every sidebar row ends in (status, unread, actions).
pub const TRAILING: f32 = Size::Default.height();
/// The orchestrator's two-line row.
pub const ORCHESTRATOR_ROW: f32 = S40 + S4;

/// The transcript and composer column.
pub const READING_WIDTH: f32 = 760.0;
/// The main pane's side inset: the toolbar title, the transcript, the composer.
pub const READING_INSET: f32 = S20;
/// Padding inside cards: messages, tool output, plans, the composer.
pub const CARD_INSET: f32 = S12;
/// The changes inspector's side inset.
pub const PANEL_INSET: f32 = S12;
/// Padding inside dialogs (rune-kit's header uses the same).
pub const DIALOG_INSET: f32 = S16;
pub const DIALOG_WIDTH: f32 = 400.0;
/// Floating cards anchored to a control: the Agents card.
pub const CARD_WIDTH: f32 = 320.0;
pub const PALETTE_WIDTH: f32 = 560.0;
/// Empty states sit this far down the transcript.
pub const EMPTY_TOP: f32 = S32 * 4.0;
/// Empty states' text measure.
pub const EMPTY_WIDTH: f32 = 360.0;

/// Control sizes. Every button, field, select and row picks one.
#[derive(Debug, Clone, Copy)]
pub enum Size {
    /// 24: row actions, inline tools, dense lists.
    Small,
    /// 28: toolbars, the composer, tree rows, dialogs.
    Default,
    /// 36: the palette's field, primary onboarding actions.
    Large,
}

impl Size {
    pub const fn height(self) -> f32 {
        match self {
            Self::Small => S24,
            Self::Default => 28.0,
            Self::Large => 36.0,
        }
    }

    pub const fn radius(self) -> f32 {
        match self {
            Self::Small | Self::Default => S6,
            Self::Large => S8,
        }
    }

    /// rune-kit's control size with the same height.
    pub const fn kit(self) -> rune_kit::ControlSize {
        match self {
            Self::Small => rune_kit::ControlSize::Regular,
            Self::Default => rune_kit::ControlSize::Large,
            Self::Large => rune_kit::ControlSize::ExtraLarge,
        }
    }
}

/// The palette, type and kits for one appearance.
#[derive(Debug)]
pub struct Theme {
    /// rune-kit's tokens, derived from the palette below.
    pub k: Kit,
    /// `k` with square main-pane corners, for the shell: the main pane sits
    /// flush against the sidebar and the window's edges, not in a card.
    pub shell: Kit,
    /// The main pane's background.
    pub bg: Color,
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
    /// `ui` at the medium weight: names, emphasis.
    pub ui_medium: TextStyle,
    /// Small labels, 11.5 on 16.
    pub small: TextStyle,
    /// `small` at the medium weight: section titles.
    pub small_medium: TextStyle,
    /// Message text, 14.5 on 23.
    pub body: TextStyle,
    /// Titles: the toolbar, empty states, 14 semibold on 20.
    pub title: TextStyle,
    /// Tool rows and code, 12 on 18.
    pub mono: TextStyle,
    /// `mono` at the medium weight, for tool names.
    pub mono_medium: TextStyle,
    /// Small mono for durations and counts, 11.5 on 16.
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
        let hairline = ink(if dark { 0.11 } else { 0.12 });
        let hover = ink(if dark { 0.06 } else { 0.05 });
        let pressed = ink(if dark { 0.09 } else { 0.08 });
        let selected = ink(if dark { 0.09 } else { 0.075 });
        let strong = ink(if dark { 0.15 } else { 0.17 });
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
        let medium = |style: TextStyle| TextStyle {
            weight: 500.0,
            ..style
        };
        let ui = TextStyle::new(13.0, 400.0).lh(20.0);
        let small = TextStyle::new(11.5, 400.0).lh(16.0);
        let body = TextStyle::new(14.5, 400.0).lh(23.0);
        let mono = TextStyle::new(12.0, 400.0).mono().lh(18.0);
        k.ty.ui = ui;
        k.ty.ui_medium = medium(ui);
        k.ty.ui_compact = ui.lh(small.line_height);
        k.ty.small = small;
        k.ty.small_medium = medium(small);
        k.ty.body = body;
        k.ty.body_medium = medium(body);
        k.ty.mono = mono;
        crate::ui::kit_glyphs(&mut k.glyph);
        k.radius.xs = RADIUS_INNER;
        k.radius.sm = RADIUS_INNER;
        k.radius.md = Size::Default.radius();
        k.radius.lg = Size::Default.radius();
        k.radius.xl = Size::Large.radius();
        let outline = Shadow {
            color: strong,
            spread: HAIRLINE,
            ..Shadow::default()
        };
        k.elevation = rune_kit::Elevation::default();
        k.elevation.floating = vec![outline];
        k.elevation.overlay = vec![outline];
        k.elevation.md = vec![outline];
        let mut shell = k.clone();
        shell.radius.lg = 0.0;
        Self {
            shell,
            bg,
            surface,
            hairline,
            hover,
            mine: raised,
            text,
            muted,
            faint,
            accent,
            selected,
            danger,
            success: hex(t.success),
            warning: hex(t.warning),
            ui,
            ui_medium: medium(ui),
            small,
            small_medium: medium(small),
            body,
            title: TextStyle::new(14.0, 600.0).lh(20.0),
            mono,
            mono_medium: medium(mono),
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
            .code_block_padding(CARD_INSET, CARD_INSET)
            .inline_code_font(0.86, 400.0)
            .code_margin(S12, S16)
            .quote_rule(self.hairline)
            .rule(self.hairline)
            .link(self.text)
            .strong_weight(620.0)
            .list(S24, S6)
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

/// How long ago `then` (Unix milliseconds) was: `now`, `4m`, `3h`, `2d`.
pub fn ago(then: i64) -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let now = i64::try_from(now.as_millis()).ok()?;
    let minutes = (now - then).max(0) / 60_000;
    (then > 0).then(|| match minutes {
        0 => "now".to_owned(),
        1..60 => format!("{minutes}m ago"),
        60..1_440 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1_440),
    })
}
