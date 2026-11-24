//! Prompt content assembly and attachment I/O.

use std::{collections::HashSet, fmt::Write as _, path::Path};

use base64::{Engine as _, prelude::BASE64_STANDARD};
use url::Url;
use viz_protocol::{Prompt, PromptRef};

use crate::{
    Capabilities, Error,
    error::Result,
    lifecycle::require,
    schema::{AudioContent, ContentBlock, ImageContent, ResourceLink, TextContent},
};

/// Builds text and resource blocks from the prompt's typed Markdown links.
///
/// # Errors
/// Returns an error for an invalid attachment path.
pub fn build_prompt(
    prompt: &Prompt,
    cwd: &Path,
    attachments: &[(&StoredAttachment, Option<&[u8]>)],
    capabilities: &Capabilities,
) -> Result<Vec<ContentBlock>> {
    let mut description = String::new();
    let mut resources = Vec::new();
    let mut emitted = HashSet::new();
    let mut end = 0;
    for anchor in prompt.anchors() {
        description.push_str(&prompt.text[end..anchor.range.start]);
        end = anchor.range.end;
        let target = match anchor.target {
            PromptRef::File { path, .. } => Url::from_file_path(cwd.join(path))
                .ok()
                .map(|uri| ResourceLink::new(path, uri.to_string())),
            PromptRef::Link { uri, name, title } => {
                Some(ResourceLink::new(name, uri).title(title.clone()))
            }
            PromptRef::GitHub { item } => {
                Some(ResourceLink::new(&anchor.label, &item.url).title(item.title.clone()))
            }
            PromptRef::Attachment { id, name, .. } => attachments
                .iter()
                .find(|(item, _)| &item.id == id && &item.name == name)
                .map(|(item, _)| {
                    file_uri(&item.path).map(|uri| {
                        ResourceLink::new(name, uri.to_string()).mime_type(item.mime.clone())
                    })
                })
                .transpose()?,
            _ => None,
        };
        if let Some(target) = target {
            let _ = write!(
                description,
                "[{}](<{}>)",
                anchor.label.replace(['[', ']'], " "),
                target.uri
            );
            if emitted.insert(anchor.index) {
                resources.push(ContentBlock::ResourceLink(target));
            }
        } else {
            description.push_str(&anchor.label);
        }
    }
    description.push_str(&prompt.text[end..]);
    let mut blocks = vec![ContentBlock::Text(TextContent::new(description))];
    blocks.extend(resources);
    let mut emitted_media = HashSet::new();
    for (attachment, bytes) in attachments {
        if let Some(bytes) = bytes
            && emitted_media.insert(attachment.id.as_str())
        {
            if capabilities.image && attachment.mime.starts_with("image/") {
                blocks.push(ContentBlock::Image(ImageContent::new(
                    BASE64_STANDARD.encode(bytes),
                    &attachment.mime,
                )));
            } else if capabilities.audio && attachment.mime.starts_with("audio/") {
                blocks.push(ContentBlock::Audio(AudioContent::new(
                    BASE64_STANDARD.encode(bytes),
                    &attachment.mime,
                )));
            }
        }
    }
    Ok(blocks)
}

pub fn validate_context(context: &[ContentBlock], capabilities: &Capabilities) -> Result<()> {
    for block in context {
        match block {
            ContentBlock::Image(_) => require(capabilities.image, "image prompts")?,
            ContentBlock::Audio(_) => require(capabilities.audio, "audio prompts")?,
            ContentBlock::Resource(_) => {
                require(capabilities.embedded_context, "embedded context")?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn file_uri(path: &Path) -> Result<Url> {
    Url::from_file_path(path).map_err(|()| Error::Path(path.to_owned()))
}

pub async fn load_attachments(
    attachments: &[StoredAttachment],
    capabilities: &Capabilities,
) -> Result<Vec<Option<Vec<u8>>>> {
    let mut loaded = HashSet::new();
    let mut remaining = viz_protocol::limits::ATTACHMENT_TOTAL_BYTES;
    let mut bytes = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let inline = (capabilities.image && attachment.mime.starts_with("image/"))
            || (capabilities.audio && attachment.mime.starts_with("audio/"));
        if inline && loaded.insert(attachment.id.as_str()) {
            let data = tokio::fs::read(&attachment.path)
                .await
                .map_err(|source| Error::Io {
                    operation: "reading prompt attachment",
                    source,
                })?;
            remaining = remaining
                .checked_sub(data.len())
                .ok_or_else(|| Error::Unsupported("prompt attachments exceed media byte limit"))?;
            bytes.push(Some(data));
        } else {
            bytes.push(None);
        }
    }
    Ok(bytes)
}

/// A stored file available to attach to a prompt.
#[derive(Debug, Clone)]
pub struct StoredAttachment {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub path: std::path::PathBuf,
}
