use anyhow::{Context, Result};
use twilight_model::id::{
    Id,
    marker::{ApplicationMarker, GuildMarker},
};

use crate::store::StoreConfig;

const DEFAULT_S3_REGION: &str = "auto";
const DEFAULT_S3_OBJECT: &str = "leaderboard.json";

pub struct Config {
    pub discord_token: String,
    /// The one server whose members' presence counts.
    pub guild_id: Id<GuildMarker>,
    /// The application the desktop host shares presence as.
    pub bitcraft_application_id: Id<ApplicationMarker>,
    pub store: StoreConfig,
}

pub fn load_config() -> Result<Config> {
    let _ = dotenvy::from_filename("apps/bitcraft/discord-bot/.env");
    let var = |name: &str| dotenvy::var(name).with_context(|| format!("{name} must be set"));

    Ok(Config {
        discord_token: var("DISCORD_BOT_TOKEN")?,
        guild_id: var("DISCORD_GUILD_ID")?
            .parse()
            .context("DISCORD_GUILD_ID must be a numeric Discord server ID")?,
        bitcraft_application_id: var("BITCRAFT_DISCORD_APPLICATION_ID")?
            .parse()
            .context("BITCRAFT_DISCORD_APPLICATION_ID must be a numeric Discord application ID")?,
        store: StoreConfig {
            endpoint: var("S3_ENDPOINT")?
                .parse()
                .context("S3_ENDPOINT must be a URL")?,
            bucket: var("S3_BUCKET")?,
            region: dotenvy::var("S3_REGION").unwrap_or_else(|_| DEFAULT_S3_REGION.into()),
            access_key_id: var("S3_ACCESS_KEY_ID")?,
            secret_access_key: var("S3_SECRET_ACCESS_KEY")?,
            object: dotenvy::var("S3_OBJECT").unwrap_or_else(|_| DEFAULT_S3_OBJECT.into()),
        },
    })
}
