use std::time::Duration;

use anyhow::{Context, Result, ensure};
use twilight_model::id::{
    Id,
    marker::{ApplicationMarker, ChannelMarker, GuildMarker},
};

use crate::store::S3Config;

const DEFAULT_S3_REGION: &str = "auto";
const DEFAULT_PUBLIC_OBJECT: &str = "leaderboard.json";
const DEFAULT_STATE_OBJECT: &str = "state.json";
const DEFAULT_PUBLISH_INTERVAL_SECS: u64 = 3 * 60 * 60;

pub struct Config {
    pub discord_token: String,
    /// The one server whose members' presence counts.
    pub guild_id: Id<GuildMarker>,
    /// The application the desktop host shares presence as.
    pub bitcraft_application_id: Id<ApplicationMarker>,
    /// Where the leaderboard is posted.
    pub leaderboard_channel_id: Id<ChannelMarker>,
    pub s3: S3Config,
    /// Where the leaderboard is published for anyone to read.
    pub public_bucket: String,
    pub public_object: String,
    /// Where every player's score is kept, out of public view.
    pub state_bucket: String,
    pub state_object: String,
    /// Time between publications of the leaderboard.
    pub publish_interval: Duration,
}

pub fn load_config() -> Result<Config> {
    let _ = dotenvy::from_filename("apps/bitcraft/discord-bot/.env");
    // A blank value, as in a copied .env.example, counts as unset.
    let setting = |name: &str| dotenvy::var(name).ok().filter(|value| !value.is_empty());
    let var = |name: &str| setting(name).with_context(|| format!("{name} must be set"));
    let optional = |name: &str, default: &str| setting(name).unwrap_or_else(|| default.into());

    let public_bucket = var("S3_PUBLIC_BUCKET")?;
    let state_bucket = var("S3_STATE_BUCKET")?;
    ensure!(
        public_bucket != state_bucket,
        "S3_STATE_BUCKET must differ from S3_PUBLIC_BUCKET, which anyone can read"
    );
    let publish_interval_secs = match setting("PUBLISH_INTERVAL_SECS") {
        Some(value) => value
            .parse::<u64>()
            .ok()
            .filter(|secs| *secs > 0)
            .context("PUBLISH_INTERVAL_SECS must be a positive number of seconds")?,
        None => DEFAULT_PUBLISH_INTERVAL_SECS,
    };

    Ok(Config {
        discord_token: var("DISCORD_BOT_TOKEN")?,
        guild_id: var("DISCORD_GUILD_ID")?
            .parse()
            .context("DISCORD_GUILD_ID must be a numeric Discord server ID")?,
        bitcraft_application_id: var("BITCRAFT_DISCORD_APPLICATION_ID")?
            .parse()
            .context("BITCRAFT_DISCORD_APPLICATION_ID must be a numeric Discord application ID")?,
        leaderboard_channel_id: var("DISCORD_LEADERBOARD_CHANNEL_ID")?
            .parse()
            .context("DISCORD_LEADERBOARD_CHANNEL_ID must be a numeric Discord channel ID")?,
        s3: S3Config {
            endpoint: var("S3_ENDPOINT")?
                .parse()
                .context("S3_ENDPOINT must be a URL")?,
            region: optional("S3_REGION", DEFAULT_S3_REGION),
            access_key_id: var("S3_ACCESS_KEY_ID")?,
            secret_access_key: var("S3_SECRET_ACCESS_KEY")?,
        },
        public_bucket,
        public_object: optional("S3_PUBLIC_OBJECT", DEFAULT_PUBLIC_OBJECT),
        state_bucket,
        state_object: optional("S3_STATE_OBJECT", DEFAULT_STATE_OBJECT),
        publish_interval: Duration::from_secs(publish_interval_secs),
    })
}
