"""Tiny A2A SDK helpers shared across executors.

The installed `a2a-sdk` (1.0.x) is the pure-protobuf shape: `Part` is a
single proto message with flat fields (`text`, `raw`, `url`, `data`,
`filename`, `media_type`) rather than a Pydantic-style discriminated
union of `TextPart`/`FilePart`. We use protobuf field accessors directly.
"""

from __future__ import annotations

from typing import Iterable

from a2a.helpers import (
    new_artifact,
    new_task_from_user_message,
    new_text_artifact,
    new_text_status_update_event,
)
from a2a.server.agent_execution import RequestContext
from a2a.server.events import EventQueue
from a2a.types import (
    Part,
    TaskArtifactUpdateEvent,
)
from a2a.types.a2a_pb2 import TaskState


# ---------------------------------------------------------------------------
# Task lifecycle emitters
# ---------------------------------------------------------------------------

async def ensure_task(context: RequestContext, event_queue: EventQueue):
    """Make sure a Task is enqueued; return it."""
    task = context.current_task or new_task_from_user_message(context.message)
    await event_queue.enqueue_event(task)
    return task


async def emit_working(
    context: RequestContext, event_queue: EventQueue, text: str
) -> None:
    await event_queue.enqueue_event(
        new_text_status_update_event(
            task_id=context.task_id,
            context_id=context.context_id,
            state=TaskState.TASK_STATE_WORKING,
            text=text,
        )
    )


async def emit_completed(
    context: RequestContext, event_queue: EventQueue
) -> None:
    await event_queue.enqueue_event(
        new_text_status_update_event(
            task_id=context.task_id,
            context_id=context.context_id,
            state=TaskState.TASK_STATE_COMPLETED,
            text='',
        )
    )


async def emit_failed(
    context: RequestContext, event_queue: EventQueue, reason: str
) -> None:
    await event_queue.enqueue_event(
        new_text_status_update_event(
            task_id=context.task_id,
            context_id=context.context_id,
            state=TaskState.TASK_STATE_FAILED,
            text=reason,
        )
    )


# ---------------------------------------------------------------------------
# Artifact emitters
# ---------------------------------------------------------------------------

async def emit_text_artifact(
    context: RequestContext,
    event_queue: EventQueue,
    name: str,
    text: str,
) -> None:
    artifact = new_text_artifact(name=name, text=text)
    await event_queue.enqueue_event(
        TaskArtifactUpdateEvent(
            task_id=context.task_id,
            context_id=context.context_id,
            artifact=artifact,
        )
    )


async def emit_dobj_artifact(
    context: RequestContext,
    event_queue: EventQueue,
    artifact_name: str,
    file_name: str,
    dobj_bytes: bytes,
    note: str | None = None,
) -> None:
    """Ship a .dobj as a file Part (+ optional text Part) in one artifact."""
    parts: list[Part] = [
        Part(
            raw=dobj_bytes,
            filename=file_name,
            media_type='application/octet-stream',
        )
    ]
    if note:
        parts.append(Part(text=note))

    artifact = new_artifact(parts=parts, name=artifact_name)
    await event_queue.enqueue_event(
        TaskArtifactUpdateEvent(
            task_id=context.task_id,
            context_id=context.context_id,
            artifact=artifact,
        )
    )


# ---------------------------------------------------------------------------
# Inbound message parsing
# ---------------------------------------------------------------------------

def extract_file_parts(message) -> list[tuple[str, bytes]]:
    """Pull every file Part out of a Message; return [(filename, bytes), ...]."""
    out: list[tuple[str, bytes]] = []
    parts: Iterable = getattr(message, 'parts', None) or []
    for part in parts:
        raw = getattr(part, 'raw', b'')
        if not raw:
            continue
        name = getattr(part, 'filename', '') or 'unknown.dobj'
        out.append((name, bytes(raw)))
    return out


def extract_text(message) -> str:
    """Concatenate every text Part's content."""
    parts: Iterable = getattr(message, 'parts', None) or []
    buf: list[str] = []
    for part in parts:
        text = getattr(part, 'text', '') or ''
        if text:
            buf.append(text)
    return ' '.join(buf)
