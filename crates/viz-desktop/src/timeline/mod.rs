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
mod markdown;
mod plan_card;
mod search_card;
mod status_line;
mod subagents;
mod task_result;
mod user_row;
mod work_log;

use std::{cell::RefCell, rc::Rc};

use rune::{El, ScrollFade, ScrollSpec, Tx, col, icon, row, text};
use rune_kit::{ScrollBar, ScrollBarStyle};

pub use self::empty::DRAFT_PROJECT;
use self::user_row::StackPos;
use crate::{
    app::{Desktop, motion_switch},
    model::{Item, Transcript, TurnSpec, TurnView, project_turn, turn_items, turn_specs},
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
    /// active turn.
    pub pending_requests: Option<El>,
}

type RowRevision = (u64, String, u64, u64);

/// Indexed timeline rows use stable keys so Rune can anchor the viewport on
/// prepend.
#[derive(Default)]
pub struct TimelineRows(RefCell<Option<(RowRevision, Rc<RowIndex>)>>);

struct RowIndex {
    turns: Vec<TurnSpec>,
    rows: Vec<RowSpec>,
}

struct RowSpec {
    key: String,
    kind: RowKind,
}

#[derive(Clone, Copy)]
enum RowKind {
    Item(usize, StackPos),
    Work(usize),
    Answer(usize),
}

impl RowIndex {
    fn new(transcript: &Transcript) -> Self {
        let turns = turn_specs(transcript);
        let mut rows = Vec::new();
        for (turn_index, turn) in turns.iter().enumerate() {
            let user_count = turn_items(transcript, turn.start, turn.end)
                .filter(|(item, _)| matches!(item, Item::User(_)))
                .count();
            let mut users = 0;
            let mut cards = Vec::new();
            let mut has_work = false;
            for (offset, (item, _)) in turn_items(transcript, turn.start, turn.end).enumerate() {
                let index = turn.start + offset;
                let pos = if matches!(item, Item::User(_)) {
                    users += 1;
                    match (users, user_count) {
                        (1, 1) => StackPos::Single,
                        (1, _) => StackPos::First,
                        (i, n) if i == n => StackPos::Last,
                        _ => StackPos::Middle,
                    }
                } else {
                    StackPos::Single
                };
                let row = || RowSpec {
                    key: item_row_key(item),
                    kind: RowKind::Item(index, pos),
                };
                match item {
                    Item::User(_)
                    | Item::Continued { .. }
                    | Item::TaskResult(_)
                    | Item::Compaction(_) => {
                        rows.push(row());
                    }
                    Item::Plan(_) => cards.push((false, row())),
                    Item::Media(_) => cards.push((true, row())),
                    Item::Thinking { .. } | Item::Tool(_) => has_work = true,
                    _ => {}
                }
            }
            if has_work || turn_index + 1 == turns.len() {
                rows.push(RowSpec {
                    key: format!("{}-work", turn.key),
                    kind: RowKind::Work(turn_index),
                });
            }
            cards.sort_by_key(|(media, _)| *media);
            rows.extend(cards.into_iter().map(|(_, row)| row));
            rows.push(RowSpec {
                key: answer_key(turn),
                kind: RowKind::Answer(turn_index),
            });
        }
        Self { turns, rows }
    }

    fn build(&self, s: &Desktop, index: usize, t: &Theme) -> El {
        let transcript = s.model.transcript();
        let kind = self.rows[index].kind;
        match kind {
            RowKind::Item(index, pos) => {
                let (item, _) = turn_items(transcript, index, index + 1)
                    .next()
                    .expect("indexed item exists");
                render_item(s, item, pos, t)
            }
            RowKind::Work(index) | RowKind::Answer(index) => {
                let last = index + 1 == self.turns.len();
                let turn = project_turn(
                    &s.model,
                    transcript,
                    &self.turns[index],
                    last,
                    matches!(kind, RowKind::Answer(_)),
                );
                if matches!(kind, RowKind::Work(_)) {
                    render_work(s, &turn, t)
                } else {
                    render_answer(s, &turn, last, t)
                }
            }
        }
    }
}

fn item_row_key(item: &Item) -> String {
    match item {
        Item::User(_) => format!("{}-row", item.key()),
        Item::Continued { key } => format!("turn-{key}-continued"),
        _ => item.key().to_owned(),
    }
}

fn answer_key(turn: &TurnSpec) -> String {
    format!("{}-answer", turn.key)
}

/// Search result destination and disclosures required to reveal it.
pub struct MessageTarget {
    pub selector: String,
    pub folds: [Option<String>; 3],
}

/// Resolves a search result to its virtual row and optional child.
pub fn message_row(
    transcript: &Transcript,
    entry: viz_client::protocol::EntryId,
) -> Option<MessageTarget> {
    let key = transcript.message_row(entry)?;
    turn_specs(transcript).into_iter().find_map(|turn| {
        turn_items(transcript, turn.start, turn.end).find_map(|(item, _)| {
            if key != item.key() && key.strip_suffix("-row") != Some(item.key()) {
                return None;
            }
            let mut folds = [None, None, None];
            let selector = match item {
                Item::User(_) | Item::TaskResult(_) => item_row_key(item),
                Item::Compaction(_) => {
                    folds[0] = Some(format!("{key}-summary"));
                    key.to_owned()
                }
                Item::Text { .. } => format!("{}/{key}", answer_key(&turn)),
                Item::Thinking { .. } | Item::Tool(_) => {
                    folds[0] = Some(format!("{}-fold", turn.key));
                    folds[1] = Some(format!("{}-live-earlier", turn.key));
                    if matches!(item, Item::Thinking { .. }) {
                        folds[2] = Some(key.to_owned());
                    }
                    format!("{}-work/{key}", turn.key)
                }
                _ => return None,
            };
            Some(MessageTarget { selector, folds })
        })
    })
}

impl TimelineRows {
    fn get(&self, s: &Desktop) -> (RowRevision, Rc<RowIndex>) {
        let rev = (
            s.model.connection.epoch,
            s.model.timeline_page_key(),
            s.model.transcript().rev,
            s.model.timeline.fold_rev,
        );
        let mut current = self.0.borrow_mut();
        let entry = current
            .get_or_insert_with(|| (rev.clone(), Rc::new(RowIndex::new(s.model.transcript()))));
        if entry.0 != rev {
            *entry = (rev, Rc::new(RowIndex::new(s.model.transcript())));
        }
        entry.clone()
    }
}

fn build_rows(s: &Desktop, t: &'static Theme) -> El {
    let (version, index) = s.widgets.timeline_rows.get(s);
    let keys = index.clone();
    let visible = index.clone();
    motion::fold_container(col().key("timeline-turns").w_full().items_center())
        .virtual_rows(
            index.rows.len(),
            84.0,
            move |_: &Desktop, i| keys.rows[i].key.clone().into(),
            move |s: &Desktop, i| index.build(s, i, t),
        )
        .virtual_rows_version(version)
        .on_visible_rows(move |s: &mut Desktop, range| visible_rows(s, &visible, range))
}

fn visible_rows(s: &mut Desktop, index: &RowIndex, range: std::ops::Range<usize>) -> bool {
    if range.is_empty() || !s.model.connected() {
        return false;
    }
    let paging = range.start <= 2
        && s.model.transcript().before.is_some()
        && s.model.history_request().is_none();
    if paging {
        s.dispatch_local(Msg::Timeline(TimelineMsg::LoadEarlier));
    }
    let requests: Vec<_> = index.rows[range]
        .iter()
        .flat_map(|spec| {
            let (start, end) = match spec.kind {
                RowKind::Item(i, _) => (i, i + 1),
                RowKind::Work(i) => (index.turns[i].start, index.turns[i].end),
                RowKind::Answer(_) => (0, 0),
            };
            turn_items(s.model.transcript(), start, end).filter(move |(item, _)| {
                matches!(spec.kind, RowKind::Work(_)) == matches!(item, Item::Tool(_))
            })
        })
        .filter_map(|(item, _)| {
            let thread = s.model.selection.selected?;
            let (entry, full) = item.partial_entry()?;
            let open = match item {
                Item::Plan(_) | Item::Compaction(_) | Item::TaskResult(_) => true,
                Item::User(user) => s.model.is_fold_open(&format!("{}-full", user.key), false),
                Item::Tool(tool) => s.model.is_fold_open(&tool.key, false),
                _ => false,
            };
            (open
                && full <= viz_client::protocol::limits::AUTO_HYDRATE_ENTRY_BYTES
                && !s.model.connection.live.has_entry_fetch(thread, entry))
            .then(|| (item.key().to_owned(), entry))
        })
        .collect();
    if requests.is_empty() {
        paging
    } else {
        s.dispatch_local(Msg::Timeline(TimelineMsg::HydrateEntries(requests)));
        true
    }
}

/// The scrolling timeline with a floating jump-to-latest pill and
/// empty, loading, or offline content when applicable.
#[expect(
    clippy::too_many_lines,
    reason = "builds the single timeline scroll viewport and flat growth stack"
)]
pub fn timeline(s: &Desktop, slots: TimelineSlots, t: &'static Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let thread = m.transcript();
    plan_card::mount_menu(s, t, layer);
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
        if thread.before.is_none()
            && let Some(source) = m
                .selection
                .selected
                .and_then(|id| m.summary(id))
                .and_then(|sum| sum.forked_from)
        {
            list = list.child(lane(dividers::forked_from_divider(s, source, t)));
        }
        list = list.child(build_rows(s, t));
        if thread.items.is_empty() && thread.acp_partial.is_empty() && m.busy() {
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

    let mut scroll = col()
        .key(page)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .transition(switch.enter(motion::TRAVEL_PANE))
        .scroll(ScrollSpec {
            stick_bottom: !m.detached(),
        })
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
    if m.detached() {
        scroll = scroll.on_at_bottom_if(detached_bottom);
    }
    if switch == motion::Switch::Fade {
        scroll = scroll.enter_delay(motion::TITLE_ENTER_DELAY);
    }

    let jump_pill = ((m.timeline.far_from_bottom || m.detached())
        && !is_empty
        && !(m.is_loading() && m.connected()))
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

fn detached_bottom(s: &mut Desktop, at_bottom: bool) -> bool {
    let Some(thread) = s.model.selection.selected else {
        return false;
    };
    let Some(window) = s.model.cache_mut().open_thread_mut(thread).history.as_mut() else {
        return false;
    };
    let prior = window.at_bottom.replace(at_bottom);
    let has_newer = window.after.is_some();
    if !at_bottom {
        return false;
    }
    if has_newer && s.model.connected() && s.model.history_request().is_none() {
        s.dispatch_local(Msg::Timeline(TimelineMsg::LoadLater));
        true
    } else if prior == Some(false) && !has_newer {
        s.dispatch_local(Msg::Timeline(TimelineMsg::ScrollToEnd));
        true
    } else {
        false
    }
}

fn render_item(s: &Desktop, item: &Item, pos: StackPos, t: &Theme) -> El {
    let card = match item {
        Item::User(user) => {
            return lane(user_row::user_row(s, user, pos, t).pb(S4)).arrives_whole();
        }
        Item::Continued { .. } => return lane(dividers::continued_divider(&item_row_key(item), t)),
        Item::TaskResult(result) => task_result::task_result(s, result, t),
        Item::Compaction(notice) => dividers::compaction_divider(s, notice, t),
        Item::Plan(plan) => plan_card::plan_card(s, plan, t),
        Item::Media(media) => dividers::media_card(s, media, t),
        _ => unreachable!("only independently rendered items have item rows"),
    };
    if let Some((entry, full)) = item.partial_entry()
        && full > viz_client::protocol::limits::AUTO_HYDRATE_ENTRY_BYTES
    {
        let key = item.key().to_owned();
        let title = match item {
            Item::Plan(_) => "Plan",
            Item::TaskResult(_) => "Task result",
            _ => "Compaction summary",
        };
        let action = rune_kit::Button::new(format!("View full {}", title.to_lowercase()))
            .variant(rune_kit::ButtonVariant::GhostMuted)
            .control_size(Size::Micro.kit())
            .disabled(!s.model.connected())
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::OpenEntryPreview {
                        key: key.clone(),
                        entry,
                        title: title.into(),
                    }),
                    tx,
                );
            })
            .build(&t.k)
            .key("view-entry");
        lane(col().child(card).child(action))
    } else {
        lane(card)
    }
}

fn render_work(s: &Desktop, turn: &TurnView<'_>, t: &Theme) -> El {
    let list = motion::fold_container(col().w_full().items_center());
    if turn.live {
        fold::append_live_working(s, turn, t, list)
    } else if !turn.work.is_empty() {
        fold::append_completed_fold(s, turn, t, list)
    } else {
        list
    }
}

fn render_answer(s: &Desktop, turn: &TurnView<'_>, is_latest: bool, t: &Theme) -> El {
    let footer = assistant_row::turn_footer(s, turn, is_latest, t);
    let mut answer = col().w_full().max_w(READING_WIDTH).pb(S16);
    if footer.is_some() || !turn.prose.iter().any(|(_, streaming)| *streaming) {
        answer = answer.hover_group();
    }
    for (item, streaming) in &turn.prose {
        if let Item::Text {
            key, text: source, ..
        } = item
        {
            answer = answer.child(assistant_row::prose(s, key, source, *streaming, t));
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
    answer.children(footer)
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
