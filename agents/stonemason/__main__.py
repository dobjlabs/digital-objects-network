"""Stonemason A2A agent entry point. Default port 9998."""

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

from shared.brain_hub import BrainEventHub, make_sse_route  # noqa: E402
from stonemason.agent_executor import StonemasonAgentExecutor  # noqa: E402


HOST = os.environ.get('A2A_HOST', '127.0.0.1')
PORT = int(os.environ.get('A2A_PORT', '9998'))
PUBLIC_URL = os.environ.get('A2A_PUBLIC_URL', f'http://{HOST}:{PORT}')


def main() -> None:
    skill = AgentSkill(
        id='supply_stone',
        name='Supply a Stone',
        description=(
            "Mines a fresh Stone using a WoodPick (bootstrapping the "
            "pick first if needed) and returns the raw .dobj as a FilePart."
        ),
        tags=['bitcraft', 'stone', 'mining', 'supplier'],
        examples=[
            'I need 1 stone',
            'mine me a stone',
            'supply a stone',
        ],
    )

    card = AgentCard(
        name='Stonemason',
        description='Owns the mining chain. Supplies Stones on request.',
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
        agent_executor=StonemasonAgentExecutor(brain_hub=brain_hub),
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
