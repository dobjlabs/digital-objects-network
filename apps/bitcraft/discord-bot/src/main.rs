use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use time::OffsetDateTime;
use tokio::sync::{Notify, mpsc, watch};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;
use twilight_gateway::{
    CloseFrame, Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _,
};
use twilight_http::Client;
use twilight_model::{
    application::interaction::{Interaction, InteractionData},
    channel::message::{AllowedMentions, MessageFlags},
    gateway::{payload::incoming::GuildCreate, presence::Presence},
    http::interaction::{InteractionResponse, InteractionResponseType},
    id::{
        Id,
        marker::{ApplicationMarker, GuildMarker, UserMarker},
    },
};
use twilight_util::builder::InteractionResponseDataBuilder;

mod announce;
mod board;
mod commands;
mod config;
mod store;

use board::{Leaderboard, presence_bits};
use commands::Moderation;
use config::{Config, load_config};
use store::Store;

const PUBLISH_INTERVAL: Duration = Duration::from_secs(60);
const NAME_REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

struct State {
    leaderboard: Leaderboard,
    unpublished: bool,
    /// When each player's display name was last looked up.
    named_at: HashMap<Id<UserMarker>, Instant>,
}

struct Bot {
    guild_id: Id<GuildMarker>,
    bitcraft_application_id: Id<ApplicationMarker>,
    http: Arc<Client>,
    state: Arc<Mutex<State>>,
    publish_now: Arc<Notify>,
    announcements: mpsc::UnboundedSender<String>,
    /// The bot's own application, known once connected.
    application_id: OnceLock<Id<ApplicationMarker>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let Config {
        discord_token,
        guild_id,
        bitcraft_application_id,
        announce_channel_id,
        store,
    } = load_config()?;
    let store = Arc::new(Store::new(store)?);
    let leaderboard = store
        .load()
        .await
        .context("Could not read the published leaderboard")?;
    info!(
        entries = leaderboard.entries.len(),
        "Loaded the leaderboard"
    );

    let http = Arc::new(Client::new(discord_token.clone()));
    let (announcements, to_announce) = mpsc::unbounded_channel();
    tokio::spawn(announce::post(
        http.clone(),
        announce_channel_id,
        to_announce,
    ));
    let bot = Bot {
        guild_id,
        bitcraft_application_id,
        http,
        state: Arc::new(Mutex::new(State {
            leaderboard,
            unpublished: false,
            named_at: HashMap::new(),
        })),
        publish_now: Arc::new(Notify::new()),
        announcements,
        application_id: OnceLock::new(),
    };
    let (stop_publishing, stopped) = watch::channel(());
    let publisher = tokio::spawn(publish(
        store,
        bot.state.clone(),
        bot.publish_now.clone(),
        stopped,
    ));

    let mut shard = Shard::new(
        ShardId::ONE,
        discord_token,
        Intents::GUILDS | Intents::GUILD_PRESENCES,
    );
    let events = EventTypeFlags::READY
        | EventTypeFlags::GUILD_CREATE
        | EventTypeFlags::PRESENCE_UPDATE
        | EventTypeFlags::INTERACTION_CREATE;
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let outcome = loop {
        tokio::select! {
            signal = &mut shutdown => break signal,
            event = shard.next_event(events) => match event {
                Some(Ok(event)) => bot.handle(event),
                Some(Err(error)) => warn!(?error, "Discord gateway error"),
                None => break Err(anyhow!(
                    "Discord closed the gateway for good; check the token and that the bot has the Presence Intent"
                )),
            },
        }
    };
    shard.close(CloseFrame::NORMAL);
    let _ = stop_publishing.send(());
    publisher.await?;
    outcome
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            interrupt = tokio::signal::ctrl_c() => interrupt?,
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

impl Bot {
    fn handle(&self, event: Event) {
        match event {
            Event::Ready(ready) => {
                info!(user = %ready.user.name, "Connected to Discord");
                let _ = self.application_id.set(ready.application.id);
                if !ready.guilds.iter().any(|guild| guild.id == self.guild_id) {
                    warn!(
                        guild_id = %self.guild_id,
                        "The bot is not in the DISCORD_GUILD_ID server; invite it with the bot and applications.commands scopes"
                    );
                }
            }
            Event::GuildCreate(guild) => {
                if let GuildCreate::Available(guild) = *guild
                    && guild.id == self.guild_id
                {
                    info!(name = %guild.name, "Following the server");
                    if let Some(application_id) = self.application_id.get() {
                        tokio::spawn(register_commands(
                            self.http.clone(),
                            *application_id,
                            self.guild_id,
                        ));
                    }
                    self.observe(&guild.presences);
                }
            }
            Event::PresenceUpdate(update) if update.guild_id == self.guild_id => {
                self.observe(std::slice::from_ref(&update.0));
            }
            Event::InteractionCreate(interaction) => self.moderate(interaction.0),
            Event::GatewayClose(frame) => info!(?frame, "Discord closed the gateway"),
            _ => {}
        }
    }

    /// A batch announces at most one new leader, however often first place changed within it.
    fn observe(&self, presences: &[Presence]) {
        let mut state = self.state.lock().unwrap();
        let leader = state.leaderboard.leader();
        for presence in presences {
            let Some(bits) = presence_bits(presence, self.bitcraft_application_id) else {
                continue;
            };
            let player = presence.user.id();
            if state.leaderboard.record(player, bits, now()) {
                debug!(%player, bits, "New high score");
                state.unpublished = true;
            }
            let name_is_due = state
                .named_at
                .get(&player)
                .is_none_or(|named_at| named_at.elapsed() >= NAME_REFRESH_INTERVAL);
            if name_is_due && state.leaderboard.has_entry(player) {
                state.named_at.insert(player, Instant::now());
                tokio::spawn(refresh_name(
                    self.http.clone(),
                    self.guild_id,
                    player,
                    self.state.clone(),
                ));
            }
        }
        if state.leaderboard.leader() != leader
            && let Some(message) = announce::new_leader(&state.leaderboard)
        {
            info!(leader = ?state.leaderboard.leader(), "New leader");
            let _ = self.announcements.send(message);
        }
    }

    fn moderate(&self, interaction: Interaction) {
        if interaction.guild_id != Some(self.guild_id) {
            return;
        }
        let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
            return;
        };
        let Some(moderation) = commands::moderation(data) else {
            return;
        };
        info!(moderator = ?interaction.author_id(), ?moderation, "Leaderboard moderation");
        let (reply, changed) = {
            let mut state = self.state.lock().unwrap();
            let leaderboard = &mut state.leaderboard;
            let outcome = match moderation {
                Moderation::Ban(player) if leaderboard.ban(player) => (
                    format!("<@{player}> is off the leaderboard, and their presence is ignored."),
                    true,
                ),
                Moderation::Ban(player) => (format!("<@{player}> is already banned."), false),
                Moderation::Unban(player) if leaderboard.unban(player) => (
                    format!("<@{player}> returns to the leaderboard with their next score."),
                    true,
                ),
                Moderation::Unban(player) => (format!("<@{player}> is not banned."), false),
            };
            state.unpublished |= outcome.1;
            outcome
        };
        if changed {
            self.publish_now.notify_one();
        }
        tokio::spawn(respond(self.http.clone(), interaction, reply));
    }
}

async fn register_commands(
    http: Arc<Client>,
    application_id: Id<ApplicationMarker>,
    guild_id: Id<GuildMarker>,
) {
    if let Err(error) = http
        .interaction(application_id)
        .set_guild_commands(guild_id, &[commands::leaderboard()])
        .await
    {
        warn!(?error, "Could not register the moderation commands");
    }
}

async fn respond(http: Arc<Client>, interaction: Interaction, reply: String) {
    let response = InteractionResponse {
        kind: InteractionResponseType::ChannelMessageWithSource,
        data: Some(
            InteractionResponseDataBuilder::new()
                .content(reply)
                .flags(MessageFlags::EPHEMERAL)
                .allowed_mentions(AllowedMentions::default())
                .build(),
        ),
    };
    if let Err(error) = http
        .interaction(interaction.application_id)
        .create_response(interaction.id, &interaction.token, &response)
        .await
    {
        warn!(?error, "Could not answer a moderation command");
    }
}

async fn refresh_name(
    http: Arc<Client>,
    guild_id: Id<GuildMarker>,
    player: Id<UserMarker>,
    state: Arc<Mutex<State>>,
) {
    let member = match http.guild_member(guild_id, player).await {
        Ok(response) => response.model().await.map_err(anyhow::Error::from),
        Err(error) => Err(error.into()),
    };
    let mut state = state.lock().unwrap();
    match member {
        Ok(member) => {
            let name = member
                .nick
                .or(member.user.global_name)
                .unwrap_or(member.user.name);
            if state.leaderboard.rename(player, &name) {
                state.unpublished = true;
            }
        }
        Err(error) => {
            warn!(%player, ?error, "Could not look up a player's name");
            state.named_at.remove(&player);
        }
    }
}

async fn publish(
    store: Arc<Store>,
    state: Arc<Mutex<State>>,
    publish_now: Arc<Notify>,
    mut stopped: watch::Receiver<()>,
) {
    let mut interval = tokio::time::interval(PUBLISH_INTERVAL);
    loop {
        let stopping = tokio::select! {
            _ = interval.tick() => false,
            _ = publish_now.notified() => false,
            _ = stopped.changed() => true,
        };
        let snapshot = {
            let mut state = state.lock().unwrap();
            if state.unpublished {
                state.unpublished = false;
                state.leaderboard.updated_at = Some(now());
                Some(state.leaderboard.clone())
            } else {
                None
            }
        };
        if let Some(leaderboard) = snapshot {
            match store.publish(&leaderboard).await {
                Ok(()) => debug!(entries = leaderboard.entries.len(), "Published"),
                Err(error) => {
                    warn!(?error, "Could not publish the leaderboard");
                    state.lock().unwrap().unpublished = true;
                }
            }
        }
        if stopping {
            return;
        }
    }
}

fn now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    now.replace_nanosecond(0).unwrap_or(now)
}
