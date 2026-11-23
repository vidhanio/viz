//! Compact sidebar threads, zero-shift hover quick actions, inline
//! rename, and drag-and-drop pinning/reordering.

use std::time::Duration;

use rune::{Anim, Color, Drag, El, Key, KeyEvent, Phase, Tx, col, el, icon, row, text, text_input};
use rune_kit::IconButtonVariant;
use viz_client::protocol::{Attention, Shelf, ThreadStatus, ThreadSummary, TriageAction};

use super::{actions, card_meta, menu};
use crate::{
    app::Desktop,
    model::{DragShelf, DropZone, SidebarOutline, format_wake_label, thread_title},
    motion,
    theme::{self, HAIRLINE, ICON_MD, RADIUS_SM, RADIUS_XS, S2, S4, S6, S10, Size, Theme},
    ui,
    update::{INLINE_RENAME_INPUT, Msg, SelectionMsg, SidebarMsg},
};

/// Pointer rest delay before opening the right-anchored thread hover preview
/// card (`sidebar.md` §5, `motion.md` `OV-03`).
const HOVER_CARD_DELAY: Duration = Duration::from_millis(450);

/// Renders one flat triage row.
#[expect(
    clippy::too_many_lines,
    reason = "cohesive thread row combining identity, title, metadata, hover actions, and drag state"
)]
pub(super) fn thread_card(
    s: &Desktop,
    outline: &SidebarOutline<'_>,
    thread: &ThreadSummary,
    shelf: Shelf,
    drag_shelf: Option<DragShelf>,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let m = &s.model;
    let id = thread.id;
    let selected = outline.open == Some(id);
    let key = format!("thread-{id}");
    let project = thread
        .project
        .as_deref()
        .and_then(|path| m.cache().project(path));
    let renaming = m
        .sidebar
        .inline_rename
        .as_ref()
        .is_some_and(|edit| edit.thread == id);
    let force_actions = m.sidebar.snooze_popover == Some(id) || m.sidebar.hover_card == Some(id);
    let lifted = m
        .sidebar
        .drag
        .as_ref()
        .filter(|drag| drag.active && drag.thread == id);

    menu::thread_context_menu(s, thread, &key, shelf, outline.now, t, layer);

    let title = title_row(s, thread, shelf, selected, renaming, t);
    let status = row_status(s, outline, thread, shelf, force_actions, t, layer);
    let identity = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(ui::project_mark(s, project, ICON_MD, t))
        .child(title.grow(1.0).shrink().min_w(0.0))
        .child(status);
    let row3 = card_meta::metadata_row(thread, project, selected, t);

    let fill = if lifted.is_some() {
        t.surface_raised
    } else if selected {
        t.selected
    } else {
        Color::TRANSPARENT
    };

    let mut card = col()
        .key(&key)
        .role("treeitem")
        .label(thread_title(thread))
        .level(1)
        .w_full()
        .px(S10)
        .py(S4)
        .gap(S2)
        .radius(RADIUS_SM)
        .fill(fill)
        .hover_group()
        .hover(if selected { t.selected } else { t.hover_wash })
        .press(t.pressed_wash)
        .focusable(!renaming)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .child(identity)
        .child(row3);

    if matches!(shelf, Shelf::Settled | Shelf::Snoozed) && !selected && lifted.is_none() {
        card = card.layer_opacity(0.82);
    }
    if thread.lifecycle.unread {
        card = card.description("Unread");
    }
    if !renaming {
        card = card.on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(Msg::Sidebar(SidebarMsg::HoverCard(None)), tx);
            s.dispatch(Msg::Selection(SelectionMsg::Select(id)), tx);
        });
    }
    if m.connected() && !renaming {
        let menu_key = key.clone();
        card = card.on_secondary_press(move |s: &mut Desktop, at, _| {
            s.dispatch_local(Msg::Sidebar(SidebarMsg::OpenRowMenuAt {
                key: menu_key.clone(),
                x: at.x,
                y: at.y,
            }));
        });
        card = card
            .hover_delays(HOVER_CARD_DELAY, Duration::ZERO)
            .on_hover_change(move |s: &mut Desktop, inside, tx| {
                if inside {
                    s.dispatch(Msg::Sidebar(SidebarMsg::HoverCard(Some(id))), tx);
                } else if s.model.sidebar.hover_card == Some(id) {
                    s.dispatch(Msg::Sidebar(SidebarMsg::HoverCard(None)), tx);
                }
            });
    }
    if let Some(shelf) = drag_shelf
        && m.connected()
        && !renaming
    {
        let now = outline.now;
        card = super::shelves::measure(card, m.sidebar.drag.is_some(), DropZone::Row(shelf, id))
            .motion(motion::REFLOW)
            .on_drag(
                move |s: &mut Desktop, drag: &mut Drag, tx: &mut Tx| match drag.phase {
                    Phase::Down => s.dispatch(
                        Msg::Sidebar(SidebarMsg::DragStart { thread: id, shelf }),
                        tx,
                    ),
                    Phase::Move => {
                        tx.animated();
                        s.dispatch(
                            Msg::Sidebar(SidebarMsg::DragMove {
                                dy: drag.dy,
                                point: drag.window_point(),
                                now,
                            }),
                            tx,
                        );
                    }
                    Phase::Up => {
                        let cancelled = drag.cancelled();
                        if !cancelled {
                            s.dispatch(
                                Msg::Sidebar(SidebarMsg::DragMove {
                                    dy: drag.dy,
                                    point: drag.window_point(),
                                    now,
                                }),
                                tx,
                            );
                        }
                        s.dispatch(Msg::Sidebar(SidebarMsg::DragEnd { cancelled, now }), tx);
                    }
                },
            );
    }
    if let Some(drag) = lifted {
        card = card
            .offset(0.0, drag.dy)
            .offset_motion(Anim::INSTANT)
            .raise();
    }

    if let Some(shelf) = drag_shelf
        && m.sidebar
            .drag
            .as_ref()
            .is_some_and(|drag| drag.target == Some((shelf, Some(id))))
    {
        card = card.child(card_meta::drop_indicator("drop-before", t).pin(
            Some(0.0),
            Some(-3.0),
            Some(0.0),
            None,
        ));
    }

    card
}

fn row_status(
    s: &Desktop,
    outline: &SidebarOutline<'_>,
    thread: &ThreadSummary,
    shelf: Shelf,
    force_actions: bool,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let m = &s.model;
    let resting = resting_status(outline, thread, shelf, t);
    let hover = m
        .connected()
        .then(|| hover_actions(s, thread, shelf, outline.now, t, layer));
    let has_hover = hover.is_some();

    let mut right = el().key("row1-right").h(18.0).equal_columns(1);
    if let Some(rest) = resting {
        let rest_opacity = if force_actions && has_hover { 0.0 } else { 1.0 };
        let hover_opacity = if has_hover { 0.0 } else { 1.0 };
        right = right.child(
            row()
                .grid_cell(0)
                .h(18.0)
                .justify_end()
                .center_y()
                .group_opacity(rest_opacity, hover_opacity)
                .child(rest),
        );
    }
    if let Some(actions_el) = hover {
        right = right.child(
            row()
                .grid_cell(0)
                .h(18.0)
                .justify_end()
                .center_y()
                .group_opacity(if force_actions { 1.0 } else { 0.0 }, 1.0)
                .child(actions_el),
        );
    }

    right
}

fn resting_status(
    outline: &SidebarOutline<'_>,
    thread: &ThreadSummary,
    shelf: Shelf,
    t: &Theme,
) -> Option<El> {
    if shelf == Shelf::Working {
        let (mark, label, color) = match thread.attention {
            Some(Attention::Approval) => (ui::dot(t.warning), "Approval".to_owned(), t.warning),
            Some(Attention::Question) => (ui::dot(t.info), "Question".to_owned(), t.info),
            _ if thread.waiting_on > 0 && thread.status != ThreadStatus::Working => (
                icon(&ui::AGENT, 11.0).color(t.delegated).a11y_hidden(),
                format!("{} tasks", thread.waiting_on),
                t.delegated,
            ),
            _ => {
                let elapsed = theme::relative_age(thread.updated_at, outline.now);
                let text_label = match elapsed.as_deref() {
                    Some("now") | None => "Working".to_owned(),
                    Some(age) => format!("Working {age}"),
                };
                (ui::radar(11.0, t.accent, t), text_label, t.accent_text)
            }
        };
        return Some(
            row()
                .gap(S4)
                .center_y()
                .a11y_hidden()
                .child(mark)
                .child(text(label).font(t.mono_micro).color(color)),
        );
    }
    if shelf == Shelf::Snoozed
        && let Some(until) = thread.lifecycle.snoozed_until
    {
        return Some(
            row()
                .gap(S4)
                .center_y()
                .a11y_hidden()
                .child(icon(&ui::CLOCK, 11.0).color(t.faint))
                .child(
                    text(format_wake_label(until, outline.now))
                        .font(t.mono_micro)
                        .color(t.faint),
                ),
        );
    }
    let age = theme::relative_age(thread.updated_at, outline.now);
    let pinned = thread.lifecycle.pin.is_some();
    if age.is_none() && !pinned {
        return None;
    }
    let mut slot = row().gap(S4).center_y().a11y_hidden();
    if pinned {
        slot = slot.child(icon(&ui::PIN, 10.0).color(t.faint));
    }
    if let Some(age) = age {
        slot = slot.child(text(age).font(t.mono_micro).color(t.faint));
    }
    Some(slot)
}

fn hover_actions(
    s: &Desktop,
    thread: &ThreadSummary,
    shelf: Shelf,
    now: i64,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let id = thread.id;
    let mut snooze_btn = || {
        let trigger = ui::icon_button(
            &ui::CLOCK,
            "Snooze",
            IconButtonVariant::GhostMuted,
            Size::Micro,
        )
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(Msg::Sidebar(SidebarMsg::ToggleSnoozePopover(id)), tx);
        })
        .build(&t.k)
        .key(format!("snooze-trigger-{id}"));
        actions::snooze_popover(s, thread, trigger, now, t, layer)
    };
    let pill = |glyph: &'static rune::Icon, label: &'static str, action: TriageAction, undo| {
        row()
            .key(format!("quick-{label}-{id}"))
            .role("button")
            .label(label)
            .h(Size::Micro.height())
            .px(S4)
            .gap(3.0)
            .center_y()
            .radius(RADIUS_SM)
            .hover_group()
            .hover(t.hover_wash)
            .press(t.pressed_wash)
            .press_scale(motion::PRESS_SCALE)
            .focusable(false)
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::Triage {
                        thread: id,
                        action: action.clone(),
                        undo_title: undo,
                        now,
                    }),
                    tx,
                );
            })
            .child(
                icon(glyph, 11.0)
                    .color(t.muted)
                    .group_color(t.text_strong)
                    .a11y_hidden(),
            )
            .child(
                text(label)
                    .font(t.micro_medium)
                    .color(t.muted)
                    .group_color(t.text_strong)
                    .a11y_hidden(),
            )
    };
    match shelf {
        Shelf::Settled => pill(&ui::ROTATE_CCW, "Reopen", TriageAction::Reopen, None),
        Shelf::Snoozed => pill(&ui::ROTATE_CCW, "Unsnooze", TriageAction::Wake, None),
        Shelf::Working => snooze_btn(),
        Shelf::Pinned | Shelf::Active | Shelf::Archived => {
            row().gap(S2).center_y().child(snooze_btn()).child(pill(
                &ui::CHECK,
                "Settle",
                TriageAction::Settle,
                Some("Thread settled"),
            ))
        }
    }
}

fn title_row(
    s: &Desktop,
    thread: &ThreadSummary,
    shelf: Shelf,
    selected: bool,
    renaming: bool,
    t: &Theme,
) -> El {
    if renaming {
        let input = text_input(&s.widgets.inline_rename, t.panel_row)
            .single_line()
            .key(INLINE_RENAME_INPUT)
            .label("Rename thread")
            .color(t.text_strong)
            .caret(t.text_strong, t.k.selection())
            .no_focus_ring()
            .w_full()
            .on_edit(|s: &mut Desktop| &mut s.widgets.inline_rename)
            .on_input_change(|s: &mut Desktop, tx: &mut Tx| {
                let value = s.widgets.inline_rename.text.clone();
                s.dispatch(Msg::Sidebar(SidebarMsg::InlineRenameChanged(value)), tx);
            })
            .on_submit(|s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(Msg::Sidebar(SidebarMsg::CommitInlineRename), tx);
            })
            .on_input_key(|s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx| {
                if ev.key == Key::Escape {
                    s.dispatch(Msg::Sidebar(SidebarMsg::CancelInlineRename), tx);
                    return true;
                }
                false
            });
        return row()
            .key("row2-rename")
            .w_full()
            .h(22.0)
            .px(S4)
            .center_y()
            .radius(RADIUS_XS)
            .fill(t.canvas)
            .border(HAIRLINE, t.accent)
            .child(input);
    }

    let dot_color = match thread.attention {
        Some(Attention::Approval) if shelf != Shelf::Working => Some(t.warning),
        Some(Attention::Question) if shelf != Shelf::Working => Some(t.info),
        Some(Attention::Failure) => Some(t.danger),
        Some(Attention::Unread) => Some(t.accent),
        None if thread.lifecycle.unread => Some(t.accent),
        Some(Attention::Approval | Attention::Question) | None => None,
    };
    let title_ink = if selected || thread.lifecycle.unread {
        t.text_strong
    } else {
        t.text
    };
    row()
        .key("row2")
        .min_h(19.0)
        .gap(S6)
        .center_y()
        .a11y_hidden()
        .children(dot_color.map(ui::dot))
        .child(
            text(thread_title(thread))
                .font(t.panel_row)
                .color(title_ink)
                .group_color(t.text_strong)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate(),
        )
}
