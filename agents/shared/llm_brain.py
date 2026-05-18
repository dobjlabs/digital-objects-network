"""Provider-agnostic LLM brain for the specialist agents.

Wraps LangChain's `create_agent` over a LiteLLM-backed chat model and
exposes the dobjd MCP server's tools to the LLM. Set `LLM_MODEL` to swap
providers — no code changes:

    LLM_MODEL=anthropic/claude-opus-4-7        # Anthropic
    LLM_MODEL=gemini/gemini-2.5-flash          # Google direct
    LLM_MODEL=vertex_ai/gemini-2.5-flash       # Vertex AI
    LLM_MODEL=openai/gpt-4o                    # OpenAI
    LLM_MODEL=ollama/llama3                    # local Ollama
    LLM_MODEL=together_ai/<repo>/<model>       # Together
    ...any other LiteLLM-supported provider...

Each provider reads its own API key from env (ANTHROPIC_API_KEY,
GEMINI_API_KEY, OPENAI_API_KEY, ...). LiteLLM handles routing.

Per-agent overrides honoured: LUMBERJACK_LLM, STONEMASON_LLM,
CRAFTSMITH_LLM beat LLM_MODEL when set.
"""

from __future__ import annotations

import inspect
import os
from collections.abc import Awaitable, Callable
from typing import Any
from urllib.parse import urlparse

from langchain.agents import create_agent
from langchain_litellm import ChatLiteLLM
from langchain_mcp_adapters.client import MultiServerMCPClient


DEFAULT_MODEL = 'anthropic/claude-opus-4-7'


def dobjd_mcp_url_from_http(dobjd_http_url: str) -> str:
    """Derive the dobjd MCP URL by bumping the HTTP port +1.

    `http://127.0.0.1:7717` → `http://127.0.0.1:7718/mcp` (per dobjd's
    `mcp_port_for_http_port` in dobjd/src/main.rs).
    """
    parsed = urlparse(dobjd_http_url)
    if not parsed.hostname or parsed.port is None:
        raise ValueError(f'cannot derive MCP url from {dobjd_http_url!r}')
    return f'{parsed.scheme}://{parsed.hostname}:{parsed.port + 1}/mcp'


def pick_model(agent_env_var: str | None = None) -> str:
    """Resolve the LiteLLM model string for this agent."""
    if agent_env_var and (override := os.environ.get(agent_env_var)):
        return override
    return os.environ.get('LLM_MODEL', DEFAULT_MODEL)


async def run_brain(
    *,
    system_prompt: str,
    user_request: str,
    mcp_url: str,
    on_step: Callable[[dict[str, Any]], Any] | None = None,
    model: str | None = None,
    max_tokens: int = 4000,
) -> str:
    """Run a tool-use loop against the given MCP server. Returns the
    final assistant text.

    `on_step({"type": "tool_call"|"tool_result"|"thought", ...})` is
    invoked for every intermediate event during the loop — wire this to
    `emit_working` in your A2A executor to forward progress to the user.
    """
    model = model or pick_model()
    # Dict form works across langchain-mcp-adapters versions where
    # StreamableHttpConnection is a TypedDict (not constructible as a class).
    mcp_client = MultiServerMCPClient(
        {'dobjd': {'transport': 'streamable_http', 'url': mcp_url}},
    )
    tools = await mcp_client.get_tools()

    agent = create_agent(
        model=ChatLiteLLM(model=model, max_tokens=max_tokens),
        tools=tools,
        system_prompt=system_prompt,
    )

    final_text = ''

    async for event in agent.astream_events(
        {'messages': [{'role': 'user', 'content': user_request}]},
        version='v2',
    ):
        kind = event.get('event')
        name = event.get('name', '')
        data = event.get('data', {}) or {}

        if kind == 'on_tool_start':
            if on_step is not None:
                await _maybe_await(on_step({
                    'type': 'tool_call',
                    'name': name,
                    'input': data.get('input'),
                }))

        elif kind == 'on_tool_end':
            output = data.get('output')
            summary = _summarize_tool_output(output)
            if on_step is not None:
                await _maybe_await(on_step({
                    'type': 'tool_result',
                    'name': name,
                    'output_summary': summary,
                }))

        elif kind == 'on_chat_model_end':
            msg = data.get('output')
            text = _extract_text(msg)
            if text:
                final_text = text
                if on_step is not None:
                    await _maybe_await(on_step({
                        'type': 'thought',
                        'text': text,
                    }))

    return final_text


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

async def _maybe_await(value: Any) -> None:
    """Allow `on_step` to be either sync (returns None) or async (returns
    a coroutine)."""
    if inspect.isawaitable(value):
        await value


def _extract_text(msg: Any) -> str:
    """LangChain message → flat text (handles str / list-of-blocks shapes)."""
    if msg is None:
        return ''
    content = getattr(msg, 'content', msg)
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts: list[str] = []
        for chunk in content:
            if isinstance(chunk, str):
                parts.append(chunk)
            elif isinstance(chunk, dict) and chunk.get('type') == 'text':
                parts.append(chunk.get('text') or '')
        return ''.join(parts)
    return str(content)


def _summarize_tool_output(output: Any) -> str:
    """Compact view of a tool result for progress emission."""
    if output is None:
        return ''
    s = str(output)
    return s if len(s) <= 200 else s[:197] + '…'
