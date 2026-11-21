//! `viz-server mcp-bridge`: a stdio MCP server that relays to viz's HTTP MCP
//! endpoint, for ACP agents that only take `command`-style MCP servers (every
//! ACP agent must support those). It reads newline-delimited JSON-RPC from
//! stdin, POSTs each message with the thread's token and the negotiated
//! protocol version, and writes the replies to stdout, which carries nothing
//! else.

use anyhow::{Context as _, Result};
use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, CONTENT_TYPE},
    redirect::Policy,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Stdout};

pub(super) const URL_ENV: &str = "VIZ_MCP_URL";
pub(super) const TOKEN_ENV: &str = "VIZ_MCP_TOKEN";
const VERSION_HEADER: &str = "MCP-Protocol-Version";
const INTERNAL_ERROR: i64 = -32603;

/// Relays stdin to the endpoint in `VIZ_MCP_URL` until stdin closes.
///
/// # Errors
///
/// When the environment is incomplete or stdio fails; a failed relay is
/// answered to the agent instead.
pub(crate) async fn run() -> Result<()> {
    let url = std::env::var(URL_ENV).with_context(|| format!("{URL_ENV} must be set"))?;
    let token = std::env::var(TOKEN_ENV).with_context(|| format!("{TOKEN_ENV} must be set"))?;
    let client = Client::builder()
        .redirect(Policy::none())
        .build()
        .context("building the HTTP client")?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    let mut version: Option<String> = None;
    while let Some(line) = lines.next_line().await.context("reading stdin")? {
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                eprintln!("mcp-bridge: ignoring a malformed message: {error}");
                continue;
            }
        };
        let request = message
            .get("method")
            .and_then(|_| message.get("id"))
            .cloned();
        let initialize = message.get("method").and_then(Value::as_str) == Some("initialize");
        match relay(&client, &url, &token, version.as_deref(), &message).await {
            Ok(replies) => {
                for reply in replies {
                    if initialize
                        && reply.get("id") == request.as_ref()
                        && let Some(negotiated) = reply
                            .pointer("/result/protocolVersion")
                            .and_then(Value::as_str)
                    {
                        version = Some(negotiated.to_owned());
                    }
                    send(&mut stdout, &reply).await?;
                }
            }
            Err(error) => {
                eprintln!("mcp-bridge: {error:#}");
                if let Some(id) = request {
                    let message = format!("{error:#}");
                    let reply = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": INTERNAL_ERROR, "message": message } });
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
    message: &Value,
) -> Result<Vec<Value>> {
    let mut request = client
        .post(url)
        .bearer_auth(token)
        .header(ACCEPT, "application/json, text/event-stream")
        .json(message);
    if let Some(version) = version {
        request = request.header(VERSION_HEADER, version);
    }
    let response = request
        .send()
        .await
        .context("sending to viz")?
        .error_for_status()
        .context("viz refused")?;
    if response.status() == StatusCode::ACCEPTED {
        return Ok(Vec::new());
    }
    let events = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    let body = response.text().await.context("reading viz's reply")?;
    if events {
        return sse_messages(&body);
    }
    if body.trim().is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str(&body).context("decoding viz's reply")? {
        Value::Array(messages) => Ok(messages),
        message => Ok(vec![message]),
    }
}

/// The JSON-RPC messages in a finished Server-Sent Events body: each event's
/// `data` lines, joined.
fn sse_messages(body: &str) -> Result<Vec<Value>> {
    let mut messages = Vec::new();
    let mut data = String::new();
    for line in body.lines().chain([""]) {
        if line.is_empty() {
            if !data.is_empty() {
                messages.push(serde_json::from_str(&data).context("decoding an event from viz")?);
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

async fn send(stdout: &mut Stdout, message: &Value) -> Result<()> {
    let mut line = message.to_string();
    line.push('\n');
    stdout
        .write_all(line.as_bytes())
        .await
        .context("writing stdout")?;
    stdout.flush().await.context("writing stdout")
}
