//! Empty states, inline status/error banners, delayed loading captions, and
//! shimmer skeleton rows.

use rune::{Align, El, Icon, Tx, col, el, icon, row, text};
use rune_kit::IconButtonVariant;

use super::{
    badge::Tone,
    icon_button::icon_button,
    icons::{CIRCLE_ALERT, CIRCLE_CHECK, CLOSE, INFO, TRIANGLE_ALERT},
    radar,
};
use crate::{
    motion,
    theme::{
        EMPTY_WIDTH, HAIRLINE, ICON, ICON_MD, ICON_SMALL, RADIUS_CONTROL, S4, S6, S8, S12, Size,
        Theme, squircle_radius,
    },
};

/// Simple centered empty state: optional mark, title, detail, and optional
/// action.
pub fn empty(mark: Option<El>, title: &str, detail: &str, action: Option<El>, t: &Theme) -> El {
    let mut body = col()
        .key("empty")
        .w_full()
        .max_w(EMPTY_WIDTH)
        .gap(S8)
        .items_center();
    if let Some(mark) = mark {
        body = body.child(mark.mb(S4));
    }
    body = body.child(text(title).font(t.title).color(t.text));
    if !detail.is_empty() {
        body = body.child(
            text(detail)
                .font(t.ui)
                .color(t.muted)
                .wrap()
                .align(Align::Center),
        );
    }
    body.children(action.map(|action| action.mt(S8)))
}

/// Structured empty state with an optional squircle icon tile, title, muted
/// description, and primary/secondary actions.
pub struct EmptyState {
    title: String,
    detail: Option<String>,
    icon: Option<&'static Icon>,
    action: Option<El>,
    secondary: Option<El>,
    compact: bool,
}

/// Starts an [`EmptyState`] with headline `title`.
pub fn empty_state(title: impl Into<String>) -> EmptyState {
    EmptyState {
        title: title.into(),
        detail: None,
        icon: None,
        action: None,
        secondary: None,
        compact: false,
    }
}

impl EmptyState {
    /// Sets a muted explanatory subtitle.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Displays a `36pt` squircle icon tile above the title.
    pub const fn icon(mut self, glyph: &'static Icon) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// Sets the primary call-to-action control.
    pub fn action(mut self, action: El) -> Self {
        self.action = Some(action);
        self
    }

    /// Sets an optional secondary action placed beside the primary action.
    pub fn secondary_action(mut self, action: El) -> Self {
        self.secondary = Some(action);
        self
    }

    /// Uses tighter spacing for sidebars, drawers, and compact lists.
    pub const fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// Builds the empty state with a calm [`motion::fade`] entrance.
    pub fn build(self, t: &Theme) -> El {
        let tile = self.icon.map(|glyph| {
            let side = if self.compact { 32.0 } else { 40.0 };
            let icon_size = if self.compact { ICON } else { ICON_MD };
            row()
                .size(side, side)
                .center()
                .radius(squircle_radius(side))
                .fill(t.surface_recessed)
                .border(HAIRLINE, t.hairline)
                .a11y_hidden()
                .child(icon(glyph, icon_size).color(t.muted))
        });

        let mut body = col()
            .key("empty-state")
            .w_full()
            .max_w(EMPTY_WIDTH)
            .gap(if self.compact { S6 } else { S8 })
            .items_center()
            .transition(motion::fade(motion::SETTLE_PAIR));

        if let Some(tile) = tile {
            body = body.child(tile);
        }
        body = body.child(
            text(self.title)
                .font(if self.compact { t.ui_medium } else { t.title })
                .color(t.text_strong)
                .align(Align::Center),
        );
        if let Some(detail) = self.detail {
            body = body.child(
                text(detail)
                    .font(if self.compact { t.small } else { t.ui })
                    .color(t.muted)
                    .wrap()
                    .align(Align::Center),
            );
        }
        if self.action.is_some() || self.secondary.is_some() {
            let mut actions = row().gap(S8).center_y().mt(S4);
            if let Some(secondary) = self.secondary {
                actions = actions.child(secondary);
            }
            if let Some(primary) = self.action {
                actions = actions.child(primary);
            }
            body = body.child(actions);
        }
        body
    }
}

/// Flat tinted inline status or error banner with semantic icon, title,
/// optional detail, optional action button, and optional dismiss control.
pub struct Banner {
    key: String,
    tone: Tone,
    title: String,
    detail: Option<String>,
    action: Option<El>,
    dismiss: Option<El>,
}

/// Starts a semantic [`Banner`] in `tone`.
pub fn status_banner(key: impl Into<String>, tone: Tone, title: impl Into<String>) -> Banner {
    Banner {
        key: key.into(),
        tone,
        title: title.into(),
        detail: None,
        action: None,
        dismiss: None,
    }
}

/// Starts a [`Tone::Danger`] error banner.
pub fn error_banner(key: impl Into<String>, title: impl Into<String>) -> Banner {
    status_banner(key, Tone::Danger, title)
}

impl Banner {
    /// Adds a quiet secondary detail line.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Adds a trailing action button (such as `Retry`).
    pub fn action(mut self, action: El) -> Self {
        self.action = Some(action);
        self
    }

    /// Adds a trailing dismiss icon button invoking `on_dismiss`.
    pub fn on_dismiss<S: 'static>(
        mut self,
        on_dismiss: impl Fn(&mut S, &mut Tx) + 'static,
        t: &Theme,
    ) -> Self {
        self.dismiss = Some(
            icon_button(
                &CLOSE,
                "Dismiss",
                IconButtonVariant::GhostMuted,
                Size::Micro,
            )
            .on_press_tx(on_dismiss)
            .build(&t.k)
            .key("dismiss"),
        );
        self
    }

    /// Builds the flat tinted banner against `t`.
    pub fn build(self, t: &Theme) -> El {
        let colors = self.tone.colors(t);
        let glyph = match self.tone {
            Tone::Danger => &CIRCLE_ALERT,
            Tone::Warning => &TRIANGLE_ALERT,
            Tone::Success => &CIRCLE_CHECK,
            Tone::Neutral | Tone::Accent | Tone::Info => &INFO,
        };
        let mut copy = col().grow(1.0).shrink().min_w(0.0).gap(2.0).child(
            text(&self.title)
                .font(t.small_medium)
                .color(colors.fg)
                .wrap(),
        );
        if let Some(detail) = &self.detail {
            copy = copy.child(text(detail).font(t.small).color(t.muted).wrap());
        }
        let mut banner = row()
            .key(&self.key)
            .role(if self.tone == Tone::Danger {
                "alert"
            } else {
                "status"
            })
            .label(&self.title)
            .w_full()
            .px(S12)
            .py(S8)
            .gap(S8)
            .items_start()
            .radius(RADIUS_CONTROL)
            .fill(colors.bg)
            .transition(motion::row_enter(motion::TRAVEL_MICRO))
            .child(
                icon(glyph, ICON_SMALL)
                    .color(colors.fg)
                    .mt(2.0)
                    .a11y_hidden(),
            )
            .child(copy);
        if let Some(action) = self.action {
            banner = banner.child(action);
        }
        if let Some(dismiss) = self.dismiss {
            banner = banner.child(dismiss);
        }
        banner
    }
}

/// Delay mode for [`loading_state`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LoadingDelay {
    /// Delays appearance by [`motion::LOADING_DELAY`] (`400ms`) so fast local
    /// loads never flash a spinner.
    #[default]
    Deferred,
    /// Displays immediately (e.g., long-running actions or galleries).
    Immediate,
}

/// Centered loading indicator with radar spinner and muted caption.
pub fn loading_state(label: &str, detail: Option<&str>, delay: LoadingDelay, t: &Theme) -> El {
    let mut stack = col()
        .key("loading-state")
        .role("status")
        .label(label)
        .w_full()
        .gap(S6)
        .items_center()
        .transition(motion::fade(motion::SETTLE_PAIR))
        .child(
            row()
                .gap(S8)
                .center_y()
                .child(radar(ICON_SMALL, t.accent, t))
                .child(text(label).font(t.small_medium).color(t.muted)),
        );
    if let Some(detail) = detail {
        stack = stack.child(
            text(detail)
                .w_full()
                .font(t.small)
                .color(t.faint)
                .wrap()
                .align(Align::Center),
        );
    }
    if delay == LoadingDelay::Deferred {
        stack = stack.enter_delay(motion::LOADING_DELAY);
    }
    stack
}

/// Flat placeholder skeleton rows with an optional [`motion::shimmer_band`]
/// sweep (unmounts cleanly when `!active`; freezes under Reduce Motion).
pub fn skeleton_rows(key: &str, count: usize, row_height: f32, active: bool, t: &Theme) -> El {
    const WIDTHS: [f32; 4] = [0.92, 0.76, 0.84, 0.62];
    let mut stack = col().key(key).w_full().gap(S6).a11y_hidden();
    for i in 0..count {
        let frac = WIDTHS[i % WIDTHS.len()];
        let bar = el()
            .key(format!("{key}-{i}"))
            .w_frac(frac)
            .h(row_height)
            .radius(RADIUS_CONTROL)
            .fill(t.surface_recessed)
            .border(HAIRLINE, t.hairline)
            .children(motion::shimmer_band(
                format!("{key}-shimmer-{i}"),
                active,
                t.muted,
                RADIUS_CONTROL,
            ));
        stack = stack.child(bar);
    }
    stack
}
