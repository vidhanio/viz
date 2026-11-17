//! `viz`: inspect configuration and talk to the backend from a terminal.

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand};
use viz_client::{
    Client, Endpoint,
    protocol::{ClientInfo, ClientKind, Notification, Origin, Submit},
};
use viz_config::Config;

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
        Command::Config => {
            let paths = &config.paths;
            println!("# config:    {}", paths.config_file().display());
            println!("# drop-ins:  {}", paths.config_dropins().display());
            println!("# state:     {}", paths.state_dir.display());
            println!("# socket:    {}", config.backend_socket().display());
            print!("{}", toml::to_string_pretty(config.document())?);
        }
        Command::Status => {
            let endpoint = Endpoint::from_config(&config)?;
            let connection = Client::connect(&endpoint, info()).await?;
            let hello = connection.hello;
            println!(
                "{endpoint}: viz-backend {} (protocol {}), orchestrator conversation {}",
                hello.version, hello.protocol, hello.root
            );
        }
        Command::Send { text } => {
            let text = text.join(" ");
            if text.is_empty() {
                bail!("nothing to send");
            }
            let endpoint = Endpoint::from_config(&config)?;
            let mut connection = Client::connect(&endpoint, info()).await?;
            let submission = connection
                .client
                .submit(&Submit {
                    text,
                    origin: Some(Origin {
                        connector: "cli".into(),
                        channel: "terminal".into(),
                        thread: None,
                        message: None,
                        author: std::env::var("USER").ok(),
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
    }
    Ok(())
}

fn info() -> ClientInfo {
    ClientInfo {
        kind: ClientKind::Connector,
        name: "cli".into(),
    }
}
