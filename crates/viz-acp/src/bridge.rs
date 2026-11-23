//! Relays MCP stdio messages from a harness to the caller's HTTP endpoint.
//!
//! Stdin and stdout carry newline-delimited JSON-RPC. Each HTTP request uses
//! the endpoint's bearer token and the negotiated MCP version.

use agent_client_protocol::{ErrorCode, RawJsonRpcMessage, RawJsonRpcResponse};
use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, CONTENT_TYPE},
    redirect::Policy,
};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Stdout};

pub(super) const URL_ENV: &str = "VIZ_MCP_URL";
pub(super) const TOKEN_ENV: &str = "VIZ_MCP_TOKEN";
const VERSION_HEADER: &str = "MCP-Protocol-Version";
use crate::{Error, error::Result};

/// Relays stdin to the endpoint in `VIZ_MCP_URL` until stdin closes.
///
/// # Errors
///
/// When the environment is incomplete or stdio fails; a failed relay is
/// answered to the agent instead.
pub async fn run() -> Result<()> {
    let url = std::env::var(URL_ENV).map_err(|_| Error::Environment(URL_ENV.to_owned()))?;
    let token = std::env::var(TOKEN_ENV).map_err(|_| Error::Environment(TOKEN_ENV.to_owned()))?;
    let client = Client::builder().redirect(Policy::none()).build()?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    let mut version: Option<String> = None;
    while let Some(line) = lines.next_line().await.map_err(|source| Error::Io {
        operation: "reading MCP stdin",
        source,
    })? {
        if line.trim().is_empty() {
            continue;
        }
        let message: RawJsonRpcMessage = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                eprintln!("mcp-bridge: ignoring a malformed message: {error}");
                continue;
            }
        };
        let request = match &message {
            RawJsonRpcMessage::Request(request) => Some(request),
            _ => None,
        };
        match relay(&client, &url, &token, version.as_deref(), &message).await {
            Ok(replies) => {
                for reply in replies {
                    if let Some(request) = request
                        && request.method.as_ref() == "initialize"
                        && let RawJsonRpcMessage::Response(RawJsonRpcResponse::Result {
                            id,
                            result,
                        }) = &reply
                        && id == &request.id
                        && let Ok(negotiated) = serde_json::from_value::<Negotiated>(result.clone())
                    {
                        version = Some(negotiated.protocol_version);
                    }
                    send(&mut stdout, &reply).await?;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "MCP bridge relay failed");
                if let Some(request) = request {
                    let reply = RawJsonRpcMessage::response(
                        request.id.clone(),
                        Err(agent_client_protocol::Error::new(
                            ErrorCode::INTERNAL_ERROR,
                            "MCP bridge relay failed",
                        )),
                    );
                    send(&mut stdout, &reply).await?;
                }
            }
        }
    }
    Ok(())
}

async fn relay(
    client: &Client,
    url: &str,
    token: &str,
    version: Option<&str>,
    message: &RawJsonRpcMessage,
) -> Result<Vec<RawJsonRpcMessage>> {
    let mut request = client
        .post(url)
        .bearer_auth(token)
        .header(ACCEPT, "application/json, text/event-stream")
        .json(message);
    if let Some(version) = version {
        request = request.header(VERSION_HEADER, version);
    }
    let response = request.send().await?.error_for_status()?;
    if response.status() == StatusCode::ACCEPTED {
        return Ok(Vec::new());
    }
    let events = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    let body = response.text().await?;
    if events {
        return sse_messages(&body);
    }
    if body.trim().is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str(&body)? {
        Messages::Batch(messages) => Ok(messages),
        Messages::Single(message) => Ok(vec![message]),
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Messages {
    Batch(Vec<RawJsonRpcMessage>),
    Single(RawJsonRpcMessage),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Negotiated {
    protocol_version: String,
}

/// The JSON-RPC messages in a finished Server-Sent Events body: each event's
/// `data` lines, joined.
fn sse_messages(body: &str) -> Result<Vec<RawJsonRpcMessage>> {
    let mut messages = Vec::new();
    let mut data = String::new();
    for line in body.lines().chain([""]) {
        if line.is_empty() {
            if !data.is_empty() {
                messages.push(serde_json::from_str(&data)?);
                data.clear();
            }
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    Ok(messages)
}

async fn send(stdout: &mut Stdout, message: &RawJsonRpcMessage) -> Result<()> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    stdout
        .write_all(line.as_bytes())
        .await
        .map_err(|source| Error::Io {
            operation: "writing MCP stdout",
            source,
        })?;
    stdout.flush().await.map_err(|source| Error::Io {
        operation: "flushing MCP stdout",
        source,
    })
}
