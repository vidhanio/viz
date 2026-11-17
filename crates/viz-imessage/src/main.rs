//! iMessage connector (scaffold): bridges iMessage through a BlueBubbles server
//! running on a Mac.
//!
//! ```toml
//! [connectors.imessage]
//! server_url = "http://mac.tailnet.ts.net:1234"
//! password = { file = "/run/secrets/viz-bluebubbles" }
//! allowed_handles = ["+15555550123", "me@example.com"]
//! ```
//!
//! The BlueBubbles bridge itself is not written yet (see `TODO.md`); this
//! binary validates its config and the backend connection so the service wiring
//! can be exercised.

use serde::Deserialize;
use viz_client::{
    Client, Endpoint,
    protocol::{ClientInfo, ClientKind},
};
use viz_config::{Config, Secret};

const NAME: &str = "imessage";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImessageConfig {
    /// BlueBubbles server base URL.
    server_url: String,
    /// BlueBubbles server password.
    password: Secret,
    /// Phone numbers / Apple IDs whose messages are forwarded.
    allowed_handles: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::load()?;
    let imessage: ImessageConfig = config.connector(NAME)?;
    imessage.password.resolve()?;
    anyhow::ensure!(
        !imessage.allowed_handles.is_empty(),
        "[connectors.imessage].allowed_handles is empty: nobody could talk to viz"
    );
    let endpoint = Endpoint::from_config(&config)?;
    let viz = Client::connect(
        &endpoint,
        ClientInfo {
            kind: ClientKind::Connector,
            name: NAME.into(),
        },
    )
    .await?;
    tracing::info!(
        "connected to viz-backend {} at {endpoint}; BlueBubbles server {}",
        viz.hello.version,
        imessage.server_url
    );
    anyhow::bail!("the BlueBubbles bridge is not implemented yet (TODO.md)")
}
