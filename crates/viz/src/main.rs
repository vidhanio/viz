//! `viz`: inspect configuration and talk to the backend from a terminal.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand};
use viz_client::{
    Client, Endpoint,
    protocol::{ClientInfo, ClientKind, Notification, Origin, Submit},
};
use viz_config::{ClientFile, Connection, Paths};

/// The CLI's client name, and the connector its inputs' origins name.
const NAME: &str = "cli";

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Use this CLI config file instead of the environment or default location.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print component config locations and the CLI's own effective settings.
    Config,
    /// Connect to the backend and print its `hello`.
    Status,
    /// Send a message to the orchestrator and print its reply.
    Send {
        /// The message.
        text: Vec<String>,
    },
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CliConfig {
    backend: Connection,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let file = cli.config.map_or_else(ClientFile::cli, ClientFile::path);
    let paths = Paths::from_env()?;
    let endpoint = || -> anyhow::Result<Endpoint> {
        let config: CliConfig = viz_config::load(&file)?;
        Ok(Endpoint::from_connection(&config.backend, &paths)?)
    };
    match cli.command {
        Command::Config => print_config(&file, &paths),
        Command::Status => status(&endpoint()?).await,
        Command::Send { text } => send(&endpoint()?, text.join(" ")).await,
    }
}

fn print_config(file: &ClientFile, paths: &Paths) -> anyhow::Result<()> {
    let backend_override = env::var_os("VIZ_CONFIG").filter(|value| !value.is_empty());
    if let Some(path) = backend_override {
        print_path("backend", Path::new(&path))?;
    } else {
        print_path("backend", &paths.config_dir.join("config.toml"))?;
        print_dropins(&paths.config_dir.join("config.d"))?;
    }
    print_path("desktop", &ClientFile::desktop().resolve()?)?;
    print_path("cli", &file.resolve()?)?;
    for name in ["discord", "imessage"] {
        print_path(name, &ClientFile::connector(name).resolve()?)?;
    }
    let CliConfig { backend } = viz_config::load(file)?;
    println!("\nCLI [backend]");
    if let Some(url) = backend.url {
        println!("url: {}", url.trim_end_matches('/'));
        println!(
            "token: {}",
            if backend.token.is_some() {
                "<configured>"
            } else {
                "<missing>"
            }
        );
    } else {
        let socket = backend.socket.unwrap_or_else(|| paths.default_socket());
        println!("socket: {}", socket.display());
    }
    Ok(())
}

fn print_dropins(dir: &Path) -> anyhow::Result<()> {
    print_path("backend drop-ins", dir)?;
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("listing {}", dir.display())),
    };
    let mut dropins = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_name().as_encoded_bytes().ends_with(b".toml") {
            dropins.push(entry.path());
        }
    }
    dropins.sort_unstable();
    for path in dropins {
        print_path("  drop-in", &path)?;
    }
    Ok(())
}

fn print_path(component: &str, path: &Path) -> anyhow::Result<()> {
    let exists = path
        .try_exists()
        .with_context(|| format!("checking {}", path.display()))?;
    println!(
        "{component}: {} ({})",
        path.display(),
        if exists { "exists" } else { "missing" }
    );
    Ok(())
}

async fn status(endpoint: &Endpoint) -> anyhow::Result<()> {
    let hello = Client::connect(endpoint, info()).await?.hello;
    println!(
        "{endpoint}: viz-backend {} (protocol {}), orchestrator conversation {}",
        hello.version, hello.protocol, hello.root
    );
    Ok(())
}

async fn send(endpoint: &Endpoint, text: String) -> anyhow::Result<()> {
    if text.is_empty() {
        bail!("nothing to send");
    }
    let mut connection = Client::connect(endpoint, info()).await?;
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
