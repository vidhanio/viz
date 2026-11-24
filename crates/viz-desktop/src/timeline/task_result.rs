//! Delegated task outcome delivery cards (`TaskResult`) with expandable
//! Markdown detail and drill-down link to the child thread.

use std::f32::consts::FRAC_PI_2;

use rune::{El, Tx, col, icon, row, text};
use rune_kit::{ButtonVariant, Markdown};

use crate::{
    app::Desktop,
    model::{TaskResult, first_non_empty_line},
    motion,
    theme::{CARD_INSET, HAIRLINE, ICON_MD, ICON_SMALL, RADIUS_CARD, S4, S6, S8, S12, Size, Theme},
    ui,
    update::{Msg, TimelineMsg},
};

/// Delegated task outcome card delivered back into its parent thread.
pub fn task_result(s: &Desktop, result: &TaskResult, t: &Theme) -> El {
    let m = &s.model;
    let fold_key = format!("{}-detail", result.key);
    let has_detail = !result.detail.is_empty();
    let open = has_detail && m.is_fold_open(&fold_key, false);
    let heading = task_result_heading(s, result, fold_key, has_detail, open, t);

    let mut card = col()
        .key("card")
        .w_full()
        .radius(RADIUS_CARD)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .clip_rounded(RADIUS_CARD)
        .child(heading);

    if open {
        let cwd = s.model.thread_state().map(|st| st.cwd.as_str());
        card = card.child(
            col().key("detail").w_full().child(ui::rule(t)).child(
                col().w_full().padding(S12).child(
                    super::markdown::with_file_pills(
                        Markdown::new(result.detail.clone())
                            .style(t.markdown().body_font(t.ui).text(t.muted).link(t.text))
                            .selectable(true),
                        cwd,
                        t,
                    )
                    .build(&t.k)
                    .w_full(),
                ),
            ),
        );
    }

    col()
        .key(result.key.clone())
        .w_full()
        .py(S4)
        .arrives_whole()
        .child(card)
}

fn task_result_heading(
    s: &Desktop,
    result: &TaskResult,
    fold_key: String,
    has_detail: bool,
    open: bool,
    t: &Theme,
) -> El {
    let task = s.model.tasks().iter().find(|task| task.id == result.task);
    let title = task
        .map(|task| task.title.as_str())
        .filter(|title| !title.is_empty())
        .or_else(|| (!result.title.is_empty()).then_some(result.title.as_str()))
        .unwrap_or("Delegated task");
    let (word, ink) = match result.status.as_str() {
        "completed" => ("finished", t.success),
        "continued" => ("continued", t.muted),
        "failed" => ("failed", t.danger),
        "cancelled" => ("was stopped", t.muted),
        _ => ("ended", t.muted),
    };
    let preview = (!open)
        .then(|| first_non_empty_line(&result.detail))
        .flatten();
    let header_key = format!("{}-head", result.key);

    let mut heading = row()
        .key(header_key.clone())
        .label(format!("{title} {word}"))
        .w_full()
        .px(CARD_INSET)
        .py(S8)
        .gap(S6)
        .center_y()
        .child(icon(&ui::INBOX, ICON_MD).color(t.delegated).a11y_hidden())
        .child(
            text(title)
                .font(t.ui_medium)
                .color(t.text)
                .truncate()
                .shrink(),
        )
        .child(text(word).font(t.small_medium).color(ink))
        .child(
            text(preview.unwrap_or_default())
                .font(t.small)
                .color(t.muted)
                .truncate()
                .shrink()
                .grow(1.0)
                .min_w(0.0),
        );

    if has_detail {
        heading = heading
            .role("button")
            .expanded(open)
            .hover(t.hover_wash)
            .hover_group()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::ToggleFold {
                        key: fold_key.clone(),
                        default_open: false,
                        anchor: Some(header_key.clone()),
                    }),
                    tx,
                );
            })
            .child(
                motion::chevron(&ui::CHEVRON, ICON_SMALL, open, FRAC_PI_2)
                    .key("chevron")
                    .color(t.faint)
                    .group_color(t.text)
                    .a11y_hidden(),
            );
    }

    if let Some((child, environment)) = task
        .filter(|task| task.reachable)
        .and_then(|task| task.thread.map(|child| (child, task.environment.clone())))
    {
        heading = heading.child(
            rune_kit::Button::new("Open")
                .variant(ButtonVariant::GhostMuted)
                .control_size(Size::Small.kit())
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(
                        Msg::Timeline(TimelineMsg::OpenThreadLink {
                            thread: child,
                            environment: Some(environment.clone()),
                        }),
                        tx,
                    );
                })
                .build(&t.k)
                .key("open"),
        );
    }

    heading
}
