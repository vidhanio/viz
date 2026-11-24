//! Prompt content assembly and attachment I/O.

use std::{collections::HashSet, fmt::Write as _, path::Path};

use base64::{Engine as _, prelude::BASE64_STANDARD};
use url::Url;
use viz_protocol::{
    PromptContext, attachment_inline_md, attachment_ref_labels, text_references_label,
};

use crate::{
    Capabilities, Error,
    error::Result,
    lifecycle::require,
    schema::{AudioContent, ContentBlock, ImageContent, ResourceLink, TextContent},
};

/// Resource links for the files and links an input mentions. Relative paths
/// are taken from `cwd`.
#[must_use]
pub fn context_blocks(context: &[PromptContext], cwd: &Path) -> Vec<ContentBlock> {
    context
        .iter()
        .filter_map(|item| match item {
            PromptContext::File { path } => {
                let path = cwd.join(path);
                let uri = Url::from_file_path(&path).ok()?;
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                Some(ContentBlock::ResourceLink(ResourceLink::new(
                    name,
                    uri.to_string(),
                )))
            }
            PromptContext::Link { uri, name, title } => Some(ContentBlock::ResourceLink(
                ResourceLink::new(name.clone(), uri.clone()).title(title.clone()),
            )),
        })
        .collect()
}

/// Creates an ordered ACP prompt from text, typed context, and loaded files.
///
/// # Errors
/// Returns an error for unsupported content or an invalid attachment path.
pub fn build_prompt(
    text: &str,
    context: &[ContentBlock],
    attachments: &[(&StoredAttachment, Option<&[u8]>)],
    capabilities: &Capabilities,
) -> Result<Vec<ContentBlock>> {
    let mut description = text.to_owned();
    if !attachments.is_empty() {
        let labels = attachment_ref_labels(attachments.iter().map(|(a, _)| a.mime.as_str()));
        let unreferenced: Vec<String> = attachments
            .iter()
            .zip(&labels)
            .filter(|(_, label)| !text_references_label(text, label))
            .map(|((attachment, _), label)| {
                attachment_inline_md(&attachment.name, &attachment.mime, label)
            })
            .collect();
        if !unreferenced.is_empty() {
            if !description.trim().is_empty() {
                description.push_str("\n\n");
            }
            description.push_str(&unreferenced.join(" "));
        }
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        for (idx, ((attachment, _), label)) in attachments.iter().zip(&labels).enumerate() {
            let uri = file_uri(&attachment.path)?;
            let escaped_title = attachment.name.replace('\\', "\\\\").replace('"', "\\\"");
            if idx > 0 {
                description.push('\n');
            }
            let _ = write!(description, "[{label}]: <{uri}> \"{escaped_title}\"");
        }
    }
    let mut blocks = vec![ContentBlock::Text(TextContent::new(description))];
    validate_context(context, capabilities)?;
    blocks.extend_from_slice(context);
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
        blocks.push(ContentBlock::ResourceLink(
            ResourceLink::new(&attachment.name, file_uri(&attachment.path)?.as_str())
                .mime_type(attachment.mime.clone()),
        ));
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
