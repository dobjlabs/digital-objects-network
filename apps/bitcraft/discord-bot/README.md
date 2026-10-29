# Bitcraft Discord bot

Keeps each player's highest known Total Inventory Value, read from the Discord presence that the Bitcraft desktop shares (see [Discord Rich Presence](../README.md#discord-rich-presence)). On a fixed schedule it publishes the top 50 as JSON to a Cloudflare R2 bucket, where clients fetch it, and posts them to a Discord channel. Players can check their place on it with `/rank`.

## How it works

- **Consent.** The desktop shares presence only while the player has **Share my Bitcraft scores on Discord** turned on in Settings. That setting is the player's consent to appear on the public leaderboard.
- **Who counts.** The bot follows one Discord server. It sees the presence of members who are online (not invisible) and whose Discord privacy settings share their activity. Only activities shared as the Bitcraft application count. A player's score only goes up: the bot keeps the highest value seen, not the current one.
- **Store.** Every player's score lives in a private bucket. The bot saves changes to it at most once a minute and on shutdown. On startup it reads the store back: a missing object starts empty, and any other failure stops startup, so an outage cannot replace the scores with nothing. Online players' scores come back from their presence after a restart, so a crash loses at most a minute of other changes, such as a peak a player has since spent below. Run one instance per store. To reset the leaderboard, for example after a change to how inventory is valued, stop the bot and delete the private object.
- **Leaderboard.** Every `PUBLISH_INTERVAL_SECS` (3 hours by default) the bot takes the top 50 from the store, publishes them to a public bucket and posts them to the leaderboard channel. Publications fall on multiples of the interval in UTC, such as 00:00, 03:00 and 06:00 for 3 hours, so restarts neither add publications nor move them. Nothing else publishes, so the public file, the posts and `/rank` all show the scores as of the last publication. If the upload fails, that publication is skipped: nothing is posted and `/rank` keeps the previous one.
- **Trust.** Scores are not verified. Anyone can share a presence as the Bitcraft application, since its ID is public, so a score can be forged.

## Published format

The public file holds the top 50:

```json
{
  "updatedAt": "2026-10-29T12:00:00Z",
  "entries": [
    {
      "userId": "1400000000000000123",
      "name": "Ada",
      "bits": 31.12,
      "achievedAt": "2026-10-29T11:58:00Z"
    }
  ]
}
```

- `entries` is sorted highest score first. Equal scores rank by who reached them first.
- `userId` is the Discord user ID, as a string.
- `name` is the player's server nickname, else their display name, else their username. The bot looks it up when it first sees the player and then daily. It is the user ID until the lookup succeeds.
- `bits` is the highest Total Inventory Value seen.
- `updatedAt` is when the bot took this top 50 from the store.

The private store has the same format with every player.

The Bitcraft app's Leaderboard window reads the public file from `LEADERBOARD_URL` in `apps/bitcraft/ui/index.html`.

## Leaderboard posts

Each publication is also posted to the leaderboard channel: a "Bitcraft leaderboard" heading, then one line per player with their place, a mention and their score. Fifty lines exceed Discord's 2,000-character limit for a message, so a post may be split across two messages between lines. Mentions show players' current server names and notify nobody. Nothing is posted while nobody has a score.

## Commands

- `/rank` shows your place and score on the last published leaderboard, and how long ago it was published, in a Discord timestamp each reader sees in their own time. A player outside the top 50 is told so, with how players get counted. After a restart it answers from the public file until the next publication.

It replies in the channel for everyone to see, with mentions that notify nobody.

## Setup

### Discord

1. In the [Discord Developer Portal](https://discord.com/developers/applications), open the Bitcraft application or create a separate one for the bot. Under **Bot**:
   - Copy the token. Reset it first if needed.
   - Turn on **Presence Intent**. Bots in fewer than 100 servers do not need Discord's review for it.
   - Turn off **Public Bot**, so that only the application's owners can add it to servers. If Discord refuses because the application has a default authorization link, set **Installation > Install Link** to **None** first.
2. Under **OAuth2 > URL Generator**, select the `bot` and `applications.commands` scopes and the **View Channels** and **Send Messages** bot permissions. Open the generated URL to add the bot to the server. Instead of granting the permissions server-wide, you can leave them out of the URL and allow them for the bot on the leaderboard channel only.
3. Turn on Developer Mode (**User Settings > Advanced**). Then right-click the server and choose **Copy Server ID**, and right-click the leaderboard channel and choose **Copy Channel ID**.

### Cloudflare R2

1. Create two buckets: a public one for the leaderboard alone, since public access exposes every object in a bucket, and a private one for the store. Leave public access off on the private bucket: no custom domain and no public development URL.
2. Under **R2 > Manage API tokens**, create an **Account API token** with **Object Read & Write** on both buckets. An account token keeps working whoever is a member of the account. It provides an access key ID and secret access key. The S3 endpoint is `https://<ACCOUNT_ID>.r2.cloudflarestorage.com`.
3. Connect a custom domain to the public bucket. The object's URL on that domain, such as `https://<domain>/leaderboard.json`, is the address clients fetch.
4. For clients that fetch from a browser or webview, add a CORS policy to the public bucket allowing `GET` from their origins.

### Run

```sh
cp apps/bitcraft/discord-bot/.env.example apps/bitcraft/discord-bot/.env   # fill it in
just bitcraft-discord-bot
```

| Variable | Required | Notes |
| --- | --- | --- |
| `DISCORD_BOT_TOKEN` | yes | The bot's token. |
| `DISCORD_GUILD_ID` | yes | The server whose members count. |
| `DISCORD_LEADERBOARD_CHANNEL_ID` | yes | The channel the leaderboard is posted to. |
| `BITCRAFT_DISCORD_APPLICATION_ID` | yes | The application the desktop shares presence as, the same ID the desktop build uses. |
| `S3_ENDPOINT` | yes | `https://<ACCOUNT_ID>.r2.cloudflarestorage.com` |
| `S3_PUBLIC_BUCKET` | yes | The public bucket, for the leaderboard. |
| `S3_STATE_BUCKET` | yes | The private bucket, for the store. Must differ from `S3_PUBLIC_BUCKET`. |
| `S3_ACCESS_KEY_ID` | yes | |
| `S3_SECRET_ACCESS_KEY` | yes | |
| `S3_PUBLIC_OBJECT` | no | Defaults to `leaderboard.json`. |
| `S3_STATE_OBJECT` | no | Defaults to `state.json`. |
| `PUBLISH_INTERVAL_SECS` | no | Time between publications. Defaults to 10800 (3 hours). |
| `S3_REGION` | no | Defaults to `auto`, R2's region. Any S3-compatible bucket works with its own region. |
| `RUST_LOG` | no | Defaults to `info`. |
