"""Tiny A2A SDK helpers shared across executors."""

from __future__ import annotations

import base64
from typing import Iterable

from a2a.helpers import new_task_from_user_message, new_text_artifact, new_text_message
from a2a.server.agent_execution import RequestContext
from a2a.server.events import EventQueue
from a2a.types import FilePart, FileWithBytes, Part, TextPart
from a2a.types.a2a_pb2 import (
    TaskArtifactUpdateEvent,
    TaskState,
    TaskStatus,
    TaskStatusUpdateEvent,
)


async def ensure_task(context: RequestContext, event_queue: EventQueue):
    """Make sure a Task is enqueued; return it."""
    task = context.current_task or new_task_from_user_message(context.message)
    await event_queue.enqueue_event(task)
    return task


async def emit_working(
    context: RequestContext, event_queue: EventQueue, text: str
) -> None:
    await event_queue.enqueue_event(
        TaskStatusUpdateEvent(
            task_id=context.task_id,
            context_id=context.context_id,
            status=TaskStatus(
                state=TaskState.TASK_STATE_WORKING,
                message=new_text_message(text),
            ),
        )
    )


async def emit_completed(
    context: RequestContext, event_queue: EventQueue
) -> None:
    await event_queue.enqueue_event(
        TaskStatusUpdateEvent(
            task_id=context.task_id,
            context_id=context.context_id,
            status=TaskStatus(state=TaskState.TASK_STATE_COMPLETED),
        )
    )


async def emit_failed(
    context: RequestContext, event_queue: EventQueue, reason: str
) -> None:
    await event_queue.enqueue_event(
        TaskStatusUpdateEvent(
            task_id=context.task_id,
            context_id=context.context_id,
            status=TaskStatus(
                state=TaskState.TASK_STATE_FAILED,
                message=new_text_message(reason),
            ),
        )
    )


async def emit_text_artifact(
    context: RequestContext,
    event_queue: EventQueue,
    name: str,
    text: str,
) -> None:
    await event_queue.enqueue_event(
        TaskArtifactUpdateEvent(
            task_id=context.task_id,
            context_id=context.context_id,
            artifact=new_text_artifact(name=name, text=text),
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
    """Ship a .dobj as a FilePart + optional TextPart in one artifact."""
    parts: list[Part] = [
        Part(
            root=FilePart(
                file=FileWithBytes(
                    bytes=base64.b64encode(dobj_bytes).decode('ascii'),
                    mime_type='application/octet-stream',
                    name=file_name,
                )
            )
        )
    ]
    if note:
        parts.append(Part(root=TextPart(text=note)))

    # Build artifact manually to control name + parts
    from a2a.types import Artifact
    import uuid as _uuid
    artifact = Artifact(
        artifact_id=str(_uuid.uuid4()),
        name=artifact_name,
        parts=parts,
    )
    await event_queue.enqueue_event(
        TaskArtifactUpdateEvent(
            task_id=context.task_id,
            context_id=context.context_id,
            artifact=artifact,
        )
    )


def extract_file_parts(message) -> list[tuple[str, bytes]]:
    """Pull every FilePart out of an A2A Message; return [(name, bytes), ...]."""
    out: list[tuple[str, bytes]] = []
    parts: Iterable = getattr(message, 'parts', None) or []
    for part in parts:
        root = getattr(part, 'root', part)
        # FilePart has .file with .bytes (base64) and .name
        f = getattr(root, 'file', None)
        if f is None:
            continue
        b64 = getattr(f, 'bytes', None)
        name = getattr(f, 'name', None) or 'unknown.dobj'
        if not b64:
            continue
        out.append((name, base64.b64decode(b64)))
    return out


def extract_text(message) -> str:
    """Concatenate any TextPart text content."""
    parts: Iterable = getattr(message, 'parts', None) or []
    buf: list[str] = []
    for part in parts:
        root = getattr(part, 'root', part)
        text = getattr(root, 'text', None)
        if text:
            buf.append(text)
    return ' '.join(buf)
