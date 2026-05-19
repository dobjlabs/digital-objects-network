"""BeeAI Framework `Tool` wrappers around A2A peer calls.

These are the Concierge's levers for delegating to Lumberjack, Stonemason,
and Craftsmith. Each tool:

  1. Sends a message via A2A to the named peer
  2. Streams chunks back, forwarding each chunk's flattened text via the
     `on_peer_chunk(peer_label, chunk)` callback so the executor can
     surface peer progress as A2A Working updates and brain-hub events
  3. Extracts the FilePart from the final chunk
  4. Ingests the bytes into the Concierge's local dobjd + verifies
     class + status=live
  5. Returns the bare filename as a `StringToolOutput`

Why custom tools and not BeeAI's stock `HandoffTool`? `HandoffTool`
delegates to a BeeAI `Runnable` (typically another in-process BeeAI
agent). Our specialists are *external* A2A endpoints, reached over HTTP,
so we wrap our `send_and_stream` peer client in BeeAI `Tool` subclasses
directly. The shape — name, description, JSON-schema input — is what
the LLM sees, so from the model's perspective it's identical to a
HandoffTool.
"""

from __future__ import annotations

import inspect
from collections.abc import Callable
from functools import cached_property
from typing import Any

from pydantic import BaseModel, Field

from beeai_framework.context import RunContext
from beeai_framework.emitter import Emitter
from beeai_framework.tools import StringToolOutput, Tool, ToolRunOptions

from concierge.peer_client import send_and_stream
from shared.dobj_verify import ingest_and_verify
from shared.dobjd_client import DobjdClient
from shared.registry import AUCTIONEER, CRAFTSMITH, STONEMASON


# `on_peer_chunk(peer_label, chunk)` — called once per streamed chunk so
# the caller can forward peer Working-state text out to its own A2A
# subscribers + brain hub. Sync or async, both are awaited if needed.
PeerChunkCb = Callable[[str, Any], Any]


# ---------------------------------------------------------------------------
# Input schemas (what the LLM fills in when calling each tool)
# ---------------------------------------------------------------------------

class _StickRequest(BaseModel):
    task: str = Field(
        default='I need 1 stick',
        description='Free-form request to send to the Lumberjack.',
    )


class _StoneRequest(BaseModel):
    task: str = Field(
        default='I need 1 stone',
        description='Free-form request to send to the Stonemason.',
    )


class _StonepickRequest(BaseModel):
    stick_file: str = Field(
        description='Stick .dobj filename returned by request_stick_from_lumberjack.',
    )
    stone_file: str = Field(
        description='Stone .dobj filename returned by request_stone_from_stonemason.',
    )


# ---------------------------------------------------------------------------
# Shared peer-call machinery
# ---------------------------------------------------------------------------

class _PeerToolBase(Tool):
    """Common plumbing for the three peer-call tools."""

    def __init__(
        self,
        *,
        name: str,
        description: str,
        dobjd: DobjdClient,
        on_peer_chunk: PeerChunkCb | None = None,
    ) -> None:
        super().__init__()
        self._name = name
        self._description = description
        self._dobjd = dobjd
        self._on_peer_chunk = on_peer_chunk

    @property
    def name(self) -> str:
        return self._name

    @property
    def description(self) -> str:
        return self._description

    def _create_emitter(self) -> Emitter:
        return Emitter.root().child(
            namespace=['tool', 'peer', self._name],
            creator=self,
        )

    async def _fetch(
        self,
        peer_label: str,
        peer_url: str,
        request_text: str,
        file_parts: list[tuple[str, bytes]] | None = None,
    ) -> tuple[str, bytes]:
        """Round-trip an A2A message to a peer, return its final FilePart."""
        seen: tuple[str, bytes] | None = None
        async for chunk in send_and_stream(peer_url, request_text, file_parts=file_parts):
            if self._on_peer_chunk is not None:
                result = self._on_peer_chunk(peer_label, chunk)
                if inspect.isawaitable(result):
                    await result
            found = find_file_part(chunk)
            if found:
                seen = found
        if seen is None:
            raise RuntimeError(f'{peer_label} returned no FilePart artifact')
        return seen


# ---------------------------------------------------------------------------
# The three concrete tools
# ---------------------------------------------------------------------------

class AuctionForStick(_PeerToolBase):
    """Ask the Auctioneer A2A peer to source one Stick. The Auctioneer
    runs a sealed-bid auction across the registered Lumberjacks (each
    advertises a `price:N` tag on its `supply_stick` skill), picks the
    cheapest, and forwards the winner's delivery. Ingests + verifies
    the returned .dobj on the Concierge's local dobjd and returns the
    Stick's filename."""

    @cached_property
    def input_schema(self) -> type[_StickRequest]:
        return _StickRequest

    async def _run(
        self,
        input: _StickRequest,
        options: ToolRunOptions | None,
        context: RunContext,
    ) -> StringToolOutput:
        name, data = await self._fetch(
            'auctioneer', AUCTIONEER.url, input.task or 'I need 1 stick',
        )
        await ingest_and_verify(self._dobjd, name, data, expected_class='Stick')
        return StringToolOutput(name)


class RequestStoneFromStonemason(_PeerToolBase):
    """Ask the Stonemason A2A peer for one Stone. Ingests + verifies and
    returns the Stone's filename."""

    @cached_property
    def input_schema(self) -> type[_StoneRequest]:
        return _StoneRequest

    async def _run(
        self,
        input: _StoneRequest,
        options: ToolRunOptions | None,
        context: RunContext,
    ) -> StringToolOutput:
        name, data = await self._fetch(
            'stonemason', STONEMASON.url, input.task or 'I need 1 stone',
        )
        await ingest_and_verify(self._dobjd, name, data, expected_class='Stone')
        return StringToolOutput(name)


class CraftStonepickWithCraftsmith(_PeerToolBase):
    """Send a Stick + Stone (already in the Concierge's local inventory)
    to the Craftsmith A2A peer. Returns the StonePick's filename after
    ingest + verify."""

    @cached_property
    def input_schema(self) -> type[_StonepickRequest]:
        return _StonepickRequest

    async def _run(
        self,
        input: _StonepickRequest,
        options: ToolRunOptions | None,
        context: RunContext,
    ) -> StringToolOutput:
        stick_bytes = await self._dobjd.read_dobj_file(input.stick_file)
        stone_bytes = await self._dobjd.read_dobj_file(input.stone_file)
        name, data = await self._fetch(
            'craftsmith',
            CRAFTSMITH.url,
            'Please assemble a StonePick from these inputs',
            file_parts=[
                (input.stick_file, stick_bytes),
                (input.stone_file, stone_bytes),
            ],
        )
        await ingest_and_verify(self._dobjd, name, data, expected_class='StonePick')
        return StringToolOutput(name)


def make_peer_tools(
    dobjd: DobjdClient,
    on_peer_chunk: PeerChunkCb | None = None,
) -> tuple[AuctionForStick, RequestStoneFromStonemason, CraftStonepickWithCraftsmith]:
    """Build the three peer-call tools, bound to this Concierge's dobjd.

    Returned as a tuple in the natural call order (stick, stone, craft)
    so the caller can `tools=[think, *peer_tools]` cleanly.

    Note: the stick tool routes through the Auctioneer rather than
    going directly to a Lumberjack. The Concierge never knows which
    specific Lumberjack served the Stick — discovery is the
    Auctioneer's job.
    """
    stick = AuctionForStick(
        name='auction_for_stick',
        description=(
            'Asks the Auctioneer A2A peer to source one Stick via a '
            "sealed-bid auction across this network's Lumberjacks. The "
            'Auctioneer reads each candidate\'s advertised price from '
            'its agent card, picks the cheapest, and forwards the '
            'winner\'s delivery. Ingests + verifies the returned .dobj '
            "on this Concierge's local dobjd. Returns the Stick filename. "
            'Call once.'
        ),
        dobjd=dobjd,
        on_peer_chunk=on_peer_chunk,
    )
    stone = RequestStoneFromStonemason(
        name='request_stone_from_stonemason',
        description=(
            'Asks the Stonemason A2A peer for one Stone. The Stonemason '
            'mines a fresh Stone with a WoodPick (chains WoodPick + Stick '
            'crafting first if needed). Ingests + verifies the returned '
            ".dobj on this Concierge's local dobjd. Returns the Stone "
            'filename. Call once.'
        ),
        dobjd=dobjd,
        on_peer_chunk=on_peer_chunk,
    )
    craft = CraftStonepickWithCraftsmith(
        name='craft_stonepick_with_craftsmith',
        description=(
            'Sends a Stick + Stone (already in this Concierge\'s local '
            'inventory, returned by the two request_* tools) to the '
            'Craftsmith A2A peer for assembly into a StonePick. Returns '
            'the StonePick filename. Call exactly once, after BOTH '
            'request_stick_from_lumberjack and request_stone_from_stonemason '
            'have returned.'
        ),
        dobjd=dobjd,
        on_peer_chunk=on_peer_chunk,
    )
    return stick, stone, craft


# ---------------------------------------------------------------------------
# Chunk parsing (shared with the executor for its on_peer_chunk forwarding)
# ---------------------------------------------------------------------------

def find_file_part(chunk: Any) -> tuple[str, bytes] | None:
    artifact_update = getattr(chunk, 'artifact_update', None)
    if artifact_update is None:
        return None
    artifact = getattr(artifact_update, 'artifact', None)
    if artifact is None:
        return None
    for p in getattr(artifact, 'parts', []) or []:
        raw = getattr(p, 'raw', b'')
        if not raw:
            continue
        name = getattr(p, 'filename', '') or 'unknown.dobj'
        return name, bytes(raw)
    return None


def flatten_chunk_text(chunk: Any) -> str:
    """Extract any text content from a streamed StreamResponse chunk.

    Used by the Concierge to forward peer Working-state lines to its
    own A2A subscribers + brain-hub dashboard."""
    out: list[str] = []
    status_update = getattr(chunk, 'status_update', None)
    if status_update is not None:
        status = getattr(status_update, 'status', None)
        msg = getattr(status, 'message', None) if status is not None else None
        if msg is not None:
            for p in getattr(msg, 'parts', []) or []:
                text = getattr(p, 'text', '') or ''
                if text:
                    out.append(text)
    artifact_update = getattr(chunk, 'artifact_update', None)
    if artifact_update is not None:
        artifact = getattr(artifact_update, 'artifact', None)
        if artifact is not None:
            for p in getattr(artifact, 'parts', []) or []:
                text = getattr(p, 'text', '') or ''
                if text:
                    out.append(text)
    return ' '.join(out)
