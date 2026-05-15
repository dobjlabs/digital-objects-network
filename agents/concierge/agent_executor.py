"""Concierge agent — orchestrates Lumberjack + Stonemason + Craftsmith.

User asks for a StonePick. Concierge:

  1. In parallel, sends `message/send` to Lumberjack ("I need 1 stick")
     and Stonemason ("I need 1 stone"). Streams each peer's
     Working-state updates back to the user.
  2. Pulls the Stick .dobj and Stone .dobj out of the final artifacts,
     ingests them into the concierge's local dobjd, verifies class +
     status=live for each.
  3. Sends `message/send` to Craftsmith with both .dobj FileParts
     attached, asking for a StonePick.
  4. Verifies the returned StonePick locally, then ships it back to
     the user as the final artifact alongside a text summary.

Framework slot: would be BeeAI in a full interop build.
"""

from __future__ import annotations

import asyncio
import base64
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from a2a.server.agent_execution import AgentExecutor, RequestContext  # noqa: E402
from a2a.server.events import EventQueue  # noqa: E402

from concierge.peer_client import send_and_stream  # noqa: E402
from shared.a2a_helpers import (  # noqa: E402
    emit_completed,
    emit_dobj_artifact,
    emit_failed,
    emit_text_artifact,
    emit_working,
    ensure_task,
)
from shared.dobj_verify import DobjVerificationError, ingest_and_verify  # noqa: E402
from shared.dobjd_client import DobjdClient  # noqa: E402
from shared.registry import CRAFTSMITH, LUMBERJACK, STONEMASON  # noqa: E402


class ConciergeAgentExecutor(AgentExecutor):
    """Coordinates three specialists to deliver a StonePick to the user."""

    def __init__(self) -> None:
        self.dobjd = DobjdClient()

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> None:
        await ensure_task(context, event_queue)

        try:
            await emit_working(
                context, event_queue,
                f'reaching out in parallel: {LUMBERJACK.url} + {STONEMASON.url}',
            )

            lumberjack_task = asyncio.create_task(
                _fetch_one(
                    peer_label='lumberjack',
                    base_url=LUMBERJACK.url,
                    request_text='I need 1 stick',
                    context=context,
                    event_queue=event_queue,
                )
            )
            stonemason_task = asyncio.create_task(
                _fetch_one(
                    peer_label='stonemason',
                    base_url=STONEMASON.url,
                    request_text='I need 1 stone',
                    context=context,
                    event_queue=event_queue,
                )
            )

            (stick_name, stick_bytes), (stone_name, stone_bytes) = await asyncio.gather(
                lumberjack_task, stonemason_task
            )

            # Verify both inputs locally before forwarding to craftsmith.
            await emit_working(context, event_queue, 'verifying Stick locally…')
            try:
                stick_obj = await ingest_and_verify(
                    self.dobjd, stick_name, stick_bytes, expected_class='Stick'
                )
            except DobjVerificationError as e:
                await emit_failed(context, event_queue, f'Stick verification failed: {e}')
                return
            await emit_text_artifact(
                context, event_queue, 'verify-stick',
                f'Stick {stick_obj["fileName"]} verified live on chain',
            )

            await emit_working(context, event_queue, 'verifying Stone locally…')
            try:
                stone_obj = await ingest_and_verify(
                    self.dobjd, stone_name, stone_bytes, expected_class='Stone'
                )
            except DobjVerificationError as e:
                await emit_failed(context, event_queue, f'Stone verification failed: {e}')
                return
            await emit_text_artifact(
                context, event_queue, 'verify-stone',
                f'Stone {stone_obj["fileName"]} verified live on chain',
            )

            # Forward both to craftsmith.
            await emit_working(
                context, event_queue,
                f'forwarding inputs to craftsmith at {CRAFTSMITH.url}',
            )
            pick_name, pick_bytes = await _fetch_one(
                peer_label='craftsmith',
                base_url=CRAFTSMITH.url,
                request_text='please assemble a stone pick from these inputs',
                context=context,
                event_queue=event_queue,
                file_parts=[(stick_name, stick_bytes), (stone_name, stone_bytes)],
            )

            await emit_working(context, event_queue, 'verifying StonePick locally…')
            try:
                pick_obj = await ingest_and_verify(
                    self.dobjd, pick_name, pick_bytes, expected_class='StonePick'
                )
            except DobjVerificationError as e:
                await emit_failed(
                    context, event_queue, f'StonePick verification failed: {e}'
                )
                return

            await emit_text_artifact(
                context, event_queue, 'summary',
                'StonePick delivered.\n'
                f'  stick: {stick_obj["fileName"]}\n'
                f'  stone: {stone_obj["fileName"]}\n'
                f'  pick:  {pick_obj["fileName"]}\n'
                f'all three live on chain.',
            )
            await emit_dobj_artifact(
                context, event_queue,
                artifact_name='stonepick',
                file_name=pick_obj['fileName'],
                dobj_bytes=pick_bytes,
                note=f'StonePick {pick_obj["fileName"]} verified live',
            )
            await emit_completed(context, event_queue)

        except Exception as e:
            await emit_failed(context, event_queue, f'concierge failed: {e}')

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        raise Exception('cancel not supported')


async def _fetch_one(
    *,
    peer_label: str,
    base_url: str,
    request_text: str,
    context: RequestContext,
    event_queue: EventQueue,
    file_parts: list[tuple[str, bytes]] | None = None,
) -> tuple[str, bytes]:
    """Send a message to one peer; stream its Working updates through to
    our own user; return the (file_name, bytes) of its single .dobj artifact.
    """
    seen_file: tuple[str, bytes] | None = None

    async for chunk in send_and_stream(base_url, request_text, file_parts=file_parts):
        # Forward any text-status updates we can detect
        text_blob = _flatten_text(chunk)
        if text_blob:
            await emit_working(
                context, event_queue, f'[{peer_label}] {text_blob}'
            )
        found = _find_file_part(chunk)
        if found:
            seen_file = found

    if seen_file is None:
        raise RuntimeError(f'{peer_label} returned no FilePart artifact')
    return seen_file


def _flatten_text(chunk: Any) -> str:
    """Best-effort text extraction for streaming status messages."""
    # The chunk can be many SDK shapes (status update, artifact update, task).
    # We try a few common attribute paths and concatenate any plain text.
    out: list[str] = []
    status = getattr(chunk, 'status', None)
    if status is not None:
        msg = getattr(status, 'message', None)
        if msg is not None:
            for p in getattr(msg, 'parts', []) or []:
                root = getattr(p, 'root', p)
                text = getattr(root, 'text', None)
                if text:
                    out.append(text)
    artifact = getattr(chunk, 'artifact', None)
    if artifact is not None:
        for p in getattr(artifact, 'parts', []) or []:
            root = getattr(p, 'root', p)
            text = getattr(root, 'text', None)
            if text:
                out.append(text)
    return ' '.join(out)


def _find_file_part(chunk: Any) -> tuple[str, bytes] | None:
    artifact = getattr(chunk, 'artifact', None)
    if artifact is None:
        return None
    for p in getattr(artifact, 'parts', []) or []:
        root = getattr(p, 'root', p)
        f = getattr(root, 'file', None)
        if f is None:
            continue
        b64 = getattr(f, 'bytes', None)
        name = getattr(f, 'name', None) or 'unknown.dobj'
        if not b64:
            continue
        return name, base64.b64decode(b64)
    return None
