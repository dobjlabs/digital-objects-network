"""Concierge A2A agent entry point. Default port 9996.

Hosts the Concierge over a2a-sdk's `DefaultRequestHandler` (same as the
three specialists) plus a `/brain-events` SSE route for the preview
dashboard to subscribe to LLM tool-call events. The BeeAI
RequirementAgent that runs *inside* the executor publishes onto the
same `BrainEventHub` the specialists use, so the dashboard sees a
unified event stream across all four agents.
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

from concierge.agent_executor import ConciergeAgentExecutor  # noqa: E402
from shared.brain_hub import BrainEventHub, make_sse_route  # noqa: E402


HOST = os.environ.get('A2A_HOST', '127.0.0.1')
PORT = int(os.environ.get('A2A_PORT', '9996'))
PUBLIC_URL = os.environ.get('A2A_PUBLIC_URL', f'http://{HOST}:{PORT}')


def main() -> None:
    skill = AgentSkill(
        id='deliver_stone_pick',
        name='Deliver a StonePick',
        description=(
            'Coordinates a Lumberjack (supplies Stick), Stonemason '
            '(supplies Stone), and Craftsmith (assembles the pick) to '
            'deliver a fully-anchored StonePick. Verifies each input '
            "and the final output on this concierge's own dobjd."
        ),
        tags=['bitcraft', 'concierge', 'orchestration', 'stonepick'],
        examples=[
            'I want a stone pick',
            'get me a stone pick please',
            'deliver a stone pick',
        ],
    )

    card = AgentCard(
        name='Concierge',
        description=(
            'Orchestrates other bitcraft agents to deliver crafted items. '
            "Currently knows how to procure a StonePick."
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
        agent_executor=ConciergeAgentExecutor(brain_hub=brain_hub),
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
