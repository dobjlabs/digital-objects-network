"""Auctioneer A2A agent entry point. Default port 9994.

No dobjd. The Auctioneer's job is pure routing — it consults the
agent cards of its candidate Lumberjacks, picks the cheapest, and
forwards the request to that winner. Brain hub is wired so the
dashboard's Auctioneer card shows live bid/winner events.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import uvicorn  # noqa: E402
from a2a.server.request_handlers import DefaultRequestHandler  # noqa: E402
from a2a.server.routes import create_agent_card_routes, create_jsonrpc_routes  # noqa: E402
from a2a.server.tasks import InMemoryTaskStore  # noqa: E402
from a2a.types import (  # noqa: E402
    AgentCapabilities,
    AgentCard,
    AgentInterface,
    AgentSkill,
)
from starlette.applications import Starlette  # noqa: E402

from auctioneer.agent_executor import AuctioneerAgentExecutor  # noqa: E402
from shared.brain_hub import BrainEventHub, make_sse_route  # noqa: E402


HOST = os.environ.get('A2A_HOST', '127.0.0.1')
PORT = int(os.environ.get('A2A_PORT', '9994'))
PUBLIC_URL = os.environ.get('A2A_PUBLIC_URL', f'http://{HOST}:{PORT}')


def main() -> None:
    skill = AgentSkill(
        id='auction_stick',
        name='Auction for a Stick',
        description=(
            'Runs a sealed-bid auction across the registered Lumberjack '
            "peers. Reads each candidate's advertised price from its "
            'agent card, picks the cheapest, and delegates the real '
            'delivery to that winner. Returns the winning .dobj as a '
            'FilePart.'
        ),
        tags=['bitcraft', 'auction', 'router', 'discovery'],
        examples=[
            'I need 1 stick',
            'find me the cheapest stick supplier',
        ],
    )

    card = AgentCard(
        name='Auctioneer',
        description=(
            'Discovers and routes to the cheapest available Lumberjack. '
            "Doesn't craft anything itself — pure routing layer."
        ),
        version='0.1.0',
        default_input_modes=['text/plain'],
        default_output_modes=['text/plain', 'application/octet-stream'],
        capabilities=AgentCapabilities(streaming=True),
        supported_interfaces=[
            AgentInterface(protocol_binding='JSONRPC', url=PUBLIC_URL)
        ],
        skills=[skill],
    )

    brain_hub = BrainEventHub()

    handler = DefaultRequestHandler(
        agent_executor=AuctioneerAgentExecutor(brain_hub=brain_hub),
        task_store=InMemoryTaskStore(),
        agent_card=card,
    )

    routes = []
    routes.extend(create_agent_card_routes(card))
    routes.extend(create_jsonrpc_routes(handler, '/'))
    routes.append(make_sse_route(brain_hub))  # GET /brain-events (SSE)

    uvicorn.run(Starlette(routes=routes), host=HOST, port=PORT)


if __name__ == '__main__':
    main()
