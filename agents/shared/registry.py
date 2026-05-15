"""Static peer registry — read from env vars for the demo.

In a hosted deployment each agent would publish its card to a real
registry (the bitcraft synchronizer would be a natural fit). For local
runs we just hardcode the URLs and let the agent know about its peers
via env.
"""

from __future__ import annotations

import os
from dataclasses import dataclass


@dataclass(frozen=True)
class PeerAgent:
    name: str
    url: str  # base URL, e.g. http://127.0.0.1:9997


def _peer(env_var: str, default_url: str, name: str) -> PeerAgent:
    return PeerAgent(name=name, url=os.environ.get(env_var, default_url).rstrip('/'))


LUMBERJACK = _peer('LUMBERJACK_URL', 'http://127.0.0.1:9997', 'lumberjack')
STONEMASON = _peer('STONEMASON_URL', 'http://127.0.0.1:9998', 'stonemason')
CRAFTSMITH = _peer('CRAFTSMITH_URL', 'http://127.0.0.1:9999', 'craftsmith')
