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
//! - Rules: one [`HAIRLINE`], `Theme::hairline` ink.
//! - Overlays (menus, popovers, dialogs, the palette, toasts): no outline; a
//!   fill one step above what they float over and a soft two-layer shadow, both
//!   from [`Theme::overlay`].
//! - Sidebar: rows fill from [`SIDEBAR_PAD`]; every label, icon and section
//!   title starts at [`SIDEBAR_INSET`] plus [`TREE_STEP`] per depth; every row
//!   ends in a [`TRAILING`]-wide slot centered under the sidebar toggle.
//! - Main pane: one [`READING_WIDTH`] column, [`READING_INSET`] from the pane's
//!   edges, shared by the toolbar title, the transcript and the composer.

mod scheme;

use std::sync::OnceLock;

use rune::{Color, Shadow, TextStyle};
use rune_kit::{
    Kit, MarkdownStyle, Palette, PaletteFocus, PaletteSeed, StateColors,
    syntax::{self, Highlighter, Paint, Syntax, SyntaxTheme},
};
use serde::Deserialize;

use self::scheme::{DEFAULT_DARK, DEFAULT_LIGHT, Scheme};

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
/// A sidebar row with a quiet second line (Recent's project names).
pub const TREE_ROW_TALL: f32 = S40;
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
/// Wide enough to show a previous-worktree label and a typical branch together.
pub const WORKSPACE_MENU_WIDTH: f32 = 280.0;
/// The model picker: wide enough for a provider's long model names beside
/// the thinking levels.
pub const MODEL_MENU_WIDTH: f32 = 320.0;
/// The model list's height before it scrolls: eight rows and a heading.
pub const MODEL_LIST_HEIGHT: f32 = S32 * 9.0;
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
    /// `k` for overlays (menus, popovers, the completion list): borderless,
    /// on their own fill; `overlay.c.layer_01` fills dialogs and toasts.
    pub overlay: Kit,
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
    /// Code colors: Markdown fences and the changes panel's diff.
    pub syntax: SyntaxTheme,
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

/// `[theme]` in `desktop.toml`: a base16 scheme per appearance, by built-in
/// name, inline `colors`, or a scheme `file` (see `desktop.example.toml`). Kept
/// as raw TOML so a mistake here never stops the rest of the file from loading:
/// [`install`] reports it and falls back to the default scheme.
#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
pub struct ThemeConfig(toml::Table);

static THEMES: OnceLock<(Theme, Option<Theme>)> = OnceLock::new();

/// Builds both appearances from `config`, once, before the window opens. A
/// side that fails gets its default scheme; the returned message says what
/// was wrong, for a toast.
pub fn install(config: &ThemeConfig, scene_theme: Option<&str>) -> Option<String> {
    if let Some(name) = scene_theme {
        match Scheme::builtin(name) {
            Ok(scheme) => {
                _ = THEMES.set((Theme::new(&scheme), None));
                return None;
            }
            Err(error) => return Some(format!("VIZ_SCENE_THEME: {error}")),
        }
    }
    let mut errors: Vec<_> = config
        .0
        .keys()
        .filter(|key| !matches!(key.as_str(), "dark" | "light"))
        .map(|key| format!("theme.{key}: unknown key; expected dark or light"))
        .collect();
    let mut side = |key: &str, default: &str| {
        config
            .0
            .get(key)
            .map_or_else(|| Ok(builtin(default)), resolve)
            .unwrap_or_else(|e| {
                errors.push(format!("theme.{key}: {e}"));
                builtin(default)
            })
    };
    let [dark, light] = [side("dark", DEFAULT_DARK), side("light", DEFAULT_LIGHT)];
    _ = THEMES.set((Theme::new(&dark), Some(Theme::new(&light))));
    (!errors.is_empty()).then(|| errors.join("\n"))
}

/// One side of `[theme]`: a built-in name, `{ colors = { base00 = … } }` or
/// `{ file = "…" }`.
fn resolve(value: &toml::Value) -> Result<Scheme, String> {
    let table = match value {
        toml::Value::String(name) => return Scheme::builtin(name),
        toml::Value::Table(table) => table,
        _ => return Err("expected a scheme name, or a table with colors or file".into()),
    };
    if let Some(key) = table
        .keys()
        .find(|key| !matches!(key.as_str(), "colors" | "file"))
    {
        return Err(format!("unknown key \"{key}\"; expected colors or file"));
    }
    match (table.get("colors"), table.get("file")) {
        (Some(colors), None) => Scheme::from_colors(
            &colors
                .clone()
                .try_into()
                .map_err(|_| "colors: expected base00 to base0F as \"#rrggbb\" strings")?,
        )
        .map_err(|e| format!("colors: {e}")),
        (None, Some(toml::Value::String(path))) => Scheme::from_file(path),
        (None, Some(_)) => Err("file: expected a path".into()),
        (Some(_), Some(_)) => Err("set colors or file, not both".into()),
        (None, None) => Err("expected colors or file".into()),
    }
}

/// A default scheme. They are vendored and parse.
fn builtin(name: &str) -> Scheme {
    Scheme::builtin(name).unwrap_or_else(|e| unreachable!("built-in scheme {name}: {e}"))
}

/// WCAG contrast ratio of two opaque colors.
fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (a.luminance(), b.luminance());
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The lowest contrast of `c` on any of `surfaces`.
fn worst(c: Color, surfaces: &[Color]) -> f32 {
    surfaces
        .iter()
        .map(|&s| contrast(c, s))
        .fold(f32::INFINITY, f32::min)
}

/// `c`, its Oklab lightness pushed away from the surfaces (lighter when
/// `dark`) just far enough to reach `min` contrast on all of them; hue and
/// chroma kept. Many base16 schemes' comment greys and light variants' yellows
/// fall short of legible text.
fn legible(c: Color, surfaces: &[Color], dark: bool, min: f32) -> Color {
    if worst(c, surfaces) >= min {
        return c;
    }
    let [l, a, b, _] = c.to_oklab();
    let end = if dark { 1.0 } else { 0.0 };
    let at = |t: f32| Color::from_oklab([(end - l).mul_add(t, l), a, b, 1.0]);
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..20 {
        let mid = f32::midpoint(low, high);
        if worst(at(mid), surfaces) >= min {
            high = mid;
        } else {
            low = mid;
        }
    }
    at(high)
}

/// `top` at opacity `t` over opaque `base`, as the window composites it: in
/// sRGB-encoded values (`App::srgb_blend`, Rune's default), not linear light.
/// Linear mixing overstates a light wash on a dark surface and understates a
/// dark one on a light surface, so washes computed with it came out faint in
/// dark schemes and heavy in light ones.
fn over(base: Color, top: Color, t: f32) -> Color {
    use rune::color::{srgb_decode, srgb_encode};
    let m =
        |a: f32, b: f32| srgb_decode((srgb_encode(b) - srgb_encode(a)).mul_add(t, srgb_encode(a)));
    Color {
        r: m(base.r, top.r),
        g: m(base.g, top.g),
        b: m(base.b, top.b),
        a: 1.0,
    }
}

/// How far (0 to 1) from `from` toward `to`, composited by [`over`], moves
/// Oklab lightness by `step`: 1 when even `to` doesn't. Washes are this much
/// of `base02`, so they read alike across schemes whose `base02` sits near or
/// far from `base00`, and in both appearances.
fn reach(from: Color, to: Color, step: f32) -> f32 {
    let lightness = |c: Color| c.to_oklab()[0];
    let distance = |c: Color| (lightness(c) - lightness(from)).abs();
    if distance(to) <= step {
        return 1.0;
    }
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..20 {
        let mid = f32::midpoint(low, high);
        if distance(over(from, to, mid)) >= step {
            high = mid;
        } else {
            low = mid;
        }
    }
    high
}

impl Theme {
    /// The theme for the window's appearance: the scheme [`install`] picked,
    /// or the default.
    pub fn get(dark: bool) -> &'static Self {
        let (night, day) = THEMES.get_or_init(|| {
            (
                Self::new(&builtin(DEFAULT_DARK)),
                Some(Self::new(&builtin(DEFAULT_LIGHT))),
            )
        });
        if dark {
            night
        } else {
            day.as_ref().unwrap_or(night)
        }
    }

    /// Every color from `scheme`'s base16 roles:
    ///
    /// | Role | Tokens |
    /// |---|---|
    /// | `base00` | `bg`, the main pane |
    /// | `base01` | `surface`: the sidebar, cards, code |
    /// | `base02` | `mine`; mixed with `base05` for interactive washes |
    /// | `base03` | `faint` |
    /// | `base04` | `muted` |
    /// | `base05` | `text`; hairlines are it at low alpha |
    /// | `base06`/`07` | strong text, ink on the accent |
    /// | `base08` | `danger`, deleted lines, properties |
    /// | `base09` | numbers, attributes |
    /// | `base0A` | `warning`, types |
    /// | `base0B` | `success`, inserted lines, strings |
    /// | `base0C` | info, regexes |
    /// | `base0D` | `accent`, functions, headings |
    /// | `base0E` | keywords |
    /// | `base0F` | (unused) |
    ///
    /// Text and statuses meet contrast floors on both backgrounds and the
    /// selected wash; code also accounts for tinted diff lines.
    #[expect(
        clippy::too_many_lines,
        reason = "the scheme's roles, then each kit token they set"
    )]
    fn new(scheme: &Scheme) -> Self {
        let b = scheme.base;
        let (bg, surface, wash) = (b[0x0], b[0x1], b[0x2]);
        let dark = b[0x5].luminance() > bg.luminance();
        let interactive = wash.mix(b[0x5], 0.35);
        // A wash must remain visible on the sidebar as well as the main pane.
        let tint = |step: f32| {
            interactive.alpha(reach(bg, interactive, step).max(reach(surface, interactive, step)))
        };
        let hover = tint(0.026);
        let pressed = tint(0.040);
        let selected = tint(0.055);
        let raised = over(bg, wash, reach(bg, wash, 0.05));
        let by_luminance = |x: &Color, y: &Color| x.luminance().total_cmp(&y.luminance());
        let lightest = b[..8].iter().copied().max_by(by_luminance).unwrap_or(bg);
        let darkest = b[..8].iter().copied().min_by(by_luminance).unwrap_or(bg);
        // Overlays sit one step above whatever they float over, the sidebar
        // included: lighter than `surface` in the dark, the scheme's lightest
        // paper in the light, where the shadow alone lifts it.
        let overlay = if dark {
            over(surface, wash, reach(surface, wash, 0.04))
        } else {
            lightest
        };
        let surfaces = [
            bg,
            surface,
            raised,
            overlay,
            over(bg, interactive, selected.a),
            over(surface, interactive, selected.a),
            over(overlay, interactive, selected.a),
        ];
        let legible = |c: Color, min: f32| legible(c, &surfaces, dark, min);
        let text = legible(b[0x5], 5.5);
        let strong = legible(b[0x6], 5.5);
        let muted = legible(b[0x4], 5.2);
        let faint = legible(b[0x3], 4.5);
        let accent = legible(b[0xD], 3.0);
        let on_accent = [bg, b[0x7], lightest, darkest]
            .into_iter()
            .max_by(|x, y| contrast(*x, accent).total_cmp(&contrast(*y, accent)))
            .unwrap_or(bg);
        let (danger, warning, success, info) = (
            legible(b[0x8], 4.5),
            legible(b[0xA], 4.5),
            legible(b[0xB], 4.5),
            legible(b[0xC], 4.5),
        );
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
        let rule = ink(if dark { 0.15 } else { 0.17 });
        let state = |fg: Color| StateColors {
            bg: surface.mix(fg, 0.12),
            fg,
            border: surface.mix(fg, 0.35),
            icon: fg,
        };
        k.c = Palette {
            page: bg,
            bg_base: bg,
            // The shell's backdrop: the sidebar.
            bg_deep: surface,
            layer_01: surface,
            layer_02: raised,
            layer_03: raised,
            layer_04: raised,
            bg_inverse: text,
            bg_contrast: text,
            bg_button_neutral: raised,
            bg_icon_button_contrast: accent,
            bg_accent: accent,
            submit_highlight: on_accent.alpha(0.0),
            submit_hover: on_accent.alpha(0.14),
            submit_fg: on_accent,
            submit_icon: on_accent,
            text_base: text,
            text_muted: muted,
            text_faint: faint,
            text_inverse: bg,
            text_contrast: bg,
            text_accent: accent,
            icon_base: text,
            icon_muted: muted,
            icon_faint: faint,
            icon_inverse: bg,
            icon_contrast: bg,
            icon_accent: accent,
            border_muted: hairline,
            border_base: hairline,
            border_strong: rule,
            border_focus: accent,
            hover,
            pressed,
            contrast_hover: bg.alpha(0.10),
            contrast_pressed: bg.alpha(0.20),
            scrim: darkest.alpha(if dark { 0.5 } else { 0.3 }),
            depth_top: lightest,
            depth_bottom: lightest.alpha(0.0),
            white: lightest,
            black: darkest,
            knob: text,
            knob_on: on_accent,
            success: state(success),
            warning: state(warning),
            danger: state(danger),
            info: state(info),
            surface_base: raised,
            surface_weak: surface,
            surface_hover: hover,
            surface_raised: surface,
            surface_raised_base: surface,
            surface_raised_hover: raised,
            surface_float: text,
            surface_disabled: raised,
            border_weak: hairline,
            border_hover: rule,
            border_active: rule,
            border_disabled: hairline,
            text_strong: strong,
            text_weak: muted,
            text_invert_strong: bg,
            icon_weaker: faint,
            std_border: rule,
            std_icon: muted,
            std_icon_invert: bg,
            std_icon_strong: strong,
            std_text: muted,
        };
        // rune-kit uses Oklab tints for diff words; they are the closest
        // those backgrounds get to the syntax ink. Use the stronger tint for
        // both appearances to leave a margin on the lighter diff washes.
        let bg_lab = bg.to_oklab();
        let diff_word = |ink: Color| {
            let lab = ink.to_oklab();
            Color::from_oklab(std::array::from_fn(|i| {
                (lab[i] - bg_lab[i]).mul_add(0.32, bg_lab[i])
            }))
        };
        let syntax_surfaces = [bg, surface, diff_word(success), diff_word(danger)];
        let code = |ink: Color| crate::theme::legible(ink, &syntax_surfaces, dark, 4.5);
        let blue = code(b[0xD]);
        let orange = code(b[0x9]);
        let neutral = code(text);
        let punctuation = code(muted);
        let syntax = [
            (Syntax::Keyword, code(b[0xE])),
            (Syntax::String, code(b[0xB])),
            (Syntax::Number, orange),
            (Syntax::Comment, code(faint)),
            (Syntax::Type, code(b[0xA])),
            (Syntax::Function, blue),
            (Syntax::Property, code(b[0x8])),
            (Syntax::Punctuation, punctuation),
            (Syntax::Inserted, code(success)),
            (Syntax::Deleted, code(danger)),
            (Syntax::Parameter, neutral),
            (Syntax::Heading, blue),
            (Syntax::Link, blue),
            (Syntax::Operator, punctuation),
            (Syntax::Regex, code(b[0xC])),
            (Syntax::Attribute, orange),
            (Syntax::Variable, neutral),
        ]
        .into_iter()
        .fold(SyntaxTheme::from_kit(&k), |theme, (kind, c)| {
            theme.with(kind, c)
        });
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
        // Two layers: a tight contact shadow that defines the edge, and a wide
        // soft one that lifts the surface. Darker on dark schemes, where a
        // shadow has less light to take away.
        let shade = |a: f32| Color::BLACK.alpha(if dark { a * 2.4 } else { a });
        let lift = |near: f32, far: (f32, f32)| {
            vec![
                Shadow {
                    y: 1.0,
                    blur: 3.0,
                    color: shade(near),
                    ..Shadow::default()
                },
                Shadow {
                    y: far.0,
                    blur: far.1,
                    color: shade(0.10),
                    ..Shadow::default()
                },
            ]
        };
        k.elevation = rune_kit::Elevation::default();
        k.elevation.floating = lift(0.08, (S6, S24));
        k.elevation.md = k.elevation.floating.clone();
        k.elevation.overlay = lift(0.10, (S16, S48));
        let mut shell = k.clone();
        shell.radius.lg = 0.0;
        // rune-kit fills menus, popovers and dialogs from these and outlines
        // the standard ones in `std_border`.
        let mut overlay_kit = k.clone();
        overlay_kit.c.layer_01 = overlay;
        overlay_kit.c.surface_raised = overlay;
        overlay_kit.c.surface_raised_hover =
            over(overlay, interactive, reach(overlay, interactive, 0.04));
        overlay_kit.c.std_border = Color::TRANSPARENT;
        Self {
            shell,
            overlay: overlay_kit,
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
            success,
            warning,
            syntax,
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

    /// The built-in highlighter in this theme's code colors, for rune-kit
    /// views that take a highlighter rather than a syntax theme (the diff).
    pub fn highlighter(&self) -> Highlighter {
        let theme = self.syntax;
        Highlighter::new(move |lang, code| {
            syntax::builtin(lang, code)
                .into_iter()
                .map(|(range, paint)| (range, Paint::Color(theme.paint(paint))))
                .collect()
        })
        .key(theme)
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
            .syntax_theme(self.syntax)
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
