//! Compact bottom-of-chat status lines mirroring T3 Code's `ThreadNow` and
//! `ThreadStatusLine` family: subagent attention, running delegated tasks,
//! connection status, and snoozed / settled / archived lifecycle controls.

use rune::{El, Tx, col, el, icon, row, text};
use rune_kit::ButtonVariant;
use viz_client::protocol::{
    Attention, Shelf, Standing, Task, TaskStatus, ThreadId, ThreadSummary, TriageAction,
};

use crate::{
    app::Desktop,
    model::Status,
    theme::{ICON_MD, ICON_SMALL, S4, S6, S16, S20, Size, Theme, now_ms},
    ui,
    update::{Msg, SidebarMsg, TimelineMsg},
};

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;

/// Renders the stack of active bottom-of-chat status lines for the currently
/// selected thread, or `None` when nothing needs attention or lifecycle chrome.
pub fn timeline_status(s: &Desktop, t: &Theme) -> Option<El> {
    let m = &s.model;
    let selected = m.selection.selected?;
    let mut lines = Vec::new();

    if let Some(line) = attention_line(s, selected, t) {
        lines.push(line);
    }
    if let Some(line) = work_line(s, selected, t) {
        lines.push(line);
    }
    if let Some(line) = sync_line(s, t) {
        lines.push(line);
    }
    if let Some(summary) = m.summary(selected)
        && let Some(line) = lifecycle_line(s, summary, t)
    {
        lines.push(line);
    }

    if lines.is_empty() {
        return None;
    }

    Some(
        col()
            .key("thread-status-lines")
            .w_full()
            .pt(S4)
            .gap(S4)
            .children(lines),
    )
}

fn attention_line(s: &Desktop, selected: ThreadId, t: &Theme) -> Option<El> {
    let m = &s.model;
    if crate::model::requests::pending(m).next().is_some() {
        return None;
    }
    let waiting: Vec<&ThreadSummary> = m
        .cache()
        .threads()
        .filter(|thread| {
            thread.parent == Some(selected)
                && matches!(
                    thread.attention,
                    Some(Attention::Approval | Attention::Question)
                )
        })
        .collect();
    let first = *waiting.first()?;
    let approval = first.attention == Some(Attention::Approval);
    let glyph = if approval { &ui::SHIELD } else { &ui::THREAD };
    let title = if waiting.len() == 1 {
        "Subagent waiting".to_owned()
    } else {
        format!("{} subagents waiting", waiting.len())
    };
    let description = waiting
        .iter()
        .map(|child| child.title.as_str())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let target = first.id;
    let action_label = if approval {
        "Open approval"
    } else {
        "Open question"
    };
    Some(status_line(
        "status-subagent-attention",
        Some("alert"),
        icon_box(icon(glyph, ICON_MD).color(t.info)),
        title,
        (!description.is_empty()).then_some(description),
        vec![inline_action(
            "open-subagent-attention",
            action_label,
            false,
            move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::OpenThreadLink {
                        thread: target,
                        environment: None,
                    }),
                    tx,
                );
            },
            t,
        )],
        t,
    ))
}

fn work_line(s: &Desktop, selected: ThreadId, t: &Theme) -> Option<El> {
    let m = &s.model;
    let active: Vec<&Task> = m
        .tasks()
        .iter()
        .filter(|task| {
            task.parent == selected
                && matches!(task.status, TaskStatus::Running | TaskStatus::Queued)
        })
        .collect();
    let first = *active.first()?;
    let title = if active.len() == 1 {
        if first.status == TaskStatus::Running {
            "Subagent running".to_owned()
        } else {
            "Subagent queued".to_owned()
        }
    } else {
        format!("{} subagents running", active.len())
    };
    let description = active
        .iter()
        .map(|task| task.title.as_str())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let mut actions = Vec::new();
    if active.len() == 1 {
        if let Some(child) = first.thread {
            let env = (first.environment != "local").then(|| first.environment.clone());
            actions.push(inline_action(
                "open-delegated-task",
                "Open",
                false,
                move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(
                        Msg::Timeline(TimelineMsg::OpenThreadLink {
                            thread: child,
                            environment: env.clone(),
                        }),
                        tx,
                    );
                },
                t,
            ));
        }
        if !m.busy() {
            let task_id = first.id.clone();
            actions.push(inline_action(
                "stop-delegated-task",
                "Stop",
                !m.connected(),
                move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(Msg::Timeline(TimelineMsg::CancelTask(task_id.clone())), tx);
                },
                t,
            ));
        }
    }
    Some(status_line(
        "status-background-work",
        Some("status"),
        icon_box(ui::radar(ICON_SMALL, t.muted, t)),
        title,
        (!description.is_empty()).then_some(description),
        actions,
        t,
    ))
}

fn sync_line(s: &Desktop, t: &Theme) -> Option<El> {
    match &s.model.connection.status {
        Status::Connected { .. } => None,
        Status::Connecting => Some(status_line(
            "status-sync",
            Some("status"),
            icon_box(icon(&ui::REFRESH_CW, ICON_MD).color(t.muted)),
            "Connecting…".to_owned(),
            None,
            Vec::new(),
            t,
        )),
        Status::Error(reason) => {
            let detail = reason.trim();
            Some(status_line(
                "status-sync",
                Some("alert"),
                icon_box(icon(&ui::CIRCLE_ALERT, ICON_MD).color(t.warning)),
                "Offline".to_owned(),
                (!detail.is_empty()).then(|| detail.to_owned()),
                Vec::new(),
                t,
            ))
        }
    }
}

fn lifecycle_line(s: &Desktop, summary: &ThreadSummary, t: &Theme) -> Option<El> {
    let m = &s.model;
    let now = if m.disclosure.tool_clock > 0 {
        m.disclosure.tool_clock
    } else {
        now_ms()
    };
    let thread = summary.id;
    let disabled = !m.connected();

    if summary.shelf == Shelf::Snoozed || summary.lifecycle.snoozed_until.is_some() {
        let label = summary
            .lifecycle
            .snoozed_until
            .filter(|&until| until > now)
            .map_or_else(
                || "Snoozed".to_owned(),
                |until| format_snoozed_label(until, now),
            );
        return Some(status_line(
            "status-lifecycle-snoozed",
            None,
            icon_box(icon(&ui::CLOCK, ICON_MD).color(t.muted)),
            label,
            None,
            vec![triage_action(
                "wake-thread",
                "Wake now",
                thread,
                TriageAction::Wake,
                disabled,
                t,
            )],
            t,
        ));
    }

    if summary.shelf == Shelf::Settled {
        let label = match summary.lifecycle.standing {
            Standing::Settled { at, .. } if at > 0 && now > 0 => {
                format!("Settled {}", format_relative_ago(at, now))
            }
            _ => "Settled".to_owned(),
        };
        return Some(status_line(
            "status-lifecycle-settled",
            None,
            icon_box(icon(&ui::CIRCLE_CHECK, ICON_MD).color(t.muted)),
            label,
            None,
            vec![triage_action(
                "unsettle-thread",
                "Un-settle",
                thread,
                TriageAction::Reopen,
                disabled,
                t,
            )],
            t,
        ));
    }

    if summary.shelf == Shelf::Archived {
        return Some(status_line(
            "status-lifecycle-archived",
            None,
            icon_box(icon(&ui::ARCHIVE, ICON_MD).color(t.muted)),
            "Archived".to_owned(),
            None,
            vec![triage_action(
                "unarchive-thread",
                "Unarchive",
                thread,
                TriageAction::Unarchive,
                disabled,
                t,
            )],
            t,
        ));
    }

    None
}

fn triage_action(
    key: &'static str,
    label: &'static str,
    thread: ThreadId,
    action: TriageAction,
    disabled: bool,
    t: &Theme,
) -> El {
    inline_action(
        key,
        label,
        disabled,
        move |s: &mut Desktop, tx: &mut Tx| {
            s.dispatch(
                Msg::Sidebar(SidebarMsg::Triage {
                    thread,
                    action: action.clone(),
                    undo_title: None,
                    now: now_ms(),
                }),
                tx,
            );
        },
        t,
    )
}

fn icon_box(glyph: El) -> El {
    el().size(S16, S16).center().a11y_hidden().child(glyph)
}

fn status_line(
    key: &'static str,
    role: Option<&'static str>,
    lead: El,
    title: String,
    description: Option<String>,
    actions: Vec<El>,
    t: &Theme,
) -> El {
    let mut line = row()
        .key(key)
        .w_full()
        .min_h(S20)
        .px(S4)
        .gap(S6)
        .center_y()
        .child(lead)
        .child(
            text(title)
                .font(t.small)
                .color(t.muted)
                .shrink()
                .min_w(0.0)
                .truncate(),
        );
    if let Some(role) = role {
        line = line.role(role);
    }
    if let Some(desc) = description {
        line = line.child(
            text(desc)
                .font(t.small)
                .color(t.faint)
                .shrink()
                .min_w(0.0)
                .truncate(),
        );
    }
    for action in actions {
        line = line.child(
            row()
                .gap(crate::theme::S2)
                .center_y()
                .child(text("·").font(t.small).color(t.faint).a11y_hidden())
                .child(action),
        );
    }
    line
}

fn inline_action(
    key: &'static str,
    label: &'static str,
    disabled: bool,
    on_press: impl Fn(&mut Desktop, &mut Tx) + 'static,
    t: &Theme,
) -> El {
    rune_kit::Button::new(label)
        .variant(ButtonVariant::GhostMuted)
        .control_size(Size::Micro.kit())
        .disabled(disabled)
        .on_press_tx(on_press)
        .build(&t.k)
        .key(key)
}

fn format_snoozed_label(until_ms: i64, now_ms: i64) -> String {
    let remaining = until_ms.saturating_sub(now_ms).max(0);
    if remaining < HOUR_MS {
        let mins = (remaining / MINUTE_MS).max(1);
        format!("Snoozed, {mins}m left")
    } else if remaining < DAY_MS {
        let hours = (remaining + HOUR_MS / 2) / HOUR_MS;
        format!("Snoozed, {hours}h left")
    } else if remaining < 2 * DAY_MS {
        "Snoozed until tomorrow".to_owned()
    } else {
        let days = (remaining + DAY_MS / 2) / DAY_MS;
        format!("Snoozed, {days}d left")
    }
}

fn format_relative_ago(at_ms: i64, now_ms: i64) -> String {
    let elapsed = now_ms.saturating_sub(at_ms).max(0);
    if elapsed < MINUTE_MS {
        "just now".to_owned()
    } else if elapsed < HOUR_MS {
        let mins = elapsed / MINUTE_MS;
        format!("{mins}m ago")
    } else if elapsed < DAY_MS {
        let hours = elapsed / HOUR_MS;
        format!("{hours}h ago")
    } else {
        let days = elapsed / DAY_MS;
        format!("{days}d ago")
    }
}
