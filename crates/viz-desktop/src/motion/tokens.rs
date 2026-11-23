//! Spring, travel, scale, stagger, and shimmer constants for `viz-desktop`.

use rune::{Spring, SpringPair};

/// Fast critically damped spring for hover entry, rapid exits, and title exits.
pub const INSTANT: Spring = Spring::new(0.10, 0.0);
/// Calm pointer-leave ink and wash fade across the window root.
pub const HOVER_OUT: Spring = Spring::new(0.12, 0.0);
/// Crisp control and micro-overlay spring (icon morphs, rail pill, tucked
/// menus).
pub const QUICK: Spring = Spring::new(0.14, 0.0);
/// Standard overlay, chevron, and rolling-counter spring.
pub const SNAP: Spring = Spring::new(0.20, 0.0);
/// Calm structural settle spring (height disclosures, thread switches, docks,
/// toasts).
pub const SETTLE: Spring = Spring::new(0.24, 0.0);
/// List reorder (FLIP), sibling step-aside, and drag-drop settle spring.
pub const REFLOW: Spring = Spring::new(0.28, 0.0);
/// Large sliding panel and hero-to-thread dock spring.
pub const GLIDE: Spring = Spring::new(0.28, 0.0);

/// Asymmetric enter/exit pair for compact menus, icon morphs, and tucked
/// completion drawers.
pub const QUICK_PAIR: SpringPair = SpringPair::new(QUICK, INSTANT);
/// Asymmetric enter/exit pair for dialogs, popovers, command palette, and
/// details cards.
pub const SNAP_PAIR: SpringPair = SpringPair::new(SNAP, QUICK);
/// Asymmetric enter/exit pair for the toolbar thread title so outgoing ink
/// clears first.
pub const TITLE_PAIR: SpringPair = SpringPair::new(SNAP, INSTANT);
/// Asymmetric enter/exit pair for transcript switches, Tasks Dock, banners, and
/// toasts.
pub const SETTLE_PAIR: SpringPair = SpringPair::new(SETTLE, Spring::new(0.18, 0.0));
/// Asymmetric enter/exit pair for entering/exiting list rows.
pub const REFLOW_PAIR: SpringPair = SpringPair::new(REFLOW, SNAP);

/// Micro travel distance for popovers, menus, and tooltips (points).
pub const TRAVEL_MICRO: f32 = 4.0;
/// Short upward emergence distance for tucked `@`/`$`/`/` completion menus
/// (points).
pub const TRAVEL_DRAWER: f32 = 6.0;
/// Compact vertical travel for toasts, Tasks Dock, composer banners, dialogs,
/// and rows (points).
pub const TRAVEL_SMALL: f32 = 12.0;
/// Toolbar title travel distance inside its `TOOLBAR_HEIGHT` clip (points).
pub const TRAVEL_TITLE: f32 = 24.0;
/// Transcript page travel distance on directional thread switches (points).
pub const TRAVEL_PANE: f32 = 40.0;

/// Tactile press scale for buttons (`El::press_scale`; auto-disabled under
/// Reduce Motion).
pub const PRESS_SCALE: f32 = 0.97;
/// Subtle enter/exit scale for dialogs, palettes, and popovers.
pub const OVERLAY_SCALE: f32 = 0.97;
/// In-place icon crossfade scale inside `El::dissolve` (above Rune's `0.5`
/// small-scale floor).
pub const MORPH_SCALE: f32 = 0.65;
/// Soft edge feather painted on the moving boundary of `fold_glide` (drops to
/// `0` at rest).
pub const FOLD_FEATHER: f32 = 12.0;

/// Delay before a new toolbar title enters so the outgoing title clears without
/// overlapping.
pub const TITLE_ENTER_DELAY: f32 = 0.08;
/// Delay before showing a thread loading caption so fast local loads never
/// flash a spinner.
pub const LOADING_DELAY: f32 = 0.40;
/// Per-item stagger interval for small list additions (seconds).
pub const STAGGER_STEP: f32 = 0.025;
/// Maximum staggered item rank (`150ms` total cap).
pub const STAGGER_MAX_ITEMS: usize = 6;
/// Row counts above this threshold skip per-row stagger and enter together.
pub const STAGGER_BULK_LIMIT: usize = 12;

/// Linear sweep period for live thinking and running tool shimmer bands
/// (seconds).
pub const SHIMMER_PERIOD: f32 = 1.8;

/// Reference width for `PaneMotion::scale_with_width` (points).
pub(super) const PANE_REF_WIDTH: f32 = 360.0;
/// Shortest width-scaled pane spring duration (seconds).
pub(super) const PANE_MIN_DURATION: f32 = 0.18;
/// Longest width-scaled pane spring duration (seconds).
pub(super) const PANE_MAX_DURATION: f32 = 0.30;
/// Trailing closed fraction across which pane contents fade.
pub(super) const PANE_FADE_TAIL: f32 = 0.15;
/// Content fade duration for panes under Reduce Motion (seconds).
pub(super) const PANE_REDUCED_FADE: f32 = 0.12;

/// Pointer travel in points before a press on a reorderable row becomes a drag.
pub(super) const DRAG_SLOP: f32 = 4.0;
