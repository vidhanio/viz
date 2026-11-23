//! Motion tokens and reusable primitives for `viz-desktop`.
//!
//! Every spring is critically damped (`bounce = 0.0`) so utility motion stays
//! calm and preserves instantaneous position and velocity on mid-flight
//! interruption. Elevation during motion (overlays, popovers, lifted drag
//! cards) uses opaque base16 fills and `1px` hairlines — never drop shadows,
//! blur, or glow.
//!
//! Surfaces map onto these primitives directly:
//! - Presence enter/exit: [`fade`], [`popover`], [`anchored_popover`],
//!   [`dialog`], [`scrim`], [`tucked_deck`], [`tucked_menu`], [`toast`],
//!   [`row_enter`], [`chip_enter`], [`stagger_delay`], [`Switch`].
//! - List reorder / FLIP: [`reflow`], [`reflow_container`].
//! - Height disclosure with scroll anchoring: [`fold_container`], [`fold_row`],
//!   [`toggle_anchored`], [`chevron`].
//! - In-place icon crossfade / morph: [`icon_morph`].
//! - Number roll for counters and diff stats: [`rolling_count`],
//!   [`rolling_diff_stat`].
//! - Live row shimmer: [`shimmer_band`].
//! - Shared-element move: [`shared_pill`], [`shared_surface`], [`launch_from`].
//! - Panel slide: [`pane_motion`], [`stable_content`].
//! - Drag settle: [`DragOrder`], [`drag_row`].

mod presence;
mod tokens;
mod widgets;

#[expect(
    unused_imports,
    reason = "motion tokens and transitions form the shared Phase 1 motion API"
)]
pub use self::{
    presence::{
        Switch, chip_enter, dialog, fade, pane_motion, popover, row_enter, scrim, stable_content,
        stagger_delay, toast, tucked_deck, tucked_menu,
    },
    tokens::{
        FOLD_FEATHER, GLIDE, HOVER_OUT, INSTANT, LOADING_DELAY, MORPH_SCALE, OVERLAY_SCALE,
        PRESS_SCALE, QUICK, QUICK_PAIR, REFLOW, REFLOW_PAIR, SETTLE, SETTLE_PAIR, SHIMMER_PERIOD,
        SNAP, SNAP_PAIR, STAGGER_BULK_LIMIT, STAGGER_MAX_ITEMS, STAGGER_STEP, TITLE_ENTER_DELAY,
        TITLE_PAIR, TRAVEL_DRAWER, TRAVEL_MICRO, TRAVEL_PANE, TRAVEL_SMALL, TRAVEL_TITLE,
    },
    widgets::{
        DragOrder, chevron, drag_row, fold_container, fold_row, icon_morph, launch_from, pressable,
        reflow, reflow_container, rolling_count, rolling_diff_stat, shared_pill, shared_surface,
        shimmer_band, toggle_anchored,
    },
};
