//! Timeline-specific asynchronous RPC helpers (`thread.fork`,
//! `thread.entry_chunk`) kept separate from `requests.rs`.

use rune::AppHandle;
use viz_client::{
    Client, Result,
    protocol::{
        EntryData, EntrySource, ForkThread, ReadAttachment, ReadEntryChunk, ReadFile, ThreadId,
        TranscriptEntry,
    },
};

use super::server::Session;
use crate::{
    app::Desktop,
    model::LoadedEntry,
    update::{Msg, ServerMsg},
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

    pub(super) fn read_entry_chunk(&self, thread: ThreadId, key: String, source: EntrySource) {
        let ui: AppHandle<Desktop> = self.ui.clone();
        let client: Client = self.client.clone();
        tokio::spawn(async move {
            let result = fetch_entry_chunks(&client, thread, &source).await;
            ui.update(move |s, tx| {
                s.dispatch(
                    Msg::Server(ServerMsg::EntryChunkLoaded {
                        thread,
                        key,
                        source,
                        result,
                    }),
                    tx,
                );
            });
        });
    }

    pub(super) fn read_file_preview(
        &self,
        request: u64,
        thread: Option<ThreadId>,
        project: Option<String>,
        path: String,
    ) {
        let ui: AppHandle<Desktop> = self.ui.clone();
        let client: Client = self.client.clone();
        tokio::spawn(async move {
            let result = client
                .request(&ReadFile {
                    thread,
                    project,
                    path,
                })
                .await
                .map_err(|error| error.to_string());
            ui.update(move |s, tx| {
                s.dispatch(
                    Msg::Server(ServerMsg::FilePreviewLoaded { request, result }),
                    tx,
                );
            });
        });
    }

    pub(super) fn read_thread_attachment(
        &self,
        thread: ThreadId,
        id: String,
        preview_request: Option<u64>,
    ) {
        let ui: AppHandle<Desktop> = self.ui.clone();
        let client: Client = self.client.clone();
        tokio::spawn(async move {
            let result = client
                .request(&ReadAttachment {
                    thread,
                    id: id.clone(),
                })
                .await
                .map_err(|error| error.to_string());
            ui.update(move |s, tx| {
                s.dispatch(
                    Msg::Server(ServerMsg::ThreadAttachmentLoaded {
                        thread,
                        id,
                        preview_request,
                        result,
                    }),
                    tx,
                );
            });
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
