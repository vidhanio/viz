//! Authenticated RPC to explicitly configured environments.

use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context as _, ensure};
use jsonrpsee_types::{Id, Response, ResponsePayload, TwoPointZero};
use reqwest::{Client, header::HeaderValue, redirect::Policy};
use serde::Serialize;
use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use url::{Host, Url};
use viz_protocol::{ClientInfo, ClientKind, HelloParams, Method};

use crate::{
    config::Config,
    error::Error,
    wire::{Environment, EnvironmentStatus, PROTOCOL_VERSION, RPC_PATH},
};

const RECENCY: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const RPC_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_PEER_CONCURRENCY: usize = 8;
const MAX_RESPONSE_BYTES: usize = viz_protocol::limits::MAX_WS_FRAME;

pub(crate) struct Peer {
    pub(crate) url: Url,
    pub(crate) token: String,
    base_url: Url,
    destination: HeaderValue,
    last: Mutex<Option<(Instant, bool)>>,
    probing: AsyncMutex<()>,
    permits: Arc<Semaphore>,
}

impl std::fmt::Debug for Peer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Peer")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) struct Peers {
    pub(crate) local: String,
    entries: BTreeMap<String, Peer>,
    client: Client,
    sequence: AtomicU64,
}

impl Peers {
    pub(crate) fn new(config: &Config) -> anyhow::Result<Self> {
        let mut entries = BTreeMap::new();
        for (id, spec) in &config.environments {
            let url = &spec.url;
            let loopback = match url.host() {
                Some(Host::Domain(host)) => host == "localhost",
                Some(Host::Ipv4(ip)) => ip.is_loopback(),
                Some(Host::Ipv6(ip)) => ip.is_loopback(),
                None => false,
            };
            ensure!(
                id != &config.environment,
                "environment {id} is already local"
            );
            ensure!(
                url.has_host() && (url.scheme() == "https" || url.scheme() == "http" && loopback),
                "environment {id}: HTTPS is required except on loopback"
            );
            ensure!(
                url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "environment {id}: URL must not contain credentials, a query, or a fragment"
            );
            ensure!(
                !spec.token.trim().is_empty() && HeaderValue::from_str(&spec.token).is_ok(),
                "environment {id}: invalid bearer token"
            );
            let destination = HeaderValue::from_str(id)
                .with_context(|| format!("environment {id}: invalid identity"))?;
            let mut rpc_url = url.clone();
            if url.scheme() == "http" && matches!(url.host(), Some(Host::Domain("localhost"))) {
                rpc_url.set_host(Some("127.0.0.1"))?;
            }
            rpc_url.set_path(&format!("{}{RPC_PATH}", url.path().trim_end_matches('/')));
            entries.insert(
                id.clone(),
                Peer {
                    url: rpc_url,
                    token: spec.token.clone(),
                    base_url: url.clone(),
                    destination,
                    last: Mutex::new(None),
                    probing: AsyncMutex::new(()),
                    permits: Arc::new(Semaphore::new(MAX_PEER_CONCURRENCY)),
                },
            );
        }
        let client = Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .resolve("localhost", SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .timeout(RPC_TIMEOUT)
            .build()?;
        Ok(Self {
            local: config.environment.clone(),
            entries,
            client,
            sequence: AtomicU64::default(),
        })
    }

    /// Credentials for the authenticated WS gateway (same path, ws/wss instead
    /// of http/https).
    pub(crate) fn get(&self, id: &str) -> Option<&Peer> {
        self.entries.get(id)
    }

    /// Tasks use the last observation. Expiring the probe cache is not an
    /// observation that the environment went offline.
    pub(crate) fn reachable(&self, id: &str) -> bool {
        id == self.local
            || self
                .entries
                .get(id)
                .and_then(Peer::observation)
                .is_some_and(|(_, reachable)| reachable)
    }

    /// Count an authenticated WS handshake as a reachability observation.
    pub(crate) fn mark(&self, id: &str, reachable: bool) {
        if let Some(peer) = self.entries.get(id) {
            peer.mark(reachable);
        }
    }

    pub(crate) async fn list(&self) -> Vec<Environment> {
        let mut list = Vec::with_capacity(self.entries.len() + 1);
        list.push(Environment {
            id: self.local.clone(),
            url: None,
            status: EnvironmentStatus::Local,
        });
        list.extend(
            futures_util::future::join_all(self.entries.iter().map(|(id, peer)| async {
                Environment {
                    id: id.clone(),
                    url: Some(peer.base_url.as_str().to_owned()),
                    status: if self.probe(id, peer).await {
                        EnvironmentStatus::Reachable
                    } else {
                        EnvironmentStatus::Unreachable
                    },
                }
            }))
            .await,
        );
        list
    }

    pub(crate) async fn call<M: Method>(
        &self,
        id: &str,
        params: &(impl Serialize + Sync),
        extra_wait: Duration,
    ) -> Result<M::Response, Error> {
        let peer = self
            .get(id)
            .ok_or_else(|| Error::unavailable(format!("unknown environment {id}")))?;
        let response = self
            .request::<M>(peer, params, RPC_TIMEOUT + extra_wait)
            .await;
        response.map_or_else(
            |()| {
                peer.mark(false);
                Err(Error::unavailable(format!(
                    "no valid response from environment {id}"
                )))
            },
            |reply| {
                peer.mark(true);
                reply
            },
        )
    }

    /// Pushes content-addressed raw blob bytes to peer `id` via
    /// `PUT /v2/blobs/{sha}` before sending a `task.receive` referencing `sha`.
    pub(crate) async fn push_blob(&self, id: &str, sha: &str, bytes: Vec<u8>) -> Result<(), Error> {
        let peer = self
            .get(id)
            .ok_or_else(|| Error::unavailable(format!("unknown environment {id}")))?;
        let _permit = peer
            .permits
            .acquire()
            .await
            .map_err(|_| Error::unavailable("peer concurrency closed"))?;
        let mut url = peer.url.clone();
        let base_path = peer.base_url.path().trim_end_matches('/');
        url.set_path(&format!("{base_path}/v2/blobs/{sha}"));
        let response = self
            .client
            .put(url)
            .bearer_auth(&peer.token)
            .header("x-viz-environment", &peer.destination)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|_| {
                peer.mark(false);
                Error::unavailable(format!("uploading blob to environment {id}"))
            })?;
        if response.status().is_success() {
            peer.mark(true);
            return Ok(());
        }
        let status = response.status();
        let message = response.text().await.unwrap_or_default();
        if status.is_client_error() {
            peer.mark(true);
            Err(Error::invalid(format!(
                "environment {id} rejected blob upload ({status}): {message}"
            )))
        } else {
            peer.mark(false);
            Err(Error::unavailable(format!(
                "environment {id} failed blob upload ({status}): {message}"
            )))
        }
    }

    async fn probe(&self, id: &str, peer: &Peer) -> bool {
        if let Some(reachable) = peer.recent() {
            return reachable;
        }
        let _probing = peer.probing.lock().await;
        if let Some(reachable) = peer.recent() {
            return reachable;
        }
        let params = HelloParams {
            protocol: PROTOCOL_VERSION,
            environment: None,
            client: ClientInfo {
                kind: ClientKind::Cli,
                name: "viz-peer".into(),
            },
            channel: viz_protocol::ConnectionChannel::Control,
        };
        let reachable = self
            .request::<HelloParams>(peer, &params, PROBE_TIMEOUT)
            .await
            .is_ok_and(|reply| {
                reply.is_ok_and(|hello| {
                    hello.environment == id && hello.protocol == PROTOCOL_VERSION
                })
            });
        peer.mark(reachable);
        reachable
    }

    async fn request<M: Method>(
        &self,
        peer: &Peer,
        params: &(impl Serialize + Sync),
        timeout: Duration,
    ) -> Result<Result<M::Response, Error>, ()> {
        #[derive(Serialize)]
        struct Request<'a, P> {
            jsonrpc: TwoPointZero,
            id: u64,
            method: &'a str,
            params: &'a P,
        }

        let _permit = peer.permits.acquire().await.map_err(|_| ())?;
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        let mut response = self
            .client
            .post(peer.url.clone())
            .bearer_auth(&peer.token)
            .header("x-viz-environment", &peer.destination)
            .json(&Request {
                jsonrpc: TwoPointZero,
                id,
                method: M::NAME,
                params,
            })
            .timeout(timeout)
            .send()
            .await
            .map_err(|_| ())?;
        if !response.status().is_success() {
            return Err(());
        }
        if response
            .content_length()
            .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
        {
            return Err(());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(());
            }
            bytes.extend_from_slice(&chunk);
        }
        let reply: Response<'_, M::Response> = serde_json::from_slice(&bytes).map_err(|_| ())?;
        if reply.jsonrpc.is_none() || reply.id != Id::Number(id) {
            return Err(());
        }
        Ok(match reply.payload {
            ResponsePayload::Success(value) => Ok(value.into_owned()),
            ResponsePayload::Error(error) => {
                Err(crate::wire::peer_error(error.code(), error.message()))
            }
        })
    }
}

impl Peer {
    fn recent(&self) -> Option<bool> {
        self.observation()
            .filter(|(when, _)| when.elapsed() < RECENCY)
            .map(|(_, reachable)| reachable)
    }

    fn observation(&self) -> Option<(Instant, bool)> {
        *self.last.lock().expect("peer reachability lock poisoned")
    }

    fn mark(&self, reachable: bool) {
        *self.last.lock().expect("peer reachability lock poisoned") =
            Some((Instant::now(), reachable));
    }
}
