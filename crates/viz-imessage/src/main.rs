//! iMessage connector (scaffold): bridges iMessage through a BlueBubbles server
//! running on a Mac.
//!
//! ```toml
//! server_url = "https://bluebubbles.example.net"
//! password = { file = "/run/secrets/viz-bluebubbles" }
//! allowed_handles = ["+15555550123", "me@example.com"]
//! ```
//!
//! The BlueBubbles bridge itself is not written yet (see `TODO.md`); this
//! binary validates its config and the backend connection so the service wiring
//! can be exercised.

use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;
use serde::Deserialize;
use viz_client::{
    Client, Endpoint,
    protocol::{ClientInfo, ClientKind},
};
use viz_config::{ClientFile, Connection, Secret};

/// The connector's name: its config filename, `hello` name and origins.
const NAME: &str = "imessage";

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Use this connector config file instead of the environment or default
    /// location.
    #[arg(long)]
    config: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImessageConfig {
    /// The environment to connect to for the configuration check.
    environment: Option<String>,
    #[serde(default)]
    backend: Connection,
    /// BlueBubbles server base URL.
    server_url: Option<String>,
    /// BlueBubbles server password.
    password: Option<Secret>,
    /// Phone numbers / Apple IDs whose messages are forwarded.
    allowed_handles: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let file = args
        .config
        .map_or_else(|| ClientFile::connector(NAME), ClientFile::path);
    let imessage: ImessageConfig = viz_config::load(&file)?;
    let server_url = imessage
        .server_url
        .context("imessage: missing server_url")?;
    // Fail early on a password that won't resolve; the bridge will use it.
    imessage
        .password
        .context("imessage: missing password")?
        .resolve()?;
    anyhow::ensure!(
        !imessage.allowed_handles.is_empty(),
        "imessage: allowed_handles is empty: nobody could talk to viz"
    );
    let endpoint = Endpoint::from_connection(&imessage.backend)?;
    let hello = Client::connect(
        &endpoint,
        ClientInfo {
            kind: ClientKind::Connector,
            name: NAME.to_owned(),
        },
        imessage.environment,
    )
    .await?
    .hello;
    tracing::info!(
        "connected to viz-backend {} at {endpoint}; BlueBubbles server {}",
        hello.version,
        server_url
    );
    anyhow::bail!("the BlueBubbles bridge is not implemented yet (TODO.md)")
}
