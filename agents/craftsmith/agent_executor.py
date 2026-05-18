"""Craftsmith agent — LLM brain assembles a StonePick from received inputs.

The harness:
  1. Pulls the Stick + Stone FileParts out of the incoming A2A message
  2. Writes them into the local dobjd's objects dir
  3. Calls list_inventory to confirm both are live on chain
  4. Hands the brain the verified input filenames; the LLM runs
     `run_action` for `CraftStonePick` (inputs in order: Stone, Stick)
     and returns the StonePick filename
  5. Ships the StonePick as a FilePart, then unlinks it locally so we
     don't re-ship the same one

Provider-agnostic via LiteLLM. Set LLM_MODEL (or CRAFTSMITH_LLM for
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
    extract_file_parts,
)
from shared.brain_hub import BrainEventHub  # noqa: E402
from shared.dobjd_client import DobjdClient  # noqa: E402
from shared.llm_brain import dobjd_mcp_url_from_http, pick_model, run_brain  # noqa: E402


def _system_prompt(stone_file: str, stick_file: str) -> str:
    return f"""You are the Craftsmith agent in a bitcraft multi-agent network.
Your job: assemble a StonePick from a Stone and a Stick that have already
been ingested into your dobjd's inventory.

The two input files are:
  - Stone: {stone_file}
  - Stick: {stick_file}

Procedure (follow exactly):
1. Call `list_inventory` first to confirm both files are present and live.
2. Run `run_action` with:
     action_id = "CraftStonePick"
     input_object_paths = ["{stone_file}", "{stick_file}"]
   NOTE: the order matters — Stone first, then Stick.
3. The action produces a StonePick. Respond with ONLY its filename. No
   prose, no "Here is the stone pick:". Just the bare filename:
       craft-basics__stonepick_0xabc1234….dobj
   The harness will parse your final message for this exact pattern."""


_STONEPICK_RE = re.compile(r'craft-basics__stonepick_0x[0-9a-fA-F]+\.dobj')


class CraftsmithAgentExecutor(AgentExecutor):
    """LLM-driven StonePick assembler."""

    def __init__(self, brain_hub: BrainEventHub | None = None) -> None:
        self.dobjd = DobjdClient()
        self.dobjd_http = os.environ.get('DOBJD_URL', 'http://127.0.0.1:7737').rstrip('/')
        self.brain_hub = brain_hub

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> None:
        await ensure_task(context, event_queue)

        try:
            # 1. Pull file parts out of the inbound request
            file_parts = extract_file_parts(context.message)
            if len(file_parts) < 2:
                await emit_failed(
                    context, event_queue,
                    f'craftsmith needs 2 FileParts (Stick + Stone), got {len(file_parts)}',
                )
                return

            # 2. Ingest both into local dobjd's objects dir
            await emit_working(context, event_queue, 'ingesting received inputs into local dobjd…')
            for name, data in file_parts:
                await self.dobjd.write_dobj_file(name, data)

            # 3. Inventory check — confirm both are recognized and live
            inv = await self.dobjd.list_inventory()
            by_file = {o.get('fileName'): o for o in inv}
            stick_file = stone_file = ''
            for name, _ in file_parts:
                obj = by_file.get(name)
                if obj is None:
                    await emit_failed(
                        context, event_queue,
                        f'craftsmith: dobjd did not recognize {name} after write',
                    )
                    return
                status = (obj.get('status') or '').lower()
                klass = obj.get('class', {}).get('name', '?')
                if status != 'live':
                    await emit_failed(
                        context, event_queue,
                        f'craftsmith: {name} status is {status!r} (class={klass!r})',
                    )
                    return
                if klass == 'Stick':
                    stick_file = name
                elif klass == 'Stone':
                    stone_file = name

            if not stick_file or not stone_file:
                await emit_failed(
                    context, event_queue,
                    f'craftsmith: expected one Stick + one Stone; got files={[n for n, _ in file_parts]}',
                )
                return

            await emit_text_artifact(
                context, event_queue, 'verify',
                f'verified inputs: Stick {stick_file} live, Stone {stone_file} live',
            )

            # 4. Hand the verified filenames to the LLM brain
            model = pick_model('CRAFTSMITH_LLM')
            await emit_working(
                context, event_queue,
                f'craftsmith brain online ({model}); planning StonePick assembly…',
            )

            async def on_step(step: dict) -> None:
                if self.brain_hub is not None:
                    self.brain_hub.publish({'agent': 'craftsmith', **step})
                await _forward_step(context, event_queue, step)

            mcp_url = dobjd_mcp_url_from_http(self.dobjd_http)
            final_text = await run_brain(
                system_prompt=_system_prompt(stone_file, stick_file),
                user_request='Please assemble a StonePick from the provided inputs.',
                mcp_url=mcp_url,
                model=model,
                on_step=on_step,
                agent_label='craftsmith',
            )

            pick_file = _parse_stonepick_filename(final_text)
            if not pick_file:
                await emit_failed(
                    context, event_queue,
                    f'craftsmith: could not parse a StonePick filename out of the LLM final response: {final_text[:200]!r}',
                )
                return

            # 5. Verify, ship, and unlink
            inv = await self.dobjd.list_inventory()
            row = next((o for o in inv if o.get('fileName') == pick_file), None)
            if row is None:
                await emit_failed(
                    context, event_queue,
                    f'craftsmith: LLM returned {pick_file!r} but it is not in inventory',
                )
                return
            status = (row.get('status') or '').lower()
            if status != 'live':
                await emit_failed(
                    context, event_queue,
                    f'craftsmith: {pick_file} status is {status!r}, not live',
                )
                return

            pick_bytes = await self.dobjd.read_dobj_file(pick_file)
            await emit_text_artifact(
                context, event_queue, 'log',
                f'craftsmith shipping StonePick {pick_file} ({len(pick_bytes):,} bytes)',
            )
            await emit_dobj_artifact(
                context, event_queue,
                artifact_name='stonepick',
                file_name=pick_file,
                dobj_bytes=pick_bytes,
                note=f'StonePick delivered by craftsmith ({pick_file})',
            )

            removed = await self.dobjd.delete_dobj_file(pick_file)
            if removed:
                await emit_working(
                    context, event_queue,
                    f'craftsmith: deleted {pick_file} from local inventory after delivery',
                )

            await emit_completed(context, event_queue)
        except Exception as e:
            await emit_failed(context, event_queue, f'craftsmith failed: {e}')

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        raise Exception('cancel not supported')


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _parse_stonepick_filename(text: str) -> str:
    if not text:
        return ''
    m = _STONEPICK_RE.search(text)
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
