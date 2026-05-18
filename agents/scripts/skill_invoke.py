"""Skill-friendly client: streams clean per-chunk progress to stdout, then
ends with a single `RESULT:` line the skill can grep for.

Also drops the delivered StonePick `.dobj` bytes into the user's local
dobjd inventory (default `~/.dobj/objects/`) so the user can see it
appear in their own /inventory, not just in the concierge's.

Output shape:
  - one line per streamed chunk, e.g.:
      [WORKING] [lumberjack] FindLog: Verifying inputs (generateProof/running)
      artifact[verify-stick] "Stick … verified live on chain"
      …
  - final line, always present:
      RESULT: StonePick → /Users/.../.dobj/objects/craft-basics__stonepick_….dobj
      RESULT: FAILED <reason>
      RESULT: UNKNOWN state=<state>

Used by commands/deliver-stone-pick/SKILL.md.  For a verbose chunk dump
during local development, use scripts/test_client.py instead.
"""

from __future__ import annotations

import asyncio
import os
import pathlib
import sys

import httpx

from a2a.client import A2ACardResolver, ClientConfig, create_client
from a2a.helpers import new_text_message
from a2a.types.a2a_pb2 import Role, SendMessageRequest, TaskState


CONCIERGE_URL = os.environ.get('CONCIERGE_URL', 'http://127.0.0.1:9996')
REQUEST_TEXT = os.environ.get('REQUEST_TEXT', 'I want a stone pick')
# Where to drop the delivered StonePick `.dobj` so the user's own dobjd can
# pick it up via its next /inventory re-scan. Override with DELIVERY_DIR.
DELIVERY_DIR = pathlib.Path(
    os.environ.get('DELIVERY_DIR', str(pathlib.Path.home() / '.dobj' / 'objects'))
)


def _state_name(state: int) -> str:
    return TaskState.Name(state).removeprefix('TASK_STATE_')


async def main() -> None:
    final_state: int | None = None
    final_failure: str = ''
    delivered_file: str = ''
    delivered_bytes: bytes = b''

    async with httpx.AsyncClient(timeout=3600.0) as h:
        try:
            card = await A2ACardResolver(h, CONCIERGE_URL).get_agent_card()
        except Exception as e:
            print(f'RESULT: FAILED could not reach concierge at {CONCIERGE_URL}: {e}')
            return

        client = await create_client(
            agent=card,
            client_config=ClientConfig(streaming=True, httpx_client=h),
        )
        try:
            req = SendMessageRequest(
                message=new_text_message(REQUEST_TEXT, role=Role.ROLE_USER)
            )
            async for chunk in client.send_message(req):
                if chunk.HasField('task'):
                    t = chunk.task
                    print(f'  task ({_state_name(t.status.state)})', flush=True)

                elif chunk.HasField('status_update'):
                    s = chunk.status_update.status
                    text = ''
                    if s.HasField('message'):
                        text = ' '.join(p.text for p in s.message.parts if p.text)
                    print(f'  [{_state_name(s.state)}] {text}', flush=True)
                    final_state = s.state
                    if s.state == TaskState.TASK_STATE_FAILED and text:
                        final_failure = text

                elif chunk.HasField('artifact_update'):
                    a = chunk.artifact_update.artifact
                    for p in a.parts:
                        if p.text:
                            print(
                                f'  artifact[{a.name}] {p.text!r}',
                                flush=True,
                            )
                        elif p.raw:
                            print(
                                f'  artifact[{a.name}] FILE {p.filename} '
                                f'({len(p.raw):,} bytes)',
                                flush=True,
                            )
                            if 'stonepick' in p.filename:
                                delivered_file = p.filename
                                delivered_bytes = bytes(p.raw)
        finally:
            await client.close()

    # If the demo delivered a StonePick, drop the bytes into the user's
    # local dobjd objects dir. The user's dobjd re-scans on every
    # /inventory call, so the new file shows up in their inventory with
    # synchronizer-determined status (live / pending / unknown).
    saved_path: pathlib.Path | None = None
    if (
        final_state == TaskState.TASK_STATE_COMPLETED
        and delivered_file
        and delivered_bytes
    ):
        try:
            DELIVERY_DIR.mkdir(parents=True, exist_ok=True)
            saved_path = DELIVERY_DIR / delivered_file
            saved_path.write_bytes(delivered_bytes)
            print(f'  saved → {saved_path}', flush=True)
        except OSError as e:
            print(f'  warning: could not write to {DELIVERY_DIR}: {e}', flush=True)
            saved_path = None

    # Single, machine-parseable summary line — the skill greps for this.
    if final_state == TaskState.TASK_STATE_COMPLETED and delivered_file:
        target = str(saved_path) if saved_path else delivered_file
        print(f'RESULT: StonePick → {target}', flush=True)
    elif final_failure:
        print(f'RESULT: FAILED {final_failure}', flush=True)
    else:
        state = _state_name(final_state) if final_state else '?'
        print(f'RESULT: UNKNOWN state={state}', flush=True)


if __name__ == '__main__':
    asyncio.run(main())
