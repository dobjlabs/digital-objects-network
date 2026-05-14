"""Test client for the bitcraft inventory A2A agent.

Fetches the agent card from `/.well-known/agent-card.json`, sends a
`message/send` request asking for inventory, prints the response. Then
repeats with the streaming surface (`message/stream`).

Run:
    uv run test_client.py
"""

import httpx

from a2a.client import A2ACardResolver, ClientConfig, create_client
from a2a.helpers import display_agent_card, new_text_message
from a2a.types.a2a_pb2 import Role, SendMessageRequest
from a2a.utils.constants import AGENT_CARD_WELL_KNOWN_PATH


BASE_URL = 'http://127.0.0.1:7720'


async def main() -> None:
    async with httpx.AsyncClient() as httpx_client:
        resolver = A2ACardResolver(
            httpx_client=httpx_client,
            base_url=BASE_URL,
        )

        print(
            f'Fetching agent card from: {BASE_URL}{AGENT_CARD_WELL_KNOWN_PATH}'
        )
        card = await resolver.get_agent_card()
        print('\nAgent card:')
        display_agent_card(card)

        print('\n--- Non-streaming ---')
        client = await create_client(
            agent=card, client_config=ClientConfig(streaming=False)
        )
        message = new_text_message(
            "what's in your inventory?", role=Role.ROLE_USER
        )
        request = SendMessageRequest(message=message)
        print('Response:')
        async for chunk in client.send_message(request):
            print(chunk)
        await client.close()

        print('\n--- Streaming ---')
        streaming_client = await create_client(
            agent=card, client_config=ClientConfig(streaming=True)
        )
        async for chunk in streaming_client.send_message(request):
            print('chunk:')
            print(chunk)
        await streaming_client.close()


if __name__ == '__main__':
    import asyncio

    asyncio.run(main())
