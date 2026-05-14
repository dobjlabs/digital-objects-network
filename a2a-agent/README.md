# bitcraft inventory A2A agent

A minimal [A2A protocol](https://a2a-protocol.org) agent that reports this
player's bitcraft inventory to peer agents. Structured to mirror the
[official a2a-samples helloworld](https://github.com/a2aproject/a2a-samples/tree/main/samples/python/agents/helloworld).

```
__main__.py          AgentCard + AgentSkill + Starlette wiring + uvicorn entry
agent_executor.py    BitcraftInventoryAgent (the brain) + AgentExecutor impl
test_client.py       Fetches the card, sends a message, prints the response
pyproject.toml       a2a-sdk + starlette + uvicorn + httpx
Containerfile        Optional container build (UBI8 + uv)
```

The agent itself is a pure adapter: A2A message in → `GET dobjd/inventory`
→ formatted text artifact out. No LLM. To make it a real reasoning peer,
replace `BitcraftInventoryAgent.invoke` with a model call that inspects the
inbound message.

## Getting started

1. Start the server (default `:7720`):

   ```bash
   uv run .
   ```

2. Run the test client in another terminal:

   ```bash
   uv run test_client.py
   ```

You'll see the agent card printed, then a non-streaming `message/send`
response, then a streaming `message/stream` response — each containing the
current bitcraft inventory as a text artifact.

If `dobjd` isn't running, the agent still responds — the artifact text will
be a clear error message and the task will complete (the SDK requires
terminal states; we don't currently surface `failed` from the executor
since it's a demo).

## Configuration

Environment variables:

| var | default | meaning |
|---|---|---|
| `A2A_HOST` | `127.0.0.1` | bind address |
| `A2A_PORT` | `7720` | bind port |
| `A2A_PUBLIC_URL` | `http://<host>:<port>` | URL advertised in the agent card |
| `DOBJD_URL` | `http://127.0.0.1:7717` | upstream dobjd REST endpoint |

## Build a container

```bash
podman build . -t bitcraft-inventory-a2a
podman run -p 7720:7720 bitcraft-inventory-a2a
```

## Validate with the A2A reference CLI

```bash
# from the a2a-samples checkout
cd samples/python/hosts/cli
uv run . --agent http://127.0.0.1:7720
```

## What's deliberately missing

Production agents need more than this. The README in the repo root covers
the bigger picture, but specifically here:

- **Auth.** The agent card has no `securitySchemes`. The A2A spec defines
  OIDC/JWT — left out for clarity. Anything on the public internet needs
  it.
- **Identity binding.** A real bitcraft A2A agent should sign responses
  with the player's wallet keypair so peers can verify the inventory
  claim is authentic. The keypair lives in dobjd; the agent would call
  into it for signing.
- **LLM brain.** `invoke()` is a single REST call. A reasoning agent
  would consult an LLM that decides whether to share inventory, what
  subset, whether to counter-offer in a trade context, etc.
- **More skills.** This agent only declares `list_inventory`. The natural
  expansion is `propose_trade`, `accept_trade`, `query_class` — each a
  new `AgentSkill` entry on the card plus a branch in the executor.

## Disclaimer

Same caveat as the upstream helloworld: any agent operating outside your
direct control is untrusted. Validate all incoming `AgentCard` fields,
messages, and artifacts before using them in prompts or business logic.
Failure to sanitize creates prompt-injection surface.
