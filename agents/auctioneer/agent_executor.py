"""Auctioneer agent — runs a sealed-bid auction among Lumberjacks.

For every incoming "I need 1 stick" request, the Auctioneer:

  1. Fetches each candidate Lumberjack's `/.well-known/agent-card.json`
     in parallel and parses the advertised `price:N` tag from their
     `supply_stick` skill.
  2. Picks the lowest price.
  3. Delegates the actual delivery to the winner via A2A, re-emitting
     the winner's stream chunks as the Auctioneer's own task events
     (so the caller — the Concierge — sees a normal A2A stream with
     working updates + a single Stick FilePart at the end).

This is mechanical, no LLM brain. The auction logic is short and an
LLM would just add latency + cost. We still publish events to
`brain_hub` (`auction_start`, `bid`, `winner`, `delegating`,
`auction_complete`) so the dashboard's Auctioneer card lights up
in real time during a request.

The Auctioneer has no dobjd. It transports the .dobj from the winning
Lumberjack to the caller without local ingest/verify — the Concierge
does final ingest + verification on its own dobjd anyway.
"""

from __future__ import annotations

import asyncio
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import httpx  # noqa: E402

from a2a.server.agent_execution import AgentExecutor, RequestContext  # noqa: E402
from a2a.server.events import EventQueue  # noqa: E402

from concierge.peer_client import send_and_stream  # noqa: E402
from shared.a2a_helpers import (  # noqa: E402
    emit_completed,
    emit_dobj_artifact,
    emit_failed,
    emit_working,
    ensure_task,
    extract_text,
)
from shared.brain_hub import BrainEventHub  # noqa: E402
from shared.peer_tools import find_file_part, flatten_chunk_text  # noqa: E402
from shared.registry import LUMBERJACK, LUMBERJACK_BACKUP  # noqa: E402


_PRICE_TAG_RE = re.compile(r'^price:(\d+)$')


class AuctioneerAgentExecutor(AgentExecutor):
    """Mechanical (no-LLM) auction router across registered Lumberjacks."""

    def __init__(self, brain_hub: BrainEventHub | None = None) -> None:
        self.brain_hub = brain_hub
        # Candidates default to the two Lumberjacks in shared/registry.py
        # (both URLs read their own env vars there, so deployers can swap
        # them without touching this code).
        self.candidates = [LUMBERJACK, LUMBERJACK_BACKUP]

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
            _log(f'auction starting: "{request_text}"')
            await emit_working(
                context, event_queue,
                f'auction starting for: "{request_text}"',
            )
            self._publish({'type': 'auction_start', 'request': request_text})

            # ----- Phase 1: collect bids in parallel ---------------------
            bids = await asyncio.gather(*(
                self._fetch_bid(c.name, c.url) for c in self.candidates
            ))
            valid = [b for b in bids if b is not None]
            if not valid:
                raise RuntimeError(
                    'no candidate Lumberjack responded with a parseable price; '
                    f'tried: {[c.url for c in self.candidates]}'
                )

            for bid in valid:
                line = f'bid: {bid["name"]} → {bid["price"]} satoshis'
                _log(line)
                await emit_working(context, event_queue, line)
                self._publish({
                    'type': 'bid',
                    'peer': bid['name'],
                    'price': bid['price'],
                })

            # ----- Phase 2: pick winner ----------------------------------
            winner = min(valid, key=lambda b: b['price'])
            line = f'winner: {winner["name"]} @ {winner["price"]} satoshis'
            _log(line)
            await emit_working(context, event_queue, line)
            self._publish({
                'type': 'winner',
                'peer': winner['name'],
                'price': winner['price'],
            })

            # ----- Phase 3: delegate to winner, forward stream -----------
            self._publish({'type': 'delegating', 'peer': winner['name']})
            seen_file: tuple[str, bytes] | None = None
            async for chunk in send_and_stream(winner['url'], request_text):
                # Mirror the winner's working-state text as ours (prefixed
                # so the Concierge can see who fulfilled the auction).
                text = flatten_chunk_text(chunk)
                if text:
                    await emit_working(
                        context, event_queue,
                        f'[{winner["name"]}] {text}',
                    )
                found = find_file_part(chunk)
                if found:
                    seen_file = found

            if seen_file is None:
                raise RuntimeError(
                    f'winner {winner["name"]} streamed no FilePart artifact'
                )

            # Re-emit the .dobj under our own task_id so the Concierge
            # receives it as the Auctioneer's reply (rather than as
            # the Lumberjack's, which would have a different task_id).
            name, data = seen_file
            await emit_dobj_artifact(
                context, event_queue,
                artifact_name='stick',
                file_name=name,
                dobj_bytes=data,
                note=f'sourced via {winner["name"]} (auction winner @ {winner["price"]})',
            )
            self._publish({'type': 'auction_complete', 'winner': winner['name']})
            _log(f'auction complete; delivered {name} via {winner["name"]}')

            await emit_completed(context, event_queue)

        except Exception as e:
            _log(f'auction failed: {e}')
            await emit_failed(context, event_queue, f'auctioneer failed: {e}')

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        raise Exception('cancel not supported')

    # ---------------------------------------------------------------------
    # Helpers
    # ---------------------------------------------------------------------

    async def _fetch_bid(self, name: str, url: str) -> dict | None:
        """Pull a peer's agent card; parse advertised STICK_PRICE.

        Returns None on any failure (peer down, no supply_stick skill,
        no price tag). The auction proceeds with whoever responds —
        no failed bidder blocks the whole round.
        """
        try:
            async with httpx.AsyncClient(timeout=5.0) as client:
                resp = await client.get(f'{url}/.well-known/agent-card.json')
                resp.raise_for_status()
                card = resp.json()
        except Exception as e:
            _log(f'bid failed: {name} ({url}) — {e}')
            self._publish({
                'type': 'bid_failed', 'peer': name, 'reason': str(e)[:200],
            })
            return None

        for skill in card.get('skills', []) or []:
            if skill.get('id') != 'supply_stick':
                continue
            for tag in skill.get('tags', []) or []:
                m = _PRICE_TAG_RE.match(tag)
                if m:
                    return {'name': name, 'url': url, 'price': int(m.group(1))}
        _log(f'bid skipped: {name} has no parseable price:N tag')
        return None

    def _publish(self, event: dict) -> None:
        if self.brain_hub is not None:
            self.brain_hub.publish({'agent': 'auctioneer', **event})


def _log(message: str) -> None:
    print(f'[auctioneer] {message}', file=sys.stdout, flush=True)
