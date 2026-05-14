"""Entry point for the bitcraft inventory A2A agent.

Builds the agent card + skill, wires the executor through the A2A SDK's
default request handler, and serves the A2A protocol on `:7720` via
Starlette + uvicorn.

Run:
    uv run .
or:
    A2A_PORT=8080 uv run .
"""

import os

import uvicorn

from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import (
    create_agent_card_routes,
    create_jsonrpc_routes,
)
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import (
    AgentCapabilities,
    AgentCard,
    AgentInterface,
    AgentSkill,
)
from agent_executor import (  # type: ignore[import-untyped]
    BitcraftInventoryAgentExecutor,
)
from starlette.applications import Starlette


HOST = os.environ.get('A2A_HOST', '127.0.0.1')
PORT = int(os.environ.get('A2A_PORT', '7720'))
PUBLIC_URL = os.environ.get('A2A_PUBLIC_URL', f'http://{HOST}:{PORT}')


if __name__ == '__main__':
    # --8<-- [start:AgentSkill]
    list_inventory_skill = AgentSkill(
        id='list_inventory',
        name='List inventory',
        description=(
            "Returns this player's current bitcraft inventory: each "
            "digital object's class (Log, Wood, Stone, …), live count, "
            "and how many copies are consumed or pending."
        ),
        tags=['bitcraft', 'inventory', 'digital-objects'],
        examples=[
            "what's in your inventory?",
            'list your objects',
            'do you have any wood?',
        ],
    )
    # --8<-- [end:AgentSkill]

    # --8<-- [start:AgentCard]
    agent_card = AgentCard(
        name='Bitcraft Inventory Agent',
        description=(
            'Reports the bitcraft digital objects this player currently '
            'holds. Talks to a locally running dobjd over HTTP.'
        ),
        version='0.1.0',
        default_input_modes=['text/plain'],
        default_output_modes=['text/plain'],
        capabilities=AgentCapabilities(streaming=True),
        supported_interfaces=[
            AgentInterface(
                protocol_binding='JSONRPC',
                url=PUBLIC_URL,
            )
        ],
        skills=[list_inventory_skill],
    )
    # --8<-- [end:AgentCard]

    request_handler = DefaultRequestHandler(
        agent_executor=BitcraftInventoryAgentExecutor(),
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )

    routes = []
    routes.extend(create_agent_card_routes(agent_card))
    routes.extend(create_jsonrpc_routes(request_handler, '/'))

    app = Starlette(routes=routes)

    uvicorn.run(app, host=HOST, port=PORT)
