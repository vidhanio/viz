//! Pure derivations for runner configuration options, the model/thinking
//! picker, and inline composer completions (`/` commands and `@` file
//! mentions).

use std::{borrow::Cow, collections::HashMap};

use viz_client::protocol::{
    ConfigOption, DependentOption, Harness, OptionChoice, OptionKind, OptionValue, SlashCommand,
    ThreadId,
};

use super::{CompletionKind, CompletionToken, DraftCursor, Model, NewThread};

/// Semantic role of a runner configuration option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionRole {
    /// Model selector.
    Model,
    /// Reasoning / thinking effort level.
    Thinking,
    /// Mode or permission policy hidden from overflow settings.
    Hidden,
    /// Additional runner-specific option shown under "More".
    Other,
}

/// Classifies `option` by its ACP category.
#[must_use]
pub fn option_role(option: &ConfigOption) -> OptionRole {
    match option.category.as_deref() {
        Some("model") => OptionRole::Model,
        Some("thought_level" | "effort") => OptionRole::Thinking,
        Some("mode" | "permission" | "permissions") => OptionRole::Hidden,
        _ => OptionRole::Other,
    }
}

/// Whose options the composer shows: a fresh draft or an open thread.
#[derive(Debug, Clone, Copy)]
pub enum OptionTarget<'a> {
    /// Local unsent draft (options sent on launch).
    Draft(&'a NewThread),
    /// Existing server thread (options applied via `thread.configure`).
    Thread(ThreadId),
}

/// Owned handle identifying an option's target for messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionOwner {
    /// Local draft id.
    Draft(u64),
    /// Existing thread id.
    Thread(ThreadId),
}

impl<'a> OptionTarget<'a> {
    /// Effective value of `option`: draft override if present, else runner's.
    #[must_use]
    pub fn value(self, option: &'a ConfigOption) -> &'a OptionValue {
        match self {
            Self::Draft(thread) => thread.options.get(&option.id).unwrap_or(&option.value),
            Self::Thread(_) => &option.value,
        }
    }

    /// Converts the borrowed target into an owned [`OptionOwner`].
    #[must_use]
    pub const fn owner(self) -> OptionOwner {
        match self {
            Self::Draft(thread) => OptionOwner::Draft(thread.id),
            Self::Thread(id) => OptionOwner::Thread(id),
        }
    }
}

/// Thinking levels: the runner's own option for its current model, or the
/// levels reported for another model a draft picked.
#[derive(Debug, Clone, Copy)]
pub enum ModelLevels<'a> {
    /// Runner's current thinking option.
    Current(&'a ConfigOption),
    /// Model-dependent thinking levels cached in the catalog.
    Model(&'a DependentOption),
}

impl<'a> ModelLevels<'a> {
    /// Option identifier sent to `thread.configure`.
    #[must_use]
    pub fn id(self) -> &'a str {
        match self {
            Self::Current(option) => &option.id,
            Self::Model(option) => &option.id,
        }
    }

    /// Human-readable option name.
    #[must_use]
    pub fn name(self) -> &'a str {
        match self {
            Self::Current(option) => &option.name,
            Self::Model(option) => &option.name,
        }
    }

    /// Available `(value, name)` pairs.
    pub fn choices(self) -> impl ExactSizeIterator<Item = (&'a str, &'a str)> {
        let len = match self {
            Self::Current(option) => option.choices.len(),
            Self::Model(option) => option.choices.len(),
        };
        (0..len).map(move |index| match self {
            Self::Current(option) => {
                let choice = &option.choices[index];
                (choice.value.as_str(), choice.name.as_str())
            }
            Self::Model(option) => {
                let choice = &option.choices[index];
                (choice.value.as_str(), choice.name.as_str())
            }
        })
    }

    /// Whether `value` is offered by this level list.
    #[must_use]
    pub fn has(self, value: &str) -> bool {
        self.choices().any(|(choice, _)| choice == value)
    }

    /// Display label for `value`.
    #[must_use]
    pub fn label(self, value: &'a str) -> Cow<'a, str> {
        self.choices()
            .find(|(choice, _)| *choice == value)
            .map_or_else(
                || thinking_label(value),
                |(_, name)| self.choice_label(value, name),
            )
    }

    /// Cleans a runner choice label, stripping redundant option-name prefixes.
    #[must_use]
    pub fn choice_label(self, value: &'a str, name: &'a str) -> Cow<'a, str> {
        let name = name
            .split_once(':')
            .filter(|(prefix, _)| prefix.trim().eq_ignore_ascii_case(self.name()))
            .map_or(name, |(_, rest)| rest)
            .trim();
        thinking_label(if name.is_empty() { value } else { name })
    }

    /// Selected level: draft's override when valid for this model, else the
    /// runner's current value.
    #[must_use]
    pub fn value(self, target: OptionTarget<'a>) -> Option<&'a str> {
        let own = match self {
            Self::Current(option) => &option.value,
            Self::Model(option) => &option.value,
        };
        let chosen = match target {
            OptionTarget::Draft(thread) => thread.options.get(self.id()),
            OptionTarget::Thread(_) => None,
        };
        [chosen, Some(own)]
            .into_iter()
            .flatten()
            .find_map(|value| match value {
                OptionValue::Select(value) if self.has(value) => Some(value.as_str()),
                _ => None,
            })
    }
}

/// Derived model and thinking picker projection for a draft or thread.
#[derive(Debug)]
pub struct ModelPicker<'a> {
    /// Option owner target.
    pub target: OptionTarget<'a>,
    /// Model option, if advertised.
    pub model: Option<&'a ConfigOption>,
    /// Thinking-levels option for the selected model, if more than one choice.
    pub levels: Option<ModelLevels<'a>>,
}

impl<'a> ModelPicker<'a> {
    /// Builds a picker projection when `options` contain a model or thinking
    /// selector.
    #[must_use]
    pub fn new(target: OptionTarget<'a>, options: &'a [ConfigOption]) -> Option<Self> {
        let find = |wanted: OptionRole| {
            options
                .iter()
                .find(|option| option_role(option) == wanted && option.kind == OptionKind::Select)
        };
        let model = find(OptionRole::Model);
        let thinking = find(OptionRole::Thinking).map(ModelLevels::Current);
        let levels = match (target, model) {
            (OptionTarget::Draft(_), Some(model)) => match target.value(model) {
                OptionValue::Select(value) if *target.value(model) != model.value => model
                    .choices
                    .iter()
                    .find(|choice| choice.value == *value)
                    .and_then(|choice| choice.thinking.as_ref())
                    .map(ModelLevels::Model),
                _ => thinking,
            },
            _ => thinking,
        }
        .filter(|levels| levels.choices().len() > 1);
        (model.is_some() || levels.is_some()).then_some(Self {
            target,
            model,
            levels,
        })
    }

    /// Currently selected model wire value.
    #[must_use]
    pub fn model_value(&self) -> Option<&'a str> {
        match self.model.map(|model| self.target.value(model)) {
            Some(OptionValue::Select(value)) => Some(value),
            _ => None,
        }
    }

    /// Currently selected model display name.
    #[must_use]
    pub fn model_label(&self) -> Option<&'a str> {
        self.model.zip(self.model_value()).map(|(model, value)| {
            model
                .choices
                .iter()
                .find(|choice| choice.value == value)
                .map_or(value, model_name)
        })
    }

    /// Selected thinking level `(levels, raw_value, label, index, total)`.
    #[must_use]
    pub fn level_selection(
        &self,
    ) -> Option<(ModelLevels<'a>, &'a str, Cow<'a, str>, usize, usize)> {
        let levels = self.levels?;
        let value = levels.value(self.target)?;
        let label = levels.label(value);
        let choices: Vec<_> = levels.choices().collect();
        let total = choices.len().max(1);
        let index = choices
            .iter()
            .position(|(choice, _)| *choice == value)
            .unwrap_or(0);
        Some((levels, value, label, index, total))
    }

    /// Accessible trigger label combining model name and thinking level.
    #[must_use]
    pub fn label(&self) -> Cow<'a, str> {
        let model = self.model_label();
        let level = self.level_selection();
        match (model, level) {
            (Some(model), Some((_, _, level, _, _))) => Cow::Owned(format!("{model} · {level}")),
            (Some(model), None) => Cow::Borrowed(model),
            (None, Some((levels, _, level, _, _))) => {
                Cow::Owned(format!("{} · {level}", sentence_label(levels.name())))
            }
            (None, None) => Cow::Borrowed("Model"),
        }
    }
}

/// One model row in the grouped picker list.
#[derive(Debug, Clone, Copy)]
pub struct ModelEntry<'a> {
    /// Wire value sent to the harness.
    pub value: &'a str,
    /// Display name.
    pub label: &'a str,
    /// Disambiguating suffix shown when duplicate labels exist in a group.
    pub detail: Option<&'a str>,
}

/// Groups models matching `query` by provider in the runner's order.
#[must_use]
pub fn model_groups<'a>(
    model: &'a ConfigOption,
    query: &str,
) -> Vec<(Option<&'a str>, Vec<ModelEntry<'a>>)> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut duplicates = HashMap::new();
    for choice in &model.choices {
        duplicates
            .entry((choice_group(choice), model_name(choice)))
            .and_modify(|duplicate| *duplicate = true)
            .or_insert(false);
    }
    let mut groups: Vec<(Option<&str>, Vec<ModelEntry<'a>>)> = Vec::new();
    for choice in &model.choices {
        let group = choice_group(choice);
        let label = model_name(choice);
        if !words.is_empty() {
            let id = group.unwrap_or_default();
            let haystack =
                format!("{} {id} {label} {}", provider_name(id), choice.value).to_lowercase();
            if !words.iter().all(|word| haystack.contains(word.as_str())) {
                continue;
            }
        }
        let entry = ModelEntry {
            value: &choice.value,
            label,
            detail: duplicates[&(group, label)].then(|| {
                choice
                    .value
                    .split_once('/')
                    .map_or(choice.value.as_str(), |(_, id)| id)
            }),
        };
        match groups.iter_mut().find(|(name, _)| *name == group) {
            Some((_, entries)) => entries.push(entry),
            None => groups.push((group, vec![entry])),
        }
    }
    groups
}

/// Whether `harness` has not yet reported thinking levels for `value`.
#[must_use]
pub fn unseen_model_levels(harness: &Harness, id: &str, value: &str) -> bool {
    harness
        .options
        .iter()
        .find(|option| option.id == id)
        .and_then(|model| model.choices.iter().find(|choice| choice.value == value))
        .is_some_and(|choice| {
            choice.value != harness_option_value(harness, id) && choice.thinking.is_none()
        })
}

fn harness_option_value<'a>(harness: &'a Harness, id: &str) -> &'a str {
    harness
        .options
        .iter()
        .find(|option| option.id == id)
        .and_then(|option| match &option.value {
            OptionValue::Select(value) => Some(value.as_str()),
            OptionValue::Boolean(_) => None,
        })
        .unwrap_or_default()
}

/// Option ids under which a draft may store thinking levels for `harness`.
#[must_use]
pub fn thinking_ids(harness: &Harness) -> Vec<String> {
    harness
        .options
        .iter()
        .flat_map(|option| match option_role(option) {
            OptionRole::Thinking => vec![option.id.clone()],
            OptionRole::Model => option
                .choices
                .iter()
                .filter_map(|choice| choice.thinking.as_ref().map(|levels| levels.id.clone()))
                .collect(),
            OptionRole::Hidden | OptionRole::Other => Vec::new(),
        })
        .collect()
}

fn choice_group(choice: &OptionChoice) -> Option<&str> {
    choice
        .group
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| choice.value.split_once('/').map(|(provider, _)| provider))
}

/// Display name for a model choice, falling back to its raw value.
#[must_use]
pub fn model_name(choice: &OptionChoice) -> &str {
    let label = choice.name.trim();
    if label.is_empty() {
        &choice.value
    } else {
        label
    }
}

/// Human-readable provider heading for a runner group id.
#[must_use]
pub fn provider_name(group: &str) -> Cow<'_, str> {
    let known = match group {
        "openai" => "OpenAI",
        "openai-codex" => "OpenAI Codex",
        "openrouter" => "OpenRouter",
        "xai" => "xAI",
        "deepseek" => "DeepSeek",
        "github-copilot" => "GitHub Copilot",
        "google-vertex" => "Google Vertex",
        "google-gemini-cli" => "Gemini CLI",
        "amazon-bedrock" => "Amazon Bedrock",
        "azure-openai-responses" => "Azure OpenAI",
        "huggingface" => "Hugging Face",
        "zai" => "Z.ai",
        "opencode" => "OpenCode",
        "opencode-go" => "OpenCode Go",
        label if label.chars().any(char::is_uppercase) => label,
        id if id.contains(['-', '_']) => {
            let mut words = String::with_capacity(id.len());
            for word in id.split(['-', '_']).filter(|word| !word.is_empty()) {
                if !words.is_empty() {
                    words.push(' ');
                }
                words.push_str(&sentence_label(word));
            }
            return Cow::Owned(words);
        }
        label => return sentence_label(label),
    };
    Cow::Borrowed(known)
}

/// Parses a leading `/command` or inline `@file` token at the composer caret.
#[must_use]
pub fn completion_token_at(text: &str, cursor: &DraftCursor) -> Option<CompletionToken> {
    if !cursor.focused || cursor.anchor != cursor.focus {
        return None;
    }
    let before = text.get(..cursor.focus)?;
    let word = before.rsplit(char::is_whitespace).next()?;
    let start = before.len() - word.len();
    let after = text.get(cursor.focus..)?;
    let end = cursor.focus + after.find(char::is_whitespace).unwrap_or(after.len());
    let (kind, query) = match word.as_bytes().first().copied() {
        Some(b'/') if start == 0 => (CompletionKind::Slash, &word[1..]),
        Some(b'@') if !word.starts_with("@\"") => (CompletionKind::File, &word[1..]),
        _ => return None,
    };
    Some(CompletionToken {
        kind,
        range: start..end,
        query: query.to_owned(),
    })
}

/// Slash commands advertised by the open thread matching `query`.
#[must_use]
pub fn matching_commands<'a>(model: &'a Model, query: &str) -> Vec<&'a SlashCommand> {
    model
        .thread_state()
        .into_iter()
        .flat_map(|state| &state.commands)
        .filter(|command| {
            command
                .name
                .get(..query.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(query))
        })
        .collect()
}

/// File completion results for the active search request.
#[must_use]
pub fn matching_files(model: &Model) -> &[String] {
    model
        .composer
        .completion
        .files
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map_or(&[], Vec::as_slice)
}

/// Formats the inserted text for a chosen slash command or file mention.
#[must_use]
pub fn format_completion_insertion(kind: CompletionKind, id: &str, trailing_text: &str) -> String {
    let mut insertion = match kind {
        CompletionKind::Slash => format!("/{id}"),
        CompletionKind::File if id.contains([' ', '\t', '\n', '"', '\\']) => {
            format!("@\"{}\"", id.replace('\\', "\\\\").replace('"', "\\\""))
        }
        CompletionKind::File => format!("@{id}"),
    };
    if !trailing_text.starts_with(char::is_whitespace) {
        insertion.push(' ');
    }
    insertion
}

fn sentence_label(label: &str) -> Cow<'_, str> {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) if first.is_lowercase() && !chars.clone().any(char::is_uppercase) => {
            Cow::Owned(first.to_uppercase().chain(chars).collect())
        }
        _ => Cow::Borrowed(label),
    }
}

fn thinking_label(label: &str) -> Cow<'_, str> {
    if ["xhigh", "x-high", "extra-high", "extra high"]
        .iter()
        .any(|id| label.eq_ignore_ascii_case(id))
    {
        Cow::Borrowed("Xhigh")
    } else {
        sentence_label(label)
    }
}
