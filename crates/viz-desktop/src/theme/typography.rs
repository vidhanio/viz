//! Typography scale and syntax highlighting theme derivation for [`Theme`].

use rune::{Color, TextStyle};
use rune_kit::{
    Kit,
    syntax::{Syntax, SyntaxTheme},
};

use super::palette::legible;

/// Complete 17-slot typography scale.
pub(super) struct Typography {
    pub(super) hero: TextStyle,
    pub(super) hero_narrow: TextStyle,
    pub(super) dialog_title: TextStyle,
    pub(super) section_heading: TextStyle,
    pub(super) title: TextStyle,
    pub(super) body: TextStyle,
    pub(super) ui: TextStyle,
    pub(super) ui_medium: TextStyle,
    pub(super) panel_row: TextStyle,
    pub(super) small: TextStyle,
    pub(super) small_medium: TextStyle,
    pub(super) micro: TextStyle,
    pub(super) micro_medium: TextStyle,
    pub(super) nano: TextStyle,
    pub(super) mono: TextStyle,
    pub(super) mono_small: TextStyle,
    pub(super) mono_micro: TextStyle,
}

/// Builds the syntax highlighting palette from base16 accents, verifying
/// `>= 4.5:1` contrast against code surfaces and diff word-change washes.
#[expect(
    clippy::too_many_arguments,
    reason = "internal helper bundling scheme palette roles for syntax highlighting"
)]
pub(super) fn build_syntax(
    k: &Kit,
    b: &[Color; 16],
    surfaces: &[Color],
    dark: bool,
    text: Color,
    muted: Color,
    faint: Color,
    success: Color,
    danger: Color,
) -> SyntaxTheme {
    let code = |ink: Color| legible(ink, surfaces, dark, 4.5);
    let blue = code(b[0xD]);
    let orange = code(b[0x9]);
    let neutral = code(text);
    let punctuation = code(muted);
    [
        (Syntax::Keyword, code(b[0xE])),
        (Syntax::String, code(b[0xB])),
        (Syntax::Number, orange),
        (Syntax::Comment, code(faint)),
        (Syntax::Type, code(b[0xA])),
        (Syntax::Function, blue),
        (Syntax::Property, code(b[0x8])),
        (Syntax::Punctuation, punctuation),
        (Syntax::Inserted, code(success)),
        (Syntax::Deleted, code(danger)),
        (Syntax::Parameter, neutral),
        (Syntax::Heading, blue),
        (Syntax::Link, blue),
        (Syntax::Operator, punctuation),
        (Syntax::Regex, code(b[0xC])),
        (Syntax::Attribute, orange),
        (Syntax::Variable, neutral),
    ]
    .into_iter()
    .fold(SyntaxTheme::from_kit(k), |theme, (kind, c)| {
        theme.with(kind, c)
    })
}

/// Populates `k.ty` and returns all [`Typography`] slots.
pub(super) fn build_typography(k: &mut Kit) -> Typography {
    let medium = |style: TextStyle| TextStyle {
        weight: 520.0,
        ..style
    };
    let ui = TextStyle::new(14.0, 400.0).lh(20.0);
    let small = TextStyle::new(12.0, 400.0).lh(16.0);
    let micro = TextStyle::new(11.0, 400.0).lh(15.0);
    let body = TextStyle::new(14.0, 400.0).lh(22.75);
    let mono = TextStyle::new(12.0, 400.0)
        .mono()
        .lh(18.0)
        .features("'tnum' 1");
    let mono_small = TextStyle::new(11.0, 400.0)
        .mono()
        .lh(16.0)
        .features("'tnum' 1");
    let mono_micro = TextStyle::new(10.0, 450.0)
        .mono()
        .lh(14.0)
        .features("'tnum' 1");

    let ui_medium = medium(ui);
    let small_medium = medium(small);
    let micro_medium = TextStyle {
        weight: 500.0,
        ..micro
    };
    k.ty.ui = ui;
    k.ty.ui_medium = ui_medium;
    k.ty.ui_compact = ui.lh(small.line_height);
    k.ty.small = small;
    k.ty.small_medium = small_medium;
    k.ty.body = body;
    k.ty.body_medium = medium(body);
    k.ty.mono = mono;

    Typography {
        hero: TextStyle::new(30.0, 400.0).lh(36.0).tracking(-0.75),
        hero_narrow: TextStyle::new(24.0, 400.0).lh(32.0).tracking(-0.60),
        dialog_title: TextStyle::new(20.0, 600.0).lh(26.0).tracking(-0.20),
        section_heading: TextStyle::new(16.0, 600.0).lh(24.0),
        title: TextStyle::new(14.0, 600.0).lh(20.0).tracking(-0.18),
        body,
        ui,
        ui_medium,
        panel_row: TextStyle::new(13.0, 500.0).lh(18.0),
        small,
        small_medium,
        micro,
        micro_medium,
        nano: TextStyle::new(10.0, 500.0).lh(12.0),
        mono,
        mono_small,
        mono_micro,
    }
}
