use std::{fmt::Write as _, path::Path};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt as _;
use viz_protocol::{RunId, ThreadId};

use super::{
    Workspace,
    sql::{self, exec, row},
};
use crate::{
    error::Error,
    wire::{Attachment, PromptAttachment, StoredAttachment},
    workspace::sql::Connection,
};

const TOTAL_LIMIT: usize = 20 * 1024 * 1024;

pub(super) struct Upload {
    reference: PromptAttachment,
    bytes: Vec<u8>,
}

pub(super) fn decode(attachments: Vec<Attachment>) -> Result<Vec<Upload>, Error> {
    let mut remaining = TOTAL_LIMIT;
    attachments
        .into_iter()
        .map(|Attachment { name, mime, data }| {
            if name.is_empty()
                || name == "."
                || name == ".."
                || name.len() > 255
                || name
                    .bytes()
                    .any(|ch| matches!(ch, b'/' | b'\\' | 0..=31 | 127))
            {
                return Err(Error::invalid("invalid attachment name"));
            }
            let decoded_len = data.len() / 4 * 3
                - data
                    .bytes()
                    .rev()
                    .take(2)
                    .take_while(|&ch| ch == b'=')
                    .count()
                    .min(data.len() / 4 * 3);
            if decoded_len > remaining {
                return Err(too_large());
            }
            let bytes = STANDARD
                .decode(data)
                .map_err(|_| Error::invalid("invalid attachment base64"))?;
            remaining = remaining.checked_sub(bytes.len()).ok_or_else(too_large)?;
            let mut digest = Sha256::new();
            for field in [name.as_bytes(), mime.as_bytes(), bytes.as_slice()] {
                digest.update(field.len().to_le_bytes());
                digest.update(field);
            }
            let id = digest
                .finalize()
                .iter()
                .fold(String::new(), |mut hex, byte| {
                    _ = write!(hex, "{byte:02x}");
                    hex
                });
            Ok(Upload {
                reference: PromptAttachment { id, name, mime },
                bytes,
            })
        })
        .collect()
}

fn too_large() -> Error {
    Error::invalid("attachments exceed 20 MiB total")
}

pub(super) async fn save(
    db: &mut Connection,
    thread: ThreadId,
    uploads: Vec<Upload>,
) -> Result<Vec<PromptAttachment>, Error> {
    let mut attachments = Vec::with_capacity(uploads.len());
    for Upload { reference, bytes } in uploads {
        exec(db, "INSERT INTO attachments (thread, id, name, mime, data) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT DO NOTHING",
            [thread.into(), reference.id.as_str().into(), reference.name.as_str().into(), reference.mime.as_str().into(), bytes.into()]).await?;
        attachments.push(reference);
    }
    Ok(attachments)
}

async fn owned(
    db: &mut Connection,
    thread: ThreadId,
    id: &str,
    budget: usize,
) -> Result<sql::Row, Error> {
    row(db, "SELECT name, mime, CASE WHEN length(data) <= ?3 THEN data END, length(data) FROM attachments WHERE thread = ?1 AND id = ?2",
        [thread.into(), id.into(), budget.into()]).await?.ok_or_else(|| {
        Error::invalid(format!("attachment {id} does not belong to thread {thread}"))
    })
}

/// References to attachments the thread already stores, to attach again.
pub(super) async fn stored(
    db: &mut Connection,
    thread: ThreadId,
    ids: &[String],
) -> Result<Vec<PromptAttachment>, Error> {
    let mut found = Vec::with_capacity(ids.len());
    for id in ids {
        let mut row = owned(db, thread, id, 0).await?;
        found.push(PromptAttachment {
            id: id.clone(),
            name: row.str(0)?,
            mime: row.str(1)?,
        });
    }
    Ok(found)
}

pub(super) async fn inputs(
    db: &mut Connection,
    thread: ThreadId,
    ids: &[String],
) -> Result<Vec<Attachment>, Error> {
    let mut remaining = TOTAL_LIMIT;
    let mut attachments = Vec::with_capacity(ids.len());
    for id in ids {
        let mut row = owned(db, thread, id, remaining).await?;
        let size = usize::try_from(row.u64(3)?).map_err(|_| too_large())?;
        remaining = remaining.checked_sub(size).ok_or_else(too_large)?;
        attachments.push(Attachment {
            name: row.str(0)?,
            mime: row.str(1)?,
            data: STANDARD.encode(row.bytes(2)?),
        });
    }
    Ok(attachments)
}

pub(super) async fn materialize(
    workspace: &Workspace,
    thread: ThreadId,
    run: RunId,
    refs: Vec<PromptAttachment>,
) -> Result<Vec<StoredAttachment>, Error> {
    if refs.is_empty() {
        return Ok(Vec::new());
    }
    let parent = workspace.attachment_dir(thread).join(run.to_string());
    let mut saved = Vec::with_capacity(refs.len());
    for PromptAttachment { id, name, mime } in refs {
        let directory = parent.join(&id);
        let path = directory.join(&name);
        if !saved.iter().any(|file: &StoredAttachment| file.id == id) {
            let data = {
                let mut db = workspace.db.lock().await;
                owned(&mut db, thread, &id, TOTAL_LIMIT).await?.bytes(2)?
            };
            write_file(&directory, &path, &data).await?;
        }
        saved.push(StoredAttachment {
            id,
            name,
            mime,
            path,
        });
    }
    Ok(saved)
}

async fn write_file(directory: &Path, path: &Path, data: &[u8]) -> std::io::Result<()> {
    tokio::fs::create_dir_all(directory).await?;
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .await?;
    file.write_all(data).await?;
    file.flush().await
}
