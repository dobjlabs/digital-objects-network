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
│ Lumberjack   │ │ Stonemason   │     │
│ A2A  :9997   │ │ A2A  :9998   │     │
│ dobjd :7717  │ │ dobjd :7727  │     │
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
       │ dobjd :7737      │           │
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
| **Stonemason** | 9998 | supply one Stone                                                                                | bootstraps a `WoodPick` if needed, then `MineStone`        |
| **Craftsmith** | 9999 | turn Stick + Stone into a StonePick                                                             | ingests received `.dobj`s, verifies, runs `CraftStonePick` |

Each agent streams `Working`-state updates back through the A2A
`message/stream` channel, so the user sees real-time progress like:

```
[concierge] reaching out in parallel: http://…:9997 + http://…:9998
[lumberjack] chopping a log…
[stonemason] no WoodPick on hand — bootstrapping…
[lumberjack] refining ….dobj into wood…
[stonemason] assembling WoodPick…
[lumberjack] splitting ….dobj into sticks…
…
[concierge] verifying Stick locally…
[concierge] forwarding inputs to craftsmith…
[craftsmith] ingesting inputs into local dobjd…
[craftsmith] running CraftStonePick…
[concierge] verifying StonePick locally…
[concierge] StonePick delivered.
```

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
Lumberjack could ship the same Stick to two parties and consume it
itself before either uses it. Adding a real `Transfer` action to
`craft-basics` would close this — out of scope here.

## Layout

```
a2a-agent/
  shared/                 cross-agent helpers
    dobjd_client.py       async wrapper around dobjd REST + objects-dir
    dobj_verify.py        ingest-and-verify: class + status=live
    a2a_helpers.py        emit working/completed, FilePart helpers
    registry.py           env-driven peer URL map
  concierge/              the orchestrator
    __main__.py           AgentCard, port 9996
    agent_executor.py     fan out, verify, forward, verify, deliver
    peer_client.py        streaming send_message wrapper
  lumberjack/             supplies Sticks
    __main__.py           port 9997
    agent_executor.py     FindLog → CraftWood → CraftSticks
  stonemason/             supplies Stones
    __main__.py           port 9998
    agent_executor.py     bootstrap WoodPick → MineStone
  craftsmith/             assembles StonePicks
    __main__.py           port 9999
    agent_executor.py     verify inputs → CraftStonePick → ship
  scripts/
    run_all.sh            spin up all four (matches default env)
    test_client.py        send "I want a stone pick" to the concierge
```

Per-folder layout follows the
[a2a-samples helloworld](https://github.com/a2aproject/a2a-samples/tree/main/samples/python/agents/helloworld).

## Run it

Each agent needs its own dobjd at its own URL. Easiest: four separate
dobjd installs on four different ports. Suggested defaults:

| Role       | dobjd                   | Notes                                         |
| ---------- | ----------------------- | --------------------------------------------- |
| lumberjack | `http://127.0.0.1:7717` | the default dobjd port                        |
| stonemason | `http://127.0.0.1:7727` | second dobjd install, different `~/.dobj` dir |
| craftsmith | `http://127.0.0.1:7737` | third                                         |
| concierge  | `http://127.0.0.1:7747` | fourth                                        |

All four point at the same hosted synchronizer + relayer.

```bash
cd a2a-agent
uv sync

# Terminal A — spin everything up
bash scripts/run_all.sh

# Terminal B — kick off a request
uv run scripts/test_client.py
```

Configure non-default ports via env:

- `LUMBERJACK_DOBJD`, `STONEMASON_DOBJD`, `CRAFTSMITH_DOBJD`, `CONCIERGE_DOBJD`
- `LUMBERJACK_URL`, `STONEMASON_URL`, `CRAFTSMITH_URL`, `CONCIERGE_URL`

## What's deliberately missing

- **Auth.** No `securitySchemes` on the cards. Localhost demo only.
- **Negotiation.** Specialists deliver unconditionally; no `input-required`
  state, no price quoting, no payment. Easy to add later as a wrapper
  around the executor body.
- **LLM brains.** Each executor runs a fixed script. To make them
  reasoning peers, drop a model call inside `execute()` that interprets
  the inbound message and decides what to do.
- **Real ownership transfer.** Sender retains a usable copy of any
  shipped `.dobj` because bitcraft has no `Transfer` action yet. See
  the MVP note above.
- **Framework variety.** All four use plain `a2a-sdk` + uvicorn. The
  natural next step is to swap each executor for its own framework
  (BeeAI / ADK / LangGraph / CrewAI) to mirror the healthcare-demo
  interop story.

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
