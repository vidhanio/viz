//! Lossless conversion between atomic editor pills and canonical prompt links.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use serde::{Deserialize, Serialize};
use viz_client::protocol::{Prompt, PromptRef};

use crate::model::DraftData;

/// Stable identity for a reference throughout editor changes and draft merges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RefId(uuid::Uuid);

impl RefId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl std::fmt::Display for RefId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// The mark carries its target through Rune's undo history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PillRef {
    id: RefId,
    pub target: PromptRef,
}

impl PillRef {
    pub fn new(target: PromptRef) -> Self {
        Self::with_id(RefId::new(), target)
    }

    pub const fn with_id(id: RefId, target: PromptRef) -> Self {
        Self { id, target }
    }

    pub fn mark(&self) -> String {
        serde_json::to_string(self).expect("prompt references serialize to JSON")
    }

    pub fn from_mark(mark: &str) -> Option<Self> {
        serde_json::from_str(mark).ok()
    }
}

/// A typed mark and the byte range occupied by its visible label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftPill {
    pub range: Range<usize>,
    pub reference: PillRef,
}

/// Encodes the currently present marks in source order and assigns wire
/// indices independently of the editor's stable identities.
pub fn encode(draft: &DraftData) -> Prompt {
    let mut prompt = Prompt::default();
    let mut indices = HashMap::new();
    let mut links = Vec::with_capacity(draft.pills.len());
    let mut end = 0;
    for pill in &draft.pills {
        let Some(before) = draft.text.get(end..pill.range.start) else {
            continue;
        };
        let Some(label) = draft.text.get(pill.range.clone()) else {
            continue;
        };
        prompt.text.push_str(before);
        let index = *indices.entry(pill.reference.id).or_insert_with(|| {
            let index = prompt.refs.len();
            prompt.refs.push(pill.reference.target.clone());
            index
        });
        let start = prompt.text.len();
        prompt.text.push_str(&Prompt::reference_link(label, index));
        links.push((start..prompt.text.len(), label));
        end = pill.range.end;
    }
    prompt.text.push_str(&draft.text[end..]);
    restore_literal_labels(&mut prompt, links);
    prompt
}

/// Markdown can suppress a link inside code or another link. Keep its label
/// in that context and move the typed link outside the surrounding Markdown.
fn restore_literal_labels(prompt: &mut Prompt, links: Vec<(Range<usize>, &str)>) {
    if links.is_empty() {
        return;
    }
    let active: HashSet<_> = prompt.anchors().map(|anchor| anchor.range.start).collect();
    let hidden = links
        .into_iter()
        .filter(|(range, _)| !active.contains(&range.start));
    let mut prefix = String::new();
    let mut body = String::new();
    let mut end = 0;
    for (range, label) in hidden {
        prefix.push_str(&prompt.text[range.clone()]);
        prefix.push_str("\n\n");
        body.push_str(&prompt.text[end..range.start]);
        body.push_str(label);
        end = range.end;
    }
    if !prefix.is_empty() {
        prefix.push_str(&body);
        prefix.push_str(&prompt.text[end..]);
        prompt.text = prefix;
    }
}

/// Reconstructs editor text and marks from any canonical prompt.
pub fn decode(prompt: &Prompt) -> DraftData {
    let mut draft = DraftData::default();
    let mut references = HashMap::new();
    let mut end = 0;
    for anchor in prompt.anchors() {
        draft.text.push_str(&prompt.text[end..anchor.range.start]);
        let start = draft.text.len();
        draft.text.push_str(&anchor.label);
        if start < draft.text.len() {
            let reference = references
                .entry(anchor.index)
                .or_insert_with(|| PillRef::new(anchor.target.clone()));
            draft.pills.push(DraftPill {
                range: start..draft.text.len(),
                reference: reference.clone(),
            });
        }
        end = anchor.range.end;
    }
    draft.text.push_str(&prompt.text[end..]);
    draft
}

/// Removes matching marks and shifts the remaining byte ranges.
pub fn remove(draft: &mut DraftData, mut matches: impl FnMut(&PromptRef) -> bool) {
    let mut text = String::with_capacity(draft.text.len());
    let mut end = 0;
    draft.pills.retain_mut(|pill| {
        let range = pill.range.clone();
        text.push_str(&draft.text[end..range.start]);
        end = range.end;
        if matches(&pill.reference.target) {
            false
        } else {
            let start = text.len();
            text.push_str(&draft.text[range]);
            pill.range = start..text.len();
            true
        }
    });
    text.push_str(&draft.text[end..]);
    draft.text = text;
}

/// Appends marks to a draft that is not currently open in the editor.
pub fn append(draft: &mut DraftData, refs: impl IntoIterator<Item = (PillRef, String)>) {
    for (reference, label) in refs {
        if !draft.text.is_empty() && !draft.text.ends_with(char::is_whitespace) {
            draft.text.push(' ');
        }
        let start = draft.text.len();
        draft.text.push_str(&label);
        draft.pills.push(DraftPill {
            range: start..draft.text.len(),
            reference,
        });
    }
}
