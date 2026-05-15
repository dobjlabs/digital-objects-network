"""Craftsmith agent — assembles a StonePick from received Stick + Stone.

Workflow:
  1. Pull the two FileParts out of the inbound A2A message.
  2. Drop both .dobj files into craftsmith's local dobjd objects dir.
  3. List inventory — the new files appear with synchronizer-determined
     status. Verify class is Stick / Stone and status is Live.
  4. Run CraftStonePick with both as inputs.
  5. Read the resulting .dobj and ship it back as a FilePart artifact.

MVP caveat: the input objects were minted on *other* dobjds, so their
proofs may rely on secrets the originating dobjd holds. If the local
CraftStonePick run fails because of that, this executor surfaces a
clear error (the verify-only path still proves end-to-end that the
shipped .dobjs are real ZK-anchored Sticks and Stones).

Framework slot: would be CrewAI in a full interop build.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from a2a.server.agent_execution import AgentExecutor, RequestContext  # noqa: E402
from a2a.server.events import EventQueue  # noqa: E402

from shared.a2a_helpers import (  # noqa: E402
    emit_completed,
    emit_dobj_artifact,
    emit_failed,
    emit_text_artifact,
    emit_working,
    ensure_task,
    extract_file_parts,
    make_progress_forwarder,
)
from shared.dobj_verify import DobjVerificationError, ingest_and_verify  # noqa: E402
from shared.dobjd_client import DobjdClient  # noqa: E402


PLUGIN = 'craft-basics'


class CraftsmithAgentExecutor(AgentExecutor):
    """Assembles a StonePick from a Stick + Stone shipped in the request."""

    def __init__(self) -> None:
        self.dobjd = DobjdClient()

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> None:
        await ensure_task(context, event_queue)

        try:
            file_parts = extract_file_parts(context.message)
            if len(file_parts) < 2:
                await emit_failed(
                    context, event_queue,
                    f'craftsmith needs 2 FileParts (Stick + Stone), got {len(file_parts)}',
                )
                return

            # Heuristic: identify by ingesting both and reading their classes.
            await emit_working(context, event_queue, 'ingesting inputs into local dobjd…')
            ingested: dict[str, dict] = {}  # class_name -> matched inventory row
            for name, data in file_parts:
                await self.dobjd.write_dobj_file(name, data)

            inventory = await self.dobjd.list_inventory()
            by_file = {o.get('fileName'): o for o in inventory}
            for name, _ in file_parts:
                obj = by_file.get(name)
                if obj is None:
                    await emit_failed(
                        context, event_queue,
                        f'dobjd did not recognize {name} after write',
                    )
                    return
                klass = obj.get('class', {}).get('name', '?')
                status = (obj.get('status') or '').lower()
                if status != 'live':
                    await emit_failed(
                        context, event_queue,
                        f'{name} is not live (status={status!r}, class={klass!r})',
                    )
                    return
                ingested[klass] = obj

            stick = ingested.get('Stick')
            stone = ingested.get('Stone')
            if stick is None or stone is None:
                await emit_failed(
                    context, event_queue,
                    f'expected one Stick and one Stone; got classes={list(ingested.keys())}',
                )
                return

            await emit_text_artifact(
                context, event_queue, 'verify',
                f'verified inputs: Stick {stick["fileName"]} live, '
                f'Stone {stone["fileName"]} live',
            )

            await emit_working(context, event_queue, 'running CraftStonePick…')
            result = await self.dobjd.run_action_with_progress(
                PLUGIN,
                'CraftStonePick',
                [stick['fileName'], stone['fileName']],
                on_progress=make_progress_forwarder(
                    context, event_queue, action_label='CraftStonePick'),
            )
            pick_file = result['outputFiles'][0]

            await emit_working(context, event_queue, f'shipping {pick_file}…')
            pick_bytes = await self.dobjd.read_dobj_file(pick_file)

            await emit_text_artifact(
                context, event_queue, 'log',
                f'crafted StonePick {pick_file} ({len(pick_bytes)} bytes)',
            )
            await emit_dobj_artifact(
                context, event_queue,
                artifact_name='stonepick',
                file_name=pick_file,
                dobj_bytes=pick_bytes,
                note=f'StonePick delivered by craftsmith ({pick_file})',
            )
            await emit_completed(context, event_queue)

        except Exception as e:
            await emit_failed(context, event_queue, f'craftsmith failed: {e}')

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        raise Exception('cancel not supported')
