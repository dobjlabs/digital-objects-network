"""End-to-end test client: ask the concierge for a stone pick, stream updates."""

from __future__ import annotations

import asyncio
import os

import httpx

from a2a.client import A2ACardResolver, ClientConfig, create_client
from a2a.helpers import display_agent_card, new_text_message
from a2a.types.a2a_pb2 import Role, SendMessageRequest


CONCIERGE_URL = os.environ.get('CONCIERGE_URL', 'http://127.0.0.1:9996')


async def main() -> None:
    # 30 min — first-run circuit cache building on each of the 4 isolated
    # dobjds can take 2-3 minutes per action. After warm-up this is way more
    # than needed.
    async with httpx.AsyncClient(timeout=1800.0) as httpx_client:
        resolver = A2ACardResolver(httpx_client=httpx_client, base_url=CONCIERGE_URL)
        card = await resolver.get_agent_card()
        print('Concierge agent card:')
        display_agent_card(card)
        print()

        # Pass the long-timeout httpx client through ClientConfig — without
        # this the SDK builds its own client with ~5s default read timeout,
        # which kills any streaming task whose proof generation takes >5s.
        client = await create_client(
            agent=card,
            client_config=ClientConfig(streaming=True, httpx_client=httpx_client),
        )
        try:
            req = SendMessageRequest(
                message=new_text_message('I want a stone pick', role=Role.ROLE_USER)
            )
            async for chunk in client.send_message(req):
                print('--- chunk ---')
                print(chunk)
        finally:
            await client.close()


if __name__ == '__main__':
    asyncio.run(main())
