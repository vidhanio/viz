//! Typed references embedded in Markdown prompts.

use std::ops::Range;

use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{GitHubItem, PromptAttachment, ThreadId};

/// Markdown with inline links into an ordered list of typed targets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Prompt {
    pub text: String,
    pub refs: Vec<PromptRef>,
}

/// A target addressed by a `ref:N` link.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptRef {
    File {
        path: String,
        directory: bool,
    },
    Attachment {
        id: String,
        name: String,
        mime: String,
    },
    GitHub {
        item: GitHubItem,
    },
    Link {
        uri: String,
        name: String,
        title: Option<String>,
    },
    Thread {
        thread: ThreadId,
    },
    Skill {
        name: String,
    },
    Environment {
        name: String,
    },
    Model {
        name: String,
    },
}

/// A valid inline reference and its complete Markdown source range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAnchor<'a> {
    pub range: Range<usize>,
    pub index: usize,
    pub target: &'a PromptRef,
    pub label: String,
}

impl Prompt {
    /// Creates a prompt from plain Markdown.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            refs: Vec::new(),
        }
    }

    /// Iterates attachment metadata in reference order.
    pub fn attachments(&self) -> impl Iterator<Item = PromptAttachment> + '_ {
        self.refs.iter().filter_map(|target| match target {
            PromptRef::Attachment { id, name, mime } => Some(PromptAttachment {
                id: id.clone(),
                name: name.clone(),
                mime: mime.clone(),
            }),
            _ => None,
        })
    }

    /// Adds an attachment link using its display name.
    pub fn append_attachment(&mut self, attachment: PromptAttachment) {
        self.append_reference(
            &attachment.name,
            PromptRef::Attachment {
                id: attachment.id,
                name: attachment.name.clone(),
                mime: attachment.mime,
            },
        );
    }

    /// Adds a reference on its own line.
    pub fn append_reference(&mut self, label: &str, target: PromptRef) {
        let link = self.reference(label, target);
        if !self.text.is_empty() {
            self.text.push_str("\n\n");
        }
        self.text.push_str(&link);
    }

    /// Visits inline links in source order. Code, escaped links, and links
    /// pointing outside the reference list do not yield anchors.
    pub fn anchors(&self) -> impl Iterator<Item = PromptAnchor<'_>> {
        let mut pending = None;
        Parser::new(&self.text)
            .into_offset_iter()
            .filter_map(move |(event, range)| match event {
                Event::Start(Tag::Link {
                    link_type: LinkType::Inline,
                    dest_url,
                    ..
                }) => {
                    pending = dest_url
                        .strip_prefix("ref:")
                        .filter(|index| {
                            !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
                        })
                        .and_then(|index| index.parse::<usize>().ok())
                        .and_then(|index| {
                            Some(PromptAnchor {
                                range,
                                index,
                                target: self.refs.get(index)?,
                                label: String::new(),
                            })
                        });
                    None
                }
                Event::End(TagEnd::Link) => pending.take(),
                Event::Text(text) | Event::Code(text) => {
                    if let Some(anchor) = &mut pending {
                        anchor.label.push_str(&text);
                    }
                    None
                }
                Event::SoftBreak | Event::HardBreak => {
                    if let Some(anchor) = &mut pending {
                        anchor.label.push(' ');
                    }
                    None
                }
                _ => None,
            })
    }

    /// Returns Markdown with reference links replaced by their display labels.
    #[must_use]
    pub fn display_text(&self) -> String {
        let mut text = String::new();
        let mut end = 0;
        for anchor in self.anchors() {
            text.push_str(&self.text[end..anchor.range.start]);
            text.push_str(&anchor.label);
            end = anchor.range.end;
        }
        text.push_str(&self.text[end..]);
        text
    }

    /// Appends a typed target and returns its escaped Markdown link.
    pub fn reference(&mut self, label: &str, target: PromptRef) -> String {
        let index = self.refs.len();
        self.refs.push(target);
        Self::reference_link(label, index)
    }

    /// Formats a Markdown reference to an existing index.
    #[must_use]
    pub fn reference_link(label: &str, index: usize) -> String {
        let mut escaped = String::with_capacity(label.len());
        for ch in label.chars() {
            if ch.is_ascii_punctuation() {
                escaped.push('\\');
            }
            escaped.push(if matches!(ch, '\r' | '\n') { ' ' } else { ch });
        }
        format!("[{escaped}](ref:{index})")
    }
}
