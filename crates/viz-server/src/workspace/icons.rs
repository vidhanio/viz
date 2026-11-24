//! Resolve project images into [`super::blobs::BlobStore`] and serve raw
//! ranges over the content channel with `ETag = blob SHA-256`.
//!
//! Uploaded icons already live in `BlobStore`, automatic icons persist once on
//! resolve (invalidated on project icon updates), and GitHub owner avatars
//! persist across restarts with a 24-hour refresh window over one shared HTTP
//! client.

use std::{path::Path, time::Duration};

use viz_core::projects::Preferences;
use viz_protocol::{ProjectIcon, ProjectIconChange};

use super::{
    Workspace, blobs,
    sql::{exec, row},
    store,
};
use crate::error::Error;

const LIMIT: usize = 1024 * 1024;
const AVATAR_REFRESH_MS: i64 = 24 * 60 * 60 * 1000;
const MODE_AUTOMATIC: &str = "automatic";
const MODE_OWNER_AVATAR: &str = "owner_avatar";

/// Result of reading one chunk of a project's icon on the content channel.
#[derive(Debug)]
pub(crate) enum ProjectIconChunk {
    Chunk {
        bytes: Vec<u8>,
        total_size: u64,
        mime: String,
        etag: String,
    },
    NotModified {
        etag: String,
    },
    Empty,
}

/// Shared HTTP client used for GitHub owner avatar downloads.
#[derive(Debug)]
pub(super) struct Icons {
    http: reqwest::Client,
}

impl Default for Icons {
    fn default() -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { http }
    }
}

#[derive(Debug)]
struct StoredIconState {
    icon: ProjectIcon,
    blob: Option<String>,
    mime: Option<String>,
    fetched_at: Option<i64>,
    mode: Option<String>,
}

fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"\0\0\x01\0") {
        Some("image/x-icon")
    } else if bytes.starts_with(b"icns") {
        Some("image/icns")
    } else if std::str::from_utf8(bytes).is_ok_and(|text| {
        let text = text.trim_start_matches('\u{feff}').trim_start();
        text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg"))
    }) {
        Some("image/svg+xml")
    } else {
        None
    }
}

impl Workspace {
    /// Verifies that an uploaded project icon blob was staged by the caller
    /// (or already belongs to `project`), fits under 1 MiB, and matches the
    /// declared image media type.
    pub(super) async fn validate_uploaded_icon(
        &self,
        principal_key: &str,
        project: &str,
        change: Option<&ProjectIconChange>,
    ) -> Result<(), Error> {
        let Some(ProjectIconChange::Uploaded { media_type, blob }) = change else {
            return Ok(());
        };
        if !blobs::is_valid_blob_id(blob) {
            return Err(Error::invalid("invalid project icon blob identifier"));
        }
        let authorized = if self.leases.has_client_stage(principal_key, blob) {
            true
        } else {
            let project = project.to_owned();
            let (_, _, prefs) = self
                .db
                .read(move |conn| store::preferences(conn, &project))
                .await?;
            matches!(&prefs.settings.icon, ProjectIcon::Uploaded { blob: existing, .. } if existing == blob)
        };
        if !authorized {
            return Err(Error::invalid(
                "project icon blob must be staged on the content channel first",
            ));
        }
        let bytes = self.blobs.read(blob, LIMIT as u64).await?;
        if bytes.len() > LIMIT || image_type(&bytes) != Some(media_type.as_str()) {
            return Err(Error::invalid("invalid project icon image or media type"));
        }
        Ok(())
    }

    pub(crate) async fn read_project_icon_chunk(
        &self,
        project: &str,
        known_etag: Option<&str>,
        offset: u64,
        len: usize,
    ) -> Result<ProjectIconChunk, Error> {
        let project_key = project.to_owned();
        let state = self
            .db
            .read(move |conn| {
                let mut record = row(
                    conn,
                    "SELECT settings, icon_blob, icon_mime, icon_fetched_at, icon_mode
                     FROM projects WHERE path = ?1",
                    [project_key.as_str().into()],
                )?
                .ok_or_else(|| Error::NoProject(project_key.clone()))?;
                let prefs: Preferences = store::from_json(&record.str(0)?)?;
                Ok(StoredIconState {
                    icon: prefs.settings.icon,
                    blob: record.opt_str(1)?,
                    mime: record.opt_str(2)?,
                    fetched_at: record.opt_i64(3)?,
                    mode: record.opt_str(4)?,
                })
            })
            .await?;

        match &state.icon {
            ProjectIcon::Lucide { .. } | ProjectIcon::Emoji { .. } => Ok(ProjectIconChunk::Empty),
            ProjectIcon::Uploaded { media_type, blob } => {
                if !blobs::is_valid_blob_id(blob) {
                    return Ok(ProjectIconChunk::Empty);
                }
                self.serve_icon_blob(blob, media_type, known_etag, offset, len)
                    .await
            }
            ProjectIcon::Automatic => {
                if state.mode.as_deref() == Some(MODE_AUTOMATIC) {
                    if let (Some(blob), Some(mime)) = (&state.blob, &state.mime) {
                        if let Some(chunk) = self
                            .try_serve_icon_blob(blob, mime, known_etag, offset, len)
                            .await?
                        {
                            return Ok(chunk);
                        }
                    } else {
                        return Ok(ProjectIconChunk::Empty);
                    }
                }
                let resolved = self.local_icon(Path::new(project)).await;
                self.persist_and_serve_icon(
                    project,
                    MODE_AUTOMATIC,
                    resolved,
                    known_etag,
                    offset,
                    len,
                )
                .await
            }
            ProjectIcon::OwnerAvatar => {
                let fresh = state.mode.as_deref() == Some(MODE_OWNER_AVATAR)
                    && state
                        .fetched_at
                        .is_some_and(|at| store::now().saturating_sub(at) < AVATAR_REFRESH_MS);
                if fresh {
                    if let (Some(blob), Some(mime)) = (&state.blob, &state.mime) {
                        if let Some(chunk) = self
                            .try_serve_icon_blob(blob, mime, known_etag, offset, len)
                            .await?
                        {
                            return Ok(chunk);
                        }
                    } else {
                        return Ok(ProjectIconChunk::Empty);
                    }
                }
                let resolved = self.avatar(Path::new(project)).await?;
                self.persist_and_serve_icon(
                    project,
                    MODE_OWNER_AVATAR,
                    resolved,
                    known_etag,
                    offset,
                    len,
                )
                .await
            }
        }
    }

    async fn serve_icon_blob(
        &self,
        blob: &str,
        mime: &str,
        known_etag: Option<&str>,
        offset: u64,
        len: usize,
    ) -> Result<ProjectIconChunk, Error> {
        if known_etag == Some(blob) {
            return Ok(ProjectIconChunk::NotModified {
                etag: blob.to_owned(),
            });
        }
        let (bytes, total_size) = self.blobs.read_range(blob, offset, len).await?;
        Ok(ProjectIconChunk::Chunk {
            bytes,
            total_size,
            mime: mime.to_owned(),
            etag: blob.to_owned(),
        })
    }

    async fn try_serve_icon_blob(
        &self,
        blob: &str,
        mime: &str,
        known_etag: Option<&str>,
        offset: u64,
        len: usize,
    ) -> Result<Option<ProjectIconChunk>, Error> {
        if known_etag == Some(blob) {
            return Ok(Some(ProjectIconChunk::NotModified {
                etag: blob.to_owned(),
            }));
        }
        match self.blobs.read_range(blob, offset, len).await {
            Ok((bytes, total_size)) => Ok(Some(ProjectIconChunk::Chunk {
                bytes,
                total_size,
                mime: mime.to_owned(),
                etag: blob.to_owned(),
            })),
            Err(error) if error.kind() == crate::error::Kind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn persist_and_serve_icon(
        &self,
        project: &str,
        mode: &'static str,
        resolved: Option<(String, Vec<u8>)>,
        known_etag: Option<&str>,
        offset: u64,
        len: usize,
    ) -> Result<ProjectIconChunk, Error> {
        let now = store::now();
        let project_key = project.to_owned();
        let Some((mime, bytes)) = resolved else {
            self.db
                .write(move |conn| {
                    exec(
                        conn,
                        "UPDATE projects
                         SET icon_blob = NULL, icon_mime = NULL, icon_fetched_at = ?2, icon_mode = ?3
                         WHERE path = ?1",
                        [project_key.as_str().into(), now.into(), mode.into()],
                    )?;
                    Ok(())
                })
                .await?;
            return Ok(ProjectIconChunk::Empty);
        };
        let (blob_id, size, permit) = self.blobs.put(bytes).await?;
        let blob_for_tx = blob_id.clone();
        let mime_for_tx = mime.clone();
        self.db
            .write_tx(move |tx| {
                exec(
                    tx,
                    "INSERT INTO blobs (id, size, mime, created_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT DO NOTHING",
                    [
                        blob_for_tx.as_str().into(),
                        size.into(),
                        mime_for_tx.as_str().into(),
                        now.into(),
                    ],
                )?;
                exec(
                    tx,
                    "UPDATE projects
                     SET icon_blob = ?2, icon_mime = ?3, icon_fetched_at = ?4, icon_mode = ?5
                     WHERE path = ?1",
                    [
                        project_key.as_str().into(),
                        blob_for_tx.as_str().into(),
                        mime_for_tx.as_str().into(),
                        now.into(),
                        mode.into(),
                    ],
                )?;
                Ok(())
            })
            .await?;
        drop(permit);
        self.serve_icon_blob(&blob_id, &mime, known_etag, offset, len)
            .await
    }

    async fn local_icon(&self, root: &Path) -> Option<(String, Vec<u8>)> {
        for candidate in viz_git::icon_candidate_paths() {
            if let Ok(Some(bytes)) =
                viz_git::read_icon(root, Path::new(candidate), &self.closing).await
                && bytes.len() <= LIMIT
                && let Some(mime) = image_type(&bytes)
            {
                return Some((mime.to_owned(), bytes));
            }
        }
        None
    }

    async fn avatar(&self, root: &Path) -> Result<Option<(String, Vec<u8>)>, Error> {
        let Ok(repo) = viz_git::Repository::discover(root, &self.closing).await else {
            return Ok(None);
        };
        let remote = repo
            .remotes(&self.closing)
            .await?
            .into_iter()
            .filter_map(|remote| remote.repository.map(|repo| (remote.name, repo)))
            .min_by_key(|(name, _)| (name != "origin", name.clone()))
            .map(|(_, repo)| repo);
        let Some(remote) = remote else {
            return Ok(None);
        };
        let github = self.github(&remote.host).await?;
        let image = github.owner_image(&remote).await?;
        download_avatar(&self.icons.http, &remote.host, &image).await
    }
}

async fn download_avatar(
    client: &reqwest::Client,
    host: &str,
    image: &str,
) -> Result<Option<(String, Vec<u8>)>, Error> {
    let url = url::Url::parse(image).map_err(|_| Error::invalid("invalid avatar URL"))?;
    let allowed = url.host_str().is_some_and(|avatar| {
        avatar.eq_ignore_ascii_case(host)
            || (host == "github.com" && avatar.ends_with(".githubusercontent.com"))
    });
    if !allowed
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
    {
        return Err(Error::invalid("avatar URL is outside the GitHub host"));
    }
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| Error::unavailable("cannot download project avatar"))?;
    if !response.status().is_success() {
        return Err(Error::unavailable("unexpected project avatar response"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::unavailable("cannot read project avatar"))?
    {
        if bytes.len() + chunk.len() > LIMIT {
            return Err(Error::invalid("project avatar exceeds 1 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(image_type(&bytes).map(|mime| (mime.to_owned(), bytes)))
}
