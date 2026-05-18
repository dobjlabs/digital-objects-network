"""In-memory pub-sub for brain events.

Each agent process owns one `BrainEventHub`. The agent's executor pushes
events into it (via `publish()`); HTTP clients subscribe via the
`/brain-events` SSE endpoint and receive every event in real time.

Slow subscribers see dropped events rather than blocking the publisher
— the brain must never stall waiting for a UI to catch up.
"""

from __future__ import annotations

import asyncio
import json
from collections.abc import AsyncIterator
from typing import Any


class BrainEventHub:
    def __init__(self, queue_size: int = 256) -> None:
        self._queue_size = queue_size
        self._subscribers: set[asyncio.Queue[dict[str, Any]]] = set()

    def publish(self, event: dict[str, Any]) -> None:
        """Non-blocking publish. Drops events for full queues."""
        for q in list(self._subscribers):
            try:
                q.put_nowait(event)
            except asyncio.QueueFull:
                pass

    async def subscribe(self) -> AsyncIterator[dict[str, Any]]:
        """Yield events as they're published. Caller cleans up by exiting
        the iterator (the queue is removed from `_subscribers` on close)."""
        q: asyncio.Queue[dict[str, Any]] = asyncio.Queue(maxsize=self._queue_size)
        self._subscribers.add(q)
        try:
            while True:
                event = await q.get()
                yield event
        finally:
            self._subscribers.discard(q)


def make_sse_route(hub: BrainEventHub, path: str = '/brain-events'):
    """Build a Starlette `Route` that serves the hub as an SSE stream.

    Each connection gets its own subscription; disconnects auto-clean
    via the iterator's `finally` block. Permissive CORS so the preview
    HTML at http://localhost:7720 can subscribe.
    """
    from sse_starlette.sse import EventSourceResponse
    from starlette.requests import Request
    from starlette.routing import Route

    async def endpoint(request: Request):
        async def event_stream():
            async for event in hub.subscribe():
                if await request.is_disconnected():
                    break
                yield {'data': json.dumps(event)}

        return EventSourceResponse(
            event_stream(),
            headers={'Access-Control-Allow-Origin': '*'},
        )

    return Route(path, endpoint, methods=['GET'])
