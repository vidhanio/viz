//! Local request submission and elicitation form state.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use viz_client::protocol::{FormValue, RequestBody, RequestId, RequestState, RunRequest};

use super::Model;

#[derive(Debug, Clone, Default)]
pub struct Requests {
    pub environment: String,
    pub sending: BTreeSet<RequestId>,
    pub resolved: BTreeSet<RequestId>,
    pub forms: BTreeMap<RequestId, Form>,
}

#[derive(Debug, Clone, Default)]
pub struct Form {
    pub step: usize,
    pub collapsed: bool,
    pub fields: BTreeMap<String, FormValue>,
    pub error: Option<String>,
}

pub fn pending(model: &Model) -> impl Iterator<Item = &RunRequest> {
    model
        .thread_state()
        .into_iter()
        .flat_map(|state| &state.requests)
        .filter(|request| {
            request.state == RequestState::Pending && !model.requests.resolved.contains(&request.id)
        })
}

pub fn question(model: &Model) -> Option<&RunRequest> {
    pending(model).find(|request| matches!(request.body, RequestBody::Question { .. }))
}

pub fn fields(schema: &Value) -> Vec<(&str, &Value)> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, field)| (name.as_str(), field))
        .collect()
}

pub fn required(schema: &Value, name: &str) -> bool {
    schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|keys| keys.iter().any(|key| key.as_str() == Some(name)))
}

pub fn choices(field: &Value) -> Vec<(&str, &str, Option<&str>)> {
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

pub fn parse_text(field: &Value, text: String) -> Result<FormValue, &'static str> {
    match field.get("type").and_then(Value::as_str) {
        Some("integer") => text
            .trim()
            .parse::<i64>()
            .map(serde_json::Number::from)
            .or_else(|_| text.trim().parse::<u64>().map(serde_json::Number::from))
            .map(FormValue::Number)
            .map_err(|_| "Enter a whole number."),
        Some("number") => text
            .trim()
            .parse::<serde_json::Number>()
            .map(FormValue::Number)
            .map_err(|_| "Enter a number."),
        Some("array" | "object" | "boolean") => Err("Select an offered value."),
        _ => Ok(FormValue::Text(text)),
    }
}
