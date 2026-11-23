//! Prompt content assembly and attachment I/O.

use std::{fmt::Write as _, path::Path};

use base64::{Engine as _, prelude::BASE64_STANDARD};
use url::Url;
use viz_protocol::PromptContext;

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
        description.push_str("\n\nAttachments (local files):");
    }
    for (attachment, _) in attachments {
        let _ = write!(
            description,
            "\n- {}: {}",
            attachment.name,
            attachment.path.display()
        );
    }
    let mut blocks = vec![ContentBlock::Text(TextContent::new(description))];
    validate_context(context, capabilities)?;
    blocks.extend_from_slice(context);
    for (attachment, bytes) in attachments {
        if let Some(bytes) = bytes {
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
    let mut bytes = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let inline = (capabilities.image && attachment.mime.starts_with("image/"))
            || (capabilities.audio && attachment.mime.starts_with("audio/"));
        bytes.push(if inline {
            Some(
                tokio::fs::read(&attachment.path)
                    .await
                    .map_err(|source| Error::Io {
                        operation: "reading prompt attachment",
                        source,
                    })?,
            )
        } else {
            None
        });
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
