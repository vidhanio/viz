//! Correlates viz tool results with the ACP tool calls that received them.

use std::{collections::HashMap, sync::Mutex};

use serde_json::Value;
use uuid::Uuid;
use viz_protocol::ThreadId;

use crate::tool::VizLink;

const RECEIPT_PREFIX: &str = "[viz-tool:";
/// A hyphenated UUID's length.
const RECEIPT_ID_LEN: usize = 36;

/// Receipts issued during one turn.
///
/// An MCP tool result carries a marker naming its receipt. The first ACP call
/// to report that receipt claims it.
#[derive(Debug, Default)]
pub struct Receipts {
    calls: Mutex<ReceiptBook>,
}

#[derive(Debug)]
struct Issued {
    link: VizLink,
    claimed_by: Option<String>,
}

impl Receipts {
    /// A marker for a viz tool's response text. Some runners only forward MCP
    /// text blocks.
    ///
    /// # Panics
    /// Panics if the receipts lock is poisoned.
    pub(super) fn issue(
        &self,
        tool: &str,
        child: Option<ThreadId>,
        environment: Option<String>,
    ) -> String {
        let id = Uuid::new_v4().hyphenated().to_string();
        let issued = Issued {
            link: VizLink {
                tool: tool.to_owned(),
                child,
                environment,
            },
            claimed_by: None,
        };
        self.with_book(|book| book.calls.insert(id.clone(), issued));
        format!("{RECEIPT_PREFIX}{id}]")
    }

    pub(super) fn with_book<T>(&self, fold: impl FnOnce(&mut ReceiptBook) -> T) -> T {
        fold(&mut self.calls.lock().expect("receipts lock poisoned"))
    }
}

#[derive(Debug, Default)]
pub struct ReceiptBook {
    calls: HashMap<String, Issued>,
}

impl ReceiptBook {
    /// `text` without this turn's markers, and the link a marker in it gives
    /// tool call `call`.
    pub fn strip(&mut self, text: &str, call: &str) -> (String, Option<VizLink>) {
        let mut link = None;
        let mut result = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find(RECEIPT_PREFIX) {
            let after = &rest[start + RECEIPT_PREFIX.len()..];
            let id = after.get(..RECEIPT_ID_LEN).filter(|id| {
                id.bytes().all(|byte| {
                    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-'
                }) && after[RECEIPT_ID_LEN..].starts_with(']')
            });
            let Some(issued) = id.and_then(|id| self.calls.get_mut(id)) else {
                result.push_str(&rest[..start + RECEIPT_PREFIX.len()]);
                rest = after;
                continue;
            };
            result.push_str(&rest[..start]);
            rest = &after[RECEIPT_ID_LEN + 1..];
            if issued.claimed_by.get_or_insert_with(|| call.to_owned()) == call {
                link = Some(issued.link.clone());
            }
        }
        result.push_str(rest);
        (result, link)
    }

    /// Strips every string in `value`.
    pub fn strip_value(&mut self, value: &mut Value, call: &str) -> Option<VizLink> {
        match value {
            Value::String(text) if text.contains(RECEIPT_PREFIX) => {
                let (stripped, link) = self.strip(text, call);
                *text = stripped;
                link
            }
            Value::Array(items) => items
                .iter_mut()
                .fold(None, |link, item| self.strip_value(item, call).or(link)),
            Value::Object(fields) => fields
                .values_mut()
                .fold(None, |link, field| self.strip_value(field, call).or(link)),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
        }
    }
}
