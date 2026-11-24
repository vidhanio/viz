//! Design tokens and base16 theme derivation for `viz-desktop`.
//!
//! Every colour role derives deterministically from the configured base16
//! scheme (`base00`–`base0F`) with Oklab contrast floors and strictly flat
//! surfaces framed by crisp `1px` hairlines — never drop shadows, blur, or
//! glow. Views consume spacing, radii, typography, and colours through this
//! module and [`crate::ui`].

mod palette;
mod scheme;
mod tokens;
mod typography;

use std::sync::OnceLock;

use rune::{Color, HelpLook, TextStyle};
use rune_kit::{Kit, MarkdownStyle, syntax::SyntaxTheme};
use serde::Deserialize;

use self::scheme::{DEFAULT_DARK, DEFAULT_LIGHT, Scheme};
#[expect(
    unused_imports,
    reason = "complete geometry and radius token set exported for feature views"
)]
pub use self::tokens::{
    CARD_INSET, CHAT_CONTENT_MAX_WIDTH, CHAT_LANE_PADDING_X, COMPOSER_DRAWER_INSET,
    COMPOSER_DRAWER_OVERLAP, DIALOG_INSET, DIALOG_MAX_WIDTH, DOT, EMPTY_TOP, EMPTY_WIDTH, HAIRLINE,
    ICON, ICON_MD, ICON_SMALL, MODEL_PICKER_BODY_HEIGHT, MODEL_PICKER_OPTIONS_WIDTH,
    MODEL_PICKER_RAIL_WIDTH, PALETTE_MAX_WIDTH, PALETTE_WIDTH, RADIUS_CARD, RADIUS_CONTROL,
    RADIUS_PILL, RADIUS_SM, RADIUS_SURFACE, RADIUS_XS, READING_INSET, READING_WIDTH, S2, S4, S6,
    S8, S10, S12, S14, S16, S20, S24, S28, S32, S40, S44, SIDEBAR_INSET, SIDEBAR_PAD, Size,
    TOOLBAR_HEIGHT, TOOLBAR_PAD, squircle_radius,
};

/// `AppSpec::families`: platform UI sans plus `"ui-monospace"`, nothing
/// bundled.
pub fn families() -> (&'static str, &'static str) {
    use fontique::{Collection, CollectionOptions, GenericFamily};

    let mut system = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: true,
    });
    let sans = [GenericFamily::SansSerif, GenericFamily::SystemUi]
        .into_iter()
        .find_map(|generic| system.generic_families(generic).next())
        .and_then(|id| system.family_name(id))
        .map_or("sans-serif", |name| name.to_owned().leak());
    (sans, "ui-monospace")
}

/// Resolved base16 palette, typography scale, and `rune-kit` configurations for
/// one appearance (dark or light).
#[derive(Debug)]
pub struct Theme {
    /// Primary `rune-kit` theme tokens.
    pub k: Kit,
    /// `k` with square main-pane corners for flush `SplitView` layouts.
    pub shell: Kit,
    /// `k` for flat opaque overlays framed by a `1px` `border_overlay` ring.
    pub overlay: Kit,
    /// Main timeline and home backdrop (`base00`).
    pub canvas: Color,
    /// Left sidebar ground (`base01`, or darkened slightly when `base01 >
    /// base00`).
    pub sidebar_bg: Color,
    /// Cards, composer shell, code blocks, and changed-files frames (`base01`).
    pub surface: Color,
    /// Quiet inset well for provider rail, segmented tracks, and filter wells.
    pub surface_recessed: Color,
    /// Opaque flat elevated surface for popovers, menus, dialogs, and palette.
    pub surface_raised: Color,
    /// Tucked composer drawer surface behind the composer hairline.
    pub surface_drawer: Color,
    /// Recessed fill for inline chips, `<Kbd>` caps, and subtle badges.
    pub chip_fill: Color,
    /// Quiet wash behind user message bubbles.
    pub mine: Color,
    /// Translucent modal dialog and command palette backdrop scrim.
    pub scrim: Color,
    /// Resting hover wash for ghost buttons, rows, and cards.
    pub hover_wash: Color,
    /// Highlighted row fill for active menus, comboboxes, palette, and threads.
    pub selected: Color,
    /// Subtle fill for a selected row when not hovered.
    pub selected_subtle: Color,
    /// Pointer-down wash for active controls.
    pub pressed_wash: Color,
    /// Subtle structural divider and pane-edge hairline.
    pub hairline: Color,
    /// Standard `1px` border for outline buttons, inputs, and code blocks.
    pub border_control: Color,
    /// Crisp `1px` frame around menus, popovers, dialogs, and floating cards.
    pub border_overlay: Color,
    /// Emphasized `1px` border for hovered outline controls and active inputs.
    pub border_strong: Color,
    /// Muted border colour for queued message cards and dashed empty slots.
    pub border_dashed: Color,
    /// Primary body and UI copy (`>= 5.5:1` contrast).
    pub text: Color,
    /// Emphasized headings, active titles, and dialog titles (`>= 5.5:1`).
    pub text_strong: Color,
    /// Secondary labels for work-log entries and details rows (`>= 5.2:1`).
    pub text_secondary: Color,
    /// Muted subtitles, project names, branches, and timestamps (`>= 5.2:1`).
    pub muted: Color,
    /// Faint hints, placeholders, resting chevrons, and empty copy (`>=
    /// 4.5:1`).
    pub faint: Color,
    /// Resting muted icon ink (`>= 4.5:1`).
    pub icon_muted: Color,
    /// Primary interactive accent (`base0D`, `>= 3.0:1` fill).
    pub accent: Color,
    /// Text-grade accent (`base0D`, `>= 4.5:1`) for links and highlights.
    pub accent_text: Color,
    /// High-contrast ink over filled `accent` controls.
    pub on_accent: Color,
    /// Steer mode, `$skill` chips, and extra-high effort accent (`base09`).
    pub steer: Color,
    /// Remote environment badge accent (`base0C`).
    pub remote: Color,
    /// Informational banner, question drawer, and `@file` chip accent
    /// (`base0C`).
    pub info: Color,
    /// Delegated subagent threads, merged PRs, and `/command` accents
    /// (`base0E`).
    pub delegated: Color,
    /// Destructive actions, errors, stop CTA, and `-N` diff stats (`base08`).
    pub danger: Color,
    /// High-contrast ink over filled `danger` controls.
    pub on_danger: Color,
    /// Positive statuses, open PRs, and `+N` diff stats (`base0B`).
    pub success: Color,
    /// Pending approvals, favourites star, and warnings (`base0A`).
    pub warning: Color,
    /// Added diff line background wash.
    pub added: Color,
    /// Added diff inline word-change wash.
    pub added_word: Color,
    /// Removed diff line background wash.
    pub removed: Color,
    /// Removed diff inline word-change wash.
    pub removed_word: Color,
    /// Nine scheme-harmonized, contrast-verified colours for project monograms.
    pub project_palette: [Color; 9],
    /// Syntax highlighting palette for fenced code blocks and diffs.
    pub syntax: SyntaxTheme,
    /// Home screen hero headline (`30/36`, weight `400`).
    pub hero: TextStyle,
    /// Narrow viewport hero headline (`24/32`, weight `400`).
    pub hero_narrow: TextStyle,
    /// Modal dialog title (`20/26`, weight `600`).
    pub dialog_title: TextStyle,
    /// Settings and panel section heading (`16/24`, weight `600`).
    pub section_heading: TextStyle,
    /// Header breadcrumb, card, and active row title (`14/20`, weight `600`).
    pub title: TextStyle,
    /// Assistant prose, user messages, and composer text (`14/22.75`, weight
    /// `400`).
    pub body: TextStyle,
    /// Standard UI controls, menus, and buttons (`14/20`, weight `400`).
    pub ui: TextStyle,
    /// Emphasized UI labels and active tabs (`14/20`, weight `520`).
    pub ui_medium: TextStyle,
    /// Dense sidebar thread titles and details card rows (`13/18`, weight
    /// `500`).
    pub panel_row: TextStyle,
    /// Metadata, descriptions, tooltips, and context strip (`12/16`, weight
    /// `400`).
    pub small: TextStyle,
    /// Section labels, pills, and compact button labels (`12/16`, weight
    /// `520`).
    pub small_medium: TextStyle,
    /// System dividers, micro controls, and hunk labels (`11/15`, weight
    /// `400`).
    pub micro: TextStyle,
    /// Emphasized micro labels (`11/15`, weight `500`).
    pub micro_medium: TextStyle,
    /// Tiny status tags and ref badges (`10/12`, weight `500`).
    pub nano: TextStyle,
    /// Code blocks, diffs, and inline code (`12/18`, tabular numerals).
    pub mono: TextStyle,
    /// Tool inspector output and diff stat counts (`11/16`, tabular numerals).
    pub mono_small: TextStyle,
    /// Compact tabular counts and keycaps (`10/14`, weight `450`).
    pub mono_micro: TextStyle,
}

/// `[theme]` in `desktop.toml`: a base16 scheme per appearance, by built-in
/// name, inline `colors`, or a scheme `file`.
#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
pub struct ThemeConfig(toml::Table);

static THEMES: OnceLock<(Theme, Option<Theme>)> = OnceLock::new();

/// Builds both appearances from `config`, once, before the window opens.
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

fn builtin(name: &str) -> Scheme {
    Scheme::builtin(name).unwrap_or_else(|e| unreachable!("built-in scheme {name}: {e}"))
}

impl Theme {
    /// Active theme for the window's appearance (`dark` vs `light`).
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

    fn new(scheme: &Scheme) -> Self {
        palette::build_theme(scheme)
    }

    /// Flat base16 tooltip appearance built via `rune_kit::TooltipStyle` for
    /// `Cx::help_look`.
    pub fn help_look(&self) -> HelpLook {
        rune_kit::TooltipStyle::from_kit(&self.overlay, rune_kit::TooltipAppearance::Compact)
            .radius(RADIUS_SM)
            .font(self.small)
            .help_look()
    }

    /// Assistant prose typography and flat code block styling.
    pub fn markdown(&self) -> MarkdownStyle {
        let heading = |size: f32, lh: f32| TextStyle {
            size,
            weight: 600.0,
            line_height: lh,
            ..self.body
        };
        MarkdownStyle::from_kit(&self.k)
            .body_font(self.body)
            .heading_font(1, heading(19.0, 28.0))
            .heading_font(2, heading(16.0, 24.0))
            .heading_font(3, heading(14.0, 22.0))
            .code_block_font(TextStyle {
                line_height: 19.0,
                ..self.mono
            })
            .code_block(self.surface, self.border_control)
            .syntax_theme(self.syntax)
            .code_block_padding(CARD_INSET, CARD_INSET)
            .inline_code_font(0.86, 400.0)
            .code_margin(S12, S16)
            .quote_rule(self.hairline)
            .rule(self.hairline)
            .link(self.accent_text)
            .strong_weight(600.0)
            .list(S24, S6)
    }
}

/// Formats milliseconds as `320ms`, `4.2s`, or `2m 5s`.
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

/// Progress fraction drawn at UI precision without losing range for token
/// counts.
pub fn fraction(used: u64, size: u64) -> f32 {
    const STEPS: u16 = 1_000;
    let steps =
        u16::try_from(u128::from(used.min(size)) * u128::from(STEPS) / u128::from(size.max(1)))
            .unwrap_or_default();
    f32::from(steps) / f32::from(STEPS)
}

/// Current wall-clock timestamp in milliseconds since Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|now| i64::try_from(now.as_millis()).ok())
        .unwrap_or_default()
}

/// Compact relative age (`now`, `2m`, `3h`, `4d`, `2w`) for sidebar rows.
pub fn relative_age(updated_at_ms: i64, now: i64) -> Option<String> {
    if updated_at_ms <= 0 {
        return None;
    }
    let secs = now.saturating_sub(updated_at_ms).max(0) / 1_000;
    let label = match secs {
        0..60 => "now".to_owned(),
        60..3_600 => format!("{}m", secs / 60),
        3_600..86_400 => format!("{}h", secs / 3_600),
        86_400..604_800 => format!("{}d", secs / 86_400),
        _ => format!("{}w", secs / 604_800),
    };
    Some(label)
}
