# Bitcraft Discord bot

Keeps a public leaderboard of each player's highest known Total Inventory Value, publishes it as JSON to a Cloudflare R2 bucket where clients fetch it, and announces each new #1 in a Discord channel. It reads the value from the Discord presence that the Bitcraft desktop shares (see [Discord Rich Presence](../README.md#discord-rich-presence)).

## How it works

- **Consent.** The desktop shares presence only while the player has **Share activity on Discord and leaderboard** turned on. That setting is the player's consent to appear on the public leaderboard.
- **Who counts.** The bot follows one Discord server. It sees the presence of members who are online (not invisible) and whose Discord privacy settings share their activity. Only activities shared as the Bitcraft application count. A player's score only goes up: the leaderboard keeps the highest value seen, not the current one.
- **State.** The published object is the bot's only state. On startup the bot reads it back: a missing object starts an empty leaderboard, and any other failure stops startup, so an outage cannot replace the leaderboard with an empty one. Changes publish at most once a minute, right after a moderation command, and on shutdown. Run one instance per object.
- **Trust.** Scores are not verified. Anyone can share a presence as the Bitcraft application, since its ID is public. Moderators remove forged scores with `/leaderboard ban`.

## Published format

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
  ],
  "banned": ["1400000000000000456"]
}
```

- `entries` is sorted highest score first. Equal scores rank by who reached them first.
- `userId` is the Discord user ID, as a string.
- `name` is the player's server nickname, else their display name, else their username. The bot looks it up when it first sees the player and then daily. It is the user ID until the lookup succeeds.
- `bits` is the highest Total Inventory Value seen.
- `banned` lists players a moderator removed. Clients can ignore it.

The Bitcraft app's Leaderboard window reads this file from `LEADERBOARD_URL` in `apps/bitcraft/ui/index.html`.

## Announcements

When a score puts a different player in first place, the bot posts the new #1 and the top three with their scores to the announcement channel. A leader raising their own score is not announced, and neither is a ban that moves the second-place player up. When the bot connects, Discord sends every current presence at once; the bot announces at most one new leader for the whole batch, and none if the leader has not changed. Players appear as mentions, which Discord shows as their current server names, and the posts notify nobody.

## Moderation

- `/leaderboard ban player:` removes the player's entry and ignores their presence from then on.
- `/leaderboard unban player:` counts their presence again. Their old score is not restored.

By default, members with the **Timeout Members** permission can use the command. Server admins can change this under **Server Settings > Integrations**. Replies are visible only to the moderator.

To correct the leaderboard by hand, stop the bot, edit or delete the object, and start the bot again. Deleting the object resets the leaderboard, for example after a change to how inventory is valued.

## Setup

### Discord

1. In the [Discord Developer Portal](https://discord.com/developers/applications), open the Bitcraft application or create a separate one for the bot. Under **Bot**:
   - Copy the token. Reset it first if needed.
   - Turn on **Presence Intent**. Bots in fewer than 100 servers do not need Discord's review for it.
   - Turn off **Public Bot**, so that only the application's owners can add it to servers. If Discord refuses because the application has a default authorization link, set **Installation > Install Link** to **None** first.
2. Under **OAuth2 > URL Generator**, select the `bot` and `applications.commands` scopes and the **View Channels** and **Send Messages** bot permissions. Open the generated URL to add the bot to the server. Instead of granting the permissions server-wide, you can leave them out of the URL and allow them for the bot on the announcement channel only.
3. Turn on Developer Mode (**User Settings > Advanced**). Then right-click the server and choose **Copy Server ID**, and right-click the announcement channel and choose **Copy Channel ID**.

### Cloudflare R2

1. Create a bucket for the leaderboard alone. Public access exposes every object in a bucket.
2. Under **R2 > Manage API tokens**, create a token with **Object Read & Write** on that bucket. It provides an access key ID and secret access key. The S3 endpoint is `https://<ACCOUNT_ID>.r2.cloudflarestorage.com`.
3. Connect a custom domain to the bucket. The object's URL on that domain, such as `https://<domain>/leaderboard.json`, is the address clients fetch.
4. For clients that fetch from a browser or webview, add a CORS policy allowing `GET` from their origins.

### Run

```sh
cp apps/bitcraft/discord-bot/.env.example apps/bitcraft/discord-bot/.env   # fill it in
just bitcraft-discord-bot
```

| Variable | Required | Notes |
| --- | --- | --- |
| `DISCORD_BOT_TOKEN` | yes | The bot's token. |
| `DISCORD_GUILD_ID` | yes | The server whose members count. |
| `DISCORD_ANNOUNCE_CHANNEL_ID` | yes | The channel where new leaders are announced. |
| `BITCRAFT_DISCORD_APPLICATION_ID` | yes | The application the desktop shares presence as, the same ID the desktop build uses. |
| `S3_ENDPOINT` | yes | `https://<ACCOUNT_ID>.r2.cloudflarestorage.com` |
| `S3_BUCKET` | yes | |
| `S3_ACCESS_KEY_ID` | yes | |
| `S3_SECRET_ACCESS_KEY` | yes | |
| `S3_OBJECT` | no | Object key. Defaults to `leaderboard.json`. |
| `S3_REGION` | no | Defaults to `auto`, R2's region. Any S3-compatible bucket works with its own region. |
| `RUST_LOG` | no | Defaults to `info`. |
