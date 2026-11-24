//! Spacing, radii, sizing, and layout geometry tokens.
//!
//! Views read these through `ui` components; bare layout constants in feature
//! views are bugs.

/// Half-step optical alignment spacing (2pt).
pub const S2: f32 = 2.0;
/// Base grid step (4pt).
pub const S4: f32 = 4.0;
/// Half-step compact spacing (6pt).
pub const S6: f32 = 6.0;
/// Standard tight gap (8pt).
pub const S8: f32 = 8.0;
/// Compact horizontal control padding (10pt).
pub const S10: f32 = 10.0;
/// Standard card and panel inset (12pt).
pub const S12: f32 = 12.0;
/// Large control horizontal padding (14pt).
pub const S14: f32 = 14.0;
/// Dialog inset and section rhythm (16pt).
pub const S16: f32 = 16.0;
/// Reading lane horizontal padding (20pt).
pub const S20: f32 = 20.0;
/// Small control height and major section spacing (24pt).
pub const S24: f32 = 24.0;
/// Compact control height (28pt).
pub const S28: f32 = 28.0;
/// Standard control height (32pt).
pub const S32: f32 = 32.0;
/// Tall sidebar row height and pane travel distance (40pt).
pub const S40: f32 = 40.0;
/// Topbar height and provider rail width (44pt).
pub const S44: f32 = 44.0;

/// Single-pixel hairline stroke width.
pub const HAIRLINE: f32 = 1.0;
/// Compact icon size for badges, chips, and inline indicators (12pt).
pub const ICON_SMALL: f32 = 12.0;
/// Medium icon size for dense controls and tree rows (14pt).
pub const ICON_MD: f32 = 14.0;
/// Standard Lucide and harness icon size (16pt).
pub const ICON: f32 = 16.0;
/// Status dot diameter (6pt).
pub const DOT: f32 = 6.0;

/// Keycaps, tiny icon highlights, and progress fills (4pt).
pub const RADIUS_XS: f32 = S4;
/// Menu/list items, badges, inline code, and small chips (6pt).
pub const RADIUS_SM: f32 = S6;
/// Standard buttons, inputs, triggers, code blocks, and small popovers (8pt).
pub const RADIUS_CONTROL: f32 = S8;
/// Thread cards, hover cards, and toasts (12pt).
pub const RADIUS_CARD: f32 = S12;
/// Composer shell, tucked drawers, user bubbles, dialogs, and palette (16pt).
pub const RADIUS_SURFACE: f32 = S16;
/// Full circular / pill corner radius.
pub const RADIUS_PILL: f32 = 9999.0;

/// Squircle corner radius (`25%` of `side`, rounded to whole points) for
/// project monogram tiles and favicons.
pub fn squircle_radius(side: f32) -> f32 {
    (side * 0.25).round()
}

/// Shell title band height shared by `SplitView` and its sidebar toggle.
pub const TOOLBAR_HEIGHT: f32 = rune_kit::shell::TOOLBAR_HEIGHT;
/// Horizontal padding at the ends of the toolbar band.
pub const TOOLBAR_PAD: f32 = S12;

/// Sidebar pane horizontal padding where row fills start and end.
pub const SIDEBAR_PAD: f32 = S8;
/// Leading inset for sidebar content, section titles, and footer.
pub const SIDEBAR_INSET: f32 = S16;

/// Standard reading lane width for timeline and composer (736pt / `46rem`).
pub const CHAT_CONTENT_MAX_WIDTH: f32 = 736.0;
/// Horizontal padding around the reading lane (20pt).
pub const CHAT_LANE_PADDING_X: f32 = S20;
/// Reading column width used by existing transcript and composer views.
pub const READING_WIDTH: f32 = CHAT_CONTENT_MAX_WIDTH;
/// Side inset around the reading column.
pub const READING_INSET: f32 = CHAT_LANE_PADDING_X;

/// Command palette maximum width (576pt).
pub const PALETTE_MAX_WIDTH: f32 = 576.0;
/// Existing palette sheet width alias.
pub const PALETTE_WIDTH: f32 = PALETTE_MAX_WIDTH;

/// Standard modal dialog width (512pt).
pub const DIALOG_MAX_WIDTH: f32 = 512.0;
/// Padding inside dialogs.
pub const DIALOG_INSET: f32 = S16;

/// Model picker body height (346pt).
pub const MODEL_PICKER_BODY_HEIGHT: f32 = 346.0;
/// Model picker left harness rail width (44pt).
pub const MODEL_PICKER_RAIL_WIDTH: f32 = S44;
/// Model picker right reasoning/options column width (192pt).
pub const MODEL_PICKER_OPTIONS_WIDTH: f32 = 192.0;

/// Horizontal inset on each side of a tucked composer drawer (22pt).
pub const COMPOSER_DRAWER_INSET: f32 = 22.0;
/// Bottom overlap tucking a composer drawer behind the composer hairline
/// (17pt).
pub const COMPOSER_DRAWER_OVERLAP: f32 = 17.0;

/// Padding inside cards, tool outputs, and plans (12pt).
pub const CARD_INSET: f32 = S12;
/// Vertical offset for centered empty states in the transcript.
pub const EMPTY_TOP: f32 = S32 * 4.0;
/// Maximum text measure for centered empty states.
pub const EMPTY_WIDTH: f32 = 360.0;

/// Control size ladder (`16`, `20`, `24`, `28`, `32`, `36`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Size {
    /// 16pt icon-only hit target (`icon-tiny`).
    Tiny,
    /// 20pt micro control (`micro` / `icon-micro`).
    Micro,
    /// 24pt dense row action and inline control (`xs` / `icon-xs`).
    Small,
    /// 28pt compact toolbar and composer control (`sm` / `icon-sm`).
    Compact,
    /// 32pt standard desktop button, input, and trigger (`default`).
    #[default]
    Default,
    /// 36pt prominent primary action and search field (`lg`).
    Large,
}

impl Size {
    /// Control height in points.
    pub const fn height(self) -> f32 {
        match self {
            Self::Tiny => S16,
            Self::Micro => S20,
            Self::Small => S24,
            Self::Compact => S28,
            Self::Default => S32,
            Self::Large => 36.0,
        }
    }

    /// Corner radius in points.
    pub const fn radius(self) -> f32 {
        match self {
            Self::Tiny => RADIUS_XS,
            Self::Micro | Self::Small => RADIUS_SM,
            Self::Compact | Self::Default | Self::Large => RADIUS_CONTROL,
        }
    }

    /// Icon glyph size suited to this control height.
    pub const fn icon_size(self) -> f32 {
        match self {
            Self::Tiny | Self::Micro => ICON_SMALL,
            Self::Small | Self::Compact => ICON_MD,
            Self::Default | Self::Large => ICON,
        }
    }

    /// Nearest `rune_kit::ControlSize` height for shell widgets
    /// (`SidebarToggle`).
    pub const fn kit(self) -> rune_kit::ControlSize {
        match self {
            Self::Tiny => rune_kit::ControlSize::Mini,
            Self::Micro => rune_kit::ControlSize::Small,
            Self::Small => rune_kit::ControlSize::Regular,
            Self::Compact | Self::Default => rune_kit::ControlSize::Large,
            Self::Large => rune_kit::ControlSize::ExtraLarge,
        }
    }
}
