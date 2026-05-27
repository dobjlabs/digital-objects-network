---
name: bitcraft-market
description: Market trade desk for bitcraft. `market setup` provisions an AgentMail inbox; `market post` announces your offer to Discord; `market check` pulls new email once, imports any object sent to you, and replies with the object you offered. Triggers on "market", "market setup", "market post", "market check".
argument-hint: [setup|post|check]
arguments: action
disable-model-invocation: true
allowed-tools: Bash, Read, Write
---

# market

Trade objects by email, driven manually — one subcommand per run, no loop. Pick the
run from `$action` (the first argument):

- `setup` → **Setup**: provision your AgentMail inbox (one-time).
- `post`  → **Post**: announce your standing offer to Discord.
- `check` → **Check**: pull new email once, import any object sent to you, reply with yours.
- empty or anything else → output `usage: market [setup|post|check]` and stop.

All email goes through the AgentMail REST API via the committed helper
`setup_inbox.py` (key in `~/.dobj/agentmail.key`) — no MCP, no OAuth, no loop. Run
the helper as `python3 "${CLAUDE_SKILL_DIR}/setup_inbox.py" <sub> …`; subs:
`signup`, `verify`, `sync-config`, `announce`, `poll`, `reply`, `mark-processed`.

## Output rules

- Plain text, one status line per action. No markdown, no preamble, no commentary.
- The helper prints `STATUS=...` lines — branch on the last one, and never print the key.
- The Setup prompts (email, username, OTP) are the only questions this command asks.
  Output each on one line, end the turn, wait. Before parsing any reply, if it is
  `cancel`/`quit`/`exit`/`q`/`nevermind` (case-insensitive, trimmed), output
  `cancelled` and stop.
- On any tool error, output the error verbatim on one line and stop.
- Status line shapes (verbatim): `agentmail inbox ready: <address>`,
  `already set up: <address>`, `username <name> is taken — pick another`,
  `run market setup first`, `posted offer #<tradeId>`, `no new trades for #<tradeId>`,
  `imported <fileName> (<status>)`, `replied to <sender> with <fileName>`,
  `fulfilled #<tradeId>`, `rejected <messageId>: expected <want>, got <class>/<status>`,
  `cannot fulfill #<tradeId>: no live <give> in inventory`, `cancelled`,
  `usage: market [setup|post|check]`.

## Config

`~/.dobj/market.json`:

```json
{
  "tradeId": "t1",
  "give": "Iron",
  "want": "Copper",
  "contactEmail": "",
  "agentmailInboxId": "",
  "discordWebhookUrl": "https://discord.com/api/webhooks/1508996202106978484/fHY5dHNXnU0y1tgl3mFcRul8kMBm2KOgRl1I5FK0Rxtxjdo_6jUSwlRha3fiNEFrXFAD"
}
```

`give`/`want` are episode-1 class names — the offer gives 1 `give`, wants 1 `want`
(`Iron`/`Copper` here). The AgentMail key lives in `~/.dobj/agentmail.key` (mode 600),
written by Setup. State markers live in `~/.dobj/` so the offer posts once and no
email is fulfilled twice.

## Setup  — run when `$action` is `setup`

1. If `~/.dobj/market.json` is missing, Write the Config template above.
2. If `~/.dobj/agentmail.key` exists AND `agentmailInboxId` in config is non-empty →
   output `already set up: <agentmailInboxId>` and stop.
3. Output exactly `your email (for the AgentMail sign-up code)?` and END THE TURN. Wait; hold the reply as `<email>`.
4. Output exactly `pick a username for your trade inbox (becomes <name>@agentmail.to)?` and END THE TURN. Wait; hold the reply as `<username>`. (Usernames are global — each user needs a unique one.)
5. Run:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/setup_inbox.py" signup "<email>" "<username>"
   ```

   `OK`/`ALREADY` → continue. `TAKEN` → output `username <username> is taken — pick another`
   and go back to step 4. Anything else → output the `STATUS=` line and stop.
6. *(Optional — only needed to email addresses other than your sign-up email. An
   unverified inbox can only send to the sign-up address, so verify if your trading
   counterpart uses a different one.)* Output exactly
   `6-digit code emailed to <email> (paste it, or 'skip')?` and END THE TURN. Wait.
   If the reply is `skip` → continue. Otherwise:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/setup_inbox.py" verify "<reply>"
   ```

   `STATUS=VERIFIED` → continue; anything else → output the `STATUS=` line and stop.
7. Output `agentmail inbox ready: <address>` (`<address>` = `agentmailInboxId` from config).

## Post  — run when `$action` is `post`

1. If `~/.dobj/agentmail.key` is missing OR `agentmailInboxId` is empty → output
   `run market setup first` and stop.
2. Run `python3 "${CLAUDE_SKILL_DIR}/setup_inbox.py" sync-config`, then read `tradeId`
   from `~/.dobj/market.json`.
3. Run:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/setup_inbox.py" announce "<tradeId>"
   ```

   `STATUS=OK` or `POSTED` → output `posted offer #<tradeId>`. Anything else → output
   the `STATUS=` line and stop.

## Check  — run when `$action` is `check`

One pass over the inbox; no loop.

1. If `~/.dobj/agentmail.key` is missing OR `agentmailInboxId` is empty → output
   `run market setup first` and stop.
2. Run `python3 "${CLAUDE_SKILL_DIR}/setup_inbox.py" sync-config`, then read `tradeId`,
   `give`, `want` from `~/.dobj/market.json`.
3. Run:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/setup_inbox.py" poll "<tradeId>"
   ```

   If the last line is `STATUS=NONE`, output `no new trades for #<tradeId>` and stop.
   Otherwise each `TRADE {…}` line is a job, shaped `{messageId, from, subject,
   attachmentPath}` — the `.dobj` is already downloaded to `attachmentPath`.
4. For each `TRADE` line:
   a. Call `import_object_file` with `path` = the line's `attachmentPath`. If it errors
      (duplicate, already-spent, bad class), output the error verbatim, run
      `python3 "${CLAUDE_SKILL_DIR}/setup_inbox.py" mark-processed "<tradeId>" "<messageId>"`,
      and move to the next line (do NOT reply).
   b. Verify with `inspect_object` on the returned `fileName`. If its class name !=
      `<want>` OR status != `live`: output `rejected <messageId>: expected <want>, got
      <class>/<status>`, run `mark-processed` as above, do NOT reply. Else output
      `imported <fileName> (live)`.
   c. Pick the object to give: `list_inventory` → first object whose class name is
      `<give>` and status is `live`. If none → output `cannot fulfill #<tradeId>: no
      live <give> in inventory` and stop (do NOT mark processed — retry on a later run).
   d. Resolve its path: `get_objects_dir`, joined with the give object's `fileName`.
   e. Reply:

      ```bash
      python3 "${CLAUDE_SKILL_DIR}/setup_inbox.py" reply "<messageId>" "<givePath>" "Here is your <give> for #<tradeId>."
      ```

      `STATUS=OK` → output `replied to <from> with <fileName>`. Anything else → output
      the `STATUS=` line and stop.
   f. Run `python3 "${CLAUDE_SKILL_DIR}/setup_inbox.py" mark-processed "<tradeId>" "<messageId>"`,
      then output `fulfilled #<tradeId>`.

## Guardrails

- Only act on YOUR `<tradeId>` — `poll` filters by subject, so you only see matching mail.
- One fulfillment per message — `mark-processed` plus `poll`'s dedupe enforce it; never reply twice.
- Never reply unless the imported object is a `live` `<want>`.
- `~/.dobj/agentmail.key` and the webhook URL are secrets — never echo them or post them anywhere.
- After you reply you still hold a local copy of the given object (no nullify-on-send yet) — expected for now.
- Re-arm / change terms: edit `~/.dobj/market.json`, then `rm ~/.dobj/.market-posted-<tradeId> ~/.dobj/.market-processed-<tradeId>.log`.
