//! `viz`: inspect configuration and talk to the backend from a terminal.

use std::env;

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand};
use viz_client::{
    Client, Endpoint,
    protocol::{ClientInfo, ClientKind, Notification, Origin, Submit},
};
use viz_config::Config;

/// The CLI's client name, and the connector its inputs' origins name.
const NAME: &str = "cli";

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print config locations and the merged config document.
    Config,
    /// Connect to the backend and print its `hello`.
    Status,
    /// Send a message to the orchestrator and print its reply.
    Send {
        /// The message.
        text: Vec<String>,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;
    match cli.command {
        Command::Config => print_config(&config),
        Command::Status => status(&config).await,
        Command::Send { text } => send(&config, text.join(" ")).await,
    }
}

fn print_config(config: &Config) -> anyhow::Result<()> {
    let paths = &config.paths;
    println!("# config:    {}", paths.config_file().display());
    println!("# drop-ins:  {}", paths.config_dropins().display());
    println!("# state:     {}", paths.state_dir.display());
    println!("# socket:    {}", config.backend_socket().display());
    print!("{}", toml::to_string_pretty(config.document())?);
    Ok(())
}

async fn status(config: &Config) -> anyhow::Result<()> {
    let endpoint = Endpoint::from_config(config)?;
    let hello = Client::connect(&endpoint, info()).await?.hello;
    println!(
        "{endpoint}: viz-backend {} (protocol {}), orchestrator conversation {}",
        hello.version, hello.protocol, hello.root
    );
    Ok(())
}

async fn send(config: &Config, text: String) -> anyhow::Result<()> {
    if text.is_empty() {
        bail!("nothing to send");
    }
    let endpoint = Endpoint::from_config(config)?;
    let mut connection = Client::connect(&endpoint, info()).await?;
    let submission = connection
        .client
        .submit(&Submit {
            text,
            origin: Some(Origin {
                connector: NAME.to_owned(),
                channel: "terminal".to_owned(),
                thread: None,
                message: None,
                author: env::var("USER").ok(),
            }),
            ..Submit::default()
        })
        .await?;
    while let Some(notification) = connection.notifications.recv().await {
        if let Notification::Reply {
            submission: id,
            text,
            status,
            ..
        } = notification
            && id == submission
        {
            println!(
                "{}",
                text.with_context(|| format!("no answer ({status:?})"))?
            );
            return Ok(());
        }
    }
    bail!("backend closed the connection before replying");
}

/// Says `hello` as a connector, not [`ClientKind::Cli`]: the backend sends
/// `reply` notifications only to connectors, and `send` waits for one.
fn info() -> ClientInfo {
    ClientInfo {
        kind: ClientKind::Connector,
        name: NAME.to_owned(),
    }
}
