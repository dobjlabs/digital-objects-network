---
name: bitcraft-market
description: Self-contained market trade desk — sets up AgentMail if needed, posts your offer, then auto-imports objects others send and replies with the object you offered.
disable-model-invocation: true
allowed-tools: Bash, Read, Write
---

# market

A self-contained market participant. On first run it sets up email — installs the
AgentMail CLI, registers the AgentMail MCP, and creates your inbox via a one-time
code sent to your human email. After that, each run announces your standing offer
(once), pulls new email, imports any `.dobj` someone sent to fulfill it, verifies
it, and replies with the object you promised.

Run it once by hand to finish setup — it asks two questions, then has you restart
Claude Code so the AgentMail MCP activates. After that, drive it on an interval:
`/loop 2m bitcraft-market`. "Reply automatically on receive" is just the loop
polling your inbox — pure outbound, no inbound webhook needed.

## Output rules

- Plain text. One status line per thing done. No markdown, no preamble, no commentary.
- Status line shapes (use verbatim):
  - `agentmail cli: installed`
  - `agentmail mcp: registered (restart Claude Code to activate)`
  - `agentmail inbox ready: <address>`
  - `setup complete — restart Claude Code, then run: /loop 2m bitcraft-market`
  - `posted offer #<tradeId>`
  - `no new trades for #<tradeId>`
  - `imported <fileName> (<status>)`
  - `replied to <sender> with <fileName>`
  - `fulfilled #<tradeId>`
  - `rejected <messageId>: expected <want>, got <class>/<status>`
  - `cannot fulfill #<tradeId>: no live <give> in inventory`
  - `cancelled`
- The two setup questions in step 0 are the ONLY questions this command asks.
  Output each on one line, end the turn, and wait for the reply. Before parsing
  any reply, if it is `cancel`/`quit`/`exit`/`q`/`nevermind` (case-insensitive,
  trimmed), output `cancelled` and stop.
- On any tool error, output the error verbatim on one line and stop.

## Config

`~/.dobj/market.json` holds the trade terms and the Discord webhook:

```json
{
  "tradeId": "t1",
  "give": "Wood",
  "want": "Log",
  "contactEmail": "",
  "agentmailInboxId": "",
  "discordWebhookUrl": "https://discord.com/api/webhooks/1508996202106978484/fHY5dHNXnU0y1tgl3mFcRul8kMBm2KOgRl1I5FK0Rxtxjdo_6jUSwlRha3fiNEFrXFAD"
}
```

If the file does NOT exist, Write that template (leave `contactEmail` and
`agentmailInboxId` empty — step 0 fills them) and continue to step 0.

`give`/`want` are bare class names (count 1 for now). The AgentMail API key is
stored separately in `~/.dobj/agentmail.key` (mode 600), written by step 0. State
markers live in `~/.dobj/` so the offer posts once and no email is fulfilled twice.

## AgentMail

Email operations run through your **AgentMail MCP tools** (list messages, get a
message + attachment, reply with attachment). Those authenticate via OAuth and
only become available after a Claude Code **restart** following `claude mcp add` —
so first-run setup registers the server, then asks you to restart. The **AgentMail
CLI** is used only in step 0 for sign-up + OTP verify (which the MCP can't do) and
to read your inbox address. A REST fallback is at the bottom.

## Steps

Step 0 is one-time + interactive. Steps 1–5 are idempotent and loop-safe.

### 0. Ensure AgentMail is set up

Each substep is idempotent. The interactive sign-up (0c) is skipped once you're
set up; the MCP-availability gate (0d) runs every time.

**0a. CLI** — install if missing:

```bash
command -v agentmail >/dev/null 2>&1 || npm install -g agentmail-cli
```

On success output `agentmail cli: installed`. If the global install fails on
permissions, use `npx -y -p agentmail-cli agentmail …` in place of bare
`agentmail` for every CLI call below.

**0b. MCP** — register for Claude Code if not already present:

```bash
if command -v claude >/dev/null 2>&1; then
  claude mcp list 2>/dev/null | grep -qi agentmail \
    || claude mcp add --transport http agentmail https://mcp.agentmail.to/mcp
fi
```

If you added it, output `agentmail mcp: registered (restart Claude Code to activate)`.
(Its OAuth runs on first MCP use; the command keeps using the CLI so you can
proceed now without restarting.)

**0c. Inbox + key** — only if `~/.dobj/agentmail.key` is missing OR `agentmailInboxId` in config is empty:

1. Output exactly `your email (for the AgentMail sign-up code)?` and END THE TURN. Wait.
2. On the reply (an email address), sign up — this creates your agent inbox and
   returns an `api_key` + `inbox_id`:

   ```bash
   agentmail agent sign-up --human-email "<reply>" --username "bitcraft-trader"
   ```

   Parse `api_key` and `inbox_id` from the output, then store the key:

   ```bash
   umask 077; printf '%s' "<api_key>" > "$HOME/.dobj/agentmail.key"
   ```

   If the username is already taken, retry with a numeric suffix (`bitcraft-trader-2`, …).
3. Output exactly `6-digit code sent to your email?` and END THE TURN. Wait.
4. On the reply (the code), verify:

   ```bash
   AGENTMAIL_API_KEY="$(cat "$HOME/.dobj/agentmail.key")" agentmail agent verify --otp-code "<reply>"
   ```
5. Resolve the inbox address (`AGENTMAIL_API_KEY="$(cat "$HOME/.dobj/agentmail.key")" agentmail inboxes list`,
   or `inboxes get --inbox-id <inbox_id>`; run `agentmail inboxes --help` if unsure).
   Write `agentmailInboxId` (the inbox id) and `contactEmail` (the inbox's email
   address) into `~/.dobj/market.json`.
6. Output `agentmail inbox ready: <address>`.

**0d. MCP gate** — if your AgentMail MCP tools are NOT available in this session
(you just registered the server in 0b and haven't restarted yet), output
`setup complete — restart Claude Code, then run: /loop 2m bitcraft-market` and
STOP. Otherwise continue to step 1.

### 1. Load config
Read `~/.dobj/market.json`. If missing → write the template (see Config) and go to
step 0. Otherwise hold `tradeId`, `give`, `want`, `contactEmail`,
`agentmailInboxId`, `discordWebhookUrl`.

### 2. Announce the offer once
If `~/.dobj/.market-posted-<tradeId>` does NOT exist, post to the Discord webhook
(fill the variables from config):

```bash
TRADE_ID="t1"; GIVE="Wood"; WANT="Log"; EMAIL="<contactEmail>"
WEBHOOK="<discordWebhookUrl>"
curl -fsS -X POST -H "Content-Type: application/json" "$WEBHOOK" \
  -d "$(printf '{"content":"**OFFER #%s** — give 1 %s, want 1 %s. Send the %s .dobj to %s with #%s in the subject."}' \
        "$TRADE_ID" "$GIVE" "$WANT" "$WANT" "$EMAIL" "$TRADE_ID")" \
  && touch "$HOME/.dobj/.market-posted-$TRADE_ID"
```

On success output `posted offer #<tradeId>`. If the marker already exists, skip silently.

### 3. Pull new trade email
Using your **AgentMail MCP tools**, list UNREAD inbox messages whose subject
contains `#<tradeId>` and whose message id is NOT already a line in
`~/.dobj/.market-processed-<tradeId>.log` (create it empty if absent). If there
are none, output `no new trades for #<tradeId>` and stop.

### 4. For each new trade message
a. Download its first `.dobj` attachment to `/tmp/market-<messageId>.dobj`.
b. Call `import_object_file` with `path="/tmp/market-<messageId>.dobj"`. If it
   errors (duplicate, already-spent, bad class), output the error verbatim, append
   `<messageId>` to the processed log, and move on (do NOT reply).
c. Verify with `inspect_object` on the returned `fileName`. If class != `<want>`
   OR status != `live`: output `rejected <messageId>: expected <want>, got
   <class>/<status>`, append `<messageId>` to the processed log, and do NOT reply.
   Else output `imported <fileName> (live)`.
d. Pick the object to give: `list_inventory` → first object with class `<give>`
   and status `live`. If none → output `cannot fulfill #<tradeId>: no live <give>
   in inventory` and stop (leave the message unprocessed so a later tick retries).
e. Resolve its path: `get_objects_dir`, joined with the give object's `fileName`.
f. Reply to the message (your AgentMail MCP reply tool) with that `.dobj`
   attached, body e.g. `Here is your <give> for #<tradeId>.` Output
   `replied to <sender> with <fileName>`.
g. Append `<messageId>` to `~/.dobj/.market-processed-<tradeId>.log`.
h. Output `fulfilled #<tradeId>`.

### 5. Stop
End the turn. The next loop tick repeats from step 1 (step 0 self-skips).

## Guardrails

- Only ever act on YOUR `<tradeId>` (subject match). Ignore every other subject.
- One fulfillment per inbound message — the processed log enforces it; never reply twice.
- Never reply unless the imported object is a `live` `<want>`.
- `~/.dobj/agentmail.key` and the webhook URL are secrets — never echo them or post them anywhere.
- After you reply you still hold a local copy of the given object (no nullify-on-send yet) — expected for now.
- Re-arm / change terms: edit `~/.dobj/market.json`, then `rm ~/.dobj/.market-posted-<tradeId> ~/.dobj/.market-processed-<tradeId>.log`.

## AgentMail REST reference

With `Authorization: Bearer $AGENTMAIL_API_KEY` (key in `~/.dobj/agentmail.key`).
The CLI handles the base URL; for raw `curl` confirm it from the AgentMail dashboard:

- List unread: `GET /inboxes/{inboxId}/messages?labels=["unread"]&limit=20`
- Get message + attachments: `GET /inboxes/{inboxId}/messages/{messageId}`
- Get attachment bytes: `GET /inboxes/{inboxId}/messages/{messageId}/attachments/{attachmentId}`
- Reply with file: `POST /inboxes/{inboxId}/messages/{messageId}/reply` body
  `{"text":"...","attachments":[{"filename":"<name>.dobj","content_type":"application/json","content":"<base64>"}]}`
- Sign up / verify (bootstrap): `agentmail agent sign-up --human-email … --username …` then `agentmail agent verify --otp-code …`
