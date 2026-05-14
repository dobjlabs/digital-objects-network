"""Bitcraft inventory agent executor.

The agent itself is a pure adapter: an A2A message comes in, it calls
dobjd's `GET /inventory` REST endpoint, formats the response, and emits
that as a text artifact. There's no LLM and no message-content parsing —
every call returns the current inventory.

To make this a real reasoning peer, replace `BitcraftInventoryAgent.invoke`
with a model call that inspects `context.message` and decides what to do.
"""

import collections
import os

import httpx

from a2a.helpers import (
    new_task_from_user_message,
    new_text_artifact,
    new_text_message,
)
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.types.a2a_pb2 import (
    TaskArtifactUpdateEvent,
    TaskState,
    TaskStatus,
    TaskStatusUpdateEvent,
)


DOBJD_URL = os.environ.get('DOBJD_URL', 'http://127.0.0.1:7717')


# --8<-- [start:BitcraftInventoryAgent]
class BitcraftInventoryAgent:
    """Calls dobjd's REST API and renders inventory as text."""

    async def invoke(self) -> str:
        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                r = await client.get(f'{DOBJD_URL}/inventory')
                r.raise_for_status()
                objects = r.json()
        except Exception as e:
            return f'could not reach dobjd at {DOBJD_URL}: {e}'
        return _format(objects)


def _format(objects: list[dict]) -> str:
    """Group by class, count live vs. consumed/pending, one line per class."""
    if not objects:
        return 'inventory is empty'

    by_class: dict[str, dict] = collections.defaultdict(
        lambda: {'live': 0, 'other': 0, 'emoji': '📦'}
    )
    for obj in objects:
        klass = obj.get('class', {}).get('name', '?')
        bucket = by_class[klass]
        bucket['emoji'] = obj.get('emoji', '📦')
        status = obj.get('status', '')
        if status.lower() == 'live':
            bucket['live'] += 1
        else:
            bucket['other'] += 1

    lines = []
    for klass, data in sorted(by_class.items()):
        label = f"{data['emoji']} {klass}: {data['live']}"
        if data['other']:
            label += f"  (+{data['other']} consumed/pending)"
        lines.append(label)
    return '\n'.join(lines)


# --8<-- [end:BitcraftInventoryAgent]


# --8<-- [start:BitcraftInventoryAgentExecutor]
class BitcraftInventoryAgentExecutor(AgentExecutor):
    """Wires BitcraftInventoryAgent into the A2A task lifecycle."""

    def __init__(self) -> None:
        self.agent = BitcraftInventoryAgent()

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> None:
        task = context.current_task or new_task_from_user_message(
            context.message
        )
        await event_queue.enqueue_event(task)

        await event_queue.enqueue_event(
            TaskStatusUpdateEvent(
                task_id=context.task_id,
                context_id=context.context_id,
                status=TaskStatus(
                    state=TaskState.TASK_STATE_WORKING,
                    message=new_text_message('Fetching inventory from dobjd…'),
                ),
            )
        )

        result = await self.agent.invoke()

        await event_queue.enqueue_event(
            TaskArtifactUpdateEvent(
                task_id=context.task_id,
                context_id=context.context_id,
                artifact=new_text_artifact(name='inventory', text=result),
            )
        )
        await event_queue.enqueue_event(
            TaskStatusUpdateEvent(
                task_id=context.task_id,
                context_id=context.context_id,
                status=TaskStatus(state=TaskState.TASK_STATE_COMPLETED),
            )
        )

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        raise Exception('cancel not supported')


# --8<-- [end:BitcraftInventoryAgentExecutor]
