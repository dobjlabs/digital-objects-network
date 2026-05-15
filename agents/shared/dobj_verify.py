"""Receive-side verification of a .dobj shipped over A2A.

The MVP accepts ZK-level proof verification + chain-level liveness from
the synchronizer as the trust boundary. Ownership is intentionally NOT
verified (sender could have already spent the file before delivery —
acceptable for the demo, would need a Transfer action to fix).

`ingest_and_verify` writes the bytes into dobjd's objects dir, triggers a
fresh `/inventory` sync (which reads the dir and asks the synchronizer
about liveness), and asserts:

  - the object loaded successfully
  - its class matches what the sender promised
  - its status is "live" on the chain right now

If any check fails it raises `DobjVerificationError`.
"""

from __future__ import annotations

from .dobjd_client import DobjdClient


class DobjVerificationError(Exception):
    """Raised when a received .dobj doesn't match what was promised."""


async def ingest_and_verify(
    dobjd: DobjdClient,
    file_name: str,
    dobj_bytes: bytes,
    expected_class: str,
) -> dict:
    """Drop the bytes into dobjd's store, sync, and verify class + liveness."""
    await dobjd.write_dobj_file(file_name, dobj_bytes)

    # list_inventory triggers load_object_files + a sync against the
    # synchronizer; status comes back as "Live" / "Nullified" / etc.
    inventory = await dobjd.list_inventory()
    match = next((o for o in inventory if o.get('fileName') == file_name), None)
    if match is None:
        raise DobjVerificationError(
            f'dobjd did not recognize {file_name} after write — '
            f'possible parse failure or unknown class'
        )

    actual_class = match.get('class', {}).get('name', '?')
    if actual_class != expected_class:
        raise DobjVerificationError(
            f'class mismatch: expected {expected_class!r}, got {actual_class!r}'
        )

    status = (match.get('status') or '').lower()
    if status != 'live':
        raise DobjVerificationError(
            f'object {file_name} is not live (status={status!r}) — '
            f'sender may have already spent or nullified it'
        )

    return match
