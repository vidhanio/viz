//! Timeline-specific asynchronous RPC helpers (`thread.fork`,
//! `thread.entry_chunk`, file previews, and content-channel attachment fetches)
//! kept separate from `requests.rs`.

use viz_client::{
    Client, ContentClient, Result,
    protocol::{
        ContentVariant, EntryData, EntrySource, FileBody, FileRoot, ForkThread, LineWindow,
        ReadEntryChunk, ReadFile, StatBlob, ThreadId, TranscriptEntry, limits,
    },
};

use super::server::Session;
use crate::{
    model::{DraftAttachment, LoadedEntry},
    update::ServerMsg,
};

impl Session<'_> {
    pub(super) async fn fork_thread(&self, params: ForkThread, load_prompt: bool) -> Result<()> {
        let forked = self.client.request(&params).await?;
        self.dispatch(ServerMsg::ThreadForked {
            forked: Box::new(forked),
            load_prompt,
        });
        Ok(())
    }

    pub(super) fn read_entry_chunk(&mut self, thread: ThreadId, key: String, source: EntrySource) {
        let client: Client = self.client.clone();
        self.spawn_msg(async move {
            let result = fetch_entry_chunks(&client, thread, &source).await;
            ServerMsg::EntryChunkLoaded {
                thread,
                key,
                source,
                result,
            }
        });
    }

    pub(super) fn read_file_preview(
        &mut self,
        request: u64,
        root: FileRoot,
        path: String,
        window: Option<LineWindow>,
    ) {
        let client: Client = self.client.clone();
        let content_client: ContentClient = self.content.clone();
        self.spawn_msg(async move {
            let result = async {
                let file = client
                    .request(&ReadFile {
                        root: root.clone(),
                        path: path.clone(),
                        window,
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                if matches!(file.body, FileBody::Image {}) {
                    let payload = content_client
                        .get_file(
                            root,
                            &path,
                            ContentVariant::Preview { max_px: 1600 },
                            limits::PREVIEW_IMAGE_BYTES,
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    Ok((file, Some(payload.bytes)))
                } else {
                    Ok((file, None))
                }
            }
            .await;
            ServerMsg::FilePreviewLoaded { request, result }
        });
    }

    pub(super) fn read_thread_attachment(
        &mut self,
        thread: ThreadId,
        id: String,
        preview_request: Option<u64>,
    ) {
        let client: Client = self.client.clone();
        let content_client: ContentClient = self.content.clone();
        self.spawn_msg(async move {
            let variant = if preview_request.is_some() {
                ContentVariant::Original
            } else {
                ContentVariant::Preview { max_px: 1024 }
            };
            let result = async {
                let payload = content_client
                    .get_attachment(thread, &id, variant, limits::ATTACHMENT_TOTAL_BYTES)
                    .await
                    .map_err(|error| error.to_string())?;
                let name = if preview_request.is_some() {
                    client
                        .request(&StatBlob {
                            thread,
                            id: id.clone(),
                        })
                        .await
                        .map_or_else(|_| String::new(), |info| info.name)
                } else {
                    String::new()
                };
                Ok(DraftAttachment {
                    name,
                    mime: payload.mime,
                    bytes: payload.bytes,
                })
            }
            .await;
            ServerMsg::ThreadAttachmentLoaded {
                thread,
                id,
                preview_request,
                result,
            }
        });
    }
}

async fn fetch_entry_chunks(
    client: &Client,
    thread: ThreadId,
    source: &EntrySource,
) -> std::result::Result<LoadedEntry, String> {
    let mut json = String::new();
    let mut offset = 0_u64;
    let mut revision = None;
    loop {
        let chunk = client
            .request(&ReadEntryChunk {
                thread,
                source: source.clone(),
                offset,
                revision: revision.clone(),
            })
            .await
            .map_err(|error| error.to_string())?;
        json.push_str(&chunk.data);
        revision = Some(chunk.revision);
        match chunk.next {
            Some(next) => offset = next,
            None => break,
        }
    }
    match source {
        EntrySource::Stored { .. } => serde_json::from_str::<EntryData>(&json)
            .map(|data| LoadedEntry::Stored(Box::new(data)))
            .map_err(|error| error.to_string()),
        EntrySource::Live { .. } => serde_json::from_str::<TranscriptEntry>(&json)
            .map(|entry| LoadedEntry::Live(Box::new(entry)))
            .map_err(|error| error.to_string()),
    }
}
