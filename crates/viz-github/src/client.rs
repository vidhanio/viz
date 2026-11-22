//! Authentication, conditional requests and rate-limit admission.

use std::{
    collections::HashMap,
    fmt,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use http::{HeaderMap, StatusCode, header};
use octocrab::Octocrab;
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::{Mutex, OnceCell};
use url::Url;
use viz_config::Secret;

const CACHE_TTL: Duration = Duration::from_secs(15);
const CACHE_CAPACITY: usize = 64;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Local gh authentication state for the configured host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthState {
    Ready,
    GhMissing,
    LoggedOut,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("gh is not installed")]
    GhMissing,
    #[error("gh has no usable login for this host")]
    LoggedOut,
    #[error("could not run gh: {0}")]
    Gh(#[source] std::io::Error),
    #[error("invalid {0}")]
    Invalid(&'static str),
    #[error("GitHub transport failed")]
    Transport,
    #[error("GitHub request timed out")]
    Timeout,
    #[error("GitHub returned HTTP {0}")]
    Http(StatusCode),
    #[error("GitHub throttled requests for {retry_after:?}")]
    RateLimited { retry_after: Duration },
    #[error("GitHub response did not match the requested shape: {0}")]
    Response(#[from] serde_json::Error),
    #[error("GitHub could not resolve the GraphQL query")]
    GraphQl,
}

/// Poll state supplied by the caller, with any server backoff taking
/// precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollState {
    Active,
    Settled,
    Failed { consecutive: u32 },
    RateLimited { retry_after: Duration },
}

/// Compute the delay between requests without consulting the clock.
#[must_use]
pub const fn poll_delay(state: PollState) -> Duration {
    match state {
        PollState::Active => Duration::from_secs(20),
        PollState::Settled => Duration::from_secs(120),
        PollState::Failed { consecutive } => {
            Duration::from_secs(10 * (1 << if consecutive > 5 { 5 } else { consecutive }))
        }
        PollState::RateLimited { retry_after } => retry_after,
    }
}

#[derive(Clone)]
pub struct GitHub(Arc<Inner>);

struct Inner {
    host: String,
    auth: OnceCell<Authenticated>,
    requests: Mutex<Requests>,
}

struct Authenticated {
    _token: Secret,
    api: Octocrab,
}

#[derive(Default)]
struct Requests {
    cache: HashMap<String, Cached>,
    blocked_until: Option<Instant>,
}

#[derive(Clone)]
struct Cached {
    etag: Option<header::HeaderValue>,
    at: Instant,
    body: Arc<str>,
}

impl fmt::Debug for GitHub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHub")
            .field("host", &self.0.host)
            .finish_non_exhaustive()
    }
}

impl GitHub {
    /// Set the hostname used for both gh authentication and HTTPS requests.
    ///
    /// # Errors
    /// Returns an error if the hostname is invalid.
    pub fn new(host: &str) -> Result<Self, Error> {
        let url =
            Url::parse(&format!("https://{host}/")).map_err(|_| Error::Invalid("GitHub host"))?;
        let normalized = url.host_str().ok_or(Error::Invalid("GitHub host"))?;
        if !host.eq_ignore_ascii_case(normalized) || url.domain().is_none() {
            return Err(Error::Invalid("GitHub host"));
        }
        Ok(Self(Arc::new(Inner {
            host: normalized.to_owned(),
            auth: OnceCell::new(),
            requests: Mutex::default(),
        })))
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.0.host
    }

    /// Probe local gh credentials. Failed lookups are retried on the next call.
    ///
    /// # Errors
    /// Returns an error if gh could not be invoked or timed out.
    pub async fn auth_state(&self) -> Result<AuthState, Error> {
        match self.auth().await {
            Ok(_) => Ok(AuthState::Ready),
            Err(Error::GhMissing) => Ok(AuthState::GhMissing),
            Err(Error::LoggedOut) => Ok(AuthState::LoggedOut),
            Err(error) => Err(error),
        }
    }

    async fn auth(&self) -> Result<&Authenticated, Error> {
        self.0
            .auth
            .get_or_try_init(|| async {
                let mut gh = tokio::process::Command::new("gh");
                gh.args(["auth", "token", "--hostname", self.host()])
                    .env("GH_PROMPT_DISABLED", "1")
                    .env_remove("GH_DEBUG")
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                let output = tokio::time::timeout(REQUEST_TIMEOUT, gh.output())
                    .await
                    .map_err(|_| Error::Timeout)?
                    .map_err(|error| {
                        if error.kind() == std::io::ErrorKind::NotFound {
                            Error::GhMissing
                        } else {
                            Error::Gh(error)
                        }
                    })?;
                if !output.status.success() {
                    return Err(Error::LoggedOut);
                }
                let token = String::from_utf8(output.stdout).map_err(|_| Error::LoggedOut)?;
                let token = Secret::Inline(token.trim().to_owned());
                let base = if self.host() == "github.com" {
                    "https://api.github.com".to_owned()
                } else {
                    format!("https://{}", self.host())
                };
                let api = Octocrab::builder()
                    .base_uri(base)
                    .map_err(|_| Error::Transport)?
                    .personal_token(token.resolve().map_err(|_| Error::LoggedOut)?)
                    .build()
                    .map_err(|_| Error::Transport)?;
                Ok(Authenticated { _token: token, api })
            })
            .await
    }

    pub(super) fn rest(&self, path: &str) -> String {
        if self.host() == "github.com" {
            path.to_owned()
        } else {
            format!("/api/v3{path}")
        }
    }

    pub(super) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        let body = self.get_body(path, None).await?;
        Ok(serde_json::from_str(&body)?)
    }

    pub(super) async fn get_body(
        &self,
        path: &str,
        accept: Option<&'static str>,
    ) -> Result<Arc<str>, Error> {
        let route = self.rest(path);
        let key = format!("{route}\n{}", accept.unwrap_or_default());
        let cached = self.0.requests.lock().await.cache.get(&key).cloned();
        if let Some(entry) = &cached
            && entry.at.elapsed() < CACHE_TTL
        {
            return Ok(Arc::clone(&entry.body));
        }
        self.admit().await?;
        let api = &self.auth().await?.api;
        let mut headers = HeaderMap::new();
        if let Some(accept) = accept {
            headers.insert(header::ACCEPT, header::HeaderValue::from_static(accept));
        }
        if let Some(etag) = cached.as_ref().and_then(|entry| entry.etag.clone()) {
            headers.insert(header::IF_NONE_MATCH, etag);
        }
        let response = within(api._get_with_headers(route.as_str(), Some(headers))).await?;
        let status = response.status();
        let etag = response.headers().get(header::ETAG).cloned();
        let throttled = self.record_limit(status, response.headers()).await;
        let entry = if status == StatusCode::NOT_MODIFIED {
            Cached {
                at: Instant::now(),
                ..cached.ok_or(Error::Http(status))?
            }
        } else {
            require_success(status, throttled)?;
            let body = within(api.body_to_string(response)).await?;
            Cached {
                etag,
                at: Instant::now(),
                body: Arc::from(body),
            }
        };
        let body = Arc::clone(&entry.body);
        let mut requests = self.0.requests.lock().await;
        if requests.cache.len() >= CACHE_CAPACITY {
            requests
                .cache
                .retain(|_, item| item.at.elapsed() < CACHE_TTL);
            if requests.cache.len() >= CACHE_CAPACITY {
                requests.cache.clear();
            }
        }
        requests.cache.insert(key, entry);
        drop(requests);
        Ok(body)
    }

    pub(super) async fn graphql<P: Serialize + Sync, T: DeserializeOwned>(
        &self,
        data: &P,
    ) -> Result<T, Error> {
        let route = if self.host() == "github.com" {
            "/graphql"
        } else {
            "/api/graphql"
        };
        self.post(route, data).await
    }

    pub(super) async fn post<P: Serialize + Sync, T: DeserializeOwned>(
        &self,
        route: &str,
        data: &P,
    ) -> Result<T, Error> {
        self.admit().await?;
        let api = &self.auth().await?.api;
        let response = within(api._post(route, Some(data))).await?;
        let status = response.status();
        let throttled = self.record_limit(status, response.headers()).await;
        require_success(status, throttled)?;
        let body = within(api.body_to_string(response)).await?;
        Ok(serde_json::from_str(&body)?)
    }

    pub(super) async fn invalidate(&self) {
        self.0.requests.lock().await.cache.clear();
    }

    async fn admit(&self) -> Result<(), Error> {
        let blocked_until = self.0.requests.lock().await.blocked_until;
        if let Some(retry_after) =
            blocked_until.and_then(|until| until.checked_duration_since(Instant::now()))
        {
            return Err(Error::RateLimited { retry_after });
        }
        Ok(())
    }

    async fn record_limit(&self, status: StatusCode, headers: &HeaderMap) -> Option<Duration> {
        let retry_after = rate_delay(status, headers, SystemTime::now())?;
        if let Some(deadline) = Instant::now().checked_add(retry_after) {
            let mut requests = self.0.requests.lock().await;
            requests.blocked_until = Some(
                requests
                    .blocked_until
                    .map_or(deadline, |previous| previous.max(deadline)),
            );
        }
        Some(retry_after)
    }
}

fn rate_delay(status: StatusCode, headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    let header = |key| headers.get(key).and_then(|value| value.to_str().ok());
    if let Some(delay) = header("retry-after").and_then(|value| value.parse::<u64>().ok()) {
        return Some(Duration::from_secs(delay.max(1)));
    }
    if header("x-ratelimit-remaining") == Some("0") {
        let reset = header("x-ratelimit-reset")?.parse::<u64>().ok()?;
        let now = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
        return Some(Duration::from_secs(reset.saturating_sub(now).max(1)));
    }
    (status == StatusCode::TOO_MANY_REQUESTS).then_some(Duration::from_secs(60))
}

fn require_success(status: StatusCode, throttled: Option<Duration>) -> Result<(), Error> {
    if status.is_success() {
        return Ok(());
    }
    if let Some(retry_after) = throttled
        && matches!(
            status,
            StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
        )
    {
        return Err(Error::RateLimited { retry_after });
    }
    Err(Error::Http(status))
}

async fn within<T>(request: impl Future<Output = octocrab::Result<T>>) -> Result<T, Error> {
    tokio::time::timeout(REQUEST_TIMEOUT, request)
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Transport)
}
