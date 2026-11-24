//! Timeline-specific asynchronous RPC helpers (`thread.fork`, content-plane
//! entry hydration, file previews, and attachment fetches) kept separate from
//! `requests.rs`.

use viz_client::{
    Client, ContentClient, Result,
    protocol::{
        ContentVariant, EntryData, EntryId, FileBody, FileRoot, ForkThread, LineWindow, ReadFile,
        StatBlob, ThreadId, limits,
    },
};

use super::server::Session;
use crate::{model::DraftAttachment, update::ServerMsg};

impl Session<'_> {
    pub(super) async fn fork_thread(&self, params: ForkThread, load_prompt: bool) -> Result<()> {
        let forked = self.client.request(&params).await?;
        self.dispatch(ServerMsg::ThreadForked {
            forked: Box::new(forked),
            load_prompt,
        });
        Ok(())
    }

    pub(super) fn read_entry(&mut self, thread: ThreadId, key: String, entry: EntryId) {
        let content: ContentClient = self.content.clone();
        self.spawn_msg(async move {
            let result = fetch_entry(&content, thread, entry).await;
            ServerMsg::EntryLoaded {
                thread,
                key,
                entry,
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

async fn fetch_entry(
    content: &ContentClient,
    thread: ThreadId,
    entry: EntryId,
) -> std::result::Result<Box<EntryData>, String> {
    let bytes = content
        .get_entry(thread, entry, limits::ATTACHMENT_TOTAL_BYTES)
        .await
        .map_err(|error| error.to_string())?;
    tokio::task::spawn_blocking(move || {
        serde_json::from_slice::<EntryData>(&bytes)
            .map(Box::new)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}
