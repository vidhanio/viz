//! Dialog body layout on top of `rune_kit::Dialog`, confirmation forms, and
//! the frameless command-palette sheet (the kit dialog always draws a header).

use rune::{El, TextEdit, Tx, col, el, row, text};
use rune_kit::ButtonVariant;

use super::button::button;
use crate::{
    motion,
    theme::{DIALOG_INSET, HAIRLINE, RADIUS_SURFACE, S8, Size, Theme},
};

/// A frameless modal sheet without a header row (for the command palette or
/// custom top search overlays).
pub fn sheet<S: 'static>(
    key: &str,
    label: &str,
    width: f32,
    close: impl Fn(&mut S) + 'static,
    body: impl IntoIterator<Item = El>,
    t: &Theme,
    layer: &mut Vec<El>,
) {
    modal(
        key,
        col().label(label).children(body),
        width,
        close,
        t,
        layer,
    );
}

/// Mounts a flat modal panel (`t.surface_raised`, `1px` `t.border_overlay`,
/// zero drop shadow) and scrim (`t.scrim`) with Escape and outside-click
/// dismissal.
fn modal<S: 'static>(
    key: &str,
    panel: El,
    width: f32,
    close: impl Fn(&mut S) + 'static,
    t: &Theme,
    layer: &mut Vec<El>,
) {
    let k = &t.overlay;
    let panel = panel
        .key(format!("{key}-dialog"))
        .role("dialog")
        .w_full()
        .max_w(width)
        .radius(RADIUS_SURFACE)
        .fill(t.surface_raised)
        .border(HAIRLINE, t.border_overlay)
        .clip_rounded(RADIUS_SURFACE)
        .transition(motion::dialog())
        .token_motion(k.motion.overlay)
        .modal(true, close);
    layer.push(
        el().key(format!("{key}-scrim"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(t.scrim)
            .transition(motion::scrim())
            .token_motion(k.motion.overlay)
            .a11y_hidden(),
    );
    layer.push(
        el().key(format!("{key}-layer"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(DIALOG_INSET)
            .center()
            .child(panel),
    );
}

/// A dialog's quiet middle-truncated subject (a path or thread title), placed
/// first in its body.
pub fn dialog_subject(subject: &str, t: &Theme) -> El {
    text(subject)
        .font(t.ui_medium)
        .color(t.text)
        .truncate_middle()
        .w_full()
        .px(DIALOG_INSET)
        .pb(S8)
}

/// A dialog's body: its fields followed by its action row at [`DIALOG_INSET`].
pub fn dialog_body(fields: impl IntoIterator<Item = El>, actions: El) -> El {
    col()
        .w_full()
        .px(DIALOG_INSET)
        .pb(DIALOG_INSET)
        .gap(DIALOG_INSET)
        .children(fields)
        .child(actions)
}

/// A dialog's buttons, right-aligned with the primary action last.
pub fn dialog_actions(buttons: impl IntoIterator<Item = El>) -> El {
    row().w_full().justify_end().gap(S8).children(buttons)
}

/// A one-field (or no-field) confirmation dialog.
pub struct DialogForm<'a> {
    pub title: &'a str,
    /// What the action is about, as [`dialog_subject`].
    pub subject: Option<&'a str>,
    pub detail: Option<&'a str>,
    pub field: Option<(&'a TextEdit, &'a str)>,
    pub action: &'a str,
    pub variant: ButtonVariant,
}

impl DialogForm<'_> {
    /// Builds and mounts the confirmation form into `layer`.
    pub fn build<S: 'static>(
        self,
        edit: impl Fn(&mut S) -> &mut TextEdit + 'static,
        close: impl Fn(&mut S) + Clone + 'static,
        submit: impl Fn(&mut S, &mut Tx) + Clone + 'static,
        t: &Theme,
        layer: &mut Vec<El>,
    ) {
        let input = self.field.map(|(value, placeholder)| {
            rune_kit::TextInput::new(value)
                .label(placeholder)
                .placeholder(placeholder)
                .width(None)
                .on_edit(edit)
                .on_submit(submit.clone())
                .build(&t.k)
                .key("edit-value")
        });
        let disabled = self
            .field
            .is_some_and(|(value, _)| value.text.trim().is_empty());
        let actions = dialog_actions([
            button("Cancel", ButtonVariant::Ghost, Size::Default)
                .on_press(close.clone())
                .build(&t.k),
            button(self.action, self.variant, Size::Default)
                .disabled(disabled)
                .on_press_tx(submit)
                .build(&t.k),
        ]);
        let mut dialog = rune_kit::Dialog::new("edit", self.title)
            .open(true)
            .fit()
            .on_close(close);
        if let Some(detail) = self.detail {
            dialog = dialog.description(detail);
        }
        if let Some(subject) = self.subject {
            dialog = dialog.child(dialog_subject(subject, t));
        }
        dialog
            .child(dialog_body(input, actions))
            .build(&t.overlay, layer);
    }
}
