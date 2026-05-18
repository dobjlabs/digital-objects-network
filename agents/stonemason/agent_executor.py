"""Stonemason agent — LLM brain mines or fetches a Stone.

The LLM checks for a live Stone first; if missing, it bootstraps a
WoodPick (FindLog → CraftWood → CraftSticks → FindLog → CraftWood →
CraftWoodPick) then runs MineStoneWithWoodPick to produce a Stone.

Provider-agnostic via LiteLLM. Set LLM_MODEL (or STONEMASON_LLM for
per-agent override) to switch providers.
"""

from __future__ import annotations

import os
import re
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
    extract_text,
)
from shared.dobjd_client import DobjdClient  # noqa: E402
from shared.llm_brain import dobjd_mcp_url_from_http, pick_model, run_brain  # noqa: E402


SYSTEM_PROMPT = """You are the Stonemason agent in a bitcraft multi-agent network.
Your job: deliver one Stone when asked.

Procedure (follow exactly):
1. Call `list_inventory` first.
2. If the inventory contains a Stone with status "Live", you're done — pick one.
3. Otherwise, mining requires a WoodPick. Check inventory again for a live
   WoodPick. If none exists, bootstrap one:
   - `run_action` "FindLog" (no inputs)            → Log #1
   - `run_action` "CraftWood" (Log #1)             → Wood #1
   - `run_action` "CraftSticks" (Wood #1)          → Stick (one of two)
   - `run_action` "FindLog" (no inputs)            → Log #2
   - `run_action` "CraftWood" (Log #2)             → Wood #2
   - `run_action` "CraftWoodPick" (Wood #2, Stick) → WoodPick
4. With a WoodPick in hand, run `run_action` "MineStoneWithWoodPick"
   passing the WoodPick filename. This consumes a bit of the pick's
   durability and outputs a Stone.
5. Respond with ONLY the Stone's filename. No prose, no explanation, no
   "Here is the stone:". Just the bare filename, e.g.:
       craft-basics__stone_0xabc1234….dobj
   The harness will parse your final message for this exact pattern."""


_STONE_RE = re.compile(r'craft-basics__stone_0x[0-9a-fA-F]+\.dobj')


class StonemasonAgentExecutor(AgentExecutor):
    """LLM-driven Stone supplier."""

    def __init__(self) -> None:
        self.dobjd = DobjdClient()
        self.dobjd_http = os.environ.get('DOBJD_URL', 'http://127.0.0.1:7727').rstrip('/')

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> None:
        await ensure_task(context, event_queue)

        try:
            user_request = (
                extract_text(context.message).strip()
                or 'Please deliver one Stone.'
            )

            model = pick_model('STONEMASON_LLM')
            await emit_working(
                context, event_queue,
                f'stonemason brain online ({model}); planning Stone delivery…',
            )

            async def on_step(step: dict) -> None:
                await _forward_step(context, event_queue, step)

            mcp_url = dobjd_mcp_url_from_http(self.dobjd_http)
            final_text = await run_brain(
                system_prompt=SYSTEM_PROMPT,
                user_request=user_request,
                mcp_url=mcp_url,
                model=model,
                on_step=on_step,
            )

            stone_file = _parse_stone_filename(final_text)
            if not stone_file:
                await emit_failed(
                    context, event_queue,
                    f'stonemason: could not parse a Stone filename out of the LLM final response: {final_text[:200]!r}',
                )
                return

            inv = await self.dobjd.list_inventory()
            row = next((o for o in inv if o.get('fileName') == stone_file), None)
            if row is None:
                await emit_failed(
                    context, event_queue,
                    f'stonemason: LLM returned {stone_file!r} but it is not in inventory',
                )
                return
            status = (row.get('status') or '').lower()
            if status != 'live':
                await emit_failed(
                    context, event_queue,
                    f'stonemason: {stone_file} status is {status!r}, not live',
                )
                return

            stone_bytes = await self.dobjd.read_dobj_file(stone_file)
            await emit_text_artifact(
                context, event_queue, 'log',
                f'stonemason shipping Stone {stone_file} ({len(stone_bytes):,} bytes)',
            )
            await emit_dobj_artifact(
                context, event_queue,
                artifact_name='stone',
                file_name=stone_file,
                dobj_bytes=stone_bytes,
                note=f'Stone delivered by stonemason ({stone_file})',
            )

            removed = await self.dobjd.delete_dobj_file(stone_file)
            if removed:
                await emit_working(
                    context, event_queue,
                    f'stonemason: deleted {stone_file} from local inventory after delivery',
                )

            await emit_completed(context, event_queue)
        except Exception as e:
            await emit_failed(context, event_queue, f'stonemason failed: {e}')

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        raise Exception('cancel not supported')


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _parse_stone_filename(text: str) -> str:
    if not text:
        return ''
    m = _STONE_RE.search(text)
    return m.group(0) if m else ''


async def _forward_step(
    context: RequestContext, event_queue: EventQueue, step: dict
) -> None:
    kind = step.get('type')
    if kind == 'tool_call':
        name = step.get('name', '?')
        inp = step.get('input')
        await emit_working(context, event_queue, f'→ {name}({_compact(inp)})')
    elif kind == 'tool_result':
        name = step.get('name', '?')
        out = step.get('output_summary', '')
        await emit_working(context, event_queue, f'← {name} → {out}')
    elif kind == 'thought':
        text = (step.get('text') or '').strip()
        if text and len(text) < 200:
            await emit_working(context, event_queue, f'💭 {text}')


def _compact(value) -> str:
    if value is None:
        return ''
    s = str(value)
    return s if len(s) <= 120 else s[:117] + '…'
