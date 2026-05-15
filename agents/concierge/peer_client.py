"""Minimal A2A peer client built on the a2a-sdk.

Streams chunks back from a peer so we can forward Working-state updates
to our own user while a downstream agent is still working.
"""

from __future__ import annotations

import base64
from collections.abc import AsyncIterable
from typing import Any

import httpx

from a2a.client import A2ACardResolver, ClientConfig, create_client
from a2a.helpers import new_text_message
from a2a.types import FilePart, FileWithBytes, Part
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
    async with httpx.AsyncClient(timeout=300.0) as httpx_client:
        resolver = A2ACardResolver(httpx_client=httpx_client, base_url=base_url)
        card = await resolver.get_agent_card()

        client = await create_client(
            agent=card, client_config=ClientConfig(streaming=True)
        )
        try:
            message = new_text_message(text, role=Role.ROLE_USER)
            if file_parts:
                extra: list[Part] = [
                    Part(
                        root=FilePart(
                            file=FileWithBytes(
                                bytes=base64.b64encode(b).decode('ascii'),
                                mime_type='application/octet-stream',
                                name=name,
                            )
                        )
                    )
                    for name, b in file_parts
                ]
                # Append additional parts onto the message
                message.parts.extend(extra)

            req = SendMessageRequest(message=message)
            async for chunk in client.send_message(req):
                yield chunk
        finally:
            await client.close()
