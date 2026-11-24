//! Resolve project images on the server and serve raw bytes over the content
//! channel with `ETag` validation.

use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use viz_protocol::{ProjectIcon, ProjectIconChange};

use super::{Workspace, blobs, store};
use crate::error::Error;

const LIMIT: usize = 1024 * 1024;
const TTL: Duration = Duration::from_secs(30);

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

#[derive(Debug, Default)]
pub(super) struct Icons {
    cache: tokio::sync::Mutex<HashMap<String, Cached>>,
}

#[derive(Debug, Clone)]
struct Cached {
    icon: ProjectIcon,
    at: Instant,
    image: Option<Image>,
}

#[derive(Debug, Clone)]
struct Image {
    etag: String,
    media_type: String,
    bytes: Vec<u8>,
}

impl Image {
    fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        let media_type = image_type(&bytes)?.to_owned();
        let etag = URL_SAFE_NO_PAD.encode(Sha256::digest(&bytes));
        Some(Self {
            etag,
            media_type,
            bytes,
        })
    }

    fn slice(self, known: Option<&str>, offset: u64, len: usize) -> ProjectIconChunk {
        if known == Some(self.etag.as_str()) {
            return ProjectIconChunk::NotModified { etag: self.etag };
        }
        let total_size = self.bytes.len() as u64;
        let start = usize::try_from(offset)
            .unwrap_or(self.bytes.len())
            .min(self.bytes.len());
        let end = start.saturating_add(len).min(self.bytes.len());
        ProjectIconChunk::Chunk {
            bytes: self.bytes[start..end].to_vec(),
            total_size,
            mime: self.media_type,
            etag: self.etag,
        }
    }
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
        let (_, _, prefs) = self
            .db
            .read(move |conn| store::preferences(conn, &project_key))
            .await?;
        let icon = prefs.settings.icon;
        let image = match &icon {
            ProjectIcon::Uploaded { media_type, blob } => {
                if blobs::is_valid_blob_id(blob) {
                    let bytes = self.blobs.read(blob, LIMIT as u64).await?;
                    if bytes.len() <= LIMIT && image_type(&bytes) == Some(media_type.as_str()) {
                        Image::from_bytes(bytes)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            ProjectIcon::Lucide { .. } | ProjectIcon::Emoji { .. } => None,
            ProjectIcon::Automatic | ProjectIcon::OwnerAvatar => {
                let cached = self.icons.cache.lock().await.get(project).cloned();
                if let Some(cached) =
                    cached.filter(|cached| cached.icon == icon && cached.at.elapsed() < TTL)
                {
                    cached.image
                } else {
                    let root = Path::new(project);
                    let image = if icon == ProjectIcon::Automatic {
                        self.local_icon(root).await
                    } else {
                        self.avatar(root).await?
                    };
                    let mut cache = self.icons.cache.lock().await;
                    cache.retain(|_, entry| entry.at.elapsed() < TTL);
                    if cache.len() >= 32
                        && let Some(oldest) = cache
                            .iter()
                            .min_by_key(|(_, entry)| entry.at)
                            .map(|(key, _)| key.clone())
                    {
                        cache.remove(&oldest);
                    }
                    cache.insert(
                        project.to_owned(),
                        Cached {
                            icon,
                            at: Instant::now(),
                            image: image.clone(),
                        },
                    );
                    drop(cache);
                    image
                }
            }
        };
        Ok(image.map_or(ProjectIconChunk::Empty, |image| {
            image.slice(known_etag, offset, len)
        }))
    }

    async fn local_icon(&self, root: &Path) -> Option<Image> {
        for candidate in viz_git::icon_candidate_paths() {
            if let Ok(Some(bytes)) =
                viz_git::read_icon(root, Path::new(candidate), &self.closing).await
                && let Some(image) = Image::from_bytes(bytes)
            {
                return Some(image);
            }
        }
        None
    }

    async fn avatar(&self, root: &Path) -> Result<Option<Image>, Error> {
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
        download_avatar(&remote.host, &image).await
    }
}

async fn download_avatar(host: &str, image: &str) -> Result<Option<Image>, Error> {
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
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| Error::unavailable("cannot initialize avatar download"))?;
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
    Ok(Image::from_bytes(bytes))
}
