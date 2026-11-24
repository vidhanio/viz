//! Preview scenes for the unified model picker and access-mode popovers:
//! Claude thread, Codex draft with speed and extra options, a multi-provider
//! harness with folds, Favorites (populated and empty), cross-harness search,
//! an open option submenu, the access popover, and unusable harnesses.

use viz_client::protocol::{
    AccessKind, AccessMode, ConfigOption, DependentChoice, DependentOption, Harness, HarnessStatus,
    NativeAccess, OptionChoice, OptionKind, OptionValue,
};

use super::{
    data::{THREAD, option, options},
    fixtures::seed_workspace,
};
use crate::{
    app::Desktop,
    model::{MODEL_PICKER, ModelMenuState, ModelRef, NewThread, PickerColumn, RailTab},
};

fn levels(values: &[&str], default: &str) -> DependentOption {
    DependentOption {
        id: "effort".into(),
        name: "Effort".into(),
        description: None,
        value: OptionValue::Select(default.into()),
        choices: values
            .iter()
            .map(|value| DependentChoice {
                value: (*value).into(),
                name: (*value).into(),
                description: None,
            })
            .collect(),
    }
}

fn models(current: &str, list: &[(&str, &str, &[&str], &str)]) -> ConfigOption {
    let pairs: Vec<_> = list
        .iter()
        .map(|(value, name, ..)| (*value, *name))
        .collect();
    let mut model = option("model", "Model", "model", current, &pairs);
    for (choice, (_, _, efforts, default)) in model.choices.iter_mut().zip(list) {
        choice.thinking = (!efforts.is_empty()).then(|| levels(efforts, default));
    }
    model
}

fn select(id: &str, name: &str, current: &str, choices: &[(&str, &str, &str)]) -> ConfigOption {
    ConfigOption {
        id: id.into(),
        name: name.into(),
        description: None,
        category: None,
        kind: OptionKind::Select,
        value: OptionValue::Select(current.into()),
        choices: choices
            .iter()
            .map(|(value, name, description)| OptionChoice {
                value: (*value).into(),
                name: (*name).into(),
                description: (!description.is_empty()).then(|| (*description).into()),
                group: None,
                thinking: None,
            })
            .collect(),
    }
}

fn toggle(id: &str, name: &str, on: bool) -> ConfigOption {
    ConfigOption {
        id: id.into(),
        name: name.into(),
        description: None,
        category: None,
        kind: OptionKind::Boolean,
        value: OptionValue::Boolean(on),
        choices: Vec::new(),
    }
}

fn mode(kind: AccessKind, label: &str, native: Option<&str>) -> AccessMode {
    AccessMode {
        kind,
        label: label.into(),
        native: native.map(|value| NativeAccess {
            option: "mode".into(),
            value: value.into(),
        }),
    }
}

const CLAUDE_EFFORT: &[&str] = &["low", "medium", "high", "max"];

fn claude_options() -> Vec<ConfigOption> {
    vec![
        models(
            "opus",
            &[
                ("opus", "Opus 5.5", CLAUDE_EFFORT, "high"),
                ("sonnet", "Sonnet 5.5", CLAUDE_EFFORT, "medium"),
                ("haiku", "Haiku 5.5", &["low", "medium", "high"], "low"),
                ("fable", "Fable 5.2", &[], ""),
            ],
        ),
        option(
            "effort",
            "Effort",
            "effort",
            "high",
            &[
                ("low", "low"),
                ("medium", "medium"),
                ("high", "high"),
                ("max", "max"),
            ],
        ),
        select(
            "context",
            "Context window",
            "1m",
            &[
                ("200k", "200K", "Standard window"),
                ("1m", "1M", "Long context, slower"),
            ],
        ),
    ]
}

fn codex_options() -> Vec<ConfigOption> {
    const EFFORT: &[&str] = &["minimal", "low", "medium", "high", "xhigh"];
    vec![
        models(
            "gpt-6-codex",
            &[
                ("gpt-6-codex", "GPT-6 Codex", EFFORT, "medium"),
                ("gpt-6", "GPT-6", EFFORT, "medium"),
                (
                    "gpt-6-mini",
                    "GPT-6 mini",
                    &["low", "medium", "high"],
                    "medium",
                ),
            ],
        ),
        option(
            "effort",
            "Effort",
            "thought_level",
            "high",
            &[
                ("minimal", "minimal"),
                ("low", "low"),
                ("medium", "medium"),
                ("high", "high"),
                ("xhigh", "xhigh"),
            ],
        ),
        select(
            "service_tier",
            "Speed",
            "priority",
            &[("default", "Standard", ""), ("priority", "Fast", "")],
        ),
        select(
            "summary",
            "Reasoning summary",
            "auto",
            &[
                ("auto", "Auto", "Let the model decide"),
                ("concise", "Concise", "One short paragraph"),
                ("detailed", "Detailed", "Step by step"),
                ("none", "None", "Hide reasoning"),
            ],
        ),
        toggle("web_search", "Web search", true),
    ]
}

fn omp_options() -> Vec<ConfigOption> {
    let mut all = options();
    if let Some(model) = all.first_mut() {
        for (value, name) in [
            ("anthropic/claude-opus-4-1", "Claude Opus 4.1"),
            ("anthropic/claude-sonnet-4-5", "Claude Sonnet 4.5"),
            ("anthropic/claude-sonnet-4", "Claude Sonnet 4"),
            ("anthropic/claude-3-7-sonnet", "Claude Sonnet 3.7"),
            ("anthropic/claude-3-5-haiku", "Claude Haiku 3.5"),
        ] {
            model.choices.insert(
                5,
                OptionChoice {
                    value: value.into(),
                    name: name.into(),
                    description: None,
                    group: None,
                    thinking: None,
                },
            );
        }
    }
    all
}

fn harness(id: &str, name: &str, default: bool, options: Vec<ConfigOption>) -> Harness {
    Harness {
        id: id.into(),
        name: name.into(),
        default,
        icon: id.into(),
        status: HarnessStatus::Ready,
        checked_at: None,
        options,
        access: Vec::new(),
        commands: Vec::new(),
    }
}

fn claude_access() -> Vec<AccessMode> {
    vec![
        mode(AccessKind::Ask, "Default", Some("default")),
        mode(AccessKind::AcceptEdits, "Accept edits", Some("acceptEdits")),
        mode(AccessKind::ReadOnly, "Plan mode", Some("plan")),
        mode(
            AccessKind::Full,
            "Bypass permissions",
            Some("bypassPermissions"),
        ),
    ]
}

fn harnesses() -> Vec<Harness> {
    let mut claude = harness("claude", "Claude Code", true, claude_options());
    claude.access = claude_access();
    let mut codex = harness("codex", "Codex", false, codex_options());
    codex.access = vec![
        mode(AccessKind::ReadOnly, "Read only", Some("read-only")),
        mode(AccessKind::Auto, "Auto", Some("auto")),
        mode(AccessKind::Full, "Full access", Some("full-access")),
    ];
    let mut agy = harness("agy", "Google Antigravity", false, Vec::new());
    agy.status = HarnessStatus::SignedOut {
        message: "Run `agy login` on the server.".into(),
    };
    let mut pi = harness("pi", "Pi", false, Vec::new());
    pi.status = HarnessStatus::LimitReached {
        message: "Usage limit reached.".into(),
        reset: Some("resets 8pm (America/Toronto)".into()),
    };
    vec![
        codex,
        claude,
        harness("omp", "Oh My Pi", false, omp_options()),
        agy,
        pi,
    ]
}

/// Workspace on the Claude thread with the catalog seeded and the picker open.
fn on_thread(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    s.model.cache_mut().harnesses = harnesses();
    if let Some(state) = s.model.cache_mut().open_thread_mut(THREAD).state.as_mut() {
        state.options = claude_options();
        state.access.modes = claude_access();
        state.access.current = mode(
            AccessKind::Full,
            "Bypass permissions",
            Some("bypassPermissions"),
        );
    }
    open(s);
}

/// A fresh draft on `harness` with the catalog seeded and the picker open.
fn on_draft(s: &mut Desktop, harness: &str) {
    seed_workspace(&mut s.model);
    s.model.cache_mut().harnesses = harnesses();
    let mut draft = NewThread::new(0, s.model.project());
    draft.harness = Some(harness.into());
    s.model.selection.selected = None;
    s.model.selection.new_thread = Some(draft);
    open(s);
}

fn open(s: &mut Desktop) {
    s.model.composer.open_picker = Some(MODEL_PICKER.into());
    s.model.composer.model_menu = ModelMenuState::default();
    s.model.composer.draft_focused = false;
}

fn finish(s: &mut Desktop) {
    s.sync_widgets_from_model();
}

fn favorite(s: &mut Desktop, harness: &str, value: &str) {
    s.model.composer.favorite_models.push(
        ModelRef {
            harness: harness.into(),
            value: value.into(),
        }
        .favorite_key(),
    );
}

pub(super) fn claude(s: &mut Desktop) {
    on_thread(s);
    favorite(s, "claude", "opus");
    s.model.composer.model_menu.highlight = Some(ModelRef {
        harness: "claude".into(),
        value: "sonnet".into(),
    });
    finish(s);
}

pub(super) fn codex(s: &mut Desktop) {
    on_draft(s, "codex");
    s.model.composer.model_menu.column = PickerColumn::Options;
    s.model.composer.model_menu.option_cursor = 3;
    finish(s);
}

pub(super) fn multi(s: &mut Desktop) {
    on_draft(s, "omp");
    finish(s);
}

pub(super) fn favorites(s: &mut Desktop) {
    on_thread(s);
    favorite(s, "claude", "opus");
    favorite(s, "codex", "gpt-6-codex");
    favorite(s, "claude", "haiku");
    favorite(s, "omp", "google/gemini-3-pro");
    s.model.composer.model_menu.tab = Some(RailTab::Favorites);
    finish(s);
}

pub(super) fn favorites_empty(s: &mut Desktop) {
    on_thread(s);
    s.model.composer.model_menu.tab = Some(RailTab::Favorites);
    finish(s);
}

pub(super) fn search(s: &mut Desktop) {
    on_draft(s, "claude");
    s.model.composer.model_menu.query = "son".into();
    finish(s);
}

pub(super) fn search_empty(s: &mut Desktop) {
    on_draft(s, "claude");
    s.model.composer.model_menu.query = "llama".into();
    finish(s);
}

pub(super) fn submenu(s: &mut Desktop) {
    on_draft(s, "codex");
    let menu = &mut s.model.composer.model_menu;
    menu.column = PickerColumn::Options;
    menu.option_cursor = 5;
    menu.submenu = Some("summary".into());
    menu.submenu_cursor = 1;
    finish(s);
}

pub(super) fn access(s: &mut Desktop) {
    on_thread(s);
    s.model.composer.model_menu.access_open = true;
    s.model.composer.model_menu.access_preview = Some(1);
    finish(s);
}

pub(super) fn status(s: &mut Desktop) {
    on_draft(s, "claude");
    s.model.composer.model_menu.tab = Some(RailTab::Harness("agy".into()));
    finish(s);
}

pub(super) fn locked(s: &mut Desktop) {
    on_thread(s);
    finish(s);
}
