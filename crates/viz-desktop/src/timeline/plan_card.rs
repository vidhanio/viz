//! Proposed Plan card: structured priority/status checklists and collapsible
//! (`416pt` max height with bottom fade) Markdown plans with overflow actions.

use rune::{El, Tx, col, el, icon, row, text};
use rune_kit::{IconButtonVariant, Markdown, Menu, MenuItem, Placement};
use viz_client::protocol::PlanPriority;

use crate::{
    app::Desktop,
    model::{PlanBody, PlanCard, PlanStep, ToolStatus, relative_path},
    motion,
    theme::{CARD_INSET, HAIRLINE, ICON_MD, RADIUS_CARD, S4, S6, S8, Size, Theme},
    ui::{self, BadgeSize, InlineStyle, Tone},
    update::{Msg, TimelineMsg},
};

/// Auto-collapse ceiling for long Markdown plans (`max-h-[26rem]` = `416pt`).
const PLAN_COLLAPSE_HEIGHT: f32 = 416.0;
/// Bottom fade band height over clipped long Markdown plans (`40pt`).
const PLAN_FADE_HEIGHT: f32 = 40.0;
/// Line threshold above which a Markdown plan offers collapse/expand.
const PLAN_COLLAPSE_LINES: usize = 16;
/// Character threshold above which a Markdown plan offers collapse/expand.
const PLAN_COLLAPSE_CHARS: usize = 1_000;

/// Renders a checklist or proposed Markdown [`PlanCard`].
pub fn plan_card(s: &Desktop, plan: &PlanCard, t: &Theme, layer: &mut Vec<El>) -> El {
    let copy_text = serialize_plan(&plan.content);
    let menu_open = s.model.timeline.open_plan_menu.as_deref() == Some(plan.key.as_str());
    let toggle_key = plan.key.clone();
    let menu_trigger = ui::icon_button(
        &ui::MORE,
        "Plan actions",
        IconButtonVariant::GhostMuted,
        Size::Micro,
    )
    .on_press(move |s: &mut Desktop| {
        s.dispatch_local(Msg::Timeline(TimelineMsg::TogglePlanMenu(
            toggle_key.clone(),
        )));
    })
    .build(&t.k)
    .key(format!("{}-more", plan.key));

    let mut menu = Menu::new(format!("{}-plan-menu", plan.key))
        .placement(Placement::BottomEnd)
        .min_width(164.0)
        .open(menu_open)
        .on_close(|s: &mut Desktop| s.dispatch_local(Msg::Timeline(TimelineMsg::ClosePlanMenu)))
        .trigger(menu_trigger);
    if menu_open {
        let copy_id = format!("copy-{}", plan.key);
        menu = menu.item(MenuItem::new("Copy plan").icon(&ui::COPY).on_select_tx(
            move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(Msg::Timeline(TimelineMsg::ClosePlanMenu), tx);
                s.dispatch(
                    Msg::Timeline(TimelineMsg::CopyWithFeedback {
                        key: copy_id.clone(),
                        text: copy_text.clone(),
                    }),
                    tx,
                );
            },
        ));
    }
    let menu_el = menu.build(&t.overlay, layer);

    let subtitle = plan_header_subtitle(s, plan, t);
    let header = row()
        .key("header")
        .w_full()
        .px(CARD_INSET)
        .py(S8)
        .gap(S8)
        .center_y()
        .fill(t.surface_recessed)
        .child(
            ui::badge("Plan")
                .tone(Tone::Accent)
                .size(BadgeSize::Nano)
                .icon(&ui::PLAN)
                .build(t)
                .key("plan-badge"),
        )
        .child(subtitle)
        .child(el().grow(1.0).min_w(0.0).a11y_hidden())
        .child(menu_el);

    let body = match &plan.content {
        PlanBody::Items(steps) => checklist_body(steps, t),
        PlanBody::Markdown { text: source, .. } => markdown_body(s, &plan.key, source, t),
    };

    let border = if t.k.dark {
        t.hairline
    } else {
        t.border_control
    };
    col()
        .key(plan.key.clone())
        .role("region")
        .label("Plan")
        .w_full()
        .my(S6)
        .radius(RADIUS_CARD)
        .fill(t.surface)
        .border(HAIRLINE, border)
        .clip_rounded(RADIUS_CARD)
        .child(header)
        .child(ui::rule(t))
        .child(body)
}

fn plan_header_subtitle(s: &Desktop, plan: &PlanCard, t: &Theme) -> El {
    match &plan.content {
        PlanBody::Items(steps) => {
            let done = steps
                .iter()
                .filter(|step| step.status == ToolStatus::Done)
                .count();
            let total = steps.len();
            #[expect(
                clippy::cast_precision_loss,
                reason = "plan step counts are tiny integers"
            )]
            let value = done as f64;
            row()
                .gap(S4)
                .center_y()
                .child(motion::rolling_count(
                    format!("{}-done", plan.key),
                    format!("{done}/{total}"),
                    value,
                    t.small_medium,
                    t.text_secondary,
                    false,
                    &t.k,
                ))
                .child(text("completed").font(t.small).color(t.muted))
        }
        PlanBody::Markdown { path, .. } => {
            let label = path
                .as_deref()
                .map_or("Proposed plan", |p| relative_path(&s.model, p));
            text(label)
                .font(if path.is_some() {
                    t.mono_small
                } else {
                    t.small_medium
                })
                .color(t.text_secondary)
                .truncate()
                .shrink()
                .min_w(0.0)
        }
    }
}

fn checklist_body(steps: &[PlanStep], t: &Theme) -> El {
    col()
        .key("steps")
        .role("list")
        .w_full()
        .padding(CARD_INSET)
        .gap(S6)
        .children(steps.iter().enumerate().map(|(index, step)| {
            let (mark, ink) = match step.status {
                ToolStatus::Done => (icon(&ui::CIRCLE_CHECK, ICON_MD).color(t.success), t.muted),
                ToolStatus::Running => (ui::spinner(t), t.text),
                ToolStatus::Pending | ToolStatus::Failed | ToolStatus::Abandoned => {
                    (icon(&ui::CIRCLE_DASHED, ICON_MD).color(t.faint), t.text)
                }
            };
            let mut r = row()
                .key(format!("step-{index}"))
                .role("listitem")
                .w_full()
                .gap(S8)
                .items_start()
                .child(ui::lead(mark, t.ui.line_height))
                .child(
                    text(&step.text)
                        .font(t.ui)
                        .color(ink)
                        .wrap()
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0),
                );
            if step.priority == PlanPriority::High && step.status != ToolStatus::Done {
                r = r.child(
                    ui::badge("High")
                        .tone(Tone::Warning)
                        .size(BadgeSize::Nano)
                        .build(t)
                        .key("priority"),
                );
            }
            r
        }))
}

fn markdown_body(s: &Desktop, key: &str, source: &str, t: &Theme) -> El {
    let collapsible = should_collapse_plan(source);
    let fold_key = format!("{key}-expanded");
    let expanded = !collapsible
        || s.model.timeline.is_open(
            s.model.selection.selected,
            &s.model.disclosure.expanded,
            &fold_key,
            None,
            false,
        );
    let md = Markdown::new(source.to_owned())
        .style(t.markdown().body_font(t.ui))
        .selectable(true)
        .copy_source(true)
        .on_link_tx(|s: &mut Desktop, url: &str, tx: &mut Tx| {
            s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.to_owned())), tx);
        })
        .build(&t.k)
        .w_full();

    let mut content = col().w_full().padding(CARD_INSET).child(md);
    if collapsible && !expanded {
        let fade = el()
            .key("plan-fade")
            .h(PLAN_FADE_HEIGHT)
            .pin(Some(0.0), None, Some(0.0), Some(0.0))
            .gradient(t.surface.alpha(0.0), t.surface)
            .a11y_hidden();
        content = content
            .max_h(PLAN_COLLAPSE_HEIGHT)
            .clip_rounded(0.0)
            .child(fade);
    }

    if !collapsible {
        return content;
    }

    let toggle_key = fold_key;
    let footer = row()
        .key("plan-toggle")
        .w_full()
        .px(CARD_INSET)
        .py(S6)
        .center()
        .child(
            ui::inline_button(if expanded {
                "Collapse plan"
            } else {
                "Expand plan"
            })
            .icon(if expanded {
                &ui::CHEVRON_UP
            } else {
                &ui::CHEVRON_OPEN
            })
            .style(InlineStyle::InkOnly)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::ToggleFold {
                        key: toggle_key.clone(),
                        legacy_key: None,
                        default_open: false,
                        anchor: None,
                    }),
                    tx,
                );
            })
            .build(t)
            .key("toggle"),
        );

    col()
        .key("markdown-body")
        .w_full()
        .child(content)
        .child(ui::rule(t))
        .child(footer)
}

fn should_collapse_plan(source: &str) -> bool {
    source.len() > PLAN_COLLAPSE_CHARS || source.lines().count() > PLAN_COLLAPSE_LINES
}

fn serialize_plan(content: &PlanBody) -> String {
    match content {
        PlanBody::Markdown { text, .. } => text.clone(),
        PlanBody::Items(steps) => steps
            .iter()
            .map(|step| {
                let mark = match step.status {
                    ToolStatus::Done => "x",
                    ToolStatus::Running => "-",
                    _ => " ",
                };
                format!("- [{mark}] {}", step.text)
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}
