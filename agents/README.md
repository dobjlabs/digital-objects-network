# bitcraft agents

Four A2A agents, each backed by its own dobjd. A `Concierge` orchestrates
a `Lumberjack`, a `Stonemason`, and a `Craftsmith` to deliver a fully
ZK-anchored StonePick.

```
                ┌─────────────────────┐
   User ──A2A──▶│ Concierge  (:9996)  │
                │ own dobjd  (:7747)  │
                └──────┬──────────────┘
                       │ parallel A2A
        ┌──────────────┼──────────────┐
        ▼              ▼              │
┌──────────────┐ ┌──────────────┐     │
│ Lumberjack-A │ │ Stonemason   │     │
│ A2A  :9997   │ │ A2A  :9998   │     │
│ dobjd :7727  │ │ dobjd :7737  │     │
└──────┬───────┘ └──────┬───────┘     │
       │  Stick.dobj    │  Stone.dobj │
       │                │             │
       └──────┬─────────┘             │
              ▼                       │
       ┌──────────────────┐           │
       │ Concierge ingest │           │
       │ + verify locally │           │
       └────────┬─────────┘           │
                │ FilePart(Stick)     │
                │ FilePart(Stone)     │
                ▼                     │
       ┌──────────────────┐           │
       │ Craftsmith :9999 │           │
       │ dobjd :7747      │           │
       │ ingest + verify  │           │
       │ CraftStonePick   │           │
       └────────┬─────────┘           │
                │ StonePick.dobj      │
                └──────────────────────┘
```

## What each agent does

| Agent          | Port | Job                                                                                             | Real bitcraft work                                         |
| -------------- | ---- | ----------------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| **Concierge**  | 9996 | parse user request, fan out to specialists, verify everything locally, ship the final StonePick | runs `list_inventory` to verify each received `.dobj`      |
| **Lumberjack** | 9997 | supply one Stick from scratch                                                                   | `FindLog` → `CraftWood` → `CraftSticks`                    |
| **Stonemason** | 9998 | supply one Stone                                                                                | bootstraps a `WoodPick` if needed, then `MineStoneWithWoodPick` |
| **Craftsmith** | 9999 | turn Stick + Stone into a StonePick                                                             | ingests received `.dobj`s, verifies, runs `CraftStonePick` |

Each agent streams `Working`-state updates back through the A2A
`message/stream` channel, **including every step from dobjd's own
`/events` SSE pipeline**, so the user sees real-time progress like:

```
reaching out in parallel: http://…:9997 + http://…:9998
[lumberjack] chopping a log…
[lumberjack] FindLog: Verifying inputs (generateProof/running)
[lumberjack] FindLog: Generating proof (generateProof/running)
[lumberjack] FindLog: Proof generation complete (generateProof/done)
[lumberjack] FindLog: Shrinking proof (commit/running)
[lumberjack] FindLog: Submitting proof to relayer (commit/running)
[lumberjack] FindLog: Waiting for synchronizer to observe commit (commit/running)
[lumberjack] FindLog: Commit complete (commit/done)
[lumberjack] refining craft-basics__log_….dobj into wood…
[lumberjack] CraftWood: …                                       ← same sub-steps
[stonemason] no WoodPick on hand — bootstrapping…
[stonemason] FindLog: …
…
[stonemason] mining stone with craft-basics__woodpick_….dobj…
[stonemason] MineStoneWithWoodPick: …
verifying Stick locally…
forwarding inputs to craftsmith…
[craftsmith] CraftStonePick: …
verifying StonePick locally…
StonePick delivered.
```

The proof-step lines come from each peer's dobjd `/events` stream,
forwarded by the executor via `make_progress_forwarder(...)` and
re-broadcast by the concierge with a `[peer]` prefix.

## How object transfer works (MVP)

Bitcraft has no Transfer action today. So when Lumberjack "gives"
Concierge a Stick, what actually happens:

1. Lumberjack runs the wood-chain on its own dobjd. The Stick lives in
   Lumberjack's `~/.dobj/objects/`.
2. Lumberjack reads the raw `.dobj` bytes off disk and ships them as an
   A2A `FilePart` artifact.
3. Concierge receives the bytes, drops them into _its_ dobjd's objects
   dir. Driver re-scans on the next `/inventory` call. Synchronizer
   answers the liveness question.
4. Concierge sees the Stick in its inventory with `status=live` and
   `class=Stick` — **the ZK proof verified and the chain agrees it's
   not nullified.**

**Known MVP weakness:** there's no notion of _ownership_ on chain.
Lumberjack could ship the same Stick to two parties before either uses
it. To close that gap partially, each specialist **deletes the shipped
`.dobj` from its own objects dir after delivery** (via
`dobjd_client.delete_dobj_file`) so it can't be re-shipped. The chain
still considers the commitment live, so the recipient's dobjd accepts
it normally. Adding a real `Transfer` action to `craft-basics` would
fully close this — out of scope here.

## LLM brains

All four agents run an LLM brain.

The three **specialists** (Lumberjack, Stonemason, Craftsmith) use
**LangChain + LiteLLM** to talk to their dobjd's MCP server
(`http://127.0.0.1:<port+1>/mcp`, derived automatically from
`DOBJD_URL`). Each follows a system prompt that says "check inventory
first; craft only if missing", then returns the chosen `.dobj` filename
which the harness ships as a FilePart.

The **Concierge** uses **BeeAI Framework**'s `RequirementAgent` with
`ThinkTool` + `ConditionalRequirement` + three custom A2A peer tools.
The framework split is intentional: it mirrors the A2AWalkthrough
healthcare-demo's "different frameworks, same A2A wire" interop story
in a single repo. Two frameworks, one provider abstraction (LiteLLM
under both, via `langchain-litellm` for the specialists and BeeAI's
`LiteLLMChatModel` adapter for the Concierge).

### Provider-agnostic via LiteLLM

Set `LLM_MODEL` to any LiteLLM-supported provider string. The easiest way:
copy `.env.example` to `.env` and edit:

```bash
cd agents
cp .env.example .env
# edit .env — set LLM_MODEL and the matching API key
```

Both `scripts/run_all.sh` and `scripts/bootstrap_dobjds.sh` auto-load
`.env` at startup. `.env` is gitignored — only `.env.example` is checked in.

Supported model strings (see LiteLLM docs for the full list):

```
LLM_MODEL=anthropic/claude-opus-4-7        # Anthropic
LLM_MODEL=gemini/gemini-2.5-flash          # Google direct (cheapest + fast)
LLM_MODEL=vertex_ai/gemini-2.5-flash       # Vertex AI
LLM_MODEL=openai/gpt-4o                    # OpenAI
LLM_MODEL=ollama/llama3                    # local Ollama (no API key needed)
LLM_MODEL=together_ai/<repo>/<model>       # Together
LLM_MODEL=anthropic/claude-haiku-4-5       # cheap Anthropic option
```

Each provider reads its own API key from env (`ANTHROPIC_API_KEY`,
`GEMINI_API_KEY`, `OPENAI_API_KEY`, …). LiteLLM handles routing.

Per-agent overrides win over `LLM_MODEL` — lets you mix providers across
the network:

```
LUMBERJACK_LLM=anthropic/claude-haiku-4-5
STONEMASON_LLM=gemini/gemini-2.5-flash
CRAFTSMITH_LLM=openai/gpt-4o
CONCIERGE_LLM=anthropic/claude-opus-4-7    # auto-translated to anthropic:...
```

Default if nothing set: `anthropic/claude-opus-4-7`.

BeeAI uses `provider:model` (colon) where LiteLLM uses `provider/model`
(slash). The Concierge auto-translates the LiteLLM shape so the same
env var works everywhere. If you pick a LiteLLM-only provider like
`together_ai` (no BeeAI adapter), set `CONCIERGE_LLM` separately to a
BeeAI-supported provider — see `_LITELLM_TO_BEEAI_PROVIDER` in
`concierge/agent_executor.py`.

## Layout

```
agents/
  shared/                       cross-agent helpers
    dobjd_client.py             async wrapper around dobjd REST + objects-dir
                                (run_action_with_progress, delete_dobj_file)
    dobj_verify.py              ingest-and-verify: class + status=live
    a2a_helpers.py              emit working/completed, file-part helpers,
                                make_progress_forwarder
    llm_brain.py                LangChain create_agent + LiteLLM + MCP adapter
                                (provider-agnostic; powers the specialists)
    brain_hub.py                in-memory pub-sub + SSE route for /brain-events
    peer_tools.py               BeeAI Tool subclasses wrapping A2A peer calls
                                (used by the Concierge)
    registry.py                 env-driven peer URL map
  concierge/                    the orchestrator (BeeAI RequirementAgent)
    __main__.py                 AgentCard + /brain-events SSE route, port 9996
    agent_executor.py           BeeAI ThinkTool + 3 peer tools + ConditionalReq
    peer_client.py              streaming send_message wrapper
  lumberjack/                   supplies Sticks (LLM-driven)
    __main__.py                 port 9997
    agent_executor.py           LLM picks: inventory vs FindLog → CraftWood → CraftSticks
  stonemason/                   supplies Stones (LLM-driven)
    __main__.py                 port 9998
    agent_executor.py           LLM picks: inventory vs bootstrap WoodPick → Mine
  craftsmith/                   assembles StonePicks (LLM-driven)
    __main__.py                 port 9999
    agent_executor.py           ingest inputs → LLM runs CraftStonePick → ship
  scripts/
    bootstrap_dobjds.sh         spin up four isolated dobjds (HOME-overridden)
    run_all.sh                  spin up the four A2A agents
    ping_dobjds.sh              health summary: status, inventory, state-root
    test_client.py              send "I want a stone pick" to the concierge
```

Per-folder layout follows the
[a2a-samples helloworld](https://github.com/a2aproject/a2a-samples/tree/main/samples/python/agents/helloworld).

## Run it

The full stack has three layers, in order:

### 1. Synchronizer + relayer (chain anchoring)

Two choices:

- **Hosted (recommended)** — use the public default endpoints. Nothing
  to start locally. Bootstrap script wires this up by default.
- **Local** — `just sync` and `just relayer` in two terminals
  (needs Postgres). `just dev` also brings them up alongside a single
  dobjd + Vite + Tauri shell you won't use here.

### 2. Four dobjd instances (one per agent)

The bootstrap script creates four isolated `~/.dobj/` data dirs under
`agents/.runtime/<name>/.dobj/` (via per-process `HOME` override) and
launches a dobjd in each on a distinct port:

| Agent      | dobjd port | MCP port | data dir                              |
| ---------- | ---------- | -------- | ------------------------------------- |
| lumberjack | 7717       | 7718     | `agents/.runtime/lumberjack/.dobj/`   |
| stonemason | 7727       | 7728     | `agents/.runtime/stonemason/.dobj/`   |
| craftsmith | 7737       | 7738     | `agents/.runtime/craftsmith/.dobj/`   |
| concierge  | 7747       | 7748     | `agents/.runtime/concierge/.dobj/`    |

One-time setup:

```bash
cargo build -p dobjd --release
just install-plugins                      # populates ~/.dobj/actions/craft-basics.pexe
```

Then in **terminal A**:

```bash
cd agents
bash scripts/bootstrap_dobjds.sh          # default: hosted sync+relayer
# or: bash scripts/bootstrap_dobjds.sh --local
```

Logs at `agents/.runtime/<name>/dobjd.log`. Ctrl-C stops all four.

To verify they're up + talking to the synchronizer:

```bash
bash scripts/ping_dobjds.sh
# agent         http  health  inv  actions  state-root
# lumberjack    7727  ok      0    7        0x570762999dd9769d…
# stonemason    7737  ok      0    7        0x570762999dd9769d…
# craftsmith    7747  ok      0    7        0x570762999dd9769d…
# concierge     7757  ok      0    7        0x570762999dd9769d…
# lumberjack_b  7767  ok      0    7        0x570762999dd9769d…
# auctioneer    7777  ok      0    7        0x570762999dd9769d…
```

### 3. Four A2A agents

In **terminal B**:

```bash
cd agents
uv sync                                   # one-time
bash scripts/run_all.sh
```

### 4. Kick off a request

In **terminal C**:

```bash
cd agents
uv run scripts/test_client.py
```

You'll watch the user request flow through: concierge fans out to
lumberjack + stonemason, each runs real bitcraft actions and streams
progress, concierge verifies and forwards to craftsmith, craftsmith
assembles, the StonePick comes back.

### TL;DR three terminals

```
A:  cd agents && bash scripts/bootstrap_dobjds.sh
B:  cd agents && bash scripts/run_all.sh
C:  cd agents && uv run scripts/test_client.py
```

### Overriding ports

Both run scripts honor env vars:

- `LUMBERJACK_DOBJD`, `STONEMASON_DOBJD`, `CRAFTSMITH_DOBJD`, `CONCIERGE_DOBJD` (dobjd URLs)
- `LUMBERJACK_URL`, `STONEMASON_URL`, `CRAFTSMITH_URL`, `CONCIERGE_URL` (A2A URLs)

## What's deliberately missing

- **Auth.** No `securitySchemes` on the cards. Localhost demo only.
- **Negotiation.** Specialists deliver unconditionally; no `input-required`
  state, no price quoting, no payment. Easy to add later as a wrapper
  around the executor body.
- **Real ownership transfer.** Sender retains a usable copy of any
  shipped `.dobj` because bitcraft has no `Transfer` action yet. See
  the MVP note above.
- **Framework variety beyond two.** Specialists are LangChain, Concierge
  is BeeAI. Adding a third framework (ADK or CrewAI) for a fifth agent
  would push the interop story further but isn't done here.

## Sanity-checking individual agents

Each agent serves its own card at the standard well-known path:

```bash
curl -s http://127.0.0.1:9997/.well-known/agent-card.json | jq
curl -s http://127.0.0.1:9998/.well-known/agent-card.json | jq
curl -s http://127.0.0.1:9999/.well-known/agent-card.json | jq
curl -s http://127.0.0.1:9996/.well-known/agent-card.json | jq
```

You can also talk directly to any specialist:

```bash
uv run python - <<'PY'
import asyncio, httpx
from a2a.client import A2ACardResolver, ClientConfig, create_client
from a2a.helpers import new_text_message
from a2a.types.a2a_pb2 import Role, SendMessageRequest

async def main():
    async with httpx.AsyncClient(timeout=300) as h:
        card = await A2ACardResolver(h, 'http://127.0.0.1:9997').get_agent_card()
        c = await create_client(agent=card, client_config=ClientConfig(streaming=True))
        async for chunk in c.send_message(SendMessageRequest(
            message=new_text_message('I need 1 stick', role=Role.ROLE_USER))):
            print(chunk)
        await c.close()

asyncio.run(main())
PY
```
