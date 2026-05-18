"""Lumberjack agent — LLM brain decides whether to use existing inventory
or craft a Stick from scratch.

The system prompt tells the LLM to:
  1. Call list_inventory first
  2. If a live Stick exists, return its filename
  3. Otherwise run FindLog → CraftWood → CraftSticks
  4. Return ONLY the chosen Stick's filename in its final message

The harness then reads the .dobj bytes, ships them as a FilePart, and
deletes the Stick from local inventory so it can't be sent twice.

Provider-agnostic via LiteLLM. Set LLM_MODEL (or LUMBERJACK_LLM for
per-agent override) to switch providers.
"""

from __future__ import annotations

import asyncio
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


SYSTEM_PROMPT = """You are the Lumberjack agent in a bitcraft multi-agent network.
Your job: deliver one Stick when asked.

Procedure (follow exactly):
1. Call `list_inventory` first.
2. If the inventory contains a Stick with status "Live", you're done — pick one.
3. Otherwise, craft one by running these actions IN ORDER:
   - `run_action` with action_id "FindLog" and no inputs
   - `run_action` with action_id "CraftWood" using the Log file from step 1's output
   - `run_action` with action_id "CraftSticks" using the Wood file from step 2's output
   CraftSticks produces TWO Sticks — pick either one.
4. After you have a live Stick, respond with ONLY its filename. No prose, no
   explanation, no "Here is the stick:". Just the bare filename, e.g.:
       craft-basics__stick_0xabc1234….dobj
   The harness will parse your final message for this exact pattern."""


_STICK_RE = re.compile(r'craft-basics__stick_0x[0-9a-fA-F]+\.dobj')


class LumberjackAgentExecutor(AgentExecutor):
    """LLM-driven Stick supplier."""

    def __init__(self) -> None:
        self.dobjd = DobjdClient()
        self.dobjd_http = os.environ.get('DOBJD_URL', 'http://127.0.0.1:7717').rstrip('/')

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> None:
        await ensure_task(context, event_queue)

        try:
            user_request = (
                extract_text(context.message).strip()
                or 'Please deliver one Stick.'
            )

            model = pick_model('LUMBERJACK_LLM')
            await emit_working(
                context, event_queue,
                f'lumberjack brain online ({model}); planning Stick delivery…',
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

            stick_file = _parse_stick_filename(final_text)
            if not stick_file:
                await emit_failed(
                    context, event_queue,
                    f'lumberjack: could not parse a Stick filename out of the LLM final response: {final_text[:200]!r}',
                )
                return

            # Confirm the Stick is actually in local inventory and live before
            # shipping. The LLM could have hallucinated a name.
            inv = await self.dobjd.list_inventory()
            row = next((o for o in inv if o.get('fileName') == stick_file), None)
            if row is None:
                await emit_failed(
                    context, event_queue,
                    f'lumberjack: LLM returned {stick_file!r} but it is not in inventory',
                )
                return
            status = (row.get('status') or '').lower()
            if status != 'live':
                await emit_failed(
                    context, event_queue,
                    f'lumberjack: {stick_file} status is {status!r}, not live',
                )
                return

            stick_bytes = await self.dobjd.read_dobj_file(stick_file)
            await emit_text_artifact(
                context, event_queue, 'log',
                f'lumberjack shipping Stick {stick_file} ({len(stick_bytes):,} bytes)',
            )
            await emit_dobj_artifact(
                context, event_queue,
                artifact_name='stick',
                file_name=stick_file,
                dobj_bytes=stick_bytes,
                note=f'Stick delivered by lumberjack ({stick_file})',
            )

            # Delete-after-send: remove from local inventory so we don't
            # re-ship the same Stick on a follow-up request.
            removed = await self.dobjd.delete_dobj_file(stick_file)
            if removed:
                await emit_working(
                    context, event_queue,
                    f'lumberjack: deleted {stick_file} from local inventory after delivery',
                )

            await emit_completed(context, event_queue)
        except Exception as e:
            await emit_failed(context, event_queue, f'lumberjack failed: {e}')

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        raise Exception('cancel not supported')


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _parse_stick_filename(text: str) -> str:
    """Pull the Stick filename out of the LLM's final response. We told the
    LLM to return only the bare filename, but be tolerant of stray prose."""
    if not text:
        return ''
    m = _STICK_RE.search(text)
    return m.group(0) if m else ''


async def _forward_step(
    context: RequestContext, event_queue: EventQueue, step: dict
) -> None:
    """Convert a brain event into a one-line A2A working update."""
    kind = step.get('type')
    if kind == 'tool_call':
        name = step.get('name', '?')
        inp = step.get('input')
        await emit_working(
            context, event_queue,
            f'→ {name}({_compact(inp)})',
        )
    elif kind == 'tool_result':
        name = step.get('name', '?')
        out = step.get('output_summary', '')
        await emit_working(
            context, event_queue,
            f'← {name} → {out}',
        )
    elif kind == 'thought':
        text = (step.get('text') or '').strip()
        if text and len(text) < 200:  # skip the giant final filename message
            await emit_working(context, event_queue, f'💭 {text}')


def _compact(value) -> str:
    """One-line representation of a tool input for progress emission."""
    if value is None:
        return ''
    s = str(value)
    return s if len(s) <= 120 else s[:117] + '…'
