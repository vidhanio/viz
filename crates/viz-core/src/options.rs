//! Reconcile thread summary labels with the options reported by ACP.

use viz_protocol::{ConfigOption, OptionValue};

/// The model and thinking labels come from the selected choices in the latest
/// snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Labels<'a> {
    pub model: Option<&'a str>,
    pub thinking: Option<&'a str>,
}

impl<'a> Labels<'a> {
    /// A selected value without a known choice retains its identifier as the
    /// label.
    #[must_use]
    pub fn from_options(options: &'a [ConfigOption]) -> Self {
        Self {
            model: option_label(options, "model"),
            thinking: option_label(options, "thought_level"),
        }
    }
}

fn option_label<'a>(options: &'a [ConfigOption], category: &str) -> Option<&'a str> {
    let option = options
        .iter()
        .find(|option| option.category.as_deref() == Some(category))?;
    let OptionValue::Select(value) = &option.value else {
        return None;
    };
    Some(
        option
            .choices
            .iter()
            .find(|item| &item.value == value)
            .map_or(value.as_str(), |item| item.name.as_str()),
    )
}
