//! Turn-grouped chat timeline: right-aligned user bubbles, canvas assistant
//! prose with horizontal-rule Markdown tables and framed code blocks, live
//! Working header and Completed Turn Fold, grouped `WorkLog` with tool
//! inspectors, `ChangedFilesTree`, subagent cards, `ProposedPlanCard`, system
//! dividers, left-gutter `TimelineMinimap`, and floating `ScrollToEndButton`.

mod ansi;
mod assistant_row;
mod changed_files;
mod diff_card;
mod dividers;
mod empty;
mod fold;
mod inspector;
mod minimap;
mod plan_card;
mod search_card;
mod subagents;
mod task_result;
mod user_row;
mod work_log;

use rune::{El, ScrollFade, ScrollSpec, Tx, col, icon, row, text};
use rune_kit::{ButtonVariant, ScrollBar, ScrollBarStyle};

pub use self::empty::DRAFT_PROJECT;
use self::user_row::StackPos;
use crate::{
    app::{Desktop, motion_switch},
    model::{Item, TurnView, project_turns},
    motion,
    theme::{
        EMPTY_TOP, HAIRLINE, ICON_SMALL, RADIUS_PILL, S4, S6, S8, S12, S16, S24, S32, Size, Theme,
    },
    ui,
    update::{Msg, TimelineMsg},
};

/// Distance in points from the scroll bottom before showing
/// [`scroll_to_end_button`].
const FAR_FROM_BOTTOM_PX: f32 = 120.0;

/// Extension slots mounted inside the timeline reading lane by sibling tracks.
#[derive(Default)]
pub struct TimelineSlots {
    /// Interactive approval or question card rendered directly beneath the
    /// active turn (wired by Phase 1 Track X).
    pub pending_requests: Option<El>,
}

/// The scrolling timeline, floating jump-to-latest pill, left-gutter minimap,
/// or empty/loading/offline state in its place.
pub fn timeline(s: &Desktop, slots: TimelineSlots, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let thread = m.transcript();
    let is_empty = thread.items.is_empty()
        && thread.before.is_none()
        && !m.busy()
        && slots.pending_requests.is_none()
        && crate::model::requests::pending(m).next().is_none();

    let turns = if is_empty || (m.is_loading() && m.connected()) {
        Vec::new()
    } else {
        project_turns(m, thread, m.busy())
    };

    let body = if m.is_loading() && m.connected() {
        empty::loading(t)
    } else if is_empty {
        empty::empty_state(s, t, layer).pt(EMPTY_TOP)
    } else {
        let mut list = col().key("items").w_full();
        if thread.before.is_some() {
            list = list.child(load_earlier(s, t).pb(S12));
        } else if let Some(source) = m
            .selection
            .selected
            .and_then(|id| m.summary(id))
            .and_then(|sum| sum.forked_from)
        {
            list = list.child(dividers::forked_from_divider(s, source, t));
        }
        let total = turns.len();
        for turn in &turns {
            let is_latest = turn.index + 1 == total;
            list = list.child(render_turn(s, turn, is_latest, t, layer));
        }
        if turns.is_empty() && m.busy() {
            list = list.child(bare_working(t));
        }
        if let Some(pending) = slots.pending_requests {
            list = list.child(col().key("pending-requests").w_full().pt(S8).child(pending));
        }
        list
    };

    let body = ui::reading(body)
        .key("timeline-body")
        .min_h(0.0)
        .pt(S24)
        .pb(S32);

    let page = match (m.selection.selected, &m.selection.new_thread) {
        (Some(id), _) => format!("page-{id}"),
        (None, Some(draft)) => format!("page-draft-{}", draft.id),
        (None, None) => "page-none".to_owned(),
    };
    let switch = motion_switch(m.selection.switch);

    let scroll = col()
        .key(page)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .transition(switch.enter(motion::TRAVEL_PANE))
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_fade(ScrollFade::strength(S24).linear())
        .scroll_anchor()
        .on_far_from_bottom_if(FAR_FROM_BOTTOM_PX, None, |s: &mut Desktop, far: bool| {
            if s.model.timeline.far_from_bottom == far {
                return false;
            }
            s.dispatch_local(Msg::Timeline(TimelineMsg::SetFarFromBottom(far)));
            true
        })
        .child(body)
        .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k));

    let map = minimap::minimap(s, &turns, t, layer);
    let jump_pill = (m.timeline.far_from_bottom && !is_empty && !(m.is_loading() && m.connected()))
        .then(|| scroll_to_end_button(m.timeline.unseen_activity, t));

    col()
        .key("timeline")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .clip_rounded(0.0)
        .child_exit(switch.exit(motion::TRAVEL_PANE))
        .child(scroll)
        .children(map)
        .children(jump_pill)
}

fn render_turn(
    s: &Desktop,
    turn: &TurnView<'_>,
    is_latest: bool,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let idx = turn.index;
    let mut section = col()
        .key(turn.key.clone())
        .w_full()
        .pb(S16)
        .hover_group()
        .on_visible(move |s: &mut Desktop, visible: bool, tx: &mut Tx| {
            if s.model.timeline.visible_turns.contains(&idx) != visible {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::SetTurnVisible {
                        index: idx,
                        visible,
                    }),
                    tx,
                );
            }
        });

    if !turn.users.is_empty() {
        let len = turn.users.len();
        let mut users = col().key("users").w_full().pb(S12);
        for (i, user) in turn.users.iter().enumerate() {
            let pos = match (len, i) {
                (1, _) => StackPos::Single,
                (_, 0) => StackPos::First,
                (n, idx) if idx + 1 == n => StackPos::Last,
                _ => StackPos::Middle,
            };
            let mut row_el = user_row::user_row(s, user, pos, t);
            if i + 1 < len {
                row_el = row_el.pb(S4);
            }
            users = users.child(row_el);
        }
        section = section.child(users);
    }

    if turn.continued {
        section = section.child(dividers::continued_divider(
            &format!("{}-continued", turn.key),
            t,
        ));
    }

    if let Some(result) = turn.task_result {
        section = section.child(task_result::task_result(s, result, t));
    }

    for notice in &turn.compactions {
        section = section.child(dividers::compaction_divider(s, notice, t));
    }

    if turn.live {
        section = section.child(fold::live_working(s, turn, t));
    } else if let Some(folded) = fold::completed_fold(s, turn, t) {
        section = section.child(folded);
    }

    if let Some(agents) = subagents::subagents_section(s, turn, t) {
        section = section.child(agents);
    }

    for plan in &turn.plans {
        section = section.child(plan_card::plan_card(s, plan, t, layer));
    }

    for media in &turn.media {
        section = section.child(dividers::media_card(media, t));
    }

    for (key, source) in &turn.omitted {
        section = section.child(dividers::omitted_divider(s, key, source, t));
    }

    for (item, streaming) in &turn.prose {
        if let Item::Text {
            key,
            entry_id,
            text: source,
            more,
            ..
        } = item
        {
            section = section.child(assistant_row::prose(
                s, key, *entry_id, source, *streaming, *more, t,
            ));
        }
    }

    if let Some(files) = changed_files::changed_files_card(s, turn, t) {
        section = section.child(files);
    }

    for (key, reason, words) in &turn.statuses {
        let redundant_cancel =
            *reason == "cancelled" && (!turn.work.is_empty() || turn.end.is_some());
        if !redundant_cancel {
            section = section.child(dividers::status_divider(key, reason, words, t));
        }
    }

    for (key, kind, message) in &turn.notices {
        if let Some(banner) = dividers::error_banner(s, key, *kind, message, t) {
            section = section.child(banner);
        }
    }

    if let Some(footer) = assistant_row::turn_footer(s, turn, is_latest, t) {
        section = section.child(footer);
    }

    section
}

fn bare_working(t: &Theme) -> El {
    row()
        .key("working-bare")
        .role("status")
        .label("Working")
        .w_full()
        .px(S4)
        .py(S8)
        .gap(S6)
        .center_y()
        .child(ui::spinner(t))
        .child(text("Working…").font(t.ui).color(t.muted))
}

/// Flat floating jump-to-latest pill (`t.surface_raised` + `1px
/// t.border_overlay`, zero drop shadow) displayed when scrolled > `120pt` above
/// the bottom.
fn scroll_to_end_button(unseen: bool, t: &Theme) -> El {
    let mut pill = row()
        .key("scroll-to-end")
        .role("button")
        .label("Jump to latest")
        .h(Size::Compact.height())
        .px(S12)
        .gap(S6)
        .center_y()
        .radius(RADIUS_PILL)
        .fill(t.surface_raised)
        .hover(t.hover_wash)
        .border(HAIRLINE, t.border_overlay)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .transition(motion::popover(motion::TRAVEL_SMALL))
        .on_press_tx(|s: &mut Desktop, tx: &mut Tx| {
            s.dispatch(Msg::Timeline(TimelineMsg::ScrollToEnd), tx);
        });
    if unseen {
        pill = pill.child(ui::dot(t.accent));
    }
    pill = pill
        .child(
            icon(&ui::ARROW_DOWN, ICON_SMALL)
                .color(t.text_secondary)
                .a11y_hidden(),
        )
        .child(
            text(if unseen {
                "New activity"
            } else {
                "Jump to latest"
            })
            .font(t.small_medium)
            .color(t.text),
        );
    row()
        .key("scroll-to-end-anchor")
        .pin(Some(0.0), None, Some(0.0), Some(S12))
        .center()
        .child(pill)
}

/// Older history, between two rules.
fn load_earlier(s: &Desktop, t: &Theme) -> El {
    let m = &s.model;
    let label = if m.is_history_loading() {
        "Loading earlier messages…"
    } else {
        "Load earlier messages"
    };
    row()
        .key("load-earlier")
        .w_full()
        .gap(S8)
        .center_y()
        .child(ui::rule(t).grow(1.0).w(0.0))
        .child(
            ui::button(label, ButtonVariant::GhostMuted, Size::Small)
                .disabled(m.is_history_loading() || !m.connected())
                .on_press_tx(|s: &mut Desktop, tx| {
                    s.dispatch(Msg::Timeline(TimelineMsg::LoadEarlier), tx);
                })
                .build(&t.k),
        )
        .child(ui::rule(t).grow(1.0).w(0.0))
}
