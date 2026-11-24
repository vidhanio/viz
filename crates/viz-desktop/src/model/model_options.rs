//! Pure derivations for the model picker's options column, footer and
//! keyboard: the previewed model's reasoning effort, extra ACP options, the
//! speed tier, the context badge, the access mode, and an owned navigation
//! snapshot for [`crate::update`].

use std::borrow::Cow;

use viz_client::protocol::{
    AccessKind, AccessMode, ConfigOption, OptionChoice, OptionKind, OptionValue, ThreadId,
};

use super::{
    Model, ModelLevels, ModelMenuState, ModelRef, OptionRole, PickerList, PickerScope, RailTab,
    option_role, picker_list,
};

/// One reasoning effort level of the previewed model.
#[derive(Debug, Clone)]
pub struct EffortLevel<'a> {
    /// Wire value.
    pub value: &'a str,
    /// Display label.
    pub label: Cow<'a, str>,
    /// Position among the levels, matched against the keyboard cursor.
    pub index: usize,
    /// The level that applies now (or would apply on choosing the model).
    pub checked: bool,
    /// The harness default for this model.
    pub default: bool,
}

/// An extra ACP option shown under the effort levels.
#[derive(Debug, Clone, Copy)]
pub enum ExtraOption<'a> {
    /// On/off switch.
    Toggle {
        /// The option.
        option: &'a ConfigOption,
        /// Its value.
        on: bool,
    },
    /// Choice opening a side submenu.
    Select {
        /// The option.
        option: &'a ConfigOption,
        /// Its chosen value, when listed.
        value: Option<&'a OptionChoice>,
    },
}

impl<'a> ExtraOption<'a> {
    /// The underlying option.
    #[must_use]
    pub const fn option(self) -> &'a ConfigOption {
        match self {
            Self::Toggle { option, .. } | Self::Select { option, .. } => option,
        }
    }
}

/// The options column for the previewed model.
#[derive(Debug, Clone)]
pub struct EffortColumn<'a> {
    /// Name of the model being configured (`decisions.md` #9).
    pub model_label: Option<&'a str>,
    /// The model the levels belong to.
    pub model: Option<ModelRef>,
    /// Thinking option id the levels set.
    pub option_id: Option<&'a str>,
    /// Effort levels, empty when the model has none.
    pub levels: Vec<EffortLevel<'a>>,
    /// Whether the model's levels are being fetched.
    pub loading: bool,
    /// Extra options of the scope's harness.
    pub extras: Vec<ExtraOption<'a>>,
    /// Whether choices here can be applied.
    pub editable: bool,
}

/// Whether `option` picks a speed tier rather than a general setting.
fn is_speed(option: &ConfigOption) -> bool {
    let id = option.id.to_ascii_lowercase();
    matches!(
        option.category.as_deref(),
        Some("speed" | "service_tier" | "tier")
    ) || ["speed", "fast", "service_tier"]
        .iter()
        .any(|needle| id.contains(needle))
}

/// Derives the options column for the list's previewed model.
#[must_use]
pub fn effort_column<'a>(list: &PickerList<'a>, state: &ModelMenuState) -> EffortColumn<'a> {
    let scope = &list.scope;
    let preview = list.preview(state);
    let browsing_own = !list.searching && list.tab == RailTab::Harness(scope.harness.to_owned());
    if preview.is_none() && !browsing_own {
        return EffortColumn {
            model_label: None,
            model: None,
            option_id: None,
            levels: Vec::new(),
            loading: false,
            extras: Vec::new(),
            editable: false,
        };
    }
    let own = preview.is_none_or(|row| row.model.harness == scope.harness);
    let current = scope.picker();
    let model_label = preview
        .map(|row| row.label)
        .or_else(|| current.as_ref().and_then(super::ModelPicker::model_label));
    let dependent = preview.and_then(|row| row.choice.thinking.as_ref());
    let default = dependent.and_then(|levels| match &levels.value {
        OptionValue::Select(value) => Some(value.as_str()),
        OptionValue::Boolean(_) => None,
    });
    let levels = if preview.is_none_or(|row| row.selected) {
        current
            .as_ref()
            .and_then(|picker| picker.levels)
            .map(|levels| (levels, levels.value(scope.target)))
    } else {
        dependent
            .map(ModelLevels::Model)
            .filter(|levels| levels.choices().len() > 1)
            .map(|levels| (levels, default))
    };
    let (option_id, levels) = levels.map_or((None, Vec::new()), |(levels, checked)| {
        let rows = levels
            .choices()
            .enumerate()
            .map(|(index, (value, name))| EffortLevel {
                value,
                label: levels.choice_label(value, name),
                index,
                checked: checked == Some(value),
                default: default == Some(value),
            })
            .collect();
        (Some(levels.id()), rows)
    });
    let extras = if own {
        scope
            .options
            .iter()
            .filter(|option| option_role(option) == OptionRole::Other && !is_speed(option))
            .map(|option| match (option.kind, scope.target.value(option)) {
                (OptionKind::Boolean, value) => ExtraOption::Toggle {
                    option,
                    on: *value == OptionValue::Boolean(true),
                },
                (OptionKind::Select, value) => ExtraOption::Select {
                    option,
                    value: option
                        .choices
                        .iter()
                        .find(|choice| OptionValue::Select(choice.value.clone()) == *value),
                },
            })
            .collect()
    } else {
        Vec::new()
    };
    EffortColumn {
        model_label,
        model: preview.map(|row| row.model.clone()),
        option_id,
        levels,
        loading: preview
            .is_some_and(|row| state.loading_levels.as_deref() == Some(row.model.value.as_str())),
        extras,
        editable: preview.is_none_or(|row| !row.disabled),
    }
}

/// A speed option rendered as `Standard | Fast`.
#[derive(Debug, Clone)]
pub struct Speed<'a> {
    /// The option.
    pub option: &'a ConfigOption,
    /// Value selecting the standard tier.
    pub standard: OptionValue,
    /// Value selecting the fast tier.
    pub fast: OptionValue,
    /// Whether fast is on.
    pub is_fast: bool,
}

/// The scope's speed option, when its harness offers a fast tier.
#[must_use]
pub fn speed<'a>(scope: &PickerScope<'a>) -> Option<Speed<'a>> {
    let option = scope.options.iter().find(|option| is_speed(option))?;
    let value = scope.target.value(option);
    let (standard, fast) = match option.kind {
        OptionKind::Boolean => (OptionValue::Boolean(false), OptionValue::Boolean(true)),
        OptionKind::Select => {
            let is_fast = |choice: &&OptionChoice| {
                let text = format!("{} {}", choice.value, choice.name).to_ascii_lowercase();
                text.contains("fast") || text.contains("priority")
            };
            let fast = option.choices.iter().find(is_fast)?;
            let standard = option.choices.iter().find(|choice| !is_fast(choice))?;
            (
                OptionValue::Select(standard.value.clone()),
                OptionValue::Select(fast.value.clone()),
            )
        }
    };
    Some(Speed {
        is_fast: *value == fast,
        option,
        standard,
        fast,
    })
}

/// A non-default context-window choice to badge on the trigger (`1M`).
#[must_use]
pub fn context_badge<'a>(scope: &PickerScope<'a>) -> Option<&'a str> {
    let option = scope.options.iter().find(|option| {
        option.kind == OptionKind::Select
            && (option.id.to_ascii_lowercase().contains("context")
                || option.category.as_deref() == Some("context"))
    })?;
    let first = option.choices.first()?;
    match scope.target.value(option) {
        OptionValue::Select(value) if *value != first.value => option
            .choices
            .iter()
            .find(|choice| choice.value == *value)
            .map(|choice| choice.name.as_str()),
        _ => None,
    }
}

/// The access modes a draft or thread can run in.
#[derive(Debug, Clone, Copy)]
pub struct AccessView<'a> {
    /// The mode in effect.
    pub current: &'a AccessMode,
    /// Modes the harness offers.
    pub modes: &'a [AccessMode],
    /// The thread to apply to; `None` for a draft.
    pub thread: Option<ThreadId>,
}

impl AccessView<'_> {
    /// Index of the current mode in [`Self::modes`].
    #[must_use]
    pub fn current_index(&self) -> Option<usize> {
        self.modes.iter().position(|mode| mode == self.current)
    }
}

/// Access modes for the current draft or thread, when its harness offers any.
#[must_use]
pub fn access_view(model: &Model) -> Option<AccessView<'_>> {
    if let Some(draft) = &model.selection.new_thread {
        let harness = draft.active_harness(model.harnesses())?;
        let offered = harness.access.as_slice();
        let current = draft
            .access
            .as_ref()
            .and_then(|chosen| offered.iter().find(|mode| *mode == chosen))
            .or_else(|| offered.iter().find(|mode| mode.kind == AccessKind::Full))
            .or_else(|| offered.first())?;
        return Some(AccessView {
            current,
            modes: offered,
            thread: None,
        });
    }
    let thread = model.selection.selected?;
    let access = &model.thread_state()?.access;
    (!access.modes.is_empty()).then_some(AccessView {
        current: &access.current,
        modes: &access.modes,
        thread: Some(thread),
    })
}

/// What an access kind lets the agent do, for the access popover.
#[must_use]
pub const fn access_summary(kind: AccessKind) -> &'static str {
    match kind {
        AccessKind::Full => "Runs commands and edits files without asking.",
        AccessKind::Auto => "Decides most actions itself and asks about risky ones.",
        AccessKind::AcceptEdits => "Edits files freely and asks before running commands.",
        AccessKind::Ask => "Asks before every edit and command.",
        AccessKind::ReadOnly => "Reads and plans without changing anything.",
    }
}

/// An extra option as the keyboard sees it.
#[derive(Debug, Clone)]
pub enum ExtraNav {
    /// On/off switch.
    Toggle {
        /// Option id.
        option: String,
        /// Current value.
        on: bool,
    },
    /// Choice with a side submenu.
    Select {
        /// Option id.
        option: String,
        /// Choice values in order.
        choices: Vec<String>,
        /// Index of the chosen value.
        current: usize,
    },
}

/// Owned snapshot of what the keyboard can reach in the open picker.
#[derive(Debug, Clone)]
pub struct PickerNav {
    /// Rail tabs in order.
    pub tabs: Vec<RailTab>,
    /// The tab shown.
    pub tab: RailTab,
    /// Whether a search hides the rail.
    pub searching: bool,
    /// Model rows with whether each is disabled.
    pub rows: Vec<(ModelRef, bool)>,
    /// The previewed model.
    pub preview: Option<ModelRef>,
    /// Effort levels `(option id, value, label)` of the previewed model.
    pub levels: Vec<(String, String, String)>,
    /// Index of the checked level.
    pub checked_level: Option<usize>,
    /// Extra options under the levels.
    pub extras: Vec<ExtraNav>,
    /// Whether options can be applied.
    pub editable: bool,
}

impl PickerNav {
    /// Rows in the options column.
    #[must_use]
    pub const fn option_rows(&self) -> usize {
        self.levels.len() + self.extras.len()
    }
}

/// Builds the keyboard snapshot of the open picker.
#[must_use]
pub fn picker_nav(model: &Model) -> Option<PickerNav> {
    let state = &model.composer.model_menu;
    let list = picker_list(model)?;
    let column = effort_column(&list, state);
    let mut tabs = vec![RailTab::Favorites];
    tabs.extend(
        list.catalogs
            .iter()
            .filter(|catalog| !list.scope.locked || catalog.own)
            .map(|catalog| RailTab::Harness(catalog.id.to_owned())),
    );
    let levels: Vec<_> = column
        .levels
        .iter()
        .map(|level| {
            (
                column.option_id.unwrap_or_default().to_owned(),
                level.value.to_owned(),
                level.label.clone().into_owned(),
            )
        })
        .collect();
    let extras = column
        .extras
        .iter()
        .map(|extra| match *extra {
            ExtraOption::Toggle { option, on } => ExtraNav::Toggle {
                option: option.id.clone(),
                on,
            },
            ExtraOption::Select { option, value } => ExtraNav::Select {
                option: option.id.clone(),
                choices: option.choices.iter().map(|c| c.value.clone()).collect(),
                current: value
                    .and_then(|value| option.choices.iter().position(|c| c == value))
                    .unwrap_or_default(),
            },
        })
        .collect();
    Some(PickerNav {
        tabs,
        tab: list.tab.clone(),
        searching: list.searching,
        rows: list
            .rows()
            .map(|row| (row.model.clone(), row.disabled))
            .collect(),
        preview: column.model.clone(),
        checked_level: column.levels.iter().position(|level| level.checked),
        levels,
        extras,
        editable: column.editable,
    })
}
