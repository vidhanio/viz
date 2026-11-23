//! Toast content, lifetimes, and wake scheduling.

use std::time::Duration;

use rune::Tx;
use rune_kit::{ToastEffect, ToastEvent, ToastPage};

use super::Desktop;
use crate::{model::UndoTriage, ui::Tone};

/// How long a toast stays up while unpaused.
const TOAST_LIFE: Duration = Duration::from_secs(6);

/// Content carried by an entry in [`super::ShellWidgets::toasts`].
#[derive(Debug, Clone)]
pub struct ToastContent {
    /// One line.
    pub title: String,
    /// More, if there is any.
    pub detail: Option<String>,
    /// Semantic tone dot color for the toast card.
    pub tone: Tone,
    /// Optional undo payload rendered as an inline `Undo` button.
    pub undo: Option<UndoTriage>,
}

impl Desktop {
    /// Adds a persistent toast for preview scenes (no expiry clock).
    pub fn preview_toast(&mut self, title: &str, detail: &str) {
        self.push_preview_toast(
            format!("{title}\n{detail}"),
            title,
            detail,
            Tone::Danger,
            None,
        );
    }

    /// Adds a persistent triage undo toast for preview scenes.
    pub fn preview_undo_toast(&mut self, title: &str, detail: &str, undo: UndoTriage) {
        let key = format!("undo-{}-{title}", undo.thread);
        self.push_preview_toast(key, title, detail, Tone::Neutral, Some(undo));
    }

    fn push_preview_toast(
        &mut self,
        key: String,
        title: &str,
        detail: &str,
        tone: Tone,
        undo: Option<UndoTriage>,
    ) {
        let _ = self.widgets.toasts.step(ToastEvent::Shown {
            key,
            options: ToastContent {
                title: title.into(),
                detail: Some(detail.into()),
                tone,
                undo,
            },
            secs: None,
            now: 0.0,
        });
    }

    /// Closes a toast early.
    pub fn dismiss_toast(&mut self, id: u64, tx: &mut Tx) {
        self.step_toasts(
            ToastEvent::Dismissed {
                id: Some(id),
                now: tx.clock(),
            },
            tx,
        );
    }

    /// Pauses or resumes toast auto-dismiss while the pointer is over the
    /// stack.
    pub fn toast_hover(&mut self, inside: bool, tx: &mut Tx) {
        let event = if inside {
            ToastEvent::PointerOver { now: tx.clock() }
        } else {
            ToastEvent::PointerLeft { now: tx.clock() }
        };
        self.step_toasts(event, tx);
    }

    /// Queues a toast whose close button was pressed for the next poll.
    pub fn dismiss_toast_local(&mut self, id: u64) {
        self.widgets.closed_toasts.push(id);
    }

    /// Dismisses closed toasts, syncs window visibility, and expires due
    /// toasts; returns true if any closed.
    pub fn expire_toasts(&mut self, clock: f64, tx: &mut Tx) -> bool {
        let mut closed = false;
        for id in std::mem::take(&mut self.widgets.closed_toasts) {
            let event = ToastEvent::Dismissed {
                id: Some(id),
                now: clock,
            };
            closed |= self.step_toasts(event, tx);
        }
        if self.widgets.toasts.entries().is_empty() {
            return closed;
        }
        let page = if tx.window_visible() {
            ToastPage::Shown
        } else {
            ToastPage::Hidden
        };
        if self.widgets.toasts.page() != page {
            self.step_toasts(ToastEvent::Page { page, now: clock }, tx);
        }
        if self
            .widgets
            .toasts
            .next_due()
            .is_some_and(|due| clock + 1e-3 >= due)
        {
            return self.step_toasts(ToastEvent::Tick { now: clock }, tx) || closed;
        }
        closed
    }

    pub(crate) fn show_toast(
        &mut self,
        tx: &mut Tx,
        title: impl Into<String>,
        detail: Option<String>,
    ) {
        let title = title.into();
        let key = format!("{title}\n{}", detail.as_deref().unwrap_or_default());
        let tone = if detail.is_some() {
            Tone::Danger
        } else {
            Tone::Neutral
        };
        let content = ToastContent {
            title,
            detail,
            tone,
            undo: None,
        };
        self.push_toast(tx, key, content, TOAST_LIFE.as_secs_f64());
    }

    pub(crate) fn show_undo_toast(
        &mut self,
        tx: &mut Tx,
        title: String,
        detail: Option<String>,
        undo: UndoTriage,
        secs: f64,
    ) {
        let key = format!("undo-{}", undo.thread);
        let content = ToastContent {
            title,
            detail,
            tone: Tone::Neutral,
            undo: Some(undo),
        };
        self.push_toast(tx, key, content, secs);
    }

    fn push_toast(&mut self, tx: &mut Tx, key: String, options: ToastContent, secs: f64) {
        let event = ToastEvent::Shown {
            key,
            options,
            secs: Some(secs),
            now: tx.clock(),
        };
        self.step_toasts(event, tx);
    }

    fn step_toasts(&mut self, event: ToastEvent<ToastContent>, tx: &mut Tx) -> bool {
        let mut closed = false;
        for effect in self.widgets.toasts.step(event) {
            match effect {
                ToastEffect::Closed(_) => closed = true,
                ToastEffect::WakeAt(due) => {
                    let wait = (due - tx.clock()).max(0.0);
                    tx.wake_after(Duration::from_secs_f64(wait));
                }
                ToastEffect::Pulse(_) => {}
            }
        }
        closed
    }
}
