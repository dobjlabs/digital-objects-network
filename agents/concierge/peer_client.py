"""Minimal A2A peer client built on the a2a-sdk.

Streams chunks back from a peer so we can forward Working-state updates
to our own user while a downstream agent is still working.

This SDK version uses pure-protobuf `Part` messages with flat fields.
"""

from __future__ import annotations

from collections.abc import AsyncIterable
from typing import Any

import httpx

from a2a.client import A2ACardResolver, ClientConfig, create_client
from a2a.helpers import new_text_message
from a2a.types import Part
from a2a.types.a2a_pb2 import Role, SendMessageRequest


async def send_and_stream(
    base_url: str,
    text: str,
    file_parts: list[tuple[str, bytes]] | None = None,
) -> AsyncIterable[Any]:
    """Send a message to an A2A peer, yield every streamed chunk.

    The peer's final artifact(s) arrive in the last chunks. Caller is
    responsible for picking out file artifacts from the stream.
    """
    async with httpx.AsyncClient(timeout=600.0) as httpx_client:
        resolver = A2ACardResolver(httpx_client=httpx_client, base_url=base_url)
        card = await resolver.get_agent_card()

        # Reuse the long-timeout httpx client; otherwise the SDK builds its
        # own with a ~5s read timeout which kills bitcraft proof generation.
        client = await create_client(
            agent=card,
            client_config=ClientConfig(streaming=True, httpx_client=httpx_client),
        )
        try:
            message = new_text_message(text, role=Role.ROLE_USER)
            if file_parts:
                file_proto_parts = [
                    Part(
                        raw=data,
                        filename=name,
                        media_type='application/octet-stream',
                    )
                    for name, data in file_parts
                ]
                # proto3 repeated composite field — extend with the iterable
                message.parts.extend(file_proto_parts)

            req = SendMessageRequest(message=message)
            async for chunk in client.send_message(req):
                yield chunk
        finally:
            await client.close()
