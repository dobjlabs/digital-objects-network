"""Stonemason agent — mines a Stone and ships it.

MineStone requires a WoodPick. The executor first checks local inventory
for a live WoodPick; if none exists it bootstraps one by running
FindLog → CraftWood → CraftSticks → FindLog → CraftWood → CraftWoodPick.
Then it runs MineStone (which consumes some of the pick's durability) and
ships the resulting Stone as a FilePart.

Framework slot in the demo: would be LangGraph + Claude in a full
interop build. Framework-agnostic here.
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
    make_progress_forwarder,
)
from shared.dobjd_client import DobjdClient  # noqa: E402


PLUGIN = 'craft-basics'


class StonemasonAgentExecutor(AgentExecutor):
    """Mines a Stone (bootstrapping a WoodPick if needed)."""

    def __init__(self) -> None:
        self.dobjd = DobjdClient()

    async def _run(
        self,
        context: RequestContext,
        event_queue: EventQueue,
        action: str,
        inputs: list[str],
    ) -> dict:
        return await self.dobjd.run_action_with_progress(
            PLUGIN, action, inputs,
            on_progress=make_progress_forwarder(
                context, event_queue, action_label=action),
        )

    async def _ensure_woodpick(
        self, context: RequestContext, event_queue: EventQueue
    ) -> str:
        existing = await self.dobjd.find_object('WoodPick', require_status='live')
        if existing:
            return existing['fileName']

        await emit_working(context, event_queue, 'no WoodPick on hand — bootstrapping…')

        # First Log → Wood → Sticks (yields 2 Sticks)
        log1 = await self._run(context, event_queue, 'FindLog', [])
        log1_file = log1['outputFiles'][0]
        wood1 = await self._run(context, event_queue, 'CraftWood', [log1_file])
        wood1_file = wood1['outputFiles'][0]
        sticks = await self._run(context, event_queue, 'CraftSticks', [wood1_file])
        stick_file = sticks['outputFiles'][0]

        # Second Log → Wood (for the pick head)
        log2 = await self._run(context, event_queue, 'FindLog', [])
        log2_file = log2['outputFiles'][0]
        wood2 = await self._run(context, event_queue, 'CraftWood', [log2_file])
        wood2_file = wood2['outputFiles'][0]

        await emit_working(context, event_queue, 'assembling WoodPick…')
        pick = await self._run(
            context, event_queue, 'CraftWoodPick', [wood2_file, stick_file]
        )
        return pick['outputFiles'][0]

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> None:
        await ensure_task(context, event_queue)

        try:
            woodpick_file = await self._ensure_woodpick(context, event_queue)

            await emit_working(context, event_queue, f'mining stone with {woodpick_file}…')
            mine_result = await self._run(
                context, event_queue, 'MineStone', [woodpick_file]
            )
            # MineStone outputs Stone (and a damaged-but-not-nullified WoodPick
            # if the action chooses that shape). Identify the Stone by class.
            stone_file = None
            for f in mine_result['outputFiles']:
                summary = await self._read_summary(f)
                if summary and summary.get('class', {}).get('name') == 'Stone':
                    stone_file = f
                    break
            if stone_file is None:
                # Fall back to first output
                stone_file = mine_result['outputFiles'][0]

            await emit_working(context, event_queue, f'shipping {stone_file}…')
            stone_bytes = await self.dobjd.read_dobj_file(stone_file)

            await emit_text_artifact(
                context, event_queue, 'log',
                f'mined Stone {stone_file} ({len(stone_bytes)} bytes)',
            )
            await emit_dobj_artifact(
                context, event_queue,
                artifact_name='stone',
                file_name=stone_file,
                dobj_bytes=stone_bytes,
                note=f'Stone delivered by stonemason ({stone_file})',
            )
            await emit_completed(context, event_queue)
        except Exception as e:
            await emit_failed(context, event_queue, f'stonemason failed: {e}')

    async def _read_summary(self, file_name: str) -> dict | None:
        # Avoid a separate REST call — find it in inventory.
        for obj in await self.dobjd.list_inventory():
            if obj.get('fileName') == file_name:
                return obj
        return None

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        raise Exception('cancel not supported')
