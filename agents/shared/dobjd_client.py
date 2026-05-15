"""Thin async client for the local dobjd REST API.

Every bitcraft A2A agent runs alongside its own dobjd process. This client
wraps the handful of endpoints the agents need:

  GET  /inventory                  list local objects with class/status
  GET  /objects/dir                where dobjd keeps its .dobj files
  POST /actions/run                run a crafting action

Sender/receiver semantics:

- To **export** a .dobj for shipping over A2A, an agent calls
  `read_dobj_file(file_name)` which reads the file directly from the dir
  returned by `/objects/dir`. (Same machine as dobjd.)

- To **import** a received .dobj, an agent calls `write_dobj_file` to
  drop the bytes into the same directory. `load_object_files` runs on
  every `/inventory` call so the new file shows up automatically with
  the synchronizer-determined status.
"""

from __future__ import annotations

import asyncio
import json
import os
import uuid
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import Any

import httpx
from httpx_sse import aconnect_sse


class DobjdClient:
    """Async client bound to a single dobjd instance."""

    def __init__(self, base_url: str | None = None, timeout: float = 1800.0) -> None:
        # 30 minutes by default — first-run circuit cache building inside
        # dobjd can take 2-3 minutes per action (we have four isolated dobjds
        # each building separately), plus proof-of-work grinding can be slow.
        # After warm-up, real actions take seconds.
        self.base_url = (base_url or os.environ.get('DOBJD_URL', 'http://127.0.0.1:7717')).rstrip('/')
        self._client = httpx.AsyncClient(timeout=timeout)
        self._objects_dir: str | None = None

    async def aclose(self) -> None:
        await self._client.aclose()

    # ---- REST -----------------------------------------------------------

    async def list_inventory(self) -> list[dict[str, Any]]:
        r = await self._client.get(f'{self.base_url}/inventory')
        r.raise_for_status()
        return r.json()

    async def get_objects_dir(self) -> str:
        if self._objects_dir is None:
            r = await self._client.get(f'{self.base_url}/objects/dir')
            r.raise_for_status()
            self._objects_dir = r.json()['path']
        return self._objects_dir

    async def run_action(
        self,
        plugin_name: str,
        action_name: str,
        input_files: list[str] | None = None,
    ) -> dict[str, Any]:
        """Block until the action finishes; return the wire RunActionResult."""
        body = {
            'input': {
                'action': {'pluginName': plugin_name, 'name': action_name},
                'inputObjectPaths': input_files or [],
            }
        }
        r = await self._client.post(f'{self.base_url}/actions/run', json=body)
        r.raise_for_status()
        return r.json()

    async def run_action_with_progress(
        self,
        plugin_name: str,
        action_name: str,
        input_files: list[str] | None = None,
        on_progress: Callable[[dict[str, Any]], Awaitable[None]] | None = None,
    ) -> dict[str, Any]:
        """Same as `run_action`, but subscribes to `/events` and forwards every
        `RunActionProgress` event for this run via `on_progress` before
        returning the final `RunActionResult`.

        Each progress event is a dict like:
          {"runId": "...", "phase": "generateProof"|"commit",
           "status": "running"|"done"|"failed",
           "message": "Adding 3446 blinding terms…",
           "oldRoot": ..., "newRoot": ..., "outputFiles": ..., ...}
        """
        run_id = uuid.uuid4().hex
        body = {
            'input': {
                'action': {'pluginName': plugin_name, 'name': action_name},
                'inputObjectPaths': input_files or [],
                'runId': run_id,
            }
        }

        # Open SSE FIRST so we don't race the action's first event.
        async with aconnect_sse(
            self._client, 'GET', f'{self.base_url}/events'
        ) as event_source:
            post_task = asyncio.create_task(
                self._client.post(f'{self.base_url}/actions/run', json=body)
            )
            try:
                async for sse in event_source.aiter_sse():
                    if not sse.data:
                        continue
                    try:
                        event = json.loads(sse.data)
                    except json.JSONDecodeError:
                        continue
                    # Wire shape — serde(tag = "type", rename_all = "kebab-case"):
                    # {"type": "run-action-progress", "runId": ..., "phase": ...,
                    #  "status": ..., "message": ...}.
                    if event.get('type') != 'run-action-progress':
                        continue
                    if event.get('runId') != run_id:
                        continue
                    if on_progress is not None:
                        await on_progress(event)
                    # Terminal events end with phase=commit, status=done|failed.
                    if (
                        event.get('phase') == 'commit'
                        and event.get('status') in ('done', 'failed')
                    ):
                        break
            except BaseException:
                post_task.cancel()
                raise

        # Outside the SSE context: collect the POST response.
        response = await post_task
        response.raise_for_status()
        return response.json()

    # ---- Filesystem (same host as dobjd) --------------------------------

    async def read_dobj_file(self, file_name: str) -> bytes:
        objects_dir = await self.get_objects_dir()
        return Path(objects_dir, file_name).read_bytes()

    async def write_dobj_file(self, file_name: str, data: bytes) -> str:
        """Drop a received .dobj into dobjd's objects dir. Returns full path."""
        objects_dir = await self.get_objects_dir()
        target = Path(objects_dir, file_name)
        target.write_bytes(data)
        return str(target)

    # ---- Convenience ----------------------------------------------------

    async def find_object(
        self,
        class_name: str,
        require_status: str = 'live',
    ) -> dict[str, Any] | None:
        """Return the first inventory object matching class + status, or None."""
        for obj in await self.list_inventory():
            klass = obj.get('class', {}).get('name', '')
            if klass != class_name:
                continue
            if (obj.get('status') or '').lower() != require_status.lower():
                continue
            return obj
        return None
