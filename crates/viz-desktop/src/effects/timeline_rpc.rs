//! Timeline-specific asynchronous RPC helpers (`thread.fork`, content-plane
//! entry hydration, file previews, and attachment fetches) kept separate from
//! `requests.rs`.

use viz_client::{
    Client, ContentClient, Result,
    protocol::{
        ContentVariant, EntryData, EntryId, FileBody, FileRoot, ForkThread, LineWindow, ReadFile,
        ThreadId, limits,
    },
};

use super::server::Session;
use crate::update::ServerMsg;

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
        window: LineWindow,
    ) {
        let client: Client = self.client.clone();
        let content_client: ContentClient = self.content.clone();
        self.spawn_msg(async move {
            let result = async {
                let file = client
                    .request(&ReadFile {
                        root: root.clone(),
                        path: path.clone(),
                        window: Some(window),
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

    pub(super) fn open_local_file(&mut self, directory: std::path::PathBuf, path: String) {
        self.spawn_opt_msg(async move {
            let result = tokio::task::spawn_blocking(move || {
                super::platform::open_local_file(&directory, &path)
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
            result.err().map(|detail| ServerMsg::RequestFailed {
                title: "Couldn’t open file".into(),
                detail,
            })
        });
    }

    pub(super) fn download_file(&mut self, root: FileRoot, path: String) {
        let content = self.content.clone();
        self.spawn_opt_msg(async move {
            save_file(&content, root, &path)
                .await
                .err()
                .map(|detail| ServerMsg::RequestFailed {
                    title: "Couldn’t download file".into(),
                    detail,
                })
        });
    }

    pub(super) fn read_thread_attachment(
        &mut self,
        thread: ThreadId,
        id: String,
        preview_request: Option<u64>,
    ) {
        let content_client: ContentClient = self.content.clone();
        self.spawn_msg(async move {
            let variant = if preview_request.is_some() {
                ContentVariant::Original
            } else {
                ContentVariant::Preview { max_px: 1024 }
            };
            let result = content_client
                .get_attachment(thread, &id, variant, limits::ATTACHMENT_TOTAL_BYTES)
                .await
                .map(|payload| (payload.mime, payload.bytes))
                .map_err(|error| error.to_string());
            ServerMsg::ThreadAttachmentLoaded {
                thread,
                id,
                preview_request,
                result,
            }
        });
    }
}

async fn save_file(content: &ContentClient, root: FileRoot, path: &str) -> Result<(), String> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned();
    let destination = tokio::task::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_title("Save file")
            .set_file_name(name)
            .save_file()
    })
    .await
    .map_err(|error| error.to_string())?;
    let Some(destination) = destination else {
        return Ok(());
    };
    let directory = destination.parent().ok_or("Choose a file destination.")?;
    let (file, temporary_path) = tempfile::NamedTempFile::new_in(directory)
        .map_err(|error| error.to_string())?
        .into_parts();
    let mut file = tokio::fs::File::from_std(file);
    content
        .download_file(root, path, &mut file)
        .await
        .map_err(|error| error.to_string())?;
    drop(file);
    temporary_path
        .persist(&destination)
        .map_err(|error| error.error.to_string())?;
    Ok(())
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
