//! Unified triage shelf stack (Drafts, Pinned, Working, Active, Snoozed) and
//! bottom-anchored Settled drawer.

use std::f32::consts::FRAC_PI_2;

use rune::{El, Icon, ScrollFade, ScrollSpec, col, icon, row, text};
use rune_kit::{ScrollBar, ScrollBarStyle};
use viz_client::protocol::Shelf;

use super::{card, card_meta, draft};
use crate::{
    app::Desktop,
    model::{DragShelf, DropZone, SHELF_SETTLED, SHELF_SNOOZED, SHELF_WORKING, SidebarOutline},
    motion,
    theme::{HAIRLINE, ICON_SMALL, RADIUS_CONTROL, RADIUS_SM, S2, S4, S6, S8, S10, S12, Theme},
    ui,
    update::{Msg, SidebarMsg},
};

/// Maximum height of the expanded bottom-anchored Settled drawer (points).
const SETTLED_MAX_HEIGHT: f32 = 120.0;

/// Primary scrollable shelves: Drafts, Pinned, Working, Active, Snoozed.
#[expect(
    clippy::too_many_lines,
    reason = "single top-to-bottom triage shelf pipeline matching sidebar shelf ordering"
)]
pub(super) fn primary_shelves(
    s: &Desktop,
    outline: &SidebarOutline<'_>,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let m = &s.model;
    let measuring = m.sidebar.drag.is_some();
    let active_drag = m.sidebar.drag.as_ref().filter(|drag| drag.active);
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

    let mut draft_slot =
        motion::fold_container(col().key("shelf-draft-slot").w_full().clip_rounded(0.0));
    if let Some(preview) = &outline.draft {
        draft_slot = draft_slot.child(motion::fold_row(draft::draft_card(s, preview, t)));
    }
    stack = stack.child(draft_slot);

    if !outline.pinned.is_empty() || dragging_to_empty_pinned {
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
        let list = if outline.pinned.is_empty() {
            empty_drop_zone(
                "drop-to-pin",
                "Drop to pin",
                m.sidebar
                    .drag
                    .as_ref()
                    .is_some_and(|drag| drag.target == Some((DragShelf::Pinned, None))),
                t,
            )
        } else {
            reorderable_shelf(s, outline, Shelf::Pinned, DragShelf::Pinned, t, layer)
        };
        stack = stack.child(measure(list, measuring, DropZone::Shelf(DragShelf::Pinned)));
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
        let list = reorderable_shelf(s, outline, Shelf::Active, DragShelf::Active, t, layer);
        stack = stack.child(measure(list, measuring, DropZone::Shelf(DragShelf::Active)));
    } else if active_drag.is_some_and(|drag| drag.source_shelf == DragShelf::Pinned) {
        let targeted =
            active_drag.is_some_and(|drag| drag.target == Some((DragShelf::Active, None)));
        stack = stack.child(measure(
            empty_drop_zone("drop-to-active", "Drop to unpin", targeted, t),
            measuring,
            DropZone::Shelf(DragShelf::Active),
        ));
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

/// Captures settled layout in window points so the pointer and commit share a
/// drop target.
pub(super) fn measure(element: El, measuring: bool, zone: DropZone) -> El {
    if measuring {
        element.on_layout_tx(move |s: &mut Desktop, rect, tx| {
            if let Some(drag) = &mut s.model.sidebar.drag {
                drag.rects.insert(zone, rect);
            }
            tx.live_only();
        })
    } else {
        element
    }
}

fn empty_drop_zone(key: &'static str, label: &'static str, targeted: bool, t: &Theme) -> El {
    row()
        .key(key)
        .w_full()
        .h(34.0)
        .center()
        .radius(RADIUS_CONTROL)
        .fill(if targeted {
            t.hover_wash
        } else {
            t.surface_recessed
        })
        .child(text(label).font(t.small_medium).color(t.muted))
}

fn reorderable_shelf(
    s: &Desktop,
    outline: &SidebarOutline<'_>,
    shelf: Shelf,
    drag_shelf: DragShelf,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let (key, threads) = match drag_shelf {
        DragShelf::Pinned => ("shelf-pinned-list", &outline.pinned),
        DragShelf::Active => ("shelf-active-list", &outline.active),
    };
    let target = s.model.sidebar.drag.as_ref().and_then(|drag| drag.target);
    motion::reflow_container(col().key(key).w_full().gap(S2))
        .children(
            threads.iter().map(|thread| {
                card::thread_card(s, outline, thread, shelf, Some(drag_shelf), t, layer)
            }),
        )
        .children((target == Some((drag_shelf, None))).then(|| {
            card_meta::drop_indicator("drop-after-last", t).pin(
                Some(0.0),
                None,
                Some(0.0),
                Some(-3.0),
            )
        }))
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
                            rune_kit::Button::new(format!("Load more ({remaining})"))
                                .variant(rune_kit::ButtonVariant::GhostMuted)
                                .control_size(crate::theme::Size::Micro.kit())
                                .on_press(|s: &mut Desktop| {
                                    s.dispatch_local(Msg::Sidebar(SidebarMsg::LoadMoreSettled));
                                })
                                .build(&t.k),
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
