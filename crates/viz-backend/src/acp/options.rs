//! ACP session config options as viz shows them, and the per-harness catalog
//! new threads start from.

use std::collections::BTreeMap;

use agent_client_protocol::v1::schema::{
    SessionConfigKind, SessionConfigOption, SessionConfigSelectOption, SessionConfigSelectOptions,
};
use serde::{Deserialize, Serialize};

use crate::{
    config::HarnessConfig,
    wire::{ConfigOption, DependentChoice, DependentOption, OptionChoice, OptionKind, OptionValue},
};

/// Option values by option id: what the user or config chose for a thread.
pub(crate) type Desired = BTreeMap<String, OptionValue>;

const MODEL: &str = "model";
const THOUGHT_LEVEL: &str = "thought_level";
/// The category of a runner's own mode selector, which viz shows but never sets
/// from config.
const MODE: &str = "mode";

/// Exact model and thinking selections for a delegated thread, applied before
/// its first prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Selection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) thinking: Option<String>,
}

/// A model's thinking levels, learned when a session first reported that model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelProfile {
    model: String,
    thinking: Option<DependentOption>,
}

/// What a harness offers a new thread: one template for model-independent
/// options, whose values stay as first observed, and a profile per observed
/// model owning its model-dependent effort.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Catalog {
    options: Vec<ConfigOption>,
    profiles: Vec<ModelProfile>,
}

impl Catalog {
    /// The catalog after a session reports `options`, or `None` when unchanged.
    /// `opened` holds the session's first-reported values, the baseline for
    /// options the catalog has not seen.
    pub(crate) fn merge(
        previous: Option<&Self>,
        options: &[ConfigOption],
        opened: &Desired,
    ) -> Option<Self> {
        let profile = model_profile(
            options,
            previous.map_or(&[][..], |catalog| catalog.profiles.as_slice()),
        );
        let template: Vec<ConfigOption> = options
            .iter()
            .filter(|option| profile.is_none() || option.category.as_deref() != Some(THOUGHT_LEVEL))
            .map(|option| {
                let baseline = previous
                    .and_then(|catalog| catalog.options.iter().find(|known| known.id == option.id))
                    .map(|known| &known.value)
                    .filter(|value| valid_value(option, value))
                    .or_else(|| opened.get(&option.id));
                match baseline {
                    Some(value) if option.value != *value && valid_value(option, value) => {
                        ConfigOption {
                            value: value.clone(),
                            ..option.clone()
                        }
                    }
                    _ => option.clone(),
                }
            })
            .collect();
        let Some(previous) = previous else {
            return Some(Self {
                options: template,
                profiles: profile.into_iter().collect(),
            });
        };
        let mut next = previous.clone();
        next.options = template;
        if let Some(profile) = profile {
            match next
                .profiles
                .iter_mut()
                .find(|known| known.model == profile.model)
            {
                Some(known) => *known = profile,
                None => next.profiles.push(profile),
            }
        }
        (next != *previous).then_some(next)
    }

    /// Whether the catalog's model option offers `model`.
    pub(crate) fn offers(&self, model: &str) -> bool {
        model_option(&self.options)
            .is_some_and(|option| valid_value(option, &OptionValue::Select(model.to_owned())))
    }

    /// Whether a session has reported `model`'s thinking levels.
    pub(crate) fn has_profile(&self, model: &str) -> bool {
        self.profiles.iter().any(|profile| profile.model == model)
    }

    /// The options a new thread starts with: `selected` model (else the
    /// configured one, else the template's), its thinking levels, and the
    /// configured defaults.
    pub(crate) fn options(
        &self,
        config: &HarnessConfig,
        selected: Option<&str>,
    ) -> Vec<ConfigOption> {
        let model = model_option(&self.options);
        let requested = selected
            .or(config.model.as_deref())
            .map(|value| OptionValue::Select(value.to_owned()));
        let value = match (model, requested) {
            (Some(model), Some(requested)) if valid_value(model, &requested) => Some(requested),
            (model, _) => model.map(|model| model.value.clone()),
        };
        let profile = match &value {
            Some(OptionValue::Select(value)) => {
                self.profiles.iter().find(|profile| profile.model == *value)
            }
            _ => None,
        };
        let model_id = model.map(|model| model.id.as_str());
        let mut options: Vec<ConfigOption> = self
            .options
            .iter()
            .filter(|option| profile.is_none() || option.category.as_deref() != Some(THOUGHT_LEVEL))
            .map(|option| match &value {
                Some(value) if Some(option.id.as_str()) == model_id => ConfigOption {
                    value: value.clone(),
                    ..option.clone()
                },
                _ => option.clone(),
            })
            .collect();
        if let Some(thinking) = profile.and_then(|profile| profile.thinking.as_ref()) {
            options.push(ConfigOption {
                id: thinking.id.clone(),
                name: thinking.name.clone(),
                description: thinking.description.clone(),
                category: Some(THOUGHT_LEVEL.to_owned()),
                kind: OptionKind::Select,
                value: thinking.value.clone(),
                choices: thinking
                    .choices
                    .iter()
                    .map(|choice| OptionChoice {
                        value: choice.value.clone(),
                        name: choice.name.clone(),
                        description: choice.description.clone(),
                        group: None,
                        thinking: None,
                    })
                    .collect(),
            });
        }
        let preferred = defaults(config, &options);
        for option in &mut options {
            if option.category.as_deref() != Some(MODEL)
                && let Some(value) = preferred.get(&option.id)
            {
                option.value = value.clone();
            }
        }
        with_model_profiles(options, &self.profiles, config.effort.as_deref())
    }
}

pub(super) fn model_option(options: &[ConfigOption]) -> Option<&ConfigOption> {
    options
        .iter()
        .find(|option| option.category.as_deref() == Some(MODEL))
}

pub(super) fn thinking_option(options: &[ConfigOption]) -> Option<&ConfigOption> {
    options.iter().find(|option| {
        option.category.as_deref() == Some(THOUGHT_LEVEL) && option.kind == OptionKind::Select
    })
}

pub(super) fn valid_value(option: &ConfigOption, value: &OptionValue) -> bool {
    match (option.kind, value) {
        (OptionKind::Boolean, OptionValue::Boolean(_)) => true,
        (OptionKind::Select, OptionValue::Select(value)) => {
            option.choices.iter().any(|choice| choice.value == *value)
        }
        _ => false,
    }
}

/// The configured `model` and `effort`, by the option each applies to, when
/// that option offers it.
pub(super) fn defaults(config: &HarnessConfig, options: &[ConfigOption]) -> Desired {
    let mut result = Desired::new();
    for (value, category, key) in [
        (&config.model, MODEL, "model"),
        (&config.effort, THOUGHT_LEVEL, "effort"),
    ] {
        let Some(value) = value else { continue };
        let option = options
            .iter()
            .find(|option| option.category.as_deref() == Some(category))
            .or_else(|| options.iter().find(|option| option.id == key));
        let value = OptionValue::Select(value.clone());
        if let Some(option) = option.filter(|option| valid_value(option, &value)) {
            result.insert(option.id.clone(), value);
        }
    }
    result
}

/// The settings in `requested` the runner still offers, at the runner's actual
/// values, so a clamped request persists as what the runner applied.
pub(super) fn reconcile(options: &[ConfigOption], requested: &Desired) -> Desired {
    options
        .iter()
        .filter(|option| requested.contains_key(&option.id) && valid_value(option, &option.value))
        .map(|option| (option.id.clone(), option.value.clone()))
        .collect()
}

/// viz's view of the runner's options: categories inferred from well-known ids,
/// model choices grouped by provider, redundant name prefixes dropped, and a
/// legacy mode selector that duplicates thinking levels removed.
pub(super) fn normalize(options: Vec<SessionConfigOption>) -> Vec<ConfigOption> {
    let normalized: Vec<ConfigOption> = options.into_iter().filter_map(convert).collect();
    let Some(thinking) = thinking_option(&normalized) else {
        return normalized;
    };
    // Older ACP clients used a legacy mode selector for thinking. Some runners
    // expose it in both forms.
    let duplicate = |option: &ConfigOption| {
        option.category.as_deref() == Some(MODE)
            && option.kind == OptionKind::Select
            && option.choices.len() == thinking.choices.len()
            && option
                .choices
                .iter()
                .all(|choice| valid_value(thinking, &OptionValue::Select(choice.value.clone())))
    };
    let keep: Vec<bool> = normalized.iter().map(|option| !duplicate(option)).collect();
    normalized
        .into_iter()
        .zip(keep)
        .filter_map(|(option, keep)| keep.then_some(option))
        .collect()
}

fn convert(option: SessionConfigOption) -> Option<ConfigOption> {
    let id = option.id.0.to_string();
    let category = option
        .category
        .and_then(|category| serde_json::to_value(category).ok())
        .and_then(|value| value.as_str().map(str::to_owned))
        .or_else(|| {
            [MODEL, THOUGHT_LEVEL, MODE]
                .contains(&id.as_str())
                .then(|| id.clone())
        });
    let (kind, value, choices) = match option.kind {
        SessionConfigKind::Select(select) => {
            let display = |item: SessionConfigSelectOption, group: Option<&str>| {
                choice(item, group, category.as_deref(), &option.name)
            };
            let choices = match select.options {
                SessionConfigSelectOptions::Ungrouped(items) => {
                    items.into_iter().map(|item| display(item, None)).collect()
                }
                SessionConfigSelectOptions::Grouped(groups) => groups
                    .into_iter()
                    .flat_map(|group| {
                        let name = group.name;
                        group
                            .options
                            .into_iter()
                            .map(move |item| (item, name.clone()))
                    })
                    .map(|(item, group)| display(item, Some(&group)))
                    .collect(),
                _ => Vec::new(),
            };
            (
                OptionKind::Select,
                OptionValue::Select(select.current_value.0.to_string()),
                choices,
            )
        }
        SessionConfigKind::Boolean(boolean) => (
            OptionKind::Boolean,
            OptionValue::Boolean(boolean.current_value),
            Vec::new(),
        ),
        _ => return None,
    };
    Some(ConfigOption {
        id,
        name: option.name,
        description: option.description,
        category,
        kind,
        value,
        choices,
    })
}

fn choice(
    item: SessionConfigSelectOption,
    group: Option<&str>,
    category: Option<&str>,
    option: &str,
) -> OptionChoice {
    let value = item.value.0.to_string();
    let mut name = item.name;
    let mut label = group.map(str::to_owned);
    match category {
        Some(MODEL) => {
            // The runner's group, else the provider before a `/` in the value;
            // a model with neither stays ungrouped.
            label = label.or_else(|| {
                value
                    .split_once('/')
                    .map(|(provider, _)| provider.to_owned())
            });
            if let Some(label) = &label {
                let prefix = format!("{}/", label.to_lowercase());
                let end = label.len() + 1;
                if name
                    .get(..end)
                    .is_some_and(|head| head.to_lowercase() == prefix)
                {
                    name = name[end..].to_owned();
                }
            }
        }
        Some(THOUGHT_LEVEL) => {
            if let Some(rest) = name
                .strip_prefix(option)
                .and_then(|rest| rest.strip_prefix(": "))
            {
                name = rest.to_owned();
            }
        }
        _ => {}
    }
    OptionChoice {
        value,
        name,
        description: item.description,
        group: label,
        thinking: None,
    }
}

/// The current model's thinking levels; a value learned on first observation is
/// kept unless the runner no longer allows it.
fn model_profile(options: &[ConfigOption], previous: &[ModelProfile]) -> Option<ModelProfile> {
    let OptionValue::Select(model) = &model_option(options)?.value else {
        return None;
    };
    let Some(option) = thinking_option(options) else {
        return Some(ModelProfile {
            model: model.clone(),
            thinking: None,
        });
    };
    let prior = previous
        .iter()
        .find(|profile| profile.model == *model)
        .and_then(|profile| profile.thinking.as_ref())
        .map(|thinking| &thinking.value)
        .filter(|value| valid_value(option, value));
    Some(ModelProfile {
        model: model.clone(),
        thinking: Some(DependentOption {
            id: option.id.clone(),
            name: option.name.clone(),
            description: option.description.clone(),
            value: prior.unwrap_or(&option.value).clone(),
            choices: option
                .choices
                .iter()
                .map(|choice| DependentChoice {
                    value: choice.value.clone(),
                    name: choice.name.clone(),
                    description: choice.description.clone(),
                })
                .collect(),
        }),
    })
}

/// Model choices annotated with their thinking levels, at the configured effort
/// where a model offers it.
fn with_model_profiles(
    options: Vec<ConfigOption>,
    profiles: &[ModelProfile],
    configured_effort: Option<&str>,
) -> Vec<ConfigOption> {
    if profiles.is_empty() {
        return options;
    }
    options
        .into_iter()
        .map(|mut option| {
            if option.category.as_deref() != Some(MODEL) {
                return option;
            }
            for choice in &mut option.choices {
                let thinking = profiles
                    .iter()
                    .find(|profile| profile.model == choice.value)
                    .and_then(|profile| profile.thinking.as_ref());
                let Some(thinking) = thinking else { continue };
                let mut thinking = thinking.clone();
                if let Some(effort) = configured_effort
                    && thinking.choices.iter().any(|choice| choice.value == effort)
                {
                    thinking.value = OptionValue::Select(effort.to_owned());
                }
                choice.thinking = Some(thinking);
            }
            option
        })
        .collect()
}
