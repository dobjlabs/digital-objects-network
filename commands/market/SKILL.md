---
name: bitcraft-market
description: Run a market trade desk — post your offer, then auto-import objects others send and reply with the object you offered.
disable-model-invocation: true
allowed-tools: Bash, Read, Write
---

# market

Be a market participant. On each run: announce a standing offer (once), pull new
email, import any `.dobj` someone sent to fulfill your offer, verify it, and
reply with the object you promised. Designed to run on an interval —
`/loop 2m bitcraft-market` — so "reply automatically on receive" is just the
loop polling your inbox (no inbound webhook; the laptop only makes outbound
calls).

## Output rules

- Plain text. One status line per thing done. No markdown, no preamble, no commentary.
- Status line shapes (use verbatim):
  - `market: created ~/.dobj/market.json — fill it in and re-run`
  - `posted offer #<tradeId>`
  - `no new trades for #<tradeId>`
  - `imported <fileName> (<status>)`
  - `replied to <sender> with <fileName>`
  - `fulfilled #<tradeId>`
  - `rejected <messageId>: expected <want>, got <class>/<status>`
  - `cannot fulfill #<tradeId>: no live <give> in inventory`
- On any tool error, output the error verbatim on one line and stop.

## Config

Read `~/.dobj/market.json` with the Read tool:

```json
{
  "tradeId": "t1",
  "give": "Wood",
  "want": "Log",
  "contactEmail": "you@agentmail.to",
  "agentmailInboxId": "you@agentmail.to",
  "discordWebhookUrl": "https://discord.com/api/webhooks/1508996202106978484/fHY5dHNXnU0y1tgl3mFcRul8kMBm2KOgRl1I5FK0Rxtxjdo_6jUSwlRha3fiNEFrXFAD"
}
```

If the file does NOT exist, Write exactly that template to `~/.dobj/market.json`,
output `market: created ~/.dobj/market.json — fill it in and re-run`, and stop.

`give`/`want` are bare class names (count 1 for now). `contactEmail` is your
AgentMail inbox address; `agentmailInboxId` is its inbox id (often the same
string). State markers live in `~/.dobj/` so the offer posts once and no email
is fulfilled twice.

## Email integration

Prefer your **AgentMail MCP tools** if registered (list messages, get a message

- attachment, reply with attachment). Otherwise drive AgentMail's REST API via
  `curl` with `Authorization: Bearer $AGENTMAIL_API_KEY` — see "AgentMail REST
  reference" at the bottom. Either way you need: list unread, download an
  attachment to a file, and reply with a file attached.

## Steps

Run top to bottom each invocation. Everything is idempotent across ticks.

### 1. Load config

Read `~/.dobj/market.json`. If missing → write the template + stop (see Config).
Otherwise hold `tradeId`, `give`, `want`, `contactEmail`, `agentmailInboxId`,
`discordWebhookUrl`.

### 2. Announce the offer once

If `~/.dobj/.market-posted-<tradeId>` does NOT exist, post to the Discord webhook
(fill the variables from config):

```bash
TRADE_ID="t1"; GIVE="Wood"; WANT="Log"; EMAIL="you@agentmail.to"
WEBHOOK="https://discord.com/api/webhooks/1508996202106978484/fHY5dHNXnU0y1tgl3mFcRul8kMBm2KOgRl1I5FK0Rxtxjdo_6jUSwlRha3fiNEFrXFAD"
curl -fsS -X POST -H "Content-Type: application/json" "$WEBHOOK" \
  -d "$(printf '{"content":"**OFFER #%s** — give 1 %s, want 1 %s. Send the %s .dobj to %s with #%s in the subject."}' \
        "$TRADE_ID" "$GIVE" "$WANT" "$WANT" "$EMAIL" "$TRADE_ID")" \
  && touch "$HOME/.dobj/.market-posted-$TRADE_ID"
```

On success output `posted offer #<tradeId>`. If the marker already exists, skip silently.

### 3. Pull new trade email

List UNREAD inbox messages whose subject contains `#<tradeId>` and whose message
id is NOT already a line in `~/.dobj/.market-processed-<tradeId>.log`
(create the file empty if absent). If there are none, output
`no new trades for #<tradeId>` and stop.

### 4. For each new trade message

a. Download its first `.dobj` attachment to `/tmp/market-<messageId>.dobj`.
b. Call `import_object_file` with `path="/tmp/market-<messageId>.dobj"`.

- If it errors (duplicate, already-spent, bad class, parse): output the error
  verbatim, append `<messageId>` to the processed log, and move on (do NOT reply).
  c. Verify with `inspect_object` on the returned `fileName`:
- If class != `<want>` OR status != `live`: output
  `rejected <messageId>: expected <want>, got <class>/<status>`, append
  `<messageId>` to the processed log, and do NOT reply.
- Else output `imported <fileName> (live)`.
  d. Pick the object to give: call `list_inventory`, find one with class `<give>`
  and status `live`. If none → output
  `cannot fulfill #<tradeId>: no live <give> in inventory` and stop. Leave the
  message UNprocessed so a later tick can retry once you have one.
  e. Resolve its path: call `get_objects_dir`, join with the give object's `fileName`.
  f. Reply to the message (AgentMail reply) with that `.dobj` attached, body e.g.
  `Here is your <give> for #<tradeId>.` Output `replied to <sender> with <fileName>`.
  g. Append `<messageId>` to `~/.dobj/.market-processed-<tradeId>.log`.
  h. Output `fulfilled #<tradeId>` (and optionally post the same to the webhook).

### 5. Stop

End the turn. The next loop tick repeats from step 1.

## Guardrails

- Only ever act on YOUR `<tradeId>` (subject match). Ignore every other subject.
- One fulfillment per inbound message — the processed log enforces it; never reply twice.
- Never reply unless the imported object is a `live` `<want>`.
- After you reply, you still hold a local copy of the given object (no
  nullify-on-send yet) — expected for now.
- To re-arm a finished trade or change terms: edit `~/.dobj/market.json` and
  `rm ~/.dobj/.market-posted-<tradeId> ~/.dobj/.market-processed-<tradeId>.log`.

## AgentMail REST reference

If using REST instead of the MCP tools, base URL is your AgentMail API root
(confirm in the AgentMail dashboard); paths below are relative, auth is
`Authorization: Bearer $AGENTMAIL_API_KEY`:

- List unread: `GET /inboxes/{inboxId}/messages?labels=["unread"]&limit=20`
- Get message + attachment metadata: `GET /inboxes/{inboxId}/messages/{messageId}`
- Get attachment bytes: `GET /inboxes/{inboxId}/messages/{messageId}/attachments/{attachmentId}`
- Reply with file: `POST /inboxes/{inboxId}/messages/{messageId}/reply` body:
  `{"text":"...","attachments":[{"filename":"<name>.dobj","content_type":"application/json","content":"<base64 of the .dobj>"}]}`
- Send (non-reply): `POST /inboxes/{inboxId}/messages/send` with `to, subject, text, attachments`.
