//! Discord connector: forwards allowed users' DMs and mentions into the
//! orchestrator and posts the replies to those inputs back where they came
//! from.
//!
//! ```toml
//! [connectors.discord]
//! bot_token = { file = "/run/secrets/viz-discord" }
//! allowed_users = [123456789012345678]
//! ```

use std::{collections::HashSet, sync::Arc};

use anyhow::Context as _;
use serde::Deserialize;
use serenity::{
    all::{ChannelId, CreateMessage, GatewayIntents, Message, MessageId, Ready, UserId},
    async_trait,
    client::{Context, EventHandler},
    http::Http,
};
use viz_client::{
    Client, Connection, Endpoint,
    protocol::{ClientInfo, ClientKind, Notification, Origin, Submit},
};
use viz_config::{Config, Secret};

/// The connector's name: its config section, `hello` name and origins.
const NAME: &str = "discord";
/// Discord's message length limit, in characters.
const MAX_MESSAGE: usize = 2000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscordConfig {
    bot_token: Secret,
    /// Users whose messages are forwarded. Everyone else is ignored.
    allowed_users: HashSet<UserId>,
}

struct Handler {
    viz: Client,
    allowed_users: HashSet<UserId>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        tracing::info!("connected to Discord as {}", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot || !self.allowed_users.contains(&msg.author.id) {
            return;
        }
        let is_dm = msg.guild_id.is_none();
        if !is_dm && !msg.mentions_me(&ctx).await.unwrap_or(false) {
            return;
        }
        let submit = Submit {
            text: msg.content,
            request_id: Some(format!("{NAME}:{}", msg.id)),
            origin: Some(Origin {
                connector: NAME.to_owned(),
                channel: msg.channel_id.to_string(),
                thread: None,
                message: Some(msg.id.to_string()),
                author: Some(msg.author.name),
            }),
            ..Submit::default()
        };
        if let Err(e) = self.viz.submit(&submit).await {
            tracing::error!("forwarding message {}: {e}", msg.id);
        }
        _ = msg.channel_id.broadcast_typing(&ctx.http).await;
    }
}

/// Posts `text` to the origin's channel in Discord-sized chunks, the first
/// replying to the origin's message.
async fn post_reply(http: &Http, origin: &Origin, text: &str) -> anyhow::Result<()> {
    let channel = ChannelId::new(origin.channel.parse().context("origin channel")?);
    let reference = origin
        .message
        .as_deref()
        .and_then(|message| message.parse().ok())
        .map(MessageId::new);
    let chars: Vec<char> = text.chars().collect();
    for (i, chunk) in chars.chunks(MAX_MESSAGE).enumerate() {
        let mut message = CreateMessage::new().content(chunk.iter().collect::<String>());
        if i == 0
            && let Some(reference) = reference
        {
            message = message.reference_message((channel, reference));
        }
        channel.send_message(http, message).await?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::load()?;
    let discord: DiscordConfig = config.connector(NAME)?;
    anyhow::ensure!(
        !discord.allowed_users.is_empty(),
        "[connectors.discord].allowed_users is empty: nobody could talk to viz"
    );
    let endpoint = Endpoint::from_config(&config)?;
    let Connection {
        client: viz,
        mut notifications,
        hello,
    } = Client::connect(
        &endpoint,
        ClientInfo {
            kind: ClientKind::Connector,
            name: NAME.to_owned(),
        },
    )
    .await?;
    tracing::info!("connected to viz-backend {} at {endpoint}", hello.version);

    let intents = GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;
    let mut client = serenity::Client::builder(discord.bot_token.resolve()?, intents)
        .event_handler(Handler {
            viz,
            allowed_users: discord.allowed_users,
        })
        .await?;
    let http = Arc::clone(&client.http);

    let replies = tokio::spawn(async move {
        while let Some(notification) = notifications.recv().await {
            let Notification::Reply { origin, text, .. } = notification else {
                continue;
            };
            let Some(text) = text.filter(|text| !text.is_empty()) else {
                continue;
            };
            if let Err(e) = post_reply(&http, &origin, &text).await {
                tracing::error!("posting reply to {}: {e:#}", origin.channel);
            }
        }
    });

    tokio::select! {
        result = client.start() => result?,
        _ = replies => anyhow::bail!("lost the connection to viz-backend"),
    }
    Ok(())
}
