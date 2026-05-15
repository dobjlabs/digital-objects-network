"""Lumberjack agent — runs the wood chain and ships a Stick.

On every `message/send`, the executor runs FindLog → CraftWood →
CraftSticks against its local dobjd, picks one of the two output Sticks,
reads the raw .dobj off disk, and returns it as a FilePart artifact.

Streams Working-state updates between each action so the caller can see
progress in real time.

Framework slot in the demo: would be Google ADK + Gemini in a "full
interop" build. For now we keep the executor framework-agnostic — drop
in your model client of choice inside the executor body.
"""

from __future__ import annotations

import sys
from pathlib import Path

# Make `shared` importable when run as `python -m lumberjack` from the
# a2a-agent root.
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
    make_progress_forwarder,
)
from shared.dobjd_client import DobjdClient  # noqa: E402


PLUGIN = 'craft-basics'


class LumberjackAgentExecutor(AgentExecutor):
    """Crafts a Stick from scratch and ships it as a FilePart."""

    def __init__(self) -> None:
        self.dobjd = DobjdClient()

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> None:
        await ensure_task(context, event_queue)

        try:
            await emit_working(context, event_queue, 'chopping a log…')
            log_result = await self.dobjd.run_action_with_progress(
                PLUGIN, 'FindLog', [],
                on_progress=make_progress_forwarder(
                    context, event_queue, action_label='FindLog'),
            )
            log_file = log_result['outputFiles'][0]

            await emit_working(context, event_queue, f'refining {log_file} into wood…')
            wood_result = await self.dobjd.run_action_with_progress(
                PLUGIN, 'CraftWood', [log_file],
                on_progress=make_progress_forwarder(
                    context, event_queue, action_label='CraftWood'),
            )
            wood_file = wood_result['outputFiles'][0]

            await emit_working(context, event_queue, f'splitting {wood_file} into sticks…')
            sticks_result = await self.dobjd.run_action_with_progress(
                PLUGIN, 'CraftSticks', [wood_file],
                on_progress=make_progress_forwarder(
                    context, event_queue, action_label='CraftSticks'),
            )
            # CraftSticks produces two Sticks; ship one, keep the other.
            stick_file = sticks_result['outputFiles'][0]

            await emit_working(context, event_queue, f'shipping {stick_file}…')
            stick_bytes = await self.dobjd.read_dobj_file(stick_file)

            await emit_text_artifact(
                context, event_queue, 'log',
                f'crafted Stick {stick_file} ({len(stick_bytes)} bytes), '
                f'one extra Stick retained',
            )
            await emit_dobj_artifact(
                context, event_queue,
                artifact_name='stick',
                file_name=stick_file,
                dobj_bytes=stick_bytes,
                note=f'Stick delivered by lumberjack ({stick_file})',
            )
            await emit_completed(context, event_queue)
        except Exception as e:
            await emit_failed(context, event_queue, f'lumberjack failed: {e}')

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        raise Exception('cancel not supported')
