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
- `post`  → **Post**: post a new offer to the market board (you give the terms).
- `check` → **Check**: pull new email once, import any object sent to you, reply with yours.
- `list`  → **List**: read open orders from the market board.
- empty or anything else → output `usage: market [setup|post|check|list]` and stop.

The committed helper `market.py` wraps every AgentMail / market-board / config
operation as a deterministic subcommand (AgentMail key in `~/.dobj/agentmail.key`;
the board lives at `marketApiUrl`) — no MCP, no OAuth, no loop. Run it as
`python3 "${CLAUDE_SKILL_DIR}/market.py" <sub> …`; subs: `signup`, `verify`,
`sync-config`, `announce`, `list-orders`, `my-offers`, `close-order`, `poll`, `reply`, `mark-processed`.

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
  `no active offers`, `posted offer #<tradeId>: <giveQty> <give> → <wantQty> <want>`,
  `cancelled`, `usage: market [setup|post|check|list]`,
  `usage: market post <giveQty> <give> <wantQty> <want>`.

## Config

`<root>/market.json` (`<root>` = `$DOBJ_HOME`, default `~/.dobj`):

```json
{
  "contactEmail": "",
  "agentmailInboxId": "",
  "marketApiUrl": "http://localhost:8088"
}
```

The config holds only your identity + board URL. Offers are **per-post** — the terms
are `market post` arguments, and each offer's **tradeId is assigned by the server**,
not stored here. `marketApiUrl` is optional (default `http://localhost:8088` — run the
board with `python3 market/server.py`). The AgentMail key lives in
`<root>/agentmail.key` (mode 600), written by Setup; a per-offer
`<root>/.market-processed-<tradeId>.log` ensures no email is fulfilled twice. **`<root>`
is `~/.dobj` by default, or `$DOBJ_HOME` when set** — the helper derives every path
(key, config, processed logs) from it, so a per-agent `DOBJ_HOME` fully isolates two
traders on one machine (see the two-agent demo in the repo `README.md`). Let the helper
manage these files; never touch them by hand.

## Setup  — run when `$action` is `setup`

1. Run `python3 "${CLAUDE_SKILL_DIR}/market.py" status`. On `STATUS=READY` it also prints
   `inbox=<address>` → output `already set up: <address>` and stop. On `STATUS=NEW`,
   continue. (The helper resolves your `.dobj` root from `DOBJ_HOME`, so this checks the
   right place in a per-agent demo — never test `~/.dobj/...` by hand.)
2. Output exactly `your email (for the AgentMail sign-up code)?` and END THE TURN. Wait; hold the reply as `<email>`.
3. Output exactly `pick a username for your trade inbox (becomes <name>@agentmail.to)?` and END THE TURN. Wait; hold the reply as `<username>`. (Usernames are global — each user needs a unique one.)
4. Run:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/market.py" signup "<email>" "<username>"
   ```

   `OK`/`ALREADY` → continue. `TAKEN` → output `username <username> is taken — pick another`
   and go back to step 3. Anything else → output the `STATUS=` line and stop.
5. Signup emailed a 6-digit code to `<email>`. Verifying is **required**: an unverified
   inbox can only send to its own sign-up address, so without it you can't fulfill a trade
   with anyone else. Output exactly `6-digit code emailed to <email> (paste it)?` and END
   THE TURN. Wait, then:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/market.py" verify "<reply>"
   ```

   `STATUS=VERIFIED` → continue; anything else → output the `STATUS=` line and stop.
6. Output `agentmail inbox ready: <address>` (`<address>` = `agentmailInboxId` from config).

## Post  — run when `$action` is `post`

Post a NEW offer; the user gives the terms, e.g. `market post 5 Iron for 2 Copper` or
`market post give 2 Iron want 1 Copper`. You can post several — each is its own offer
with its own server-assigned tradeId.

1. Run `python3 "${CLAUDE_SKILL_DIR}/market.py" status`. On `STATUS=NEW` → output
   `run market setup first` and stop.
2. Parse the user's offer into `<giveQty> <give> <wantQty> <want>` (positive-integer
   quantities; `give`/`want` are episode-1 class names). If the user gave no terms,
   output `usage: market post <giveQty> <give> <wantQty> <want>` and stop.
3. Run:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/market.py" announce "<giveQty>" "<give>" "<wantQty>" "<want>"
   ```

   On `STATUS=OK` the helper prints `tradeId=<token>` — output
   `posted offer #<token>: <giveQty> <give> → <wantQty> <want>`. On `BADOFFER` /
   `INCOMPLETE` / anything else, output the `STATUS=` line and stop.

## Check  — run when `$action` is `check`

One pass over all your open offers; no loop.

1. Run `python3 "${CLAUDE_SKILL_DIR}/market.py" status`. On `STATUS=NEW` → output
   `run market setup first` and stop.
2. Run `python3 "${CLAUDE_SKILL_DIR}/market.py" sync-config`, then list your open offers:

   ```bash
   python3 "${CLAUDE_SKILL_DIR}/market.py" my-offers
   ```

   Each `OFFER {…}` line is one of your offers, shaped `{tradeId, give, giveQty, want,
   wantQty}`. If `count=0`, output `no active offers` and stop.
3. For each `OFFER` (use its own `tradeId`, `give`, `giveQty`, `want`, `wantQty`):
   a. Poll its inbox tag — `python3 "${CLAUDE_SKILL_DIR}/market.py" poll "<tradeId>"`. If
      the last line is `STATUS=NONE`, output `no new trades for #<tradeId>` and move to
      the next offer. Otherwise each `TRADE {…}` line is `{messageId, from, subject,
      attachmentPaths}` — the sender's `.dobj` files are already downloaded.
   b. For each `TRADE`:
      i. Import every path in `attachmentPaths` (`import_object_file` on each). On any
         import error, output it verbatim, run `python3 "${CLAUDE_SKILL_DIR}/market.py"
         mark-processed "<tradeId>" "<messageId>"`, and move to the next TRADE (no reply).
      ii. Verify with `inspect_object`: you need `<wantQty>` imports that are class
          `<want>` AND status `live`. If fewer: output `rejected <messageId>: expected
          <wantQty> <want>, got <n> <class>/<status>`, run `mark-processed`, do NOT reply.
          Else output `imported <n> <want> (live)`.
      iii. Pick `<giveQty>` live `<give>` objects (`list_inventory`). If fewer than
           `<giveQty>` → output `cannot fulfill #<tradeId>: no live <give> in inventory`
           and move to the next offer (do NOT mark processed).
      iv. Resolve each give object's path (`get_objects_dir` + `fileName`) and reply with
          ALL of them attached:

          ```bash
          python3 "${CLAUDE_SKILL_DIR}/market.py" reply "<messageId>" "Here is your <giveQty> <give> for #<tradeId>." <givePath1> …
          ```

          `STATUS=OK` → output `replied to <from> with <giveQty> <give>` (on success the
          helper has already moved those objects out of your inventory into `objects/.sent/`
          — they're the counterpart's now); else output the `STATUS=` line and stop.
      v. Run `python3 "${CLAUDE_SKILL_DIR}/market.py" mark-processed "<tradeId>" "<messageId>"`,
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
- On a successful reply the helper moves each object you sent out of your inventory
  (into `objects/.sent/`) — it's the counterpart's now, so you no longer hold it live;
  the `.dobj` stays on disk, recoverable. There's no cryptographic nullify-on-send yet;
  this is the honest-removal convention.
- Post more offers anytime with `market post <giveQty> <give> <wantQty> <want>` — each is independent with its own server tradeId. To re-accept on a tradeId, `rm ~/.dobj/.market-processed-<tradeId>.log`.
