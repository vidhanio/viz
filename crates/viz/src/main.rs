//! `viz`: inspect configuration and talk to the server from a terminal.

use std::{
    io,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand};
use viz_client::{
    Client, Endpoint,
    protocol::{ClientInfo, ClientKind, SendInput, ThreadId, WaitThread},
};
use viz_config::{Component, Connection, DEFAULT_URL};

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
    /// Print component config locations and the CLI's own effective settings.
    Config,
    /// Connect to the server and print its `hello`.
    Status,
    /// Send a message to a thread and print its reply.
    Send {
        /// Target thread id (overrides the CLI config).
        #[arg(long)]
        thread: Option<u64>,
        /// The message.
        text: Vec<String>,
    },
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CliConfig {
    server: Connection,
    thread: Option<ThreadId>,
    environment: Option<String>,
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
    let config: CliConfig = Component::Cli.load(cli.config.as_deref())?;
    let endpoint = || Endpoint::from_connection(&config.server);
    match cli.command {
        Command::Config => print_config(cli.config.as_deref(), config),
        Command::Status => status(&endpoint()?, config.environment).await,
        Command::Send { thread, text } => {
            let thread = thread
                .map(ThreadId)
                .or(config.thread)
                .filter(|id| id.0 > 0)
                .context("specify --thread or configure a positive thread id in CLI config")?;
            send(&endpoint()?, config.environment, thread, text.join(" ")).await
        }
    }
}

fn print_config(explicit: Option<&Path>, config: CliConfig) -> anyhow::Result<()> {
    print_path("server", &Component::Server.path(None)?)?;
    print_path("desktop", &Component::Desktop.path(None)?)?;
    print_path("cli", &Component::Cli.path(explicit)?)?;
    let CliConfig {
        server,
        thread,
        environment,
    } = config;
    println!("\n[cli.server]");
    let url = server.url.as_deref().unwrap_or(DEFAULT_URL);
    println!(
        "url: {}",
        if url.contains(['@', '?', '#']) {
            "<invalid origin>"
        } else {
            url
        }
    );
    println!(
        "token: {}",
        if server.token.is_some() {
            "<configured>"
        } else {
            "<not configured>"
        }
    );
    if let Some(thread) = thread {
        println!("thread: {thread}");
    }
    if let Some(environment) = environment {
        println!("environment: {environment}");
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

async fn status(endpoint: &Endpoint, environment: Option<String>) -> anyhow::Result<()> {
    let hello = Client::connect(endpoint, info(), environment).await?.hello;
    println!(
        "{endpoint}: viz-server {} (protocol {}), environment {}",
        hello.version, hello.protocol, hello.environment
    );
    Ok(())
}

async fn send(
    endpoint: &Endpoint,
    environment: Option<String>,
    thread: ThreadId,
    text: String,
) -> anyhow::Result<()> {
    if text.is_empty() {
        bail!("nothing to send");
    }
    let client = Client::connect(endpoint, info(), environment).await?.client;
    let run = client
        .send(&SendInput {
            thread,
            text,
            origin: None,
            request_id: None,
            attachments: Vec::new(),
            stored: Vec::new(),
            context: Vec::new(),
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
        if let Some(answer) = run.answer {
            println!("{answer}");
            return Ok(());
        }
        if let Some(error) = run.error {
            bail!("{error}");
        }
        bail!("no answer ({:?})", run.status);
    }
}

fn info() -> ClientInfo {
    ClientInfo {
        kind: ClientKind::Cli,
        name: NAME.to_owned(),
    }
}
