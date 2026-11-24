//! `viz`: inspect configuration and talk to the server from a terminal.

use std::{
    io,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand};
use viz_client::{
    Client, Endpoint,
    protocol::{ClientInfo, ClientKind, RunStatus, SendInput, ThreadId, WaitThread},
};
use viz_config::{Component, Connection, default_socket};

/// The CLI's client name.
const NAME: &str = "cli";

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Use this config file instead of `$VIZ_CLI_CONFIG` or the default
    /// location.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print config file locations and the effective server connection.
    Config,
    /// Connect to the server and print its `hello`.
    Status,
    /// Send a message to a thread and print its reply.
    Send {
        /// Target thread id.
        #[arg(long)]
        thread: u64,
        /// The message.
        text: Vec<String>,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "viz_client=warn".into()),
        )
        .with_writer(io::stderr)
        .init();
    let client = Component::Cli.client(cli.config.as_deref())?;
    let endpoint = || Endpoint::from_connection(&client.server);
    match cli.command {
        Command::Config => print_config(cli.config.as_deref(), &client.server),
        Command::Status => status(&endpoint()?).await,
        Command::Send { thread, text } => {
            let thread = Some(thread)
                .filter(|id| *id > 0)
                .map(ThreadId)
                .context("--thread must be a positive thread id")?;
            send(&endpoint()?, thread, text.join(" ")).await
        }
    }
}

fn print_config(explicit: Option<&Path>, server: &Connection) -> anyhow::Result<()> {
    print_path("server", &Component::Server.path(None)?)?;
    print_path("desktop", &Component::Desktop.path(None)?)?;
    print_path("cli", &Component::Cli.path(explicit)?)?;
    println!("\n[client.server]");
    if let Some(url) = server.url.as_deref() {
        println!(
            "url: {}",
            if url.contains(['@', '?', '#']) {
                "<invalid origin>"
            } else {
                url
            }
        );
    } else {
        let socket = server.socket.as_ref().map_or_else(
            || {
                default_socket().map_or_else(
                    |e| format!("<unavailable: {e}>"),
                    |path| path.display().to_string(),
                )
            },
            |path| path.display().to_string(),
        );
        println!("socket: {socket}");
    }
    println!(
        "token: {}",
        if server.token.is_some() {
            "<configured>"
        } else {
            "<not configured>"
        }
    );
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
    let hello = Client::connect(endpoint, info(), None).await?.hello;
    println!(
        "{endpoint}: viz-server {} (protocol {}), environment {}",
        hello.version, hello.protocol, hello.environment
    );
    Ok(())
}

async fn send(endpoint: &Endpoint, thread: ThreadId, text: String) -> anyhow::Result<()> {
    if text.is_empty() {
        bail!("nothing to send");
    }
    let client = Client::connect(endpoint, info(), None).await?.client;
    let run = client
        .send(&SendInput {
            thread,
            prompt: viz_client::protocol::Prompt::new(text),
            origin: None,
            request_id: None,
            mode: None,
        })
        .await?;
    loop {
        let result = client
            .request(&WaitThread {
                thread,
                run: Some(run),
                timeout_ms: None,
            })
            .await?;
        if result.timed_out {
            continue;
        }
        let run = result.run.context("run no longer exists")?;
        match run.status {
            RunStatus::Completed => {
                if let Some(answer) = run.answer {
                    println!("{answer}");
                    return Ok(());
                }
                bail!("run completed without an answer");
            }
            RunStatus::Failed | RunStatus::Cancelled | RunStatus::Interrupted => {
                if let Some(error) = run.error {
                    bail!("{error}");
                }
                bail!("run ended without completing ({:?})", run.status);
            }
            RunStatus::Queued | RunStatus::Running => {
                bail!("wait returned non-terminal status ({:?})", run.status);
            }
        }
    }
}

fn info() -> ClientInfo {
    ClientInfo {
        kind: ClientKind::Cli,
        name: NAME.to_owned(),
    }
}
