//! Shared interpretation of primitive ACP elicitation fields.

use serde_json::Value;

use crate::FormValue;

/// Normalized field kind supported by the question UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Integer,
    Number,
    Boolean,
    Choices,
    Unsupported,
}

/// A field with its normalized type, choices, and presentation metadata.
#[derive(Debug, Clone)]
pub struct QuestionField<'a> {
    pub name: &'a str,
    pub title: &'a str,
    pub description: Option<&'a str>,
    pub required: bool,
    pub kind: FieldKind,
    pub options: Vec<(&'a str, &'a str, Option<&'a str>)>,
}

/// Interprets the stored ACP schema identically for editing and validation.
#[must_use]
pub fn question_fields(schema: &Value) -> Vec<QuestionField<'_>> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, field)| {
            let kind = match field.get("type").and_then(Value::as_str) {
                Some("integer") => FieldKind::Integer,
                Some("number") => FieldKind::Number,
                Some("boolean") => FieldKind::Boolean,
                Some("array") => FieldKind::Choices,
                Some("string") | None => FieldKind::Text,
                Some(_) => FieldKind::Unsupported,
            };
            let options = match kind {
                FieldKind::Boolean => vec![("true", "Yes", None), ("false", "No", None)],
                FieldKind::Text | FieldKind::Choices => choices(field),
                _ => Vec::new(),
            };
            QuestionField {
                name,
                title: field.get("title").and_then(Value::as_str).unwrap_or(name),
                description: field.get("description").and_then(Value::as_str),
                required: schema
                    .get("required")
                    .and_then(Value::as_array)
                    .is_some_and(|keys| keys.iter().any(|key| key.as_str() == Some(name))),
                kind,
                options,
            }
        })
        .collect()
}

impl QuestionField<'_> {
    /// Parses free text according to the field's primitive type.
    ///
    /// # Errors
    /// Returns an error for malformed numbers or a selection-only field.
    pub fn parse_text(&self, text: String) -> Result<FormValue, &'static str> {
        match self.kind {
            FieldKind::Integer => text
                .trim()
                .parse::<i64>()
                .map(serde_json::Number::from)
                .or_else(|_| text.trim().parse::<u64>().map(serde_json::Number::from))
                .map(FormValue::Number)
                .map_err(|_| "Enter a whole number."),
            FieldKind::Number => text
                .trim()
                .parse::<serde_json::Number>()
                .map(FormValue::Number)
                .map_err(|_| "Enter a number."),
            FieldKind::Text if self.options.is_empty() => Ok(FormValue::Text(text)),
            _ => Err("Select an offered value."),
        }
    }

    /// Checks the primitive type and offered choices before a reply is
    /// accepted.
    #[must_use]
    pub fn accepts(&self, value: &FormValue) -> bool {
        let offered = |value: &str| self.options.iter().any(|(option, ..)| *option == value);
        match (self.kind, value) {
            (FieldKind::Boolean, FormValue::Boolean(_))
            | (FieldKind::Number, FormValue::Number(_)) => true,
            (FieldKind::Integer, FormValue::Number(number)) => number.is_i64() || number.is_u64(),
            (FieldKind::Text, FormValue::Text(text)) => {
                (!self.required || !text.trim().is_empty())
                    && (self.options.is_empty() || offered(text))
            }
            (FieldKind::Choices, FormValue::Choices(values)) => {
                (!self.required || !values.is_empty()) && values.iter().all(|value| offered(value))
            }
            _ => false,
        }
    }
}

fn choices(field: &Value) -> Vec<(&str, &str, Option<&str>)> {
    let source = field.get("items").unwrap_or(field);
    if let Some(variants) = source
        .get("oneOf")
        .or_else(|| source.get("anyOf"))
        .and_then(Value::as_array)
    {
        return variants
            .iter()
            .filter_map(|variant| {
                let value = variant.get("const")?.as_str()?;
                Some((
                    value,
                    variant
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or(value),
                    variant.get("description").and_then(Value::as_str),
                ))
            })
            .collect();
    }
    source
        .get("enum")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, value)| {
            let value = value.as_str()?;
            let label = source
                .get("enumNames")
                .and_then(|names| names.get(index))
                .and_then(Value::as_str)
                .unwrap_or(value);
            Some((value, label, None))
        })
        .collect()
}
