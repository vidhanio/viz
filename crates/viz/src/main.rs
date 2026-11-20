//! `viz`: inspect configuration and talk to the backend from a terminal.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand};
use viz_client::{
    Client, Endpoint,
    protocol::{ClientInfo, ClientKind, SendInput, ThreadId, WaitThread},
};
use viz_config::{ClientFile, Connection, DEFAULT_URL, Paths};

/// The CLI's client name.
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
    backend: Connection,
    thread: Option<ThreadId>,
    environment: Option<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let file = cli.config.map_or_else(ClientFile::cli, ClientFile::path);
    let config: CliConfig = viz_config::load(&file)?;
    let endpoint = || Endpoint::from_connection(&config.backend);
    match cli.command {
        Command::Config => print_config(&file, &Paths::from_env()?, config),
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

fn print_config(file: &ClientFile, paths: &Paths, config: CliConfig) -> anyhow::Result<()> {
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
    let CliConfig {
        backend,
        thread,
        environment,
    } = config;
    println!("\nCLI [backend]");
    let url = backend.url.as_deref().unwrap_or(DEFAULT_URL);
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
        if backend.token.is_some() {
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

async fn status(endpoint: &Endpoint, environment: Option<String>) -> anyhow::Result<()> {
    let hello = Client::connect(endpoint, info(), environment).await?.hello;
    println!(
        "{endpoint}: viz-backend {} (protocol {}), environment {}",
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
