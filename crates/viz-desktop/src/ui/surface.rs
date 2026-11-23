//! Cards, flat popovers, combobox shells, tucked drawers, scroll viewports,
//! and inline pickers.

use rune::{El, ScrollFade, ScrollSpec, col, row};
use rune_kit::{Menu, MenuItem, Placement, ScrollBar, ScrollBarStyle};

use super::rule;
use crate::{
    motion,
    theme::{
        CARD_INSET, COMPOSER_DRAWER_INSET, COMPOSER_DRAWER_OVERLAP, DECK_INSET, DECK_OVERLAP,
        HAIRLINE, RADIUS_CARD, RADIUS_DECK, RADIUS_SURFACE, S4, S6, S8, S12, S16, Theme,
    },
};

/// The model picker's trigger max width.
pub const MODEL_TRIGGER_MAX: f32 = 240.0;
/// A menu's narrowest width.
pub const MENU_WIDTH: f32 = 196.0;

/// Flat base16 container card variants (zero blur, zero drop shadows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    /// Elevated surface with a hairline: tool output, composer shell, panels.
    Surface,
    /// Hairline-only container: plans, subagents, compact callouts.
    Outline,
    /// Quiet wash behind user messages.
    Message,
    /// Quiet inset well: filter wells, provider rails, preview mats.
    Recessed,
    /// Dashed outline for queued messages or empty drop slots.
    Dashed,
}

impl Card {
    /// The surface alone, for compound cards whose children own their insets.
    pub fn frame(self, t: &Theme) -> El {
        match self {
            Self::Surface => col()
                .w_full()
                .radius(RADIUS_CARD)
                .fill(t.surface)
                .border(HAIRLINE, t.hairline),
            Self::Outline => col()
                .w_full()
                .radius(RADIUS_CARD)
                .border(HAIRLINE, t.hairline),
            Self::Message => col().w_full().radius(RADIUS_CARD).fill(t.mine),
            Self::Recessed => col()
                .w_full()
                .radius(RADIUS_CARD)
                .fill(t.surface_recessed)
                .border(HAIRLINE, t.hairline),
            Self::Dashed => col()
                .w_full()
                .radius(RADIUS_CARD)
                .fill(t.surface_recessed)
                .border(HAIRLINE, t.border_dashed)
                .border_dash(4.0, 3.0),
        }
    }

    /// The surface with [`CARD_INSET`] padding and [`S8`] vertical gap.
    pub fn build(self, t: &Theme) -> El {
        self.frame(t).padding(CARD_INSET).gap(S8)
    }
}

/// Flat opaque floating surface for popovers, context menus, and details cards
/// (`t.surface_raised` + `1px` `t.border_overlay`, zero drop shadow).
pub fn popover_frame(key: impl Into<String>, width: f32, content: El, t: &Theme) -> El {
    col()
        .key(key)
        .w(width)
        .radius(RADIUS_SURFACE)
        .fill(t.surface_raised)
        .border(HAIRLINE, t.border_overlay)
        .clip_rounded(RADIUS_SURFACE)
        .child(content)
}

/// Anchors a flat [`popover_frame`] to `anchor_key` using
/// [`motion::anchored_popover`] with viewport flipping and `SNAP_PAIR` motion.
pub fn anchored_popover(
    key: impl Into<String>,
    anchor_key: &str,
    placement: Placement,
    width: f32,
    content: El,
    t: &Theme,
) -> El {
    let frame = popover_frame(key, width, content, t);
    motion::anchored_popover(frame, anchor_key, placement, S4)
}

/// Reusable flat combobox / command-palette shell: optional plain search
/// header, hairline rule, instant-update scrollable list with linear fades,
/// empty or loading fallback, and optional footer.
pub struct ComboboxShell {
    key: String,
    width: Option<f32>,
    max_height: f32,
    search: Option<El>,
    items: Vec<El>,
    empty: Option<El>,
    footer: Option<El>,
}

/// Starts a flat [`ComboboxShell`] keyed by `key`.
pub fn combobox_shell(key: impl Into<String>) -> ComboboxShell {
    ComboboxShell {
        key: key.into(),
        width: Some(280.0),
        max_height: 280.0,
        search: None,
        items: Vec::new(),
        empty: None,
        footer: None,
    }
}

impl ComboboxShell {
    /// Sets the shell's fixed width in points (default `280pt`).
    pub const fn width(mut self, points: f32) -> Self {
        self.width = Some(points);
        self
    }

    /// Stretches the shell to fill its parent container width.
    pub const fn fill_width(mut self) -> Self {
        self.width = None;
        self
    }

    /// Sets the scrollable list's maximum height in points (default `280pt`).
    pub const fn max_height(mut self, points: f32) -> Self {
        self.max_height = points;
        self
    }

    /// Sets the top search input slot (typically [`super::SearchInput`] with
    /// [`super::SearchStyle::Plain`]).
    pub fn search(mut self, input: El) -> Self {
        self.search = Some(input);
        self
    }

    /// Populates the scrollable option rows.
    pub fn items(mut self, items: impl IntoIterator<Item = El>) -> Self {
        self.items = items.into_iter().collect();
        self
    }

    /// Fallback content shown when `items` is empty.
    pub fn empty(mut self, fallback: El) -> Self {
        self.empty = Some(fallback);
        self
    }

    /// Optional sticky footer separated by a hairline (e.g., effort controls or
    /// keybind hints).
    pub fn footer(mut self, footer: El) -> Self {
        self.footer = Some(footer);
        self
    }

    /// Builds the flat combobox shell against `t`.
    pub fn build(self, t: &Theme) -> El {
        let mut shell = col()
            .key(&self.key)
            .radius(RADIUS_SURFACE)
            .fill(t.surface_raised)
            .border(HAIRLINE, t.border_overlay)
            .clip_rounded(RADIUS_SURFACE);
        shell = match self.width {
            Some(points) => shell.w(points),
            None => shell.w_full(),
        };

        if let Some(search) = self.search {
            shell = shell
                .child(row().w_full().px(S6).py(S4).child(search))
                .child(rule(t));
        }

        if self.items.is_empty() {
            if let Some(empty) = self.empty {
                shell = shell.child(row().w_full().padding(S12).center().child(empty));
            }
        } else {
            let list = col()
                .key(format!("{}-list", self.key))
                .role("listbox")
                .w_full()
                .padding(S4)
                .gap(2.0)
                .instant_changes()
                .children(self.items);
            let viewport = col()
                .key(format!("{}-scroll", self.key))
                .w_full()
                .max_h(self.max_height)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .scroll_fade(ScrollFade::strength(S16).linear())
                .scroll_margin(S8)
                .child(list)
                .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.overlay));
            shell = shell.child(viewport);
        }

        if let Some(footer) = self.footer {
            shell = shell
                .child(rule(t))
                .child(row().w_full().px(S8).py(S6).child(footer));
        }
        shell
    }
}

/// Flat drawer tucked cleanly behind the composer's top border (`@`/`$`/`/`
/// completion menus, approvals, questions, queued prompts).
pub fn tucked_drawer(key: impl Into<String>, order: i16, body: El, t: &Theme) -> El {
    col()
        .key(key)
        .paint_order(order)
        .mx(COMPOSER_DRAWER_INSET)
        .mb(-COMPOSER_DRAWER_OVERLAP)
        .pb(COMPOSER_DRAWER_OVERLAP)
        .corners([RADIUS_CARD, RADIUS_CARD, 0.0, 0.0])
        .fill(t.surface_drawer)
        .border(HAIRLINE, t.hairline)
        .transition(motion::tucked_menu())
        .child(body)
}

/// A flat deck tucked behind the top edge of the composer below it, with inset
/// horizontal width and a crisp hairline border.
pub fn tucked_deck(key: impl Into<String>, order: i16, fill: rune::Color, t: &Theme) -> El {
    col()
        .key(key)
        .paint_order(order)
        .mx(DECK_INSET)
        .mb(-DECK_OVERLAP)
        .pb(DECK_OVERLAP)
        .corners([RADIUS_DECK, RADIUS_DECK, 0.0, 0.0])
        .fill(fill)
        .border(HAIRLINE, t.hairline)
}

/// Vertical scroll behavior for [`scroll_viewport`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScrollMode {
    /// Standard top-anchored list or document scroll with overflow anchoring.
    #[default]
    Top,
    /// Chat/transcript scroll that stays pinned to the bottom while streaming.
    StickBottom,
}

/// Standard scroll viewport with linear `16pt` scroll edge fades, scroll
/// anchoring, `12pt` reveal margin, and themed overlay scrollbar.
pub fn scroll_viewport(key: impl Into<String>, mode: ScrollMode, content: El, t: &Theme) -> El {
    col()
        .key(key)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: mode == ScrollMode::StickBottom,
        })
        .scroll_anchor()
        .scroll_fade(ScrollFade::strength(S16).linear())
        .scroll_margin(S12)
        .child(content)
        .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k))
}

/// Appends an iterator of [`MenuItem`]s onto `menu`.
pub fn menu_items(menu: Menu, items: impl IntoIterator<Item = MenuItem>) -> Menu {
    items.into_iter().fold(menu, Menu::item)
}
