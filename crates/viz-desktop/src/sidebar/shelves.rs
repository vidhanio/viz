//! Unified triage shelf stack (Drafts, Pinned, Working, Active, Snoozed) and
//! bottom-anchored Settled drawer.

use std::f32::consts::FRAC_PI_2;

use rune::{El, Icon, ScrollFade, ScrollSpec, col, icon, row, text};
use rune_kit::{ScrollBar, ScrollBarStyle};
use viz_client::protocol::Shelf;

use super::{
    card::{self, DragSlot},
    card_meta, draft,
};
use crate::{
    app::Desktop,
    model::{DragShelf, SHELF_SETTLED, SHELF_SNOOZED, SHELF_WORKING, SidebarOutline},
    motion,
    theme::{HAIRLINE, ICON_SMALL, RADIUS_CONTROL, RADIUS_SM, S2, S4, S6, S8, S10, S12, Theme},
    ui::{self, InlineStyle},
    update::{Msg, SidebarMsg},
};

/// Maximum height of the expanded bottom-anchored Settled drawer (points).
const SETTLED_MAX_HEIGHT: f32 = 180.0;

/// Primary scrollable shelves: Drafts, Pinned, Working, Active, Snoozed.
#[expect(
    clippy::too_many_lines,
    reason = "single top-to-bottom triage shelf pipeline matching sidebar.md §3 ordering"
)]
pub(super) fn primary_shelves(
    s: &Desktop,
    outline: &SidebarOutline<'_>,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let m = &s.model;
    let active_drag = m.sidebar.drag.filter(|drag| drag.active);
    let dragging_to_empty_pinned = active_drag
        .is_some_and(|drag| drag.source_shelf == DragShelf::Active && outline.pinned.is_empty());

    let mut stack = motion::fold_container(
        col()
            .key("sidebar-shelves")
            .role("tree")
            .label("Threads")
            .w_full()
            .pb(S8)
            .gap(S2),
    );

    if let Some(preview) = &outline.draft {
        stack = stack.child(draft::draft_card(s, preview, t));
    }

    if !outline.pinned.is_empty() || dragging_to_empty_pinned {
        let pin_target_lit = active_drag.is_some_and(|drag| drag.target_shelf == DragShelf::Pinned);
        stack = stack.child(
            row()
                .key("shelf-pinned-header")
                .w_full()
                .h(24.0)
                .px(S8)
                .gap(S6)
                .center_y()
                .a11y_hidden()
                .child(icon(&ui::PIN, 11.0).color(t.faint))
                .child(text("Pinned").font(t.micro_medium).color(t.muted))
                .child(rune::spacer())
                .child(shelf_count("shelf-pinned-count", outline.pinned.len(), t)),
        );
        if outline.pinned.is_empty() {
            stack = stack.child(
                row()
                    .key("drop-to-pin")
                    .w_full()
                    .h(34.0)
                    .px(S10)
                    .center()
                    .radius(RADIUS_CONTROL)
                    .fill(if pin_target_lit {
                        t.accent.alpha(0.10)
                    } else {
                        t.surface_recessed
                    })
                    .border(
                        HAIRLINE,
                        if pin_target_lit {
                            t.accent
                        } else {
                            t.border_dashed
                        },
                    )
                    .border_dash(4.0, 3.0)
                    .child(
                        text("Drop to pin")
                            .font(t.small_medium)
                            .color(if pin_target_lit {
                                t.accent_text
                            } else {
                                t.muted
                            }),
                    ),
            );
        } else {
            let mut list =
                motion::reflow_container(col().key("shelf-pinned-list").w_full().gap(S2));
            for (index, thread) in outline.pinned.iter().enumerate() {
                if active_drag.is_some_and(|drag| {
                    drag.target_shelf == DragShelf::Pinned
                        && drag.target_index == index
                        && drag.thread != thread.id
                }) {
                    list = list.child(card_meta::drop_indicator(format!("pin-drop-{index}"), t));
                }
                list = list.child(card::thread_card(
                    s,
                    outline,
                    thread,
                    Shelf::Pinned,
                    Some(DragSlot {
                        shelf: DragShelf::Pinned,
                        index,
                    }),
                    t,
                    layer,
                ));
            }
            if active_drag.is_some_and(|drag| {
                drag.target_shelf == DragShelf::Pinned && drag.target_index >= outline.pinned.len()
            }) {
                list = list.child(card_meta::drop_indicator("pin-drop-end", t));
            }
            stack = stack.child(list);
        }
        if !outline.working.is_empty() || !outline.active.is_empty() {
            stack = stack.child(shelf_divider("pinned-divider", t));
        }
    }

    if !outline.working.is_empty() {
        let open = m.sidebar.is_shelf_open(SHELF_WORKING);
        stack = stack.child(collapsible_shelf_header(
            "shelf-working-header",
            SHELF_WORKING,
            None,
            "Working",
            outline.working.len(),
            open,
            26.0,
            t,
        ));
        if open {
            let body = motion::fold_row(
                motion::reflow_container(col().key("shelf-working-body").w_full().gap(S2))
                    .children(outline.working.iter().map(|thread| {
                        card::thread_card(s, outline, thread, Shelf::Working, None, t, layer)
                    })),
            );
            stack = stack.child(body);
        }
        if !outline.active.is_empty() {
            stack = stack.child(shelf_divider("working-divider", t));
        }
    }

    if !outline.active.is_empty() {
        let mut list = motion::reflow_container(col().key("shelf-active-list").w_full().gap(S2));
        for (index, thread) in outline.active.iter().enumerate() {
            if active_drag.is_some_and(|drag| {
                drag.target_shelf == DragShelf::Active
                    && drag.target_index == index
                    && drag.thread != thread.id
            }) {
                list = list.child(card_meta::drop_indicator(format!("active-drop-{index}"), t));
            }
            list = list.child(card::thread_card(
                s,
                outline,
                thread,
                Shelf::Active,
                Some(DragSlot {
                    shelf: DragShelf::Active,
                    index,
                }),
                t,
                layer,
            ));
        }
        if active_drag.is_some_and(|drag| {
            drag.target_shelf == DragShelf::Active && drag.target_index >= outline.active.len()
        }) {
            list = list.child(card_meta::drop_indicator("active-drop-end", t));
        }
        stack = stack.child(list);
    } else if outline.primary_empty() {
        stack = stack.child(draft::empty_primary_state(s, t));
    }

    if !outline.snoozed.is_empty() {
        let open = m.sidebar.is_shelf_open(SHELF_SNOOZED);
        if !outline.pinned.is_empty() || !outline.working.is_empty() || !outline.active.is_empty() {
            stack = stack.child(shelf_divider("snoozed-divider", t));
        }
        stack = stack.child(collapsible_shelf_header(
            "shelf-snoozed-header",
            SHELF_SNOOZED,
            Some(&ui::CLOCK),
            "Snoozed",
            outline.snoozed.len(),
            open,
            28.0,
            t,
        ));
        if open {
            let body = motion::fold_row(
                motion::reflow_container(col().key("shelf-snoozed-body").w_full().gap(S2))
                    .children(outline.snoozed.iter().map(|thread| {
                        card::thread_card(s, outline, thread, Shelf::Snoozed, None, t, layer)
                    })),
            );
            stack = stack.child(body);
        }
    }

    stack
}

/// Bottom-anchored Settled drawer always visible above the sidebar footer.
pub(super) fn settled_drawer(
    s: &Desktop,
    outline: &SidebarOutline<'_>,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let m = &s.model;
    let open = m.sidebar.is_shelf_open(SHELF_SETTLED);
    let total = outline.settled.len();
    let mut drawer = motion::fold_container(col().key("shelf-settled-drawer").w_full())
        .child(ui::rule(t).key("settled-rule"))
        .child(collapsible_shelf_header(
            "shelf-settled-header",
            SHELF_SETTLED,
            None,
            "Settled",
            total,
            open,
            32.0,
            t,
        ));

    if open {
        let limit = m.sidebar.settled_limit;
        let visible = outline.settled.len().min(limit);
        let remaining = total.saturating_sub(visible);
        let mut inner = col().key("shelf-settled-list").w_full().pb(S6).gap(S2);
        if total == 0 {
            inner = inner.child(
                text("No settled threads")
                    .key("settled-empty")
                    .font(t.small)
                    .color(t.faint)
                    .px(S10)
                    .py(S8),
            );
        } else {
            inner = inner.children(outline.settled[..visible].iter().map(|thread| {
                card::thread_card(s, outline, thread, Shelf::Settled, None, t, layer)
            }));
            if remaining > 0 {
                inner = inner.child(
                    row()
                        .key("settled-load-more")
                        .w_full()
                        .pt(S4)
                        .center()
                        .child(
                            ui::inline_button(format!("Load more ({remaining})"))
                                .style(InlineStyle::Wash)
                                .on_press(|s: &mut Desktop| {
                                    s.dispatch_local(Msg::Sidebar(SidebarMsg::LoadMoreSettled));
                                })
                                .build(t),
                        ),
                );
            }
        }
        let scroll = col()
            .key("shelf-settled-scroll")
            .w_full()
            .max_h(SETTLED_MAX_HEIGHT)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .scroll_anchor()
            .scroll_fade(ScrollFade::strength(S12).linear())
            .child(inner)
            .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k));
        drawer = drawer.child(motion::fold_row(scroll));
    }
    drawer
}

fn shelf_count(key: impl Into<String>, count: usize, t: &Theme) -> El {
    #[expect(
        clippy::cast_precision_loss,
        reason = "thread counts fit comfortably within f64 exact range"
    )]
    let value = count as f64;
    motion::rolling_count(
        key,
        count.to_string(),
        value,
        t.mono_micro,
        t.faint,
        true,
        &t.k,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "shared collapsible shelf header builder for Working, Snoozed, and Settled"
)]
fn collapsible_shelf_header(
    header_key: &'static str,
    shelf_key: &'static str,
    leading: Option<&'static Icon>,
    label: &'static str,
    count: usize,
    open: bool,
    height: f32,
    t: &Theme,
) -> El {
    let mut header = row()
        .key(header_key)
        .role("button")
        .label(format!("{label} ({count})"))
        .expanded(open)
        .w_full()
        .h(height)
        .px(S8)
        .gap(S6)
        .center_y()
        .radius(RADIUS_SM)
        .hover_group()
        .hover(t.hover_wash)
        .press(t.pressed_wash)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx| {
            motion::toggle_anchored(header_key, tx);
            s.dispatch(Msg::Sidebar(SidebarMsg::ToggleShelf(shelf_key)), tx);
        });
    if let Some(glyph) = leading {
        header = header.child(icon(glyph, 13.0).color(t.faint).a11y_hidden());
    }
    header
        .child(
            text(label)
                .font(t.small_medium)
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(rune::spacer())
        .child(shelf_count(format!("{header_key}-count"), count, t))
        .child(
            motion::chevron(&ui::CHEVRON, ICON_SMALL, open, FRAC_PI_2)
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        )
}

fn shelf_divider(key: &'static str, t: &Theme) -> El {
    row()
        .key(key)
        .w_full()
        .px(S8)
        .py(S6)
        .a11y_hidden()
        .child(ui::rule(t))
}
