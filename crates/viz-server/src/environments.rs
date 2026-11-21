//! Authenticated RPC to explicitly configured environments.

use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context as _, ensure};
use jsonrpsee_types::{Id, Response, ResponsePayload, TwoPointZero};
use reqwest::{Client, header::HeaderValue, redirect::Policy};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;
use url::{Host, Url};

use crate::{
    config::Config,
    wire::{Environment, EnvironmentStatus, Hello, PROTOCOL_VERSION, RPC_PATH, RpcError},
};

const RECENCY: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const RPC_TIMEOUT: Duration = Duration::from_secs(8);
const DEFAULT_WAIT_MS: u64 = 30_000;
const MAX_WAIT_MS: u64 = 120_000;

pub(crate) struct Peer {
    pub(crate) url: Url,
    pub(crate) token: String,
    base_url: Url,
    destination: HeaderValue,
    last: Mutex<Option<(Instant, bool)>>,
    probing: AsyncMutex<()>,
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
    source: HeaderValue,
    sequence: AtomicU64,
}

impl Peers {
    pub(crate) fn new(config: &Config) -> anyhow::Result<Self> {
        let source = HeaderValue::from_str(&config.environment)
            .context("invalid local environment identity")?;
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
            source,
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

    pub(crate) async fn call(
        &self,
        id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        let peer = self
            .get(id)
            .ok_or_else(|| RpcError::unavailable(format!("unknown environment {id}")))?;
        let timeout = if method == "thread.wait"
            || (method == "task.status" && params["wait"].as_bool() == Some(true))
        {
            let wait = params["timeoutMs"]
                .as_u64()
                .unwrap_or(DEFAULT_WAIT_MS)
                .min(MAX_WAIT_MS);
            RPC_TIMEOUT + Duration::from_millis(wait)
        } else {
            RPC_TIMEOUT
        };
        let response = self.request(peer, method, &params, timeout).await;
        response.map_or_else(
            |()| {
                peer.mark(false);
                Err(RpcError::unavailable(format!(
                    "no valid response from environment {id}"
                )))
            },
            |reply| {
                peer.mark(true);
                reply
            },
        )
    }

    async fn probe(&self, id: &str, peer: &Peer) -> bool {
        if let Some(reachable) = peer.recent() {
            return reachable;
        }
        let _probing = peer.probing.lock().await;
        if let Some(reachable) = peer.recent() {
            return reachable;
        }
        let params =
            json!({"protocol": PROTOCOL_VERSION, "client": {"kind": "cli", "name": "viz-peer"}});
        let reachable = match self.request(peer, "hello", &params, PROBE_TIMEOUT).await {
            Ok(Ok(reply)) => serde_json::from_value::<Hello>(reply)
                .is_ok_and(|hello| hello.environment == id && hello.protocol == PROTOCOL_VERSION),
            _ => false,
        };
        peer.mark(reachable);
        reachable
    }

    async fn request(
        &self,
        peer: &Peer,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Result<Value, RpcError>, ()> {
        #[derive(Serialize)]
        struct Request<'a> {
            jsonrpc: TwoPointZero,
            id: u64,
            method: &'a str,
            params: &'a Value,
        }

        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        let response = self
            .client
            .post(peer.url.clone())
            .bearer_auth(&peer.token)
            .header("x-viz-environment", &peer.destination)
            .header("x-viz-source", &self.source)
            .json(&Request {
                jsonrpc: TwoPointZero,
                id,
                method,
                params,
            })
            .timeout(timeout)
            .send()
            .await
            .map_err(|_| ())?;
        if !response.status().is_success() {
            return Err(());
        }
        let bytes = response.bytes().await.map_err(|_| ())?;
        let reply: Response<'_, Value> = serde_json::from_slice(&bytes).map_err(|_| ())?;
        if reply.jsonrpc.is_none() || reply.id != Id::Number(id) {
            return Err(());
        }
        Ok(match reply.payload {
            ResponsePayload::Success(value) => Ok(value.into_owned()),
            ResponsePayload::Error(error) => {
                Err(RpcError::new(i64::from(error.code()), error.message()))
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
