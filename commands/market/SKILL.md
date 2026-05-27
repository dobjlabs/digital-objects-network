---
name: bitcraft-market
description: Market trade desk — provision an email for your agent, post/read offers, and fulfill trades by email.
when_to_use: For the bitcraft market order board. `market setup` provisions an email for your agent; `market post` posts your offer; `market check` pulls email once and replies with your object; `market list` reads open orders. Triggers on "market", "market setup", "market post", "market check", "market list".
argument-hint: [setup|post|check|list]
arguments: action
disable-model-invocation: true
allowed-tools: Bash, Read, Write
---

# market

Trade objects by email, driven manually — one subcommand per run, no loop. Pick the
run from `$action` (the first argument):

- `setup` → **Setup**: provision your AgentMail inbox (one-time).
- `post`  → **Post**: post your standing offer to the market board.
- `check` → **Check**: pull new email once, import any object sent to you, reply with yours.
- `list`  → **List**: read open orders from the market board.
- empty or anything else → output `usage: market [setup|post|check|list]` and stop.

The committed helper `market.py` wraps every AgentMail / market-board / config
operation as a deterministic subcommand (AgentMail key in `~/.dobj/agentmail.key`;
the board lives at `marketApiUrl`) — no MCP, no OAuth, no loop. Run it as
`python3 "${CLAUDE_SKILL_DIR}/market.py" <sub> …`; subs: `signup`, `verify`,
`sync-config`, `set-offer`, `announce`, `list-orders`, `close-order`, `poll`, `reply`, `mark-processed`.

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
  `imported <n> <want> (live)`, `replied to <sender> with <giveQty> <give>`,
  `fulfilled #<tradeId>`, `rejected <messageId>: expected <wantQty> <want>, got <n> <class>/<status>`,
  `cannot fulfill #<tradeId>: no live <give> in inventory`, `no open orders`,
  `cancelled`, `usage: market [setup|post|check|list]`.

## Config

`~/.dobj/market.json`:

```json
{
  "tradeId": "t1",
  "give": "Iron",
  "giveQty": 1,
  "want": "Copper",
  "wantQty": 1,
  "contactEmail": "",
  "agentmailInboxId": "",
  "marketApiUrl": "http://localhost:8088"
}
```

`give`/`want` are episode-1 class names and `giveQty`/`wantQty` their counts — the
offer gives `giveQty` `give` for `wantQty` `want` (1 Iron for 1 Copper here), and
`market post` can override them per run. `marketApiUrl` points at your market board
server (default `http://localhost:8088` — run it with `python3 market/server.py`).
The AgentMail key lives in `~/.dobj/agentmail.key` (mode 600), written by Setup. The
`~/.dobj/.market-processed-<tradeId>.log` marker ensures no email is fulfilled twice.

## Setup  — run when `$action` is `setup`

1. If `~/.dobj/market.json` is missing, Write the Config template above.
2. If `~/.dobj/agentmail.key` exists AND `agentmailInboxId` in config is non-empty →
   output `already set up: <agentmailInboxId>` and stop.
3. Output exactly `your email (for the AgentMail sign-up code)?` and END THE TURN. Wait; hold the reply as `<email>`.
4. Output exactly `pick a username for your trade inbox (becomes <name>@agentmail.to)?` and END THE TURN. Wait; hold the reply as `<username>`. (Usernames are global — each user needs a unique one.)
5. Run:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/market.py" signup "<email>" "<username>"
   ```

   `OK`/`ALREADY` → continue. `TAKEN` → output `username <username> is taken — pick another`
   and go back to step 4. Anything else → output the `STATUS=` line and stop.
6. *(Optional — only needed to email addresses other than your sign-up email. An
   unverified inbox can only send to the sign-up address, so verify if your trading
   counterpart uses a different one.)* Output exactly
   `6-digit code emailed to <email> (paste it, or 'skip')?` and END THE TURN. Wait.
   If the reply is `skip` → continue. Otherwise:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/market.py" verify "<reply>"
   ```

   `STATUS=VERIFIED` → continue; anything else → output the `STATUS=` line and stop.
7. Output `agentmail inbox ready: <address>` (`<address>` = `agentmailInboxId` from config).

## Post  — run when `$action` is `post`

The offer is configurable inline — the user may say e.g. `market post 5 Iron for 2 Copper`,
`market post give 2 Iron want 1 Copper`, or just `market post` (reuse the saved offer).

1. If `~/.dobj/agentmail.key` is missing OR `agentmailInboxId` is empty → output
   `run market setup first` and stop.
2. If the user specified an offer, parse it into `<giveQty> <give> <wantQty> <want>`
   (positive-integer quantities; `give`/`want` are episode-1 class names) and save it:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/market.py" set-offer "<giveQty>" "<give>" "<wantQty>" "<want>"
   ```

   `STATUS=OK` → continue; `BADOFFER`/`USAGE` → output it and stop. If the user gave no
   offer, skip this step — `announce` uses the saved offer.
3. Run `python3 "${CLAUDE_SKILL_DIR}/market.py" sync-config`, then read `tradeId` from `~/.dobj/market.json`.
4. Run:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/market.py" announce "<tradeId>"
   ```

   `STATUS=OK` → output `posted offer #<tradeId>` (the helper echoes the quantities).
   Anything else → output the `STATUS=` line and stop.

## Check  — run when `$action` is `check`

One pass over the inbox; no loop.

1. If `~/.dobj/agentmail.key` is missing OR `agentmailInboxId` is empty → output
   `run market setup first` and stop.
2. Run `python3 "${CLAUDE_SKILL_DIR}/market.py" sync-config`, then read `tradeId`,
   `give`, `giveQty`, `want`, `wantQty` from `~/.dobj/market.json` (quantities default to 1).
3. Run:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/market.py" poll "<tradeId>"
   ```

   If the last line is `STATUS=NONE`, output `no new trades for #<tradeId>` and stop.
   Otherwise each `TRADE {…}` line is a job, shaped `{messageId, from, subject,
   attachmentPaths}` — the sender's `.dobj` files are already downloaded to the paths
   in `attachmentPaths`.
4. For each `TRADE` line:
   a. Import every path in `attachmentPaths`: call `import_object_file` on each. If any
      errors (duplicate, already-spent, bad class), output the error verbatim, run
      `python3 "${CLAUDE_SKILL_DIR}/market.py" mark-processed "<tradeId>" "<messageId>"`,
      and move to the next TRADE (do NOT reply).
   b. Verify each imported `fileName` with `inspect_object`. You need `<wantQty>` of them
      to be class `<want>` AND status `live`. If fewer: output
      `rejected <messageId>: expected <wantQty> <want>, got <n> <class>/<status>`, run
      `mark-processed` as above, do NOT reply. Else output `imported <n> <want> (live)`.
   c. Pick `<giveQty>` objects to give: `list_inventory` → objects whose class name is
      `<give>` and status `live`. If fewer than `<giveQty>` → output `cannot fulfill
      #<tradeId>: no live <give> in inventory` and stop (do NOT mark processed — retry
      later once you have enough).
   d. Resolve each give object's path: `get_objects_dir` joined with its `fileName`.
   e. Reply, attaching ALL the give files (text first, then every path):

      ```bash
      python3 "${CLAUDE_SKILL_DIR}/market.py" reply "<messageId>" "Here is your <giveQty> <give> for #<tradeId>." <givePath1> <givePath2> …
      ```

      `STATUS=OK` → output `replied to <from> with <giveQty> <give>`. Anything else →
      output the `STATUS=` line and stop.
   f. Run `python3 "${CLAUDE_SKILL_DIR}/market.py" mark-processed "<tradeId>" "<messageId>"`,
      then output `fulfilled #<tradeId>`.

## List  — run when `$action` is `list`

Read the open orders on the board:

```bash
python3 "${CLAUDE_SKILL_DIR}/market.py" list-orders
```

Each `ORDER {…}` line is an open order shaped `{id, tradeId, give, giveQty, want, wantQty, contact, status}`.
Render them one per line for the user, e.g. `#<tradeId>  <giveQty> <give> → <wantQty> <want>  <contact>`.
If the helper prints `count=0`, output `no open orders`. On a non-`OK` `STATUS=` line
(e.g. `NOAPI`, `FAIL`), output it and stop.

The board is append-only; to retire an order, click **close** on the web board
(`:8088`) or run `python3 "${CLAUDE_SKILL_DIR}/market.py" close-order <id>` — it marks
the order `closed` (the row stays).

## Guardrails

- Only act on YOUR `<tradeId>` — `poll` filters by subject, so you only see matching mail.
- One fulfillment per message — `mark-processed` plus `poll`'s dedupe enforce it; never reply twice.
- Never reply unless the imported object is a `live` `<want>`.
- `~/.dobj/agentmail.key` is a secret — never echo it or post it anywhere.
- After you reply you still hold a local copy of the given object (no nullify-on-send yet) — expected for now.
- Change terms anytime with `market post <giveQty> <give> <wantQty> <want>` (posts a fresh order). To re-accept a fulfilled trade, `rm ~/.dobj/.market-processed-<tradeId>.log`.
