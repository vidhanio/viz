//! Turn-grouped chat timeline: right-aligned user bubbles, canvas assistant
//! prose with horizontal-rule Markdown tables and framed code blocks, live
//! Working header and Completed Turn Fold, grouped `WorkLog` with tool
//! inspectors, `ChangedFilesTree`, subagent cards, `ProposedPlanCard`, system
//! dividers, and a floating jump-to-latest button.

mod ansi;
mod assistant_row;
mod changed_files;
mod diff_card;
mod dividers;
mod empty;
pub mod file_preview;
mod fold;
mod inspector;
mod plan_card;
mod search_card;
mod status_line;
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
        EMPTY_TOP, HAIRLINE, ICON_SMALL, RADIUS_PILL, READING_INSET, READING_WIDTH, S4, S6, S8,
        S12, S16, S24, S32, Size, Theme,
    },
    ui,
    update::{Msg, TimelineMsg},
};

/// Constrains a direct child of the timeline's flat growth stack to the
/// centered reading lane width.
pub fn lane(child: El) -> El {
    child.w_full().max_w(READING_WIDTH)
}

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

/// The scrolling timeline with a floating jump-to-latest pill and
/// empty, loading, or offline content when applicable.
#[expect(
    clippy::too_many_lines,
    reason = "builds the single timeline scroll viewport and flat growth stack"
)]
pub fn timeline(s: &Desktop, slots: TimelineSlots, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let thread = m.transcript();
    let status = status_line::timeline_status(s, t);
    let is_empty = thread.items.is_empty()
        && thread.before.is_none()
        && !m.busy()
        && slots.pending_requests.is_none()
        && status.is_none()
        && crate::model::requests::pending(m).next().is_none();

    let body = if m.is_loading() && m.connected() {
        ui::reading(empty::loading(t))
            .key("timeline-body")
            .min_h(0.0)
            .pt(S24)
            .pb(S32)
    } else if is_empty {
        ui::reading(empty::empty_state(s, t, layer).pt(EMPTY_TOP))
            .key("timeline-body")
            .min_h(0.0)
            .pt(S24)
            .pb(S32)
    } else {
        let mut list = motion::fold_container(
            col()
                .key("timeline-body")
                .w_full()
                .px(READING_INSET)
                .items_center()
                .min_h(0.0)
                .pt(S24)
                .pb(S32),
        )
        .child_exit(motion::fade(motion::SETTLE_PAIR));
        if thread.before.is_some() {
            list = list.child(lane(load_earlier(s, t).pb(S12)));
        } else if let Some(source) = m
            .selection
            .selected
            .and_then(|id| m.summary(id))
            .and_then(|sum| sum.forked_from)
        {
            list = list.child(lane(dividers::forked_from_divider(s, source, t)));
        }
        let turns = project_turns(m, thread, m.busy());
        for (index, turn) in turns.iter().enumerate() {
            let is_latest = index + 1 == turns.len();
            list = append_turn(s, turn, is_latest, t, layer, list);
        }
        if turns.is_empty() && m.busy() {
            list = list.child(lane(bare_working(t)));
        }
        if let Some(pending) = slots.pending_requests {
            list = list.child(lane(
                col().key("pending-requests").w_full().pt(S8).child(pending),
            ));
        }
        if let Some(status) = status {
            list = list.child(lane(status));
        }
        list
    };

    let page = m.timeline_page_key();
    let switch = motion_switch(m.selection.switch);
    let scroll_pos = s.widgets.timeline_scroll.clone();

    let mut scroll = col()
        .key(page)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .transition(switch.enter(motion::TRAVEL_PANE))
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_fade(ScrollFade::strength(S24).linear())
        .scroll_anchor()
        .on_scroll_offset(move |pos| {
            scroll_pos.set([pos.offset[1], pos.max[1]]);
        })
        .on_far_from_bottom_if(FAR_FROM_BOTTOM_PX, None, |s: &mut Desktop, far: bool| {
            if s.model.timeline.far_from_bottom == far {
                return false;
            }
            s.dispatch_local(Msg::Timeline(TimelineMsg::SetFarFromBottom(far)));
            true
        })
        .child(body)
        .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k));
    if switch == motion::Switch::Fade {
        scroll = scroll.enter_delay(motion::TITLE_ENTER_DELAY);
    }

    let jump_pill = (m.timeline.far_from_bottom && !is_empty && !(m.is_loading() && m.connected()))
        .then(|| scroll_to_end_button(m.timeline.unseen_activity, t));

    let pages = col()
        .key("timeline-pages")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .clip_rounded(0.0)
        .child_exit(switch.exit(motion::TRAVEL_PANE))
        .child(scroll);

    col()
        .key("timeline")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(pages)
        .children(jump_pill)
}

#[expect(
    clippy::too_many_lines,
    reason = "appends every flat growth-stack row for one conversation turn"
)]
fn append_turn(
    s: &Desktop,
    turn: &TurnView<'_>,
    is_latest: bool,
    t: &Theme,
    layer: &mut Vec<El>,
    mut list: El,
) -> El {
    if !turn.users.is_empty() {
        let len = turn.users.len();
        let mut users = col().key(format!("{}-users", turn.key)).w_full().pb(S4);
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
        list = list.child(lane(users).arrives_whole());
    }

    if turn.continued {
        list = list.child(lane(dividers::continued_divider(
            &format!("{}-continued", turn.key),
            t,
        )));
    }

    if let Some(result) = turn.task_result {
        list = list.child(lane(task_result::task_result(s, result, t)));
    }

    for notice in &turn.compactions {
        list = list.child(lane(dividers::compaction_divider(s, notice, t)));
    }

    if turn.live {
        list = fold::append_live_working(s, turn, t, list);
    } else if !turn.work.is_empty() {
        list = fold::append_completed_fold(s, turn, t, list);
    }

    for plan in &turn.plans {
        list = list.child(lane(plan_card::plan_card(s, plan, t, layer)));
    }

    for media in &turn.media {
        list = list.child(lane(dividers::media_card(media, t)));
    }

    for (key, source) in &turn.omitted {
        list = list.child(lane(dividers::omitted_divider(s, key, source, t)));
    }

    let footer = assistant_row::turn_footer(s, turn, is_latest, t);
    let has_streaming_prose = turn.prose.iter().any(|(_, streaming)| *streaming);
    if has_streaming_prose && footer.is_none() {
        for (item, streaming) in &turn.prose {
            if let Item::Text {
                key,
                entry_id,
                text: source,
                more,
                ..
            } = item
            {
                list = list.child(lane(assistant_row::prose(
                    s, key, *entry_id, source, *streaming, *more, t,
                )));
            }
        }
        if let Some(files) = changed_files::changed_files_card(s, turn, t) {
            list = list.child(lane(files));
        }
        for (key, reason, words) in &turn.statuses {
            let redundant_cancel =
                *reason == "cancelled" && (!turn.work.is_empty() || turn.end.is_some());
            if !redundant_cancel {
                list = list.child(lane(dividers::status_divider(key, reason, words, t)));
            }
        }
        for (key, kind, message) in &turn.notices {
            if let Some(banner) = dividers::error_banner(s, key, *kind, message, t) {
                list = list.child(lane(banner));
            }
        }
        list = list.child(lane(
            rune::el()
                .key(format!("{}-pad", turn.key))
                .h(S16)
                .a11y_hidden(),
        ));
    } else {
        let mut answer = col()
            .key(format!("{}-answer", turn.key))
            .w_full()
            .pb(S16)
            .hover_group();
        for (item, streaming) in &turn.prose {
            if let Item::Text {
                key,
                entry_id,
                text: source,
                more,
                ..
            } = item
            {
                answer = answer.child(assistant_row::prose(
                    s, key, *entry_id, source, *streaming, *more, t,
                ));
            }
        }
        if let Some(files) = changed_files::changed_files_card(s, turn, t) {
            answer = answer.child(files);
        }
        for (key, reason, words) in &turn.statuses {
            let redundant_cancel =
                *reason == "cancelled" && (!turn.work.is_empty() || turn.end.is_some());
            if !redundant_cancel {
                answer = answer.child(dividers::status_divider(key, reason, words, t));
            }
        }
        for (key, kind, message) in &turn.notices {
            if let Some(banner) = dividers::error_banner(s, key, *kind, message, t) {
                answer = answer.child(banner);
            }
        }
        if let Some(footer) = footer {
            answer = answer.child(footer);
        }
        list = list.child(lane(answer));
    }

    list
}

fn bare_working(t: &Theme) -> El {
    let lead = row()
        .gap(S6)
        .center_y()
        .child(ui::spinner(t))
        .child(text("Working…").font(t.ui).color(t.muted));
    row()
        .key("working-bare")
        .role("status")
        .label("Working")
        .w_full()
        .px(S4)
        .py(S8)
        .center_y()
        .child(motion::text_shimmer(
            "working-bare-shimmer",
            true,
            lead,
            rune::Color::WHITE,
            if t.k.dark { 0.9 } else { 0.62 },
        ))
}

/// Frosted floating jump-to-latest pill displayed when scrolled > `120pt` above
/// the bottom.
fn scroll_to_end_button(unseen: bool, t: &Theme) -> El {
    let frost = t.surface_raised.alpha(if t.k.dark { 0.82 } else { 0.88 });
    let mut pill = row()
        .key("scroll-to-end")
        .role("button")
        .label("Jump to latest")
        .h(Size::Compact.height())
        .px(S12)
        .gap(S6)
        .center_y()
        .radius(RADIUS_PILL)
        .backdrop_blur(10.0)
        .fill(frost)
        .hover(t.hover_wash)
        .shadow(
            4.0,
            16.0,
            rune::Color::BLACK.alpha(if t.k.dark { 0.36 } else { 0.14 }),
        )
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
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
        .transition(motion::popover(motion::TRAVEL_SMALL))
        .child(pill)
}

/// Quiet top-of-history auto-loading indicator with manual fallback.
fn load_earlier(s: &Desktop, t: &Theme) -> El {
    let m = &s.model;
    let center = if m.is_history_loading() {
        row().gap(S6).center_y().child(ui::spinner(t)).child(
            text("Loading earlier messages…")
                .font(t.small)
                .color(t.muted),
        )
    } else {
        rune_kit::Button::new("Load earlier messages")
            .variant(ButtonVariant::GhostMuted)
            .control_size(Size::Small.kit())
            .disabled(!m.connected())
            .on_press_tx(|s: &mut Desktop, tx| {
                s.dispatch(Msg::Timeline(TimelineMsg::LoadEarlier), tx);
            })
            .build(&t.k)
    };
    row()
        .key("load-earlier")
        .w_full()
        .gap(S8)
        .center_y()
        .child(ui::rule(t).grow(1.0).w(0.0))
        .child(center)
        .child(ui::rule(t).grow(1.0).w(0.0))
}
