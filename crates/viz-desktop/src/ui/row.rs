//! Interactive list rows for menus, comboboxes, command palette, and details
//! cards.

use rune::{Color, El, Icon, Tx, col, icon, row, text};

use crate::theme::{HAIRLINE, ICON, RADIUS_XS, S2, S8, Theme};

type ElHook = Box<dyn FnOnce(El) -> El>;

/// Vertical row density for [`ListRow`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RowDensity {
    /// Dense single-line row (`28pt` — sidebar filters, compact menus, branch
    /// pickers).
    Compact,
    /// Standard single-line row (`32pt` — model rows, comboboxes, command
    /// palette).
    #[default]
    Default,
    /// Two-line row (`44pt` — title + quiet subtitle).
    Tall,
}

impl RowDensity {
    const fn height(self) -> f32 {
        match self {
            Self::Compact => 28.0,
            Self::Default => 32.0,
            Self::Tall => 44.0,
        }
    }
}

/// Interactive list item for menus, comboboxes, command palette, and sidebar
/// lists.
///
/// Always truncates long titles and subtitles with `.help_when_truncated(...)`
/// so trailing badges, shortcuts, or actions never clip or overflow. Supports
/// `focus_ring_for(input_key)` so keyboard-highlighted combobox rows show their
/// focus ring while focus stays in the search input.
pub struct ListRow {
    key: String,
    title: String,
    subtitle: Option<String>,
    density: RowDensity,
    leading: Option<El>,
    leading_icon: Option<&'static Icon>,
    trailing: Option<El>,
    detail: Option<String>,
    selected: bool,
    highlighted: bool,
    disabled: bool,
    focus_for: Option<String>,
    press: Option<ElHook>,
}

/// Starts a [`ListRow`] keyed by `key` with primary `title`.
pub fn list_row(key: impl Into<String>, title: impl Into<String>) -> ListRow {
    ListRow {
        key: key.into(),
        title: title.into(),
        subtitle: None,
        density: RowDensity::Default,
        leading: None,
        leading_icon: None,
        trailing: None,
        detail: None,
        selected: false,
        highlighted: false,
        disabled: false,
        focus_for: None,
        press: None,
    }
}

impl ListRow {
    /// Adds a quiet truncated second line and switches density to
    /// [`RowDensity::Tall`] unless overridden.
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        if self.density == RowDensity::Default {
            self.density = RowDensity::Tall;
        }
        self
    }

    /// Sets [`RowDensity`] (`Compact`, `Default`, or `Tall`).
    pub const fn density(mut self, density: RowDensity) -> Self {
        self.density = density;
        self
    }

    /// Places a custom leading element (such as a project monogram or harness
    /// mark).
    pub fn leading(mut self, el: El) -> Self {
        self.leading = Some(el);
        self
    }

    /// Places a leading Lucide icon that automatically shifts ink on hover and
    /// selection.
    pub const fn leading_icon(mut self, glyph: &'static Icon) -> Self {
        self.leading_icon = Some(glyph);
        self
    }

    /// Places a trailing element (badge, keycap group, or action button).
    pub fn trailing(mut self, el: El) -> Self {
        self.trailing = Some(el);
        self
    }

    /// Places quiet right-aligned metadata text (`t.mono_micro`, `t.faint`).
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Marks the row as the chosen/active item (`t.selected_subtle` or
    /// `t.selected` fill and strong ink).
    pub const fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Marks the row as keyboard-highlighted (`t.selected` fill, e.g., active
    /// descendant in a combobox or palette).
    pub const fn highlighted(mut self, highlighted: bool) -> Self {
        self.highlighted = highlighted;
        self
    }

    /// Draws the keyboard focus ring on this row whenever the input keyed
    /// `input_key` holds keyboard focus (`El::focus_ring_for`).
    pub fn focus_ring_for(mut self, input_key: impl Into<String>) -> Self {
        self.focus_for = Some(input_key.into());
        self
    }

    /// Disables activation and dims the row.
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Registers a click/activation callback.
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.press = Some(Box::new(move |el| el.on_press(f)));
        self
    }

    /// Registers a transaction-aware click/activation callback.
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.press = Some(Box::new(move |el| el.on_press_tx(f)));
        self
    }

    /// Builds the list row element against `t`.
    pub fn build(self, t: &Theme) -> El {
        let active = self.selected || self.highlighted;
        let fill = if self.highlighted {
            t.selected
        } else if self.selected {
            t.selected_subtle
        } else {
            Color::TRANSPARENT
        };
        let title_color = if self.disabled {
            t.faint
        } else if active {
            t.text_strong
        } else {
            t.text
        };
        let icon_color = if active { t.text } else { t.icon_muted };

        let mut title = text(&self.title)
            .font(if self.density == RowDensity::Compact {
                t.panel_row
            } else if active {
                t.ui_medium
            } else {
                t.ui
            })
            .color(title_color)
            .truncate()
            .help_when_truncated(&self.title);
        if !self.disabled {
            title = title.group_color(t.text_strong);
        }

        let mut labels = col()
            .min_w(0.0)
            .shrink()
            .grow(1.0)
            .gap(S2)
            .a11y_hidden()
            .child(title);
        if let Some(subtitle) = &self.subtitle {
            labels = labels.child(
                text(subtitle)
                    .font(t.small)
                    .color(t.muted)
                    .truncate()
                    .help_when_truncated(subtitle),
            );
        }

        let mut item = row()
            .key(&self.key)
            .role("option")
            .label(&self.title)
            .focus_selected(self.selected)
            .w_full()
            .h(self.density.height())
            .px(S8)
            .gap(S8)
            .center_y()
            .radius(RADIUS_XS)
            .fill(fill)
            .disabled(self.disabled);

        if let Some(subtitle) = &self.subtitle {
            item = item.description(subtitle);
        }
        if !self.disabled {
            item = item.hover_group();
            if !self.highlighted {
                item = item.hover(t.hover_wash).press(t.pressed_wash);
            }
            if let Some(input_key) = self.focus_for {
                if self.highlighted {
                    item = item.focus_ring_for(input_key, HAIRLINE, -HAIRLINE, t.accent);
                }
            } else {
                item = item
                    .focusable(true)
                    .focus_ring(HAIRLINE, -HAIRLINE, t.accent);
            }
        }
        if let Some(leading) = self.leading {
            item = item.child(leading.a11y_hidden());
        } else if let Some(glyph) = self.leading_icon {
            let mut mark = icon(glyph, ICON).color(icon_color).a11y_hidden();
            if !self.disabled {
                mark = mark.group_color(t.text);
            }
            item = item.child(mark);
        }
        item = item.child(labels);
        if let Some(detail) = self.detail {
            item = item.child(text(detail).font(t.mono_micro).color(t.faint).a11y_hidden());
        }
        if let Some(trailing) = self.trailing {
            item = item.child(trailing);
        }
        if self.disabled {
            item = item.layer_opacity(0.50);
        } else if let Some(press) = self.press {
            item = press(item);
        }
        item
    }
}
