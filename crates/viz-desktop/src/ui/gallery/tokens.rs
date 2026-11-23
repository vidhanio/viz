//! Palette swatches, typography scale, project monograms, harness marks, and
//! effort glyphs column for the `components` gallery.

use rune::{Color, El, TextStyle, col, row, text};

use super::{gallery_card, swatch_chip};
use crate::{
    theme::{ICON, RADIUS_SM, S4, S6, S8, Theme},
    ui,
};

pub(super) fn palette_card(t: &Theme) -> El {
    let surfaces = row()
        .w_full()
        .gap(S6)
        .child(swatch_chip("canvas", t.canvas, t.hairline, t))
        .child(swatch_chip("sidebar", t.sidebar_bg, t.hairline, t))
        .child(swatch_chip("surface", t.surface, t.hairline, t))
        .child(swatch_chip("recess", t.surface_recessed, t.hairline, t))
        .child(swatch_chip("raised", t.surface_raised, t.border_overlay, t))
        .child(swatch_chip("drawer", t.surface_drawer, t.hairline, t))
        .child(swatch_chip("chip", t.chip_fill, t.hairline, t))
        .child(swatch_chip("mine", t.mine, t.hairline, t));

    let borders_washes = row()
        .w_full()
        .gap(S6)
        .child(swatch_chip("hair", t.canvas, t.hairline, t))
        .child(swatch_chip("ctrl", t.canvas, t.border_control, t))
        .child(swatch_chip("ovly", t.surface_raised, t.border_overlay, t))
        .child(swatch_chip("strg", t.canvas, t.border_strong, t))
        .child(swatch_chip("dash", t.surface_recessed, t.border_dashed, t))
        .child(swatch_chip("hover", t.hover_wash, t.hairline, t))
        .child(swatch_chip("sel", t.selected, t.hairline, t))
        .child(swatch_chip("press", t.pressed_wash, t.hairline, t));

    let semantics = row()
        .w_full()
        .gap(S6)
        .child(swatch_chip("accent", t.accent, t.accent, t))
        .child(swatch_chip("ok", t.success, t.success, t))
        .child(swatch_chip("warn", t.warning, t.warning, t))
        .child(swatch_chip("err", t.danger, t.danger, t))
        .child(swatch_chip("info", t.info, t.info, t))
        .child(swatch_chip("steer", t.steer, t.steer, t))
        .child(swatch_chip("deleg", t.delegated, t.delegated, t))
        .child(swatch_chip("rem", t.remote, t.remote, t));

    let diffs = row()
        .w_full()
        .gap(S6)
        .child(swatch_chip("+line", t.added, t.hairline, t))
        .child(swatch_chip("+word", t.added_word, t.hairline, t))
        .child(swatch_chip("-line", t.removed, t.hairline, t))
        .child(swatch_chip("-word", t.removed_word, t.hairline, t))
        .child(swatch_chip("selSub", t.selected_subtle, t.hairline, t))
        .child(swatch_chip("scrim", t.scrim, t.hairline, t))
        .child(contrast_pill("onAcc", t.accent, t.on_accent, t))
        .child(contrast_pill("onErr", t.danger, t.on_danger, t));

    let inks = row()
        .w_full()
        .gap(S8)
        .center_y()
        .child(text("strong").font(t.small_medium).color(t.text_strong))
        .child(text("text").font(t.small_medium).color(t.text))
        .child(
            text("secondary")
                .font(t.small_medium)
                .color(t.text_secondary),
        )
        .child(text("muted").font(t.small_medium).color(t.muted))
        .child(text("faint").font(t.small_medium).color(t.faint))
        .child(text("accent").font(t.small_medium).color(t.accent_text));

    let body = col()
        .w_full()
        .gap(S8)
        .child(surfaces)
        .child(borders_washes)
        .child(semantics)
        .child(diffs)
        .child(inks);
    gallery_card(
        "card-palette",
        "Palette & surfaces",
        "Oklab base16",
        body,
        t,
    )
}

fn contrast_pill(label: &str, fill: Color, ink: Color, t: &Theme) -> El {
    row()
        .px(S6)
        .h(18.0)
        .center()
        .radius(RADIUS_SM)
        .fill(fill)
        .child(text(label).font(t.nano).color(ink))
}

fn type_sample(name: &str, preview: &str, style: TextStyle, color: Color, t: &Theme) -> El {
    row()
        .w_full()
        .gap(S8)
        .center_y()
        .child(
            text(name)
                .font(t.mono_micro)
                .color(t.faint)
                .w(68.0)
                .truncate(),
        )
        .child(
            text(preview)
                .font(style)
                .color(color)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate(),
        )
}

pub(super) fn typography_card(t: &Theme) -> El {
    let entries: [(&str, &str, TextStyle, Color); 17] = [
        ("hero", "What should we work on?", t.hero, t.text_strong),
        (
            "hero_n",
            "Compact hero heading",
            t.hero_narrow,
            t.text_strong,
        ),
        ("dialog", "Project settings", t.dialog_title, t.text_strong),
        ("sect", "Section heading", t.section_heading, t.text_strong),
        ("title", "Breadcrumb & card title", t.title, t.text),
        ("body", "Assistant prose body 14/22.75", t.body, t.text),
        ("ui", "Standard UI control label", t.ui, t.text),
        (
            "ui_med",
            "Active tab & emphasized UI",
            t.ui_medium,
            t.text_strong,
        ),
        (
            "panel",
            "Sidebar thread card title 13/18",
            t.panel_row,
            t.text,
        ),
        (
            "small",
            "Metadata, timestamps & hints 12/16",
            t.small,
            t.muted,
        ),
        (
            "sm_med",
            "Section labels & compact pills",
            t.small_medium,
            t.text_secondary,
        ),
        (
            "micro",
            "System dividers & hunk headers 11/15",
            t.micro,
            t.muted,
        ),
        (
            "mic_med",
            "Emphasized micro metadata",
            t.micro_medium,
            t.text_secondary,
        ),
        ("nano", "OPEN · DRAFT · MAIN 10/12", t.nano, t.muted),
        (
            "mono",
            "fn build(t: &Theme) -> El { 12/18 }",
            t.mono,
            t.text,
        ),
        (
            "mono_sm",
            "+148 -12  crates/viz-desktop",
            t.mono_small,
            t.muted,
        ),
        ("mono_mc", "01:42 · 128k / 200k", t.mono_micro, t.faint),
    ];
    let body = col().w_full().gap(S4).children(
        entries
            .into_iter()
            .map(|(name, preview, style, color)| type_sample(name, preview, style, color, t)),
    );
    gallery_card(
        "card-type",
        "Typography scale",
        "17 semantic slots",
        body,
        t,
    )
}

pub(super) fn identity_card(t: &Theme) -> El {
    let projects = [
        "viz",
        "workbench",
        "field-notes",
        "api-core",
        "docs",
        "infra",
        "ops-bot",
        "mobile",
        "scratch",
    ];
    let monograms = row()
        .w_full()
        .gap(S6)
        .center_y()
        .children(
            projects
                .iter()
                .enumerate()
                .map(|(i, name)| ui::project_monogram(name, 24.0, Some(i), t)),
        )
        .child(ui::project_icon(
            "viz",
            Some(&ui::FOLDER_GIT2),
            24.0,
            None,
            t,
        ))
        .child(ui::project_icon(
            "docs",
            Some(&ui::FOLDER_CODE),
            24.0,
            Some(2),
            t,
        ))
        .child(ui::no_project_mark(24.0, t));

    let harnesses = row()
        .w_full()
        .gap(S8)
        .center_y()
        .child(text("Harnesses").font(t.small).color(t.muted))
        .child(ui::harness_glyph("claude", "Claude Code", ICON, true, t))
        .child(ui::harness_glyph("codex", "Codex", ICON, true, t))
        .child(ui::harness_glyph("pi", "pi", ICON, true, t))
        .child(ui::harness_glyph("omp", "oh-my-pi", ICON, true, t))
        .child(ui::harness_glyph(
            "antigravity",
            "Antigravity",
            ICON,
            true,
            t,
        ))
        .child(ui::harness_glyph(
            "custom",
            "Generic harness",
            ICON,
            false,
            t,
        ));

    let body = col().w_full().gap(S8).child(monograms).child(harnesses);
    gallery_card(
        "card-identity",
        "Projects & harnesses",
        "9-slot hash + glyphs",
        body,
        t,
    )
}
