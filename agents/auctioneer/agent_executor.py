"""Auctioneer agent — LLM-driven sealed-bid router.

Every agent in the network is LLM + tools + prompt. The Auctioneer is
no exception. Its prompt says "run an auction" and gives it three tools:

  - `list_candidates`        — what URLs can I auction across?
  - `fetch_agent_card(url)`  — what does that peer advertise?
  - `delegate_request(url, request)` — send the work to a chosen peer

The bidding/comparison/selection logic is no longer hard-coded — the
LLM reads each candidate's `price:N` tag from its agent card and picks
the cheapest. Adding more candidates means editing `registry.py` and
nothing else; the same prompt scales.

The Auctioneer still has no dobjd — it doesn't craft, just routes.
`delegate_request` captures the winning peer's FilePart into the
executor's `_captured_file` slot; after the LLM's run completes the
executor re-emits it under the Auctioneer's task_id so the Concierge
sees a clean reply.

Side-effect events for the dashboard (`bid`, `winner`, `delegating`,
`auction_complete`) are published by the tools themselves so the
preview HTML still gets cinematic auction lines on top of the generic
BeeAI tool_call / tool_result stream.

Framework: BeeAI (same as the Concierge — both are routing agents
that don't need MCP). Provider via AUCTIONEER_LLM / LLM_MODEL env vars.
"""

from __future__ import annotations

import os
import sys
import traceback
from collections.abc import Awaitable, Callable
from functools import cached_property
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import httpx  # noqa: E402

from pydantic import BaseModel, Field  # noqa: E402

from a2a.server.agent_execution import AgentExecutor, RequestContext  # noqa: E402
from a2a.server.events import EventQueue  # noqa: E402

from beeai_framework.agents.requirement import RequirementAgent  # noqa: E402
from beeai_framework.agents.requirement.requirements.conditional import (  # noqa: E402
    ConditionalRequirement,
)
from beeai_framework.backend.chat import ChatModel  # noqa: E402
from beeai_framework.backend.types import ChatModelParameters  # noqa: E402
from beeai_framework.context import RunContext  # noqa: E402
from beeai_framework.emitter import Emitter, EmitterOptions  # noqa: E402
from beeai_framework.errors import FrameworkError  # noqa: E402
from beeai_framework.memory import UnconstrainedMemory  # noqa: E402
from beeai_framework.tools import StringToolOutput, Tool, ToolRunOptions  # noqa: E402
from beeai_framework.tools.think import ThinkTool  # noqa: E402

from concierge.peer_client import send_and_stream  # noqa: E402
from shared.a2a_helpers import (  # noqa: E402
    emit_completed,
    emit_dobj_artifact,
    emit_failed,
    emit_working,
    ensure_task,
    extract_text,
)
from shared.beeai_helpers import (  # noqa: E402
    classify_event,
    force_parallel_tool_calls,
    summarize_for_working,
    to_beeai_model_string,
)
from shared.brain_hub import BrainEventHub  # noqa: E402
from shared.dobj_verify import ingest_and_verify  # noqa: E402
from shared.dobjd_client import DobjdClient  # noqa: E402
from shared.llm_brain import pick_model  # noqa: E402
from shared.peer_tools import find_file_part, flatten_chunk_text  # noqa: E402
from shared.registry import LUMBERJACK, LUMBERJACK_BACKUP, PeerAgent  # noqa: E402


INSTRUCTIONS = """You are the Auctioneer agent in a bitcraft multi-agent network.
Your job: source the resource the caller asked for at the lowest advertised
price, and verify that whatever the winner delivers is actually live on chain
before forwarding it to your caller.

You have these tools:
  - `think`             — plan your next steps (call once at the start).
  - `list_candidates`   — returns the URLs of registered candidate
                          suppliers you can bid across.
  - `fetch_agent_card(url)` — fetches one peer's agent card and returns
                          a JSON summary. Look at the skills array;
                          each relevant skill carries a `price:N` tag
                          in its `tags` list (N is in satoshis). The skill
                          name and description tell you what class of
                          .dobj the peer delivers (e.g. a `supply_stick`
                          skill named "Supply a Stick" delivers a Stick).
  - `delegate_request(peer_url, request_text, expected_class)` —
                          forwards the original request to the chosen
                          peer over A2A, verifies the returned .dobj is
                          a live `expected_class` object on chain, and
                          returns its filename.

Procedure:
1. Call `think` to plan.
2. Call `list_candidates` to discover the URLs to bid across.
3. Call `fetch_agent_card` once per candidate (the framework runs these
   in parallel where it can).
4. From the cards, identify (a) the LOWEST price and (b) what class of
   .dobj that peer delivers — the skill name/description usually says
   it directly (e.g. "Supply a Stick" → class is "Stick").
5. Call `delegate_request(winner_url, <the original request>, "<class>")`.
6. Respond with ONLY the filename returned by delegate_request. No prose,
   no "Here is the …". Just the bare filename like:
       craft-basics__stick_0xabc1234….dobj
The harness will parse your final message for this exact pattern."""


# ---------------------------------------------------------------------------
# BeeAI Tool subclasses
# ---------------------------------------------------------------------------

class _Empty(BaseModel):
    """No-arg schema. BeeAI requires `input_schema` even for nullary tools."""
    pass


class _UrlArg(BaseModel):
    url: str = Field(description="The peer's base URL (e.g. http://127.0.0.1:9997).")


class _DelegateArgs(BaseModel):
    peer_url: str = Field(description="The chosen peer's base URL.")
    request_text: str = Field(
        description="The original user request to forward verbatim.",
    )
    expected_class: str = Field(
        description=(
            "The .dobj class you expect this peer to deliver "
            "(e.g. 'Stick' for a Lumberjack, 'Stone' for a Stonemason). "
            "Used by the Auctioneer's dobjd to verify the bytes match "
            "what was advertised before forwarding."
        ),
    )


PublishFn = Callable[[dict], None]


class _ListCandidatesTool(Tool):
    """Returns the URLs of the candidate suppliers configured in the registry."""

    def __init__(self, candidates: list[PeerAgent]) -> None:
        super().__init__()
        self._candidates = candidates

    @property
    def name(self) -> str:
        return 'list_candidates'

    @property
    def description(self) -> str:
        return (
            'Returns the URLs of the candidate suppliers this Auctioneer '
            'is configured to bid across. Output is one URL per line.'
        )

    @cached_property
    def input_schema(self) -> type[_Empty]:
        return _Empty

    def _create_emitter(self) -> Emitter:
        return Emitter.root().child(
            namespace=['tool', 'list_candidates'], creator=self,
        )

    async def _run(
        self,
        input: _Empty,
        options: ToolRunOptions | None,
        context: RunContext,
    ) -> StringToolOutput:
        lines = [c.url for c in self._candidates]
        return StringToolOutput('\n'.join(lines))


class _FetchAgentCardTool(Tool):
    """Fetches a peer's /.well-known/agent-card.json and returns it as JSON."""

    def __init__(self, publish: PublishFn) -> None:
        super().__init__()
        self._publish = publish

    @property
    def name(self) -> str:
        return 'fetch_agent_card'

    @property
    def description(self) -> str:
        return (
            "Fetches a peer's A2A agent card (the standard "
            '`/.well-known/agent-card.json` document) and returns it as '
            "JSON. Look at the `skills` array; each skill's `tags` may "
            "include a `price:N` tag where N is the asking price in "
            'satoshis. Use that to compare bids.'
        )

    @cached_property
    def input_schema(self) -> type[_UrlArg]:
        return _UrlArg

    def _create_emitter(self) -> Emitter:
        return Emitter.root().child(
            namespace=['tool', 'fetch_agent_card'], creator=self,
        )

    async def _run(
        self,
        input: _UrlArg,
        options: ToolRunOptions | None,
        context: RunContext,
    ) -> StringToolOutput:
        url = input.url.rstrip('/')
        try:
            async with httpx.AsyncClient(timeout=5.0) as client:
                resp = await client.get(f'{url}/.well-known/agent-card.json')
                resp.raise_for_status()
                card = resp.json()
        except Exception as e:
            self._publish({'type': 'bid_failed', 'peer': url, 'reason': str(e)[:200]})
            return StringToolOutput(f'ERROR: could not fetch {url}: {e}')

        # Side-effect: emit a `bid` event for the dashboard if the card
        # carries a price tag. Tool's *real* output is the raw card JSON
        # so the LLM does its own parsing.
        peer_name = card.get('name', url)
        for skill in card.get('skills', []) or []:
            for tag in skill.get('tags', []) or []:
                if isinstance(tag, str) and tag.startswith('price:'):
                    try:
                        price = int(tag.split(':', 1)[1])
                    except ValueError:
                        continue
                    self._publish({
                        'type': 'bid', 'peer': peer_name,
                        'price': price, 'skill': skill.get('id'),
                    })

        # Trim the card to the bits the LLM actually needs — full card
        # JSON is verbose and most of it (capabilities, interfaces,
        # input modes) is irrelevant to picking a winner.
        compact = {
            'name': card.get('name'),
            'description': card.get('description'),
            'skills': [
                {
                    'id': s.get('id'),
                    'name': s.get('name'),
                    'tags': s.get('tags', []),
                }
                for s in card.get('skills', []) or []
            ],
        }
        import json
        return StringToolOutput(json.dumps(compact, indent=2))


class _DelegateRequestTool(Tool):
    """Forwards a request to a peer; verifies + captures the winning FilePart."""

    def __init__(
        self,
        *,
        publish: PublishFn,
        on_peer_chunk: Callable[[str, str], Awaitable[None]],
        capture: Callable[[str, bytes, str], None],
        dobjd: DobjdClient,
    ) -> None:
        super().__init__()
        self._publish = publish
        self._on_peer_chunk = on_peer_chunk
        self._capture = capture
        self._dobjd = dobjd

    @property
    def name(self) -> str:
        return 'delegate_request'

    @property
    def description(self) -> str:
        return (
            "Forwards the original request to the chosen peer's A2A "
            'endpoint, captures the .dobj file the peer delivers, '
            'ingests + verifies it against this Auctioneer\'s local '
            'dobjd (class match + status=live on chain), then returns '
            'the filename. Throws if the delivered .dobj does not '
            "match the expected class or isn't live on chain. Call "
            'this exactly once, after you have decided which '
            'candidate has the best price.'
        )

    @cached_property
    def input_schema(self) -> type[_DelegateArgs]:
        return _DelegateArgs

    def _create_emitter(self) -> Emitter:
        return Emitter.root().child(
            namespace=['tool', 'delegate_request'], creator=self,
        )

    async def _run(
        self,
        input: _DelegateArgs,
        options: ToolRunOptions | None,
        context: RunContext,
    ) -> StringToolOutput:
        peer_url = input.peer_url.rstrip('/')
        peer_label = _peer_label_from_url(peer_url)
        self._publish({'type': 'delegating', 'peer': peer_label, 'url': peer_url})

        # ---- 1. Stream from the winning peer, capture the FilePart ----
        seen: tuple[str, bytes] | None = None
        async for chunk in send_and_stream(peer_url, input.request_text):
            text = flatten_chunk_text(chunk)
            if text:
                await self._on_peer_chunk(peer_label, text)
            found = find_file_part(chunk)
            if found:
                seen = found

        if seen is None:
            raise RuntimeError(
                f'delegate_request: peer {peer_label} streamed no FilePart artifact'
            )

        name, data = seen

        # ---- 2. Ingest + verify on the Auctioneer's local dobjd -------
        # This is the Auctioneer's trust boundary: it's the agent that
        # *chose* this supplier, so it validates the supplier actually
        # delivered what they advertised before forwarding to the caller.
        # If the .dobj fails verification (wrong class, not live, parse
        # error), this raises and the auction round fails — the caller
        # (Concierge) sees the error.
        self._publish({'type': 'verifying', 'peer': peer_label, 'file': name})
        await ingest_and_verify(
            self._dobjd, name, data, expected_class=input.expected_class,
        )
        self._publish({
            'type': 'verified', 'peer': peer_label, 'file': name,
            'expected_class': input.expected_class,
        })

        # ---- 3. Delete-after-forward to keep our inventory clean ------
        # Same pattern the specialists use: the chain still considers
        # the .dobj live, so when the Concierge ingests it on its side
        # the verification passes. We hold the bytes in memory; on
        # disk we keep nothing.
        await self._dobjd.delete_dobj_file(name)

        # Hand the bytes back to the executor for re-emission under our
        # task_id after the LLM run finishes.
        self._capture(name, data, peer_label)
        self._publish({'type': 'auction_complete', 'winner': peer_label, 'file': name})
        return StringToolOutput(name)


def _peer_label_from_url(url: str) -> str:
    """Best-effort label for a peer URL (port → name) so dashboard events
    don't show raw `http://127.0.0.1:9995`. Falls back to the URL itself."""
    for peer in (LUMBERJACK, LUMBERJACK_BACKUP):
        if peer.url.rstrip('/') == url.rstrip('/'):
            return peer.name
    return url


# ---------------------------------------------------------------------------
# Executor
# ---------------------------------------------------------------------------

class AuctioneerAgentExecutor(AgentExecutor):
    """LLM-driven auction router with a local dobjd for verification."""

    def __init__(self, brain_hub: BrainEventHub | None = None) -> None:
        self.brain_hub = brain_hub
        self.candidates: list[PeerAgent] = [LUMBERJACK, LUMBERJACK_BACKUP]
        # The Auctioneer has its own dobjd (see scripts/bootstrap_dobjds.sh
        # — port 7777 by default, configured via DOBJD_URL). That dobjd
        # acts as the Auctioneer's trust boundary: `delegate_request`
        # ingests + verifies every winning .dobj on it before forwarding.
        self.dobjd = DobjdClient()
        # Per-request slot — `delegate_request` fills it; execute() drains it.
        self._captured_file: tuple[str, bytes, str] | None = None

    def _publish(self, event: dict) -> None:
        if self.brain_hub is not None:
            self.brain_hub.publish({'agent': 'auctioneer', **event})

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> None:
        await ensure_task(context, event_queue)

        try:
            request_text = (
                extract_text(context.message).strip()
                or 'I need 1 stick'
            )
            litellm_model = pick_model('AUCTIONEER_LLM')
            beeai_model = to_beeai_model_string(litellm_model)
            _log(f'brain online (beeai → {beeai_model})')
            await emit_working(
                context, event_queue,
                f'auctioneer brain online (beeai → {beeai_model}); '
                f'starting auction for: "{request_text}"',
            )
            self._publish({'type': 'auction_start', 'request': request_text})

            # ----- Tool plumbing ----------------------------------------
            self._captured_file = None

            def capture(name: str, data: bytes, peer_label: str) -> None:
                self._captured_file = (name, data, peer_label)

            async def on_peer_chunk(peer_label: str, text: str) -> None:
                await emit_working(
                    context, event_queue, f'[{peer_label}] {text}',
                )

            think_tool = ThinkTool()
            list_tool = _ListCandidatesTool(self.candidates)
            fetch_tool = _FetchAgentCardTool(publish=self._publish)
            delegate_tool = _DelegateRequestTool(
                publish=self._publish,
                on_peer_chunk=on_peer_chunk,
                capture=capture,
                dobjd=self.dobjd,
            )

            # ----- BeeAI agent ------------------------------------------
            chat_model = ChatModel.from_name(
                beeai_model,
                ChatModelParameters(temperature=1),
                allow_parallel_tool_calls=True,
            )
            force_parallel_tool_calls(chat_model)

            agent = RequirementAgent(
                name='Auctioneer',
                description='Routes resource requests to the cheapest registered supplier.',
                llm=chat_model,
                memory=UnconstrainedMemory(),
                tools=[think_tool, list_tool, fetch_tool, delegate_tool],
                requirements=[
                    ConditionalRequirement(
                        think_tool, force_at_step=1,
                        consecutive_allowed=False,
                    ),
                    ConditionalRequirement(list_tool, max_invocations=1),
                    # Allow one fetch per candidate (cap generous for safety).
                    ConditionalRequirement(fetch_tool, max_invocations=8),
                    # Delegate exactly once, only after at least one fetch
                    # (so we never delegate before seeing prices).
                    ConditionalRequirement(
                        delegate_tool,
                        only_after=[fetch_tool],
                        max_invocations=1,
                    ),
                ],
                role='Auctioneer',
                instructions=INSTRUCTIONS,
            )

            handler = self._make_emitter_handler(context, event_queue)
            response = await agent.run(request_text).observe(
                lambda em: em.on(
                    '*.*', handler, EmitterOptions(match_nested=True),
                )
            )
            final_text = response.last_message.text if response else ''

            if self._captured_file is None:
                await emit_failed(
                    context, event_queue,
                    'auctioneer: LLM finished without calling delegate_request — '
                    f'final answer was: {final_text[:200]!r}',
                )
                return

            name, data, peer_label = self._captured_file
            await emit_dobj_artifact(
                context, event_queue,
                artifact_name='stick',
                file_name=name,
                dobj_bytes=data,
                note=f'sourced via {peer_label} (auction winner)',
            )
            await emit_completed(context, event_queue)
            _log(f'auction complete; delivered {name} via {peer_label}')

        except Exception as e:
            detail = _explain_exception(e)
            print('[auctioneer] EXCEPTION:', file=sys.stderr, flush=True)
            traceback.print_exc()
            print(f'[auctioneer] explained: {detail}', file=sys.stderr, flush=True)
            await emit_failed(context, event_queue, f'auctioneer failed: {detail}')

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        raise Exception('cancel not supported')

    def _make_emitter_handler(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> Any:
        async def on_event(data: Any, event: Any) -> None:
            kind = classify_event(event, data)
            if kind is None:
                return
            payload: dict[str, Any] = {'agent': 'auctioneer', **kind}
            line = summarize_for_working(kind)
            if line:
                _log(line)
                await emit_working(context, event_queue, line)
            if self.brain_hub is not None:
                self.brain_hub.publish(payload)
        return on_event


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _log(message: str) -> None:
    print(f'[auctioneer] {message}', file=sys.stdout, flush=True)


def _explain_exception(e: BaseException) -> str:
    """Walk the full __cause__ chain; surface BeeAI's rich error context.

    Identical pattern to the Concierge — duplicated rather than shared
    so each agent file is self-contained for skim-reading.
    """
    if isinstance(e, FrameworkError):
        try:
            return e.explain()
        except Exception:
            pass
    parts: list[str] = [f'{type(e).__name__}: {e}']
    cur: BaseException | None = e.__cause__
    depth = 1
    while cur is not None and depth < 8:
        parts.append(f'  caused by {type(cur).__name__}: {cur}')
        cur = cur.__cause__
        depth += 1
    return '\n'.join(parts)
