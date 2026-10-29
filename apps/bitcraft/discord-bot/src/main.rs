use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use time::OffsetDateTime;
use tokio::{sync::watch, time::MissedTickBehavior};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;
use twilight_gateway::{
    CloseFrame, Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _,
};
use twilight_http::Client;
use twilight_model::{
    application::interaction::{Interaction, InteractionData},
    channel::message::AllowedMentions,
    gateway::{payload::incoming::GuildCreate, presence::Presence},
    http::interaction::{InteractionResponse, InteractionResponseType},
    id::{
        Id,
        marker::{ApplicationMarker, ChannelMarker, GuildMarker, UserMarker},
    },
};
use twilight_util::builder::InteractionResponseDataBuilder;

mod board;
mod commands;
mod config;
mod messages;
mod store;

use board::{Leaderboard, TOP_SIZE, presence_bits};
use config::{Config, load_config};
use store::Store;

/// How often changed scores are saved. Online players' scores come back from
/// their presence after a restart, so only this long of other changes, such as
/// a peak a player has since spent below, can be lost.
const SAVE_INTERVAL: Duration = Duration::from_secs(60);
const NAME_REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

struct State {
    leaderboard: Leaderboard,
    unsaved: bool,
    /// The leaderboard as last published, which `/rank` answers from.
    published: Leaderboard,
    /// When each player's display name was last looked up.
    named_at: HashMap<Id<UserMarker>, Instant>,
}

struct Bot {
    guild_id: Id<GuildMarker>,
    bitcraft_application_id: Id<ApplicationMarker>,
    http: Arc<Client>,
    state: Arc<Mutex<State>>,
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
        leaderboard_channel_id,
        s3,
        public_bucket,
        public_object,
        state_bucket,
        state_object,
        publish_interval,
    } = load_config()?;
    let public_store = Store::new(&s3, public_bucket, public_object)?;
    let state_store = Store::new(&s3, state_bucket, state_object)?;
    let leaderboard = state_store
        .load()
        .await
        .context("Could not read the saved leaderboard")?;
    info!(
        entries = leaderboard.entries.len(),
        "Loaded the leaderboard"
    );
    // Until the next publication, `/rank` answers from what was published before.
    let published = public_store.load().await.unwrap_or_else(|error| {
        warn!(?error, "Could not read the published leaderboard");
        Leaderboard::default()
    });

    let http = Arc::new(Client::new(discord_token.clone()));
    let bot = Bot {
        guild_id,
        bitcraft_application_id,
        http: http.clone(),
        state: Arc::new(Mutex::new(State {
            leaderboard,
            unsaved: false,
            published,
            named_at: HashMap::new(),
        })),
        application_id: OnceLock::new(),
    };
    let (stop, stopped) = watch::channel(());
    let saver = tokio::spawn(save_loop(
        state_store,
        bot.state.clone(),
        SAVE_INTERVAL,
        stopped.clone(),
    ));
    let publisher = tokio::spawn(publish_loop(
        public_store,
        http,
        leaderboard_channel_id,
        bot.state.clone(),
        publish_interval,
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
    let _ = stop.send(());
    publisher.await?;
    saver.await?;
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
            Event::InteractionCreate(interaction) => self.answer(interaction.0),
            Event::GatewayClose(frame) => info!(?frame, "Discord closed the gateway"),
            _ => {}
        }
    }

    fn observe(&self, presences: &[Presence]) {
        let mut state = self.state.lock().unwrap();
        for presence in presences {
            let Some(bits) = presence_bits(presence, self.bitcraft_application_id) else {
                continue;
            };
            let player = presence.user.id();
            if state.leaderboard.record(player, bits, now()) {
                debug!(%player, bits, "New high score");
                state.unsaved = true;
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
    }

    fn answer(&self, interaction: Interaction) {
        if interaction.guild_id != Some(self.guild_id) {
            return;
        }
        let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
            return;
        };
        let Some(commands::Rank(player)) = commands::rank(data) else {
            return;
        };
        let Some(player) = player.or(interaction.author_id()) else {
            return;
        };
        let reply = messages::rank(&self.state.lock().unwrap().published, player);
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
        .set_guild_commands(guild_id, &commands::definitions())
        .await
    {
        warn!(?error, "Could not register the commands");
    }
}

async fn respond(http: Arc<Client>, interaction: Interaction, reply: String) {
    let response = InteractionResponse {
        kind: InteractionResponseType::ChannelMessageWithSource,
        data: Some(
            InteractionResponseDataBuilder::new()
                .content(reply)
                .allowed_mentions(AllowedMentions::default())
                .build(),
        ),
    };
    if let Err(error) = http
        .interaction(interaction.application_id)
        .create_response(interaction.id, &interaction.token, &response)
        .await
    {
        warn!(?error, "Could not answer a command");
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
                state.unsaved = true;
            }
        }
        Err(error) => {
            warn!(%player, ?error, "Could not look up a player's name");
            state.named_at.remove(&player);
        }
    }
}

async fn save_loop(
    store: Store,
    state: Arc<Mutex<State>>,
    interval: Duration,
    mut stopped: watch::Receiver<()>,
) {
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let stopping = tokio::select! {
            _ = ticks.tick() => false,
            _ = stopped.changed() => true,
        };
        save_leaderboard(&store, &state).await;
        if stopping {
            return;
        }
    }
}

async fn save_leaderboard(store: &Store, state: &Mutex<State>) {
    let leaderboard = {
        let mut state = state.lock().unwrap();
        if !state.unsaved {
            return;
        }
        state.unsaved = false;
        state.leaderboard.updated_at = Some(now());
        state.leaderboard.clone()
    };
    match store.save(&leaderboard).await {
        Ok(()) => debug!(entries = leaderboard.entries.len(), "Saved the leaderboard"),
        Err(error) => {
            warn!(?error, "Could not save the leaderboard");
            state.lock().unwrap().unsaved = true;
        }
    }
}

/// Publishes and posts the leaderboard at every multiple of `interval` since
/// the Unix epoch, so restarts neither add publications nor move them.
async fn publish_loop(
    store: Store,
    http: Arc<Client>,
    channel_id: Id<ChannelMarker>,
    state: Arc<Mutex<State>>,
    interval: Duration,
    mut stopped: watch::Receiver<()>,
) {
    let period = interval.as_secs();
    let mut next = next_publication(period, unix_now());
    loop {
        let wait = Duration::from_secs(next.saturating_sub(unix_now()));
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = stopped.changed() => return,
        }
        let top = publish_top(&store, &state).await;
        post(&http, channel_id, &top).await;
        next = next_publication(period, next.max(unix_now()));
    }
}

/// The first multiple of `period` seconds after `after`, in Unix seconds.
fn next_publication(period: u64, after: u64) -> u64 {
    (after / period + 1) * period
}

fn unix_now() -> u64 {
    u64::try_from(OffsetDateTime::now_utc().unix_timestamp()).unwrap_or(0)
}

async fn publish_top(store: &Store, state: &Mutex<State>) -> Leaderboard {
    let top = {
        let mut state = state.lock().unwrap();
        let mut top = state.leaderboard.top(TOP_SIZE);
        top.updated_at = Some(now());
        state.published = top.clone();
        top
    };
    match store.save(&top).await {
        Ok(()) => info!(entries = top.entries.len(), "Published the leaderboard"),
        Err(error) => warn!(?error, "Could not publish the leaderboard"),
    }
    top
}

async fn post(http: &Client, channel_id: Id<ChannelMarker>, top: &Leaderboard) {
    let no_pings = AllowedMentions::default();
    for message in messages::standings(top) {
        if let Err(error) = http
            .create_message(channel_id)
            .content(&message)
            .allowed_mentions(Some(&no_pings))
            .await
        {
            warn!(?error, "Could not post the leaderboard");
            return;
        }
    }
}

fn now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    now.replace_nanosecond(0).unwrap_or(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(service: &store::mock::Shared, object: &str) -> Option<Leaderboard> {
        let service = service.lock().unwrap();
        let bytes = service.objects.get(object)?;
        Some(serde_json::from_slice(bytes).unwrap())
    }

    fn writes(service: &store::mock::Shared, object: &str) -> usize {
        let service = service.lock().unwrap();
        service
            .writes
            .iter()
            .filter(|(written, _)| written == object)
            .count()
    }

    async fn wait_for(condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    fn players(count: u64) -> Leaderboard {
        let mut leaderboard = Leaderboard::default();
        for player in 1..=count {
            leaderboard.record(Id::new(player), player as f64, now());
        }
        leaderboard.banned.insert(Id::new(999));
        leaderboard
    }

    #[tokio::test]
    async fn changed_scores_are_saved_on_each_tick_and_on_stop() {
        const SAVED: &str = "private/state.json";
        let (s3, service) = store::mock::start().await;
        let state = Arc::new(Mutex::new(State {
            leaderboard: players(3),
            unsaved: true,
            published: Leaderboard::default(),
            named_at: HashMap::new(),
        }));
        let (stop, stopped) = watch::channel(());
        let saver = tokio::spawn(save_loop(
            Store::new(&s3, "private".into(), "state.json".into()).unwrap(),
            state.clone(),
            Duration::from_millis(20),
            stopped,
        ));
        let record = |player: u64, bits: f64| {
            let mut state = state.lock().unwrap();
            assert!(state.leaderboard.record(Id::new(player), bits, now()));
            state.unsaved = true;
        };

        wait_for(|| writes(&service, SAVED) == 1).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            writes(&service, SAVED),
            1,
            "unchanged scores are not saved again"
        );
        record(1, 10.0);
        wait_for(|| writes(&service, SAVED) == 2).await;
        assert_eq!(stored(&service, SAVED).unwrap().entries[0].bits, 10.0);

        record(3, 30.0);
        stop.send(()).unwrap();
        saver.await.unwrap();
        let saved = stored(&service, SAVED).unwrap();
        assert_eq!(saved.entries[0].bits, 30.0);
        assert_eq!(saved.entries.len(), 3);
        assert!(saved.banned.contains(&Id::new(999)));
    }

    #[tokio::test]
    async fn the_published_leaderboard_is_the_top_without_bans() {
        let (s3, service) = store::mock::start().await;
        let state = Mutex::new(State {
            leaderboard: players(TOP_SIZE as u64 + 5),
            unsaved: false,
            published: Leaderboard::default(),
            named_at: HashMap::new(),
        });
        let store = Store::new(&s3, "public".into(), "leaderboard.json".into()).unwrap();
        let top = publish_top(&store, &state).await;
        assert_eq!(stored(&service, "public/leaderboard.json").unwrap(), top);
        assert_eq!(top.entries.len(), TOP_SIZE);
        assert_eq!(top.entries[0].user_id, Id::new(TOP_SIZE as u64 + 5));
        assert!(top.banned.is_empty());
        assert!(top.updated_at.is_some());
        assert_eq!(
            state.lock().unwrap().published,
            top,
            "/rank answers from it"
        );
    }

    #[test]
    fn publications_fall_on_multiples_of_the_interval() {
        let three_hours = 3 * 60 * 60;
        assert_eq!(next_publication(three_hours, 0), three_hours);
        assert_eq!(next_publication(three_hours, three_hours - 1), three_hours);
        assert_eq!(next_publication(three_hours, three_hours), 2 * three_hours);
        // 2026-10-28T10:48:00Z publishes next at 12:00 UTC.
        assert_eq!(next_publication(three_hours, 1_793_184_480), 1_793_188_800);
    }
}
