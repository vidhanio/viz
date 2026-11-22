//! viz-server runs ACP threads and delegates work to configured environments.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the binary uses pub(crate) explicitly for APIs shared across modules"
)]

mod acp;
mod config;
mod daemon;
mod environments;
mod server;
mod tasks;
mod tools;
mod wire;
mod workspace;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;

use crate::{
    acp::mcp::{Dispatch, Mcp},
    config::Config,
    environments::Peers,
    server::Server,
    tasks::Tasks,
    tools::Tools,
    workspace::Workspace,
};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Read one server TOML file instead of the default layers.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the server (also the default when no command is given).
    Serve,
    /// Write JSON Schema for the shared wire types.
    Schema {
        /// Write to this file instead of stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    #[command(hide = true)]
    McpBridge,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let _ = rustls::crypto::ring::default_provider().install_default();
    match cli.command.unwrap_or(Command::Serve) {
        Command::McpBridge => runtime()?.block_on(acp::bridge::run()),
        Command::Schema { output } => {
            let schema = serde_json::json!({
                "protocol": wire::PROTOCOL_VERSION,
                "methods": viz_protocol::method_schemas(),
                "notification": schemars::schema_for!(wire::Notification),
            });
            let mut json = serde_json::to_string_pretty(&schema)?;
            json.push('\n');
            if let Some(output) = output {
                std::fs::write(&output, json)
                    .with_context(|| format!("writing {}", output.display()))?;
            } else {
                print!("{json}");
            }
            Ok(())
        }
        Command::Serve => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "viz_server=info".into()),
                )
                .with_writer(std::io::stderr)
                .init();
            let config = Config::load(cli.config.as_deref())?;
            let activation = daemon::activated_listener(config.listen)?;
            runtime()?.block_on(run(config, activation))
        }
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting Tokio")
}

async fn run(config: Config, activation: Option<std::net::TcpListener>) -> Result<()> {
    let listener = daemon::listener(config.listen, activation).await?;
    let stop = CancellationToken::new();
    let signals = daemon::shutdown_signals(stop.clone())?;
    let peers = Arc::new(Peers::new(&config)?);
    let mcp = Mcp::start(tools::definitions()).await?;
    let workspace = Workspace::open(&config, mcp.clone()).await?;
    let tasks = Tasks::new(Arc::clone(&workspace), Arc::clone(&peers));
    let dispatch: Arc<dyn Dispatch> = Arc::new(Tools::new(
        Arc::clone(&workspace),
        Arc::clone(&peers),
        Arc::clone(&tasks),
    ));
    mcp.set_dispatch(&dispatch);
    let server = Arc::new(Server::new(
        &config,
        Arc::clone(&workspace),
        peers,
        Arc::clone(&tasks),
        stop.clone(),
    ));
    let result = async {
        workspace.resume().await?;
        tasks.resume();
        server.run(&config, listener).await
    }
    .await;
    stop.cancel();
    signals.abort();
    tasks.shutdown().await;
    workspace.shutdown().await;
    mcp.stop();
    drop(dispatch);
    acp::drain_processes().await;
    result
}
