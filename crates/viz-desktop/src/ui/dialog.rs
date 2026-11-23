//! Flat modal dialogs, frameless command-palette sheets, confirmation forms,
//! and floating toasts.

use rune::{El, TextEdit, Tx, col, el, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant};

use super::{
    badge::{Tone, dot},
    button::button,
    icon_button::icon_button,
    icons::CLOSE,
    input::text_field,
};
use crate::{
    motion,
    theme::{
        CARD_INSET, CARD_WIDTH, DIALOG_INSET, DIALOG_WIDTH, HAIRLINE, RADIUS_CARD, RADIUS_SURFACE,
        S2, S4, S8, Size, Theme,
    },
};

/// Maximum title lines before truncation in dialogs and toasts.
const TITLE_LINES: usize = 2;
/// Maximum detail lines before truncation in toasts.
const TOAST_DETAIL_LINES: usize = 4;

/// A flat modal dialog with a title, optional middle-truncated `subject`,
/// description, close button, and body inset like [`dialog_body`].
pub struct Dialog<'a> {
    key: &'a str,
    title: &'a str,
    subject: Option<&'a str>,
    detail: Option<&'a str>,
    width: f32,
}

/// Starts a modal [`Dialog`] keyed by `key`.
pub const fn dialog<'a>(key: &'a str, title: &'a str, detail: Option<&'a str>) -> Dialog<'a> {
    Dialog {
        key,
        title,
        subject: None,
        detail,
        width: DIALOG_WIDTH,
    }
}

impl<'a> Dialog<'a> {
    /// Quiet middle-truncated subject line (such as a path or thread title).
    pub const fn subject(mut self, subject: &'a str) -> Self {
        self.subject = Some(subject);
        self
    }

    /// Overrides the modal max width (defaults to `DIALOG_WIDTH`; use
    /// `DIALOG_WIDE_MAX_WIDTH` for settings or folder pickers).
    pub const fn width(mut self, points: f32) -> Self {
        self.width = points;
        self
    }

    /// Mounts the scrim and dialog into `layer`.
    pub fn build<S: 'static>(
        self,
        close: impl Fn(&mut S) + Clone + 'static,
        body: El,
        t: &Theme,
        layer: &mut Vec<El>,
    ) {
        let mut head = col().grow(1.0).shrink().min_w(0.0).gap(S4).child(
            text(self.title)
                .font(t.dialog_title)
                .color(t.text_strong)
                .wrap_anywhere()
                .max_lines(TITLE_LINES)
                .w_full(),
        );
        if let Some(subject) = self.subject {
            head = head.child(
                text(subject)
                    .font(t.ui_medium)
                    .color(t.text)
                    .truncate_middle()
                    .w_full(),
            );
        }
        if let Some(detail) = self.detail {
            let mut detail_el = text(detail)
                .font(t.ui)
                .color(t.muted)
                .wrap_anywhere()
                .w_full();
            if self.subject.is_some() {
                detail_el = detail_el.mt(S4);
            }
            head = head.child(detail_el);
        }
        let dismiss = icon_button(&CLOSE, "Close", IconButtonVariant::Ghost, Size::Small)
            .on_press(close.clone())
            .build(&t.overlay)
            .key("close")
            .mt(-S2)
            .mr(-S4);
        let header = row()
            .key("header")
            .w_full()
            .padding(DIALOG_INSET)
            .gap(S8)
            .items_start()
            .child(head)
            .child(dismiss);
        let panel = col()
            .label(self.title)
            .description(self.detail.unwrap_or_default())
            .child(header)
            .child(col().key("body").w_full().child(body));
        modal(self.key, panel, self.width, close, t, layer);
    }
}

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
    /// What the action is about, as [`Dialog::subject`].
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
            text_field(value, placeholder, placeholder, Size::Default)
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
        let dlg = dialog("edit", self.title, self.detail);
        match self.subject {
            Some(subject) => dlg.subject(subject),
            None => dlg,
        }
        .build(close, dialog_body(input, actions), t, layer);
    }
}

/// A flat toast notification card (`t.surface_raised`, `1px`
/// `t.border_overlay`, zero drop shadow) with optional detail, mark, and close
/// handler.
pub fn toast<S: 'static>(
    key: String,
    title: &str,
    detail: Option<&str>,
    mark: El,
    close: impl Fn(&mut S, &mut Tx) + 'static,
    t: &Theme,
) -> El {
    let dismiss = icon_button(&CLOSE, "Close", IconButtonVariant::GhostMuted, Size::Small)
        .on_press_tx(close)
        .build(&t.overlay)
        .key("close")
        .mt(-S2)
        .mr(-S4);
    toast_frame(key, title, detail, mark, None, Some(dismiss), t)
}

/// Builder for a flat toast card with semantic tone dot, optional detail,
/// optional inline action (such as `Undo`), and optional dismiss button.
pub struct ToastCard {
    key: String,
    title: String,
    detail: Option<String>,
    tone: Tone,
    action: Option<El>,
    dismiss: Option<El>,
}

/// Starts a flat [`ToastCard`] keyed by `key`.
pub fn toast_card(key: impl Into<String>, title: impl Into<String>) -> ToastCard {
    ToastCard {
        key: key.into(),
        title: title.into(),
        detail: None,
        tone: Tone::Neutral,
        action: None,
        dismiss: None,
    }
}

impl ToastCard {
    /// Sets the semantic tone dot colour.
    pub const fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Adds a quiet secondary detail line.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Attaches an inline trailing action button (e.g., `Undo`).
    pub fn action(mut self, action: El) -> Self {
        self.action = Some(action);
        self
    }

    /// Attaches a close button invoking `close`.
    pub fn on_close<S: 'static>(
        mut self,
        close: impl Fn(&mut S, &mut Tx) + 'static,
        t: &Theme,
    ) -> Self {
        self.dismiss = Some(
            icon_button(&CLOSE, "Close", IconButtonVariant::GhostMuted, Size::Small)
                .on_press_tx(close)
                .build(&t.overlay)
                .key("close")
                .mt(-S2)
                .mr(-S4),
        );
        self
    }

    /// Builds the flat toast card against `t`.
    pub fn build(self, t: &Theme) -> El {
        let mark = dot(self.tone.colors(t).fg).mt(6.0);
        toast_frame(
            self.key,
            &self.title,
            self.detail.as_deref(),
            mark,
            self.action,
            self.dismiss,
            t,
        )
    }
}

fn toast_frame(
    key: String,
    title: &str,
    detail: Option<&str>,
    mark: El,
    action: Option<El>,
    dismiss: Option<El>,
    t: &Theme,
) -> El {
    let mut content = col()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(S4)
        .a11y_hidden()
        .child(
            text(title)
                .font(t.ui_medium)
                .color(t.text_strong)
                .wrap_anywhere()
                .max_lines(TITLE_LINES)
                .w_full(),
        );
    if let Some(detail) = detail {
        content = content.child(
            text(detail)
                .font(t.small)
                .color(t.muted)
                .wrap_anywhere()
                .max_lines(TOAST_DETAIL_LINES)
                .w_full(),
        );
    }
    let mut body = row()
        .w_full()
        .gap(S8)
        .items_start()
        .child(mark.a11y_hidden())
        .child(content);
    if let Some(action) = action {
        body = body.child(action);
    }
    if let Some(dismiss) = dismiss {
        body = body.child(dismiss);
    }
    col()
        .key(key)
        .role("status")
        .label(title)
        .description(detail.unwrap_or_default())
        .w(CARD_WIDTH)
        .padding(CARD_INSET)
        .radius(RADIUS_CARD)
        .fill(t.surface_raised)
        .border(HAIRLINE, t.border_overlay)
        .child(body)
        .transition(motion::toast())
        .token_motion(t.overlay.motion.overlay)
}
