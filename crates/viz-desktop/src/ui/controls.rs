//! Flat base16 toggle switches and segmented choice controls.

use rune::{El, Tx, col, el, row, text};
use rune_kit::{SegmentedControl, State};

use crate::{
    motion,
    theme::{HAIRLINE, RADIUS_PILL, S2, S8, Theme},
};

type ElHook = Box<dyn FnOnce(El) -> El>;

/// Strictly flat base16 toggle switch (zero gradients, zero knob shadows) with
/// spring thumb glide on [`motion::QUICK`].
pub struct Switch {
    checked: bool,
    label: Option<String>,
    description: Option<String>,
    disabled: bool,
    press: Option<ElHook>,
}

/// Starts a flat [`Switch`] in the `checked` state.
pub const fn switch(checked: bool) -> Switch {
    Switch {
        checked,
        label: None,
        description: None,
        disabled: false,
        press: None,
    }
}

impl Switch {
    /// Visible label placed beside the toggle track (also sets accessible
    /// name).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Quiet secondary description rendered below the label.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Disables toggling and dims the control.
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Registers a toggle callback.
    pub fn on_toggle<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.press = Some(Box::new(move |el| el.on_press(f)));
        self
    }

    /// Registers a transaction-aware toggle callback.
    pub fn on_toggle_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.press = Some(Box::new(move |el| el.on_press_tx(f)));
        self
    }

    /// Builds the flat switch element against `t`.
    pub fn build(self, t: &Theme) -> El {
        const TRACK_W: f32 = 30.0;
        const TRACK_H: f32 = 16.0;
        const THUMB: f32 = 12.0;
        const INSET: f32 = 2.0;

        let track_fill = if self.checked {
            t.accent
        } else {
            t.surface_recessed
        };
        let track_border = if self.checked {
            t.accent
        } else {
            t.border_control
        };
        let thumb_fill = if self.checked { t.on_accent } else { t.muted };
        let thumb_x = if self.checked {
            TRACK_W - INSET - THUMB
        } else {
            INSET
        };

        let thumb = el()
            .key("thumb")
            .size(THUMB, THUMB)
            .absolute(thumb_x, INSET)
            .radius(THUMB / 2.0)
            .fill(thumb_fill)
            .motion(motion::QUICK);

        let track = el()
            .key("track")
            .size(TRACK_W, TRACK_H)
            .radius(RADIUS_PILL)
            .fill(track_fill)
            .border(HAIRLINE, track_border)
            .motion(motion::QUICK)
            .a11y_hidden()
            .child(thumb);

        let mut root = row()
            .role("switch")
            .checked(self.checked)
            .gap(S8)
            .center_y()
            .radius(RADIUS_PILL)
            .disabled(self.disabled)
            .focusable(!self.disabled)
            .focus_ring(HAIRLINE, S2, t.accent)
            .child(track);

        if let Some(label) = self.label {
            root = root.label(&label);
            let mut copy = col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .a11y_hidden()
                .child(text(label).font(t.ui).color(t.text).truncate());
            if let Some(description) = self.description {
                root = root.description(&description);
                copy = copy.child(text(description).font(t.small).color(t.muted).wrap());
            }
            root = root.child(copy);
        } else if let Some(description) = self.description {
            root = root.description(&description);
        }

        if self.disabled {
            root = root.layer_opacity(0.50);
        } else if let Some(press) = self.press {
            root = press(root);
        }
        root
    }
}

/// Sizing behavior for [`Segmented`].
#[expect(
    dead_code,
    reason = "Full width and fixed width variants are used by Phase 1 panels and dialogs"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum SegmentedWidth {
    /// Equal segments sized to the widest label.
    #[default]
    Fit,
    /// Stretch equally across the parent's width.
    Fill,
    /// Fixed total width in points.
    Fixed(f32),
}

/// Flat base16 segmented control wrapping `rune_kit::SegmentedControl` with
/// `motion::QUICK` gliding indicator.
pub struct Segmented {
    labels: Vec<String>,
    selected: Option<usize>,
    width: SegmentedWidth,
    label: Option<String>,
    disabled: Vec<usize>,
    pick: Option<Box<dyn FnOnce(SegmentedControl) -> SegmentedControl>>,
}

/// Starts a [`Segmented`] choice control.
pub fn segmented<S: AsRef<str>>(labels: &[S], selected: Option<usize>) -> Segmented {
    Segmented {
        labels: labels.iter().map(|s| s.as_ref().to_owned()).collect(),
        selected,
        width: SegmentedWidth::Fit,
        label: None,
        disabled: Vec::new(),
        pick: None,
    }
}

impl Segmented {
    /// Sets the accessible radiogroup label.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Sets [`SegmentedWidth`] (`Fit`, `Fill`, or `Fixed`).
    pub const fn width(mut self, width: SegmentedWidth) -> Self {
        self.width = width;
        self
    }

    /// Disables the segment at `index`.
    pub fn disable_segment(mut self, index: usize) -> Self {
        self.disabled.push(index);
        self
    }

    /// Fires when the user selects a segment index.
    pub fn on_pick<S: 'static>(
        mut self,
        f: impl Fn(&mut S, usize, &mut Tx) + Clone + 'static,
    ) -> Self {
        self.pick = Some(Box::new(move |seg| seg.on_pick(f)));
        self
    }

    /// Builds the segmented control against `t`.
    pub fn build(self, t: &Theme) -> El {
        let mut control = SegmentedControl::new(&self.labels, self.selected);
        if let Some(label) = self.label {
            control = control.label(label);
        }
        control = match self.width {
            SegmentedWidth::Fit => control.fit(),
            SegmentedWidth::Fill => control.width(None),
            SegmentedWidth::Fixed(pts) => control.width(Some(pts)),
        };
        for index in self.disabled {
            control = control.segment_state(index, State::Disabled);
        }
        if let Some(pick) = self.pick {
            control = pick(control);
        }
        let mut seg_kit = t.k.clone();
        seg_kit.c.layer_01 = t.surface_recessed;
        seg_kit.c.bg_base = t.surface_raised;
        seg_kit.c.border_base = t.hairline;
        seg_kit.c.border_strong = t.border_control;
        control.build(&seg_kit)
    }
}
