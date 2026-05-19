"""Concierge agent — BeeAI Framework `RequirementAgent` orchestrates the
three specialist peers.

Different framework choice from the specialists by design. The specialists
need an MCP tool source so they use LangChain + `langchain-mcp-adapters`
(over LiteLLM). The Concierge has no MCP needs — it just delegates to
three peer A2A agents — so it gets BeeAI's `RequirementAgent` +
`ThinkTool` + `ConditionalRequirement` pattern, mirroring the
A2AWalkthrough's `a2a_healthcare_agent.py` orchestrator. Two frameworks
in one demo on purpose: it shows A2A is provider- *and* framework-agnostic.

Peer calls go through custom BeeAI `Tool`s in `shared/peer_tools.py`
that wrap our existing `send_and_stream` A2A client — we can't use
BeeAI's stock `HandoffTool` because that expects a BeeAI `Runnable`
target, not an external A2A endpoint.

Provider via LLM_MODEL / CONCIERGE_LLM env vars, auto-translated from
LiteLLM's `provider/model` shape to BeeAI's `provider:model` shape so
the same env var works for both the specialists and the Concierge.
"""

from __future__ import annotations

import os
import re
import sys
import traceback
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from a2a.server.agent_execution import AgentExecutor, RequestContext  # noqa: E402
from a2a.server.events import EventQueue  # noqa: E402

from beeai_framework.agents.requirement import RequirementAgent  # noqa: E402
from beeai_framework.agents.requirement.requirements.conditional import (  # noqa: E402
    ConditionalRequirement,
)
from beeai_framework.backend.chat import ChatModel  # noqa: E402
from beeai_framework.backend.types import ChatModelParameters  # noqa: E402
from beeai_framework.emitter import EmitterOptions  # noqa: E402
from beeai_framework.errors import FrameworkError  # noqa: E402
from beeai_framework.memory import UnconstrainedMemory  # noqa: E402
from beeai_framework.tools.think import ThinkTool  # noqa: E402
from beeai_framework.tools.types import ToolOutput  # noqa: E402

from shared.a2a_helpers import (  # noqa: E402
    emit_completed,
    emit_dobj_artifact,
    emit_failed,
    emit_text_artifact,
    emit_working,
    ensure_task,
    extract_text,
)
from shared.brain_hub import BrainEventHub  # noqa: E402
from shared.dobjd_client import DobjdClient  # noqa: E402
from shared.llm_brain import pick_model  # noqa: E402
from shared.peer_tools import flatten_chunk_text, make_peer_tools  # noqa: E402


_STONEPICK_RE = re.compile(r'craft-basics__stonepick_0x[0-9a-fA-F]+\.dobj')


INSTRUCTIONS = """You are the Concierge agent in a bitcraft multi-agent network.
Your job: deliver a fully-anchored StonePick by coordinating three peer agents.

You have these tools (each calls a peer over A2A, ingests the returned
.dobj into this concierge's local dobjd, and verifies it's live on chain
before returning):
  - request_stick_from_lumberjack    →  Stick filename
  - request_stone_from_stonemason    →  Stone filename
  - craft_stonepick_with_craftsmith  (stick_file, stone_file) → StonePick filename

You also have a `think` tool — call it once at the start to plan, then
proceed to the peer calls.

Procedure:
1. Call `think` once with your plan.
2. Call BOTH request_stick_from_lumberjack AND request_stone_from_stonemason
   (the framework runs independent tool calls in parallel where it can).
3. Pass the two filenames they return to craft_stonepick_with_craftsmith.
4. Respond with ONLY the StonePick filename. No prose, no
   "Here is the stone pick:". Just the bare filename, e.g.:
       craft-basics__stonepick_0xabc1234….dobj
The harness will parse your final message for this exact pattern."""


# LiteLLM provider names that don't match BeeAI's provider IDs exactly.
# Add aliases here as you adopt more providers.
_LITELLM_TO_BEEAI_PROVIDER = {
    'vertex_ai': 'vertexai',
}


def _to_beeai_model_string(litellm_style: str) -> str:
    """Translate `provider/model` (LiteLLM) → `provider:model` (BeeAI).

    Lets the user set `LLM_MODEL=anthropic/claude-opus-4-7` once and
    have it work for both the LangChain specialists and the BeeAI
    Concierge. If the env var already uses BeeAI's colon shape (e.g.
    `anthropic:claude-opus-4-7`), it's returned unchanged.
    """
    if ':' in litellm_style:
        return litellm_style
    if '/' not in litellm_style:
        return litellm_style
    provider, _, model = litellm_style.partition('/')
    provider = _LITELLM_TO_BEEAI_PROVIDER.get(provider, provider)
    return f'{provider}:{model}'


class ConciergeAgentExecutor(AgentExecutor):
    """LLM-driven orchestrator built on BeeAI's RequirementAgent."""

    def __init__(self, brain_hub: BrainEventHub | None = None) -> None:
        self.dobjd = DobjdClient()
        self.dobjd_http = os.environ.get('DOBJD_URL', 'http://127.0.0.1:7747').rstrip('/')
        self.brain_hub = brain_hub

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> None:
        await ensure_task(context, event_queue)

        try:
            user_request = (
                extract_text(context.message).strip()
                or 'I want a stone pick'
            )

            litellm_model = pick_model('CONCIERGE_LLM')
            beeai_model = _to_beeai_model_string(litellm_model)
            _log(f'brain online (beeai → {beeai_model})')
            await emit_working(
                context, event_queue,
                f'concierge brain online (beeai → {beeai_model}); planning…',
            )

            # ----- Forward each peer stream chunk as a [peer] working line ----
            # We forward to A2A working updates ONLY (so test_client users
            # see live "what is Lumberjack/Stonemason doing right now"
            # progress while the Concierge is blocked on the peer call).
            #
            # We deliberately do NOT push these to the Concierge brain hub
            # or stdout — the specialist's own brain hub + stdout already
            # publish those exact events on its own dashboard card and
            # mprocs pane. Duplicating them on the Concierge card would
            # make the orchestrator look like it's doing the specialists'
            # work. The Concierge card stays focused on its own tool calls.
            async def on_peer_chunk(peer_label: str, chunk) -> None:
                text = flatten_chunk_text(chunk)
                if not text:
                    return
                await emit_working(
                    context, event_queue, f'[{peer_label}] {text}',
                )

            stick_tool, stone_tool, craft_tool = make_peer_tools(
                self.dobjd, on_peer_chunk=on_peer_chunk,
            )
            think_tool = ThinkTool()

            # Two non-default model knobs:
            #
            # `temperature=1` — Claude Opus 4.x extended-thinking models
            # reject `temperature=0` (BeeAI's default) with "temperature is
            # deprecated for this model". Setting to 1 is the model-required
            # value and a no-op for providers that accept any value.
            #
            # `allow_parallel_tool_calls=True` — needed for `parallel`
            # but NOT sufficient. See `_force_parallel_tool_calls()` below
            # for the actual fix.
            chat_model = ChatModel.from_name(
                beeai_model,
                ChatModelParameters(temperature=1),
                allow_parallel_tool_calls=True,
            )
            _force_parallel_tool_calls(chat_model)
            agent = RequirementAgent(
                name='Concierge',
                description='Orchestrates Lumberjack, Stonemason, Craftsmith to deliver a StonePick.',
                llm=chat_model,
                memory=UnconstrainedMemory(),
                tools=[think_tool, stick_tool, stone_tool, craft_tool],
                requirements=[
                    # think first
                    ConditionalRequirement(
                        think_tool,
                        force_at_step=1,
                        consecutive_allowed=False,
                    ),
                    # each peer call exactly once
                    ConditionalRequirement(stick_tool, max_invocations=1),
                    ConditionalRequirement(stone_tool, max_invocations=1),
                    # craft only after both inputs are gathered, exactly once
                    ConditionalRequirement(
                        craft_tool,
                        only_after=[stick_tool, stone_tool],
                        max_invocations=1,
                    ),
                ],
                role='Concierge',
                instructions=INSTRUCTIONS,
            )

            # Surface BeeAI tool start/success + chat-model events as A2A
            # Working updates + brain-hub dashboard entries. `agent.emitter`
            # only sees agent-level events (start/success/final_answer) —
            # tool emitters are SIBLINGS under Emitter.root(), not children
            # of agent.emitter. `Run.observe()` hooks the per-run emitter
            # which DOES see everything (verified empirically: agent.emitter
            # → 9 events, Run.observe → 40 including tool.think.*,
            # backend.anthropic.chat.*, requirement.conditionthink.*).
            handler = self._make_emitter_handler(context, event_queue)
            response = await agent.run(user_request).observe(
                lambda em: em.on(
                    '*.*', handler, EmitterOptions(match_nested=True),
                )
            )
            final_text = response.last_message.text if response else ''

            pick_file = _parse_stonepick_filename(final_text)
            if not pick_file:
                await emit_failed(
                    context, event_queue,
                    f'concierge: could not parse a StonePick filename out of '
                    f'the LLM final response: {final_text[:200]!r}',
                )
                return

            # The peer tool already ingested + verified, but a final inventory
            # check protects against the LLM hallucinating a different name.
            inv = await self.dobjd.list_inventory()
            row = next((o for o in inv if o.get('fileName') == pick_file), None)
            if row is None:
                await emit_failed(
                    context, event_queue,
                    f'concierge: LLM returned {pick_file!r} but it is not in inventory',
                )
                return
            status = (row.get('status') or '').lower()
            if status != 'live':
                await emit_failed(
                    context, event_queue,
                    f'concierge: {pick_file} status is {status!r}, not live',
                )
                return

            pick_bytes = await self.dobjd.read_dobj_file(pick_file)
            await emit_text_artifact(
                context, event_queue, 'summary',
                f'StonePick delivered.\n  pick: {pick_file} ({len(pick_bytes):,} bytes)\n'
                'verified live on chain.',
            )
            await emit_dobj_artifact(
                context, event_queue,
                artifact_name='stonepick',
                file_name=pick_file,
                dobj_bytes=pick_bytes,
                note=f'StonePick {pick_file} verified live',
            )
            await emit_completed(context, event_queue)

        except Exception as e:
            # BeeAI FrameworkError.__str__ collapses to just the class label —
            # walk the cause chain so the real underlying error (bad model id,
            # auth failure, API 4xx, etc.) actually reaches the user.
            detail = _explain_exception(e)
            # Full traceback to stdout so run_all.sh / mprocs captures it.
            print('[concierge] EXCEPTION:', file=sys.stderr, flush=True)
            traceback.print_exc()
            print(f'[concierge] explained: {detail}', file=sys.stderr, flush=True)
            await emit_failed(context, event_queue, f'concierge failed: {detail}')

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        raise Exception('cancel not supported')

    # ---------------------------------------------------------------------
    # BeeAI emitter handler — translates framework events into the same
    # `{type: tool_call|tool_result|thought}` shape the specialists publish,
    # so the preview HTML doesn't need a separate code path for the Concierge.
    # ---------------------------------------------------------------------

    def _make_emitter_handler(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> Any:
        async def on_event(data: Any, event: Any) -> None:
            kind = _classify_event(event, data)
            if kind is None:
                return

            payload: dict[str, Any] = {'agent': 'concierge', **kind}

            # A2A working line — short, single-line. Skip empty payloads.
            line = _summarize_for_working(kind)
            if line:
                # stdout for run_all.sh / mprocs, mirroring the specialists'
                # `_log()` format from shared/llm_brain.py.
                _log(line)
                await emit_working(context, event_queue, line)

            if self.brain_hub is not None:
                self.brain_hub.publish(payload)

        return on_event


# ---------------------------------------------------------------------------
# Event classification helpers
# ---------------------------------------------------------------------------

def _classify_event(event: Any, data: Any) -> dict[str, Any] | None:
    """Map BeeAI emitter event → the dashboard's
    `{type: tool_call | tool_result | thought, ...}` shape.

    BeeAI's `Run.observe()` stream is fire-hose-y: every tool call shows
    up at TWO paths (`run.tool.<name>.<phase>` from the RunContext wrap
    plus `tool.<name>.<phase>` from the tool's own emitter), plus
    framework events for requirement evaluation, chat model rounds, and
    the synthetic FinalAnswerTool. We filter to the cleanest, lowest-
    noise subset:

      - `tool.<name>.start`     → tool_call
      - `tool.<name>.success`   → tool_result
      - `agent.requirement.final_answer` → thought (the actual LLM reply)

    Skips `run.tool.*` (duplicates of `tool.*`), `backend.*.chat.*` (chat
    model rounds — noisy, no per-step value), and the FinalAnswerTool's
    own tool events (its content overlaps with `final_answer`).
    """
    name = getattr(event, 'name', '')
    path = getattr(event, 'path', '')

    # 1) Final answer — surface as a "thought" the dashboard styles.
    if path == 'agent.requirement.final_answer' and data is not None:
        text = getattr(data, 'output', '') or ''
        if text:
            return {'type': 'thought', 'text': text}

    # 2) Tool events only at the bare `tool.<name>.*` namespace.
    if not path.startswith('tool.'):
        return None
    # Skip the synthetic FinalAnswerTool — duplicates the final_answer event.
    if path.startswith('tool.final_answer.'):
        return None

    creator = getattr(event, 'creator', None)
    tool_name = getattr(creator, 'name', None) or type(creator).__name__

    if name == 'start' and data is not None and hasattr(data, 'input'):
        return {
            'type': 'tool_call',
            'name': tool_name,
            'input': _dump(data.input),
        }
    if name == 'success' and data is not None and hasattr(data, 'output'):
        return {
            'type': 'tool_result',
            'name': tool_name,
            'output_summary': _stringify_output(data.output),
        }
    return None


def _summarize_for_working(kind: dict[str, Any]) -> str:
    t = kind.get('type')
    if t == 'tool_call':
        return f'→ {kind.get("name", "?")}({_compact(kind.get("input"))})'
    if t == 'tool_result':
        out = kind.get('output_summary', '')
        return f'← {kind.get("name", "?")} → {out}'
    if t == 'thought':
        text = (kind.get('text') or '').strip()
        # The final answer is the bare filename — short. If a model emits
        # a long thought it'd flood the UI, so cap it.
        if text and len(text) < 200:
            return f'💭 {text}'
    return ''


def _stringify_output(output: Any) -> str:
    if output is None:
        return ''
    if isinstance(output, ToolOutput):
        s = output.get_text_content()
    else:
        s = str(output)
    return s if len(s) <= 200 else s[:197] + '…'


def _dump(value: Any) -> Any:
    """Render a Pydantic input model (or anything else) as a JSON-safe dict.

    Pydantic models from BeeAI tool schemas serialize cleanly; primitives
    pass through; everything else stringifies.
    """
    if value is None:
        return ''
    dump = getattr(value, 'model_dump', None)
    if callable(dump):
        try:
            return dump()
        except Exception:
            pass
    if isinstance(value, (str, int, float, bool, dict, list)):
        return value
    return str(value)


def _compact(value: Any) -> str:
    if value is None:
        return ''
    s = str(value)
    return s if len(s) <= 120 else s[:117] + '…'


def _force_parallel_tool_calls(chat_model: Any) -> None:
    """Force `parallel_tool_calls=True` on every LiteLLM call this model makes.

    Works around a BeeAI bug: `allow_parallel_tool_calls=True` on a
    `ChatModel` only governs whether the *runner* accepts multi-call
    responses (backend/chat.py:658). It is NEVER propagated to the
    `ChatModelInput.parallel_tool_calls` field that LiteLLM actually
    sends, because `RequirementAgent`'s runner builds `ChatModelOptions`
    without `parallel_tool_calls` set. Result: `input.parallel_tool_calls`
    stays `None`, line 284 of `adapters/litellm/chat.py` does
    `bool(None) → False`, and LiteLLM translates `parallel_tool_calls=False`
    into Anthropic's `tool_choice.disable_parallel_tool_use=True` — which
    EXPLICITLY forbids parallel tool calls. So even though our prompt
    says "call both in parallel" and we set `allow_parallel_tool_calls=True`,
    Anthropic is told the opposite.

    The fix wraps `chat_model._transform_input` so that just before
    LiteLLM serializes the request, `input.parallel_tool_calls` flips
    from None to True (only when None, so an explicit caller-provided
    value still wins). `ChatModelInput.model_config` has `frozen=False`
    so mutation is allowed.

    Verified empirically: without this patch the LiteLLM dict carries
    `parallel_tool_calls=False`; with it, `True`.
    """
    original = chat_model._transform_input

    def patched(input):
        if input.parallel_tool_calls is None:
            input.parallel_tool_calls = True
        return original(input)

    chat_model._transform_input = patched


def _log(message: str) -> None:
    """Print one brain event to stdout with the `[concierge] ` prefix,
    flushed immediately. Mirrors `shared/llm_brain.py::_log()` so the
    Concierge's lines look identical to the specialists' in mprocs."""
    print(f'[concierge] {message}', file=sys.stdout, flush=True)


def _parse_stonepick_filename(text: str) -> str:
    if not text:
        return ''
    m = _STONEPICK_RE.search(text)
    return m.group(0) if m else ''


def _explain_exception(e: BaseException) -> str:
    """Format an exception including its full __cause__ chain.

    BeeAI's FrameworkError has an .explain() method that walks the cause
    chain and includes each layer's context dict (e.g. a ChatModelError
    contains the raw LiteLLM response, an HTTPStatusError the server
    body). Plain Pythonics fall back to a manual `cause: …` walk so we
    still surface the root cause of LiteLLM 4xx / Anthropic SDK errors.
    """
    if isinstance(e, FrameworkError):
        try:
            return e.explain()
        except Exception:
            pass  # fall through to the manual walk
    parts: list[str] = [f'{type(e).__name__}: {e}']
    cur: BaseException | None = e.__cause__
    depth = 1
    while cur is not None and depth < 8:
        parts.append(f'  caused by {type(cur).__name__}: {cur}')
        cur = cur.__cause__
        depth += 1
    return '\n'.join(parts)
