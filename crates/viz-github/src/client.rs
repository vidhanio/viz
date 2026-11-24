//! Authentication, conditional requests and rate-limit admission.

use std::{
    collections::HashMap,
    error::Error as StdError,
    fmt,
    process::Stdio,
    sync::{Arc, LazyLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use http::{HeaderMap, StatusCode, header};
use http_body_util::{BodyExt as _, Limited};
use octocrab::Octocrab;
use regex::Regex;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::Mutex;
use url::Url;
use viz_config::Secret;

const CACHE_TTL: Duration = Duration::from_secs(15);
const CACHE_CAPACITY: usize = 64;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

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
    #[error("GitHub transport failed: {0}")]
    Transport(#[source] TransportFailure),
    #[error("GitHub request timed out")]
    Timeout,
    #[error("GitHub returned HTTP {0}")]
    Http(StatusCode),
    #[error("GitHub throttled requests for {retry_after:?}")]
    RateLimited { retry_after: Duration },
    #[error("GitHub response did not match the requested shape (line {line}, column {column})")]
    Response { line: usize, column: usize },
    #[error("GitHub could not resolve the GraphQL query: {messages:?}")]
    GraphQl { messages: Vec<String> },
}

/// A bounded, sanitized description of the upstream transport cause.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct TransportFailure {
    message: String,
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Response {
            line: error.line(),
            column: error.column(),
        }
    }
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
    api_origin: String,
    enterprise_server: bool,
    auth: Mutex<Option<Arc<Authenticated>>>,
}

struct Authenticated {
    token: Secret,
    api: Octocrab,
    requests: Mutex<Requests>,
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
    /// Use the web hostname for GitHub.com, Enterprise Server, or GHE.com.
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
        let cloud = normalized == "github.com" || normalized.ends_with(".ghe.com");
        let api_origin = if cloud {
            format!("https://api.{normalized}")
        } else {
            format!("https://{normalized}")
        };
        Ok(Self(Arc::new(Inner {
            host: normalized.to_owned(),
            api_origin,
            enterprise_server: !cloud,
            auth: Mutex::default(),
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

    async fn auth(&self) -> Result<Arc<Authenticated>, Error> {
        let mut current = self.0.auth.lock().await;
        if let Some(auth) = current.as_ref() {
            return Ok(Arc::clone(auth));
        }
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
        let api = Octocrab::builder()
            .base_uri(self.0.api_origin.as_str())
            .map_err(|error| transport(&error, &token))?
            .personal_token(token.resolve().map_err(|_| Error::LoggedOut)?)
            .build()
            .map_err(|error| transport(&error, &token))?;
        let auth = Arc::new(Authenticated {
            token,
            api,
            requests: Mutex::default(),
        });
        *current = Some(Arc::clone(&auth));
        drop(current);
        Ok(auth)
    }

    async fn forget_unauthorized(&self, status: StatusCode, auth: &Arc<Authenticated>) {
        if status == StatusCode::UNAUTHORIZED {
            let mut current = self.0.auth.lock().await;
            if current
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, auth))
            {
                *current = None;
            }
        }
    }

    pub(super) fn rest(&self, path: &str) -> String {
        if self.0.enterprise_server {
            format!("/api/v3{path}")
        } else {
            path.to_owned()
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
        let auth = self.auth().await?;
        let cached = auth.requests.lock().await.cache.get(&key).cloned();
        if let Some(entry) = &cached
            && entry.at.elapsed() < CACHE_TTL
        {
            return Ok(Arc::clone(&entry.body));
        }
        auth.admit().await?;
        let api = &auth.api;
        let mut headers = HeaderMap::new();
        if let Some(accept) = accept {
            headers.insert(header::ACCEPT, header::HeaderValue::from_static(accept));
        }
        if let Some(etag) = cached.as_ref().and_then(|entry| entry.etag.clone()) {
            headers.insert(header::IF_NONE_MATCH, etag);
        }
        let response = within(
            api._get_with_headers(route.as_str(), Some(headers)),
            &auth.token,
        )
        .await?;
        let status = response.status();
        let etag = response.headers().get(header::ETAG).cloned();
        self.forget_unauthorized(status, &auth).await;
        let throttled = auth.record_limit(status, response.headers()).await;
        let entry = if status == StatusCode::NOT_MODIFIED {
            Cached {
                at: Instant::now(),
                ..cached.ok_or(Error::Http(status))?
            }
        } else {
            require_success(status, throttled)?;
            let body = read_bounded_body(response, &auth.token).await?;
            Cached {
                etag,
                at: Instant::now(),
                body: Arc::from(body),
            }
        };
        let body = Arc::clone(&entry.body);
        let mut requests = auth.requests.lock().await;
        if requests.cache.len() >= CACHE_CAPACITY
            && !requests.cache.contains_key(&key)
            && let Some(oldest) = requests
                .cache
                .iter()
                .min_by_key(|(_, item)| item.at)
                .map(|(key, _)| key.clone())
        {
            requests.cache.remove(&oldest);
        }
        requests.cache.insert(key, entry);
        drop(requests);
        Ok(body)
    }

    pub(super) async fn graphql<P: Serialize + Sync, T: DeserializeOwned>(
        &self,
        data: &P,
    ) -> Result<T, Error> {
        let route = if self.0.enterprise_server {
            "/api/graphql"
        } else {
            "/graphql"
        };
        let auth = self.auth().await?;
        let response: GraphResponse<T> = self.post_with(&auth, route, data).await?;
        if let Some(errors) = response.errors
            && !errors.is_empty()
        {
            if errors
                .iter()
                .any(|error| error.kind.as_deref() == Some("RATE_LIMITED"))
            {
                return Err(Error::RateLimited {
                    retry_after: Duration::from_secs(60),
                });
            }
            return Err(Error::GraphQl {
                messages: errors
                    .into_iter()
                    .take(10)
                    .map(|error| sanitized(&error.message, &auth.token))
                    .collect(),
            });
        }
        response
            .data
            .ok_or_else(|| missing_graphql("response has no data"))
    }

    pub(super) async fn post<P: Serialize + Sync, T: DeserializeOwned>(
        &self,
        route: &str,
        data: &P,
    ) -> Result<T, Error> {
        let auth = self.auth().await?;
        self.post_with(&auth, route, data).await
    }

    async fn post_with<P: Serialize + Sync, T: DeserializeOwned>(
        &self,
        auth: &Arc<Authenticated>,
        route: &str,
        data: &P,
    ) -> Result<T, Error> {
        auth.admit().await?;
        let api = &auth.api;
        let response = within(api._post(route, Some(data)), &auth.token).await?;
        let status = response.status();
        self.forget_unauthorized(status, auth).await;
        let throttled = auth.record_limit(status, response.headers()).await;
        require_success(status, throttled)?;
        let body = read_bounded_body(response, &auth.token).await?;
        Ok(serde_json::from_str(&body)?)
    }

    pub(super) async fn invalidate_pull_search(&self) {
        let auth = self.0.auth.lock().await.clone();
        if let Some(auth) = auth {
            let search = self.rest("/search/issues?");
            auth.requests
                .lock()
                .await
                .cache
                .retain(|key, _| !key.starts_with(&search));
        }
    }
}

impl Authenticated {
    async fn admit(&self) -> Result<(), Error> {
        let blocked_until = self.requests.lock().await.blocked_until;
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
            let mut requests = self.requests.lock().await;
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

#[derive(Deserialize)]
struct GraphResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphError>>,
}

#[derive(Deserialize)]
struct GraphError {
    #[serde(rename = "type")]
    kind: Option<String>,
    message: String,
}

pub fn missing_graphql(message: &'static str) -> Error {
    Error::GraphQl {
        messages: vec![message.to_owned()],
    }
}

async fn read_bounded_body<B>(response: http::Response<B>, token: &Secret) -> Result<String, Error>
where
    B: http_body_util::BodyExt,
    B::Error: StdError + Send + Sync + 'static,
{
    if response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|len| len > MAX_RESPONSE_BYTES)
    {
        return Err(Error::Transport(TransportFailure {
            message: format!("response body exceeded {MAX_RESPONSE_BYTES} bytes"),
        }));
    }
    let collected = tokio::time::timeout(
        REQUEST_TIMEOUT,
        Limited::new(response.into_body(), MAX_RESPONSE_BYTES).collect(),
    )
    .await
    .map_err(|_| Error::Timeout)?
    .map_err(|error| {
        let message = sanitized(&error.to_string(), token);
        Error::Transport(TransportFailure { message })
    })?;
    let bytes = collected.to_bytes();
    String::from_utf8(bytes.to_vec()).map_err(|_| {
        Error::Transport(TransportFailure {
            message: "response body is not UTF-8".to_owned(),
        })
    })
}

async fn within<T>(
    request: impl Future<Output = octocrab::Result<T>>,
    token: &Secret,
) -> Result<T, Error> {
    tokio::time::timeout(REQUEST_TIMEOUT, request)
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|error| transport(&error, token))
}

fn transport(error: &octocrab::Error, token: &Secret) -> Error {
    let message = match error {
        octocrab::Error::GitHub { .. } | octocrab::Error::Graphql { .. } => {
            "unexpected API error envelope".to_owned()
        }
        octocrab::Error::InvalidUtf8 { .. } => "response body is not UTF-8".to_owned(),
        _ => {
            let mut causes = Vec::new();
            let mut source = error.source();
            while let Some(cause) = source.filter(|_| causes.len() < 4) {
                causes.push(sanitized(&cause.to_string(), token));
                source = cause.source();
            }
            if causes.is_empty() {
                sanitized(&error.to_string(), token)
            } else {
                causes.join(": ")
            }
        }
    };
    Error::Transport(TransportFailure { message })
}

fn sanitized(message: &str, token: &Secret) -> String {
    static SENSITIVE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)header|authorization|cookie|bearer|basic |\b(?:[a-z][a-z0-9]*(?:-[a-z0-9]+)+|accept|host|etag|date|link|vary|location)["']?\s*[:=]"#)
            .expect("header diagnostic regex is valid")
    });
    static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:gh[pousr]_|github_pat_)[a-z0-9_]+")
            .expect("token diagnostic regex is valid")
    });
    static URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)[a-z][a-z0-9+.-]*://[^\s<>"']+"#).expect("URL diagnostic regex is valid")
    });
    let plain: String = message
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    if SENSITIVE.is_match(&plain) {
        return "diagnostic contains HTTP authentication or headers".to_owned();
    }
    let scrubbed = token.resolve().ok().map_or_else(
        || plain.clone(),
        |secret| plain.replace(&secret, "[redacted]"),
    );
    let scrubbed = TOKEN.replace_all(&scrubbed, "[redacted]");
    URL.replace_all(&scrubbed, "[URL]")
        .chars()
        .take(512)
        .collect()
}
