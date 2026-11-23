//! Oklab contrast derivation mapping a base16 [`Scheme`] onto [`Theme`].

use rune::{Color, Shadow};
use rune_kit::{Kit, Palette, PaletteFocus, PaletteSeed, StateColors};

use super::{
    Theme,
    scheme::Scheme,
    tokens::{HAIRLINE, RADIUS_CONTROL, RADIUS_SM, Size},
    typography::{build_syntax, build_typography},
};

/// WCAG relative-luminance contrast ratio of two opaque colors.
pub(super) fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (a.luminance(), b.luminance());
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Lowest contrast ratio of `c` across every opaque surface in `surfaces`.
fn worst(c: Color, surfaces: &[Color]) -> f32 {
    surfaces
        .iter()
        .map(|&s| contrast(c, s))
        .fold(f32::INFINITY, f32::min)
}

/// Pushes `c`'s Oklab lightness away from `surfaces` (lighter when `dark`) just
/// far enough to meet `min` contrast on every surface, keeping hue and chroma.
pub(super) fn legible(c: Color, surfaces: &[Color], dark: bool, min: f32) -> Color {
    if worst(c, surfaces) >= min {
        return c;
    }
    let [l, a, b, _] = c.to_oklab();
    let end = if dark { 1.0 } else { 0.0 };
    let at = |t: f32| Color::from_oklab([(end - l).mul_add(t, l), a, b, 1.0]);
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..20 {
        let mid = f32::midpoint(low, high);
        if worst(at(mid), surfaces) >= min {
            high = mid;
        } else {
            low = mid;
        }
    }
    at(high)
}

/// Composites `top` at opacity `t` over opaque `base` in sRGB space
/// (`App::srgb_blend`).
pub(super) fn over(base: Color, top: Color, t: f32) -> Color {
    use rune::color::{srgb_decode, srgb_encode};
    let mix =
        |a: f32, b: f32| srgb_decode((srgb_encode(b) - srgb_encode(a)).mul_add(t, srgb_encode(a)));
    Color {
        r: mix(base.r, top.r),
        g: mix(base.g, top.g),
        b: mix(base.b, top.b),
        a: 1.0,
    }
}

/// Solves the opacity `t` in `0..=1` where `over(from, to, t)` shifts Oklab
/// lightness by `step`, ensuring washes read consistently across schemes.
fn reach(from: Color, to: Color, step: f32) -> f32 {
    let lightness = |c: Color| c.to_oklab()[0];
    let distance = |c: Color| (lightness(c) - lightness(from)).abs();
    if distance(to) <= step {
        return 1.0;
    }
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..20 {
        let mid = f32::midpoint(low, high);
        if distance(over(from, to, mid)) >= step {
            high = mid;
        } else {
            low = mid;
        }
    }
    high
}

/// Shifts `c`'s Oklab lightness by `delta` while preserving hue and chroma.
fn shift_lightness(c: Color, delta: f32) -> Color {
    let [l, a, b, alpha] = c.to_oklab();
    Color::from_oklab([(l + delta).clamp(0.0, 1.0), a, b, alpha])
}

/// Builds a complete flat, hairline [`Theme`] strictly from `scheme`'s 16
/// slots.
#[expect(
    clippy::too_many_lines,
    reason = "single-pass derivation keeps surface contrast surfaces and kit tokens aligned"
)]
pub(super) fn build_theme(scheme: &Scheme) -> Theme {
    let b = scheme.base;
    let (bg, surface, wash) = (b[0x0], b[0x1], b[0x2]);
    let dark = b[0x5].luminance() > bg.luminance();
    let by_luminance = |x: &Color, y: &Color| x.luminance().total_cmp(&y.luminance());
    let lightest = b[..8].iter().copied().max_by(by_luminance).unwrap_or(bg);
    let darkest = b[..8].iter().copied().min_by(by_luminance).unwrap_or(bg);

    let sidebar_bg = if dark {
        if surface.luminance() > bg.luminance() {
            shift_lightness(bg, -0.015)
        } else {
            surface
        }
    } else if surface.luminance() > bg.luminance() {
        over(bg, wash, reach(bg, wash, 0.024))
    } else {
        surface
    };

    let surface_recessed = if dark {
        over(bg, surface, 0.60)
    } else {
        over(bg, wash, reach(bg, wash, 0.032))
    };
    let surface_drawer = over(bg, surface, 0.85);
    let interactive = wash.mix(b[0x5], 0.35);
    let tint = |step: f32| {
        interactive.alpha(
            reach(bg, interactive, step)
                .max(reach(surface, interactive, step))
                .max(reach(sidebar_bg, interactive, step)),
        )
    };
    let hover_wash = tint(0.030);
    let selected_subtle = tint(0.040);
    let selected = tint(0.055);
    let pressed_wash = tint(0.065);
    let mine = over(bg, wash, reach(bg, wash, if dark { 0.038 } else { 0.032 }));
    let chip_fill = if dark {
        over(surface, wash, 0.35)
    } else {
        over(bg, wash, reach(bg, wash, 0.028))
    };
    let overlay_fill = if dark {
        over(surface, wash, reach(surface, wash, 0.045))
    } else {
        lightest
    };
    let scrim = darkest.alpha(if dark { 0.50 } else { 0.32 });

    let surfaces = [
        bg,
        sidebar_bg,
        surface,
        surface_recessed,
        mine,
        overlay_fill,
        over(bg, interactive, selected.a),
        over(surface, interactive, selected.a),
        over(sidebar_bg, interactive, selected.a),
        over(overlay_fill, interactive, selected.a),
    ];
    let leg = |c: Color, min: f32| legible(c, &surfaces, dark, min);
    let text = leg(b[0x5], 5.5);
    let chroma = |c: Color| {
        let [_, a, ch_b, _] = c.to_oklab();
        a.hypot(ch_b)
    };
    let strong_base = if chroma(b[0x6]) > 0.04 || contrast(b[0x6], bg) <= contrast(text, bg) {
        shift_lightness(text, if dark { 0.08 } else { -0.08 })
    } else {
        b[0x6]
    };
    let text_strong = leg(strong_base, 5.5);
    let text_secondary = leg(b[0x5].mix(b[0x4], 0.35), 5.2);
    let muted = leg(b[0x4], 5.2);
    let faint = leg(b[0x3], 4.5);
    let icon_muted = faint;

    let accent = leg(b[0xD], 3.0);
    let accent_text = leg(b[0xD], 4.5);
    let steer = leg(b[0x9], 4.5);
    let on_fill = |fill: Color| {
        [bg, b[0x7], lightest, darkest]
            .into_iter()
            .max_by(|x, y| contrast(*x, fill).total_cmp(&contrast(*y, fill)))
            .unwrap_or(bg)
    };
    let on_accent = on_fill(accent);
    let (danger, warning, success, info, delegated) = (
        leg(b[0x8], 4.5),
        leg(b[0xA], 4.5),
        leg(b[0xB], 4.5),
        leg(b[0xC], 4.5),
        leg(b[0xE], 4.5),
    );
    let on_danger = on_fill(danger);

    let project_palette = [
        leg(b[0xD], 4.5),
        leg(b[0xB], 4.5),
        leg(b[0xE], 4.5),
        leg(b[0x9], 4.5),
        leg(b[0xC], 4.5),
        leg(b[0x8], 4.5),
        leg(b[0xA], 4.5),
        leg(b[0xF], 4.5),
        muted,
    ];

    let mut k = Kit::from_palette_focus(
        PaletteSeed {
            bg,
            fg: text,
            accent,
        },
        PaletteFocus::Accent,
    );
    let ink = |a: f32| k.alpha(text, a);
    let hairline = ink(if dark { 0.11 } else { 0.12 });
    let border_control = ink(if dark { 0.15 } else { 0.17 });
    let border_overlay = ink(if dark { 0.16 } else { 0.17 });
    let border_strong = ink(0.24);
    let border_dashed = muted.alpha(0.45);
    let state = |fg: Color| StateColors {
        bg: surface.mix(fg, 0.12),
        fg,
        border: surface.mix(fg, 0.35),
        icon: fg,
    };
    k.c = Palette {
        page: bg,
        bg_base: bg,
        bg_deep: sidebar_bg,
        layer_01: surface,
        layer_02: mine,
        layer_03: chip_fill,
        layer_04: mine,
        bg_inverse: text,
        bg_contrast: text,
        bg_button_neutral: chip_fill,
        bg_icon_button_contrast: accent,
        bg_accent: accent,
        submit_highlight: on_accent.alpha(0.0),
        submit_hover: on_accent.alpha(0.14),
        submit_fg: on_accent,
        submit_icon: on_accent,
        text_base: text,
        text_muted: muted,
        text_faint: faint,
        text_inverse: bg,
        text_contrast: bg,
        text_accent: accent_text,
        icon_base: text,
        icon_muted: muted,
        icon_faint: faint,
        icon_inverse: bg,
        icon_contrast: bg,
        icon_accent: accent_text,
        border_muted: hairline,
        border_base: border_control,
        border_strong,
        border_focus: accent,
        hover: hover_wash,
        pressed: pressed_wash,
        contrast_hover: bg.alpha(0.10),
        contrast_pressed: bg.alpha(0.20),
        scrim,
        depth_top: lightest,
        depth_bottom: lightest.alpha(0.0),
        white: lightest,
        black: darkest,
        knob: text,
        knob_on: on_accent,
        success: state(success),
        warning: state(warning),
        danger: state(danger),
        info: state(info),
        surface_base: mine,
        surface_weak: surface,
        surface_hover: hover_wash,
        surface_raised: surface,
        surface_raised_base: surface,
        surface_raised_hover: hover_wash,
        surface_float: text,
        surface_disabled: surface_recessed,
        border_weak: hairline,
        border_hover: border_strong,
        border_active: border_strong,
        border_disabled: hairline,
        text_strong,
        text_weak: muted,
        text_invert_strong: bg,
        icon_weaker: faint,
        std_border: border_control,
        std_icon: muted,
        std_icon_invert: bg,
        std_icon_strong: text_strong,
        std_text: muted,
    };

    let added = over(bg, success, if dark { 0.14 } else { 0.12 });
    let added_word = over(bg, success, if dark { 0.28 } else { 0.24 });
    let removed = over(bg, danger, if dark { 0.14 } else { 0.12 });
    let removed_word = over(bg, danger, if dark { 0.28 } else { 0.24 });
    let syntax = build_syntax(
        &k,
        &b,
        &[bg, surface, added_word, removed_word],
        dark,
        text,
        muted,
        faint,
        success,
        danger,
    );

    let ty = build_typography(&mut k);
    crate::ui::kit_glyphs(&mut k.glyph);
    k.radius.xs = RADIUS_SM;
    k.radius.sm = RADIUS_SM;
    k.radius.md = RADIUS_CONTROL;
    k.radius.lg = RADIUS_CONTROL;
    k.radius.xl = Size::Large.radius();
    k.motion.control = crate::motion::QUICK;
    k.motion.overlay = crate::motion::SNAP;
    k.motion.disclosure = crate::motion::SETTLE;
    k.elevation = rune_kit::Elevation::default();
    k.elevation.floating = Vec::new();
    k.elevation.md = Vec::new();
    k.elevation.overlay = Vec::new();

    let mut shell = k.clone();
    shell.radius.lg = 0.0;

    let ring = Shadow {
        spread: HAIRLINE,
        color: border_overlay,
        ..Shadow::default()
    };
    let mut overlay = k.clone();
    overlay.c.layer_01 = overlay_fill;
    overlay.c.surface_raised = overlay_fill;
    overlay.c.hover = selected;
    overlay.c.surface_raised_hover = hover_wash;
    overlay.c.std_border = Color::TRANSPARENT;
    overlay.elevation.floating = vec![ring];
    overlay.elevation.md = vec![ring];
    overlay.elevation.overlay = vec![ring];

    Theme {
        shell,
        overlay,
        canvas: bg,
        sidebar_bg,
        surface,
        surface_recessed,
        surface_raised: overlay_fill,
        surface_drawer,
        chip_fill,
        mine,
        scrim,
        hover_wash,
        selected,
        selected_subtle,
        pressed_wash,
        hairline,
        border_control,
        border_overlay,
        border_strong,
        border_dashed,
        text,
        text_strong,
        text_secondary,
        muted,
        faint,
        icon_muted,
        accent,
        accent_text,
        on_accent,
        steer,
        remote: info,
        info,
        delegated,
        danger,
        on_danger,
        success,
        warning,
        added,
        added_word,
        removed,
        removed_word,
        project_palette,
        syntax,
        hero: ty.hero,
        hero_narrow: ty.hero_narrow,
        dialog_title: ty.dialog_title,
        section_heading: ty.section_heading,
        title: ty.title,
        body: ty.body,
        ui: ty.ui,
        ui_medium: ty.ui_medium,
        panel_row: ty.panel_row,
        small: ty.small,
        small_medium: ty.small_medium,
        micro: ty.micro,
        micro_medium: ty.micro_medium,
        nano: ty.nano,
        mono: ty.mono,
        mono_small: ty.mono_small,
        mono_micro: ty.mono_micro,
        k,
    }
}
