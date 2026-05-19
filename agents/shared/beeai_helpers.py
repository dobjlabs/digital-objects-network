"""Shared helpers for BeeAI-based agents (Concierge, Auctioneer, …).

Three things every BeeAI agent in this network needs:

  1. **Model-string translation** — the rest of the demo uses LiteLLM's
     `provider/model` shape (`anthropic/claude-opus-4-7`), but BeeAI's
     `ChatModel.from_name` expects `provider:model`. `to_beeai_model_string`
     auto-translates so the same `LLM_MODEL` env var works everywhere.

  2. **Parallel tool calls on the wire** — BeeAI's `allow_parallel_tool_calls=True`
     only governs framework-side acceptance of multi-call responses; it
     never propagates to `ChatModelInput.parallel_tool_calls`, so LiteLLM
     ends up sending `parallel_tool_calls=False` to the provider (which
     for Anthropic becomes `disable_parallel_tool_use=True`).
     `force_parallel_tool_calls(chat_model)` monkey-patches the model's
     `_transform_input` to flip the field to True before serialization.
     Verified empirically: without the patch the wire carries `False`;
     with it, `True`.

  3. **Emitter → dashboard event classification.** `Run.observe()` fires
     a fire-hose of events (every tool start/success at TWO paths plus
     framework internals). `classify_event` filters to the cleanest
     subset — bare `tool.<name>.*` events plus the agent's `final_answer` —
     and maps them onto the `{type: tool_call | tool_result | thought}`
     shape the preview dashboard already renders.
"""

from __future__ import annotations

from typing import Any

from beeai_framework.tools.types import ToolOutput


# LiteLLM provider names that don't match BeeAI's provider IDs exactly.
# Add aliases here as you adopt more providers.
_LITELLM_TO_BEEAI_PROVIDER = {
    'vertex_ai': 'vertexai',
}


def to_beeai_model_string(litellm_style: str) -> str:
    """Translate `provider/model` (LiteLLM) → `provider:model` (BeeAI).

    Lets a single `LLM_MODEL=anthropic/claude-opus-4-7` env var work for
    both the LangChain specialists and the BeeAI agents. If the env var
    already uses BeeAI's colon shape, it's returned unchanged.
    """
    if ':' in litellm_style:
        return litellm_style
    if '/' not in litellm_style:
        return litellm_style
    provider, _, model = litellm_style.partition('/')
    provider = _LITELLM_TO_BEEAI_PROVIDER.get(provider, provider)
    return f'{provider}:{model}'


def force_parallel_tool_calls(chat_model: Any) -> None:
    """Force `parallel_tool_calls=True` on every LiteLLM call this model makes.

    Workaround for a BeeAI/LiteLLM plumbing gap — see module docstring.
    Idempotent: safe to call once at construction time. `ChatModelInput`
    has `frozen=False` so mutating the field is allowed.
    """
    original = chat_model._transform_input

    def patched(input):
        if input.parallel_tool_calls is None:
            input.parallel_tool_calls = True
        return original(input)

    chat_model._transform_input = patched


def classify_event(event: Any, data: Any) -> dict[str, Any] | None:
    """Map a BeeAI emitter event onto `{type, …}` for the dashboard.

    `Run.observe()` events fire at both bare `tool.<name>.start` and
    runcontext-wrapped `run.tool.<name>.start` namespaces. We keep only
    the bare form (skipping duplicates), plus the agent's final answer.
    Skipped: chat model rounds, requirement-evaluation events, and the
    synthetic FinalAnswerTool — all redundant for human-readable telemetry.
    """
    name = getattr(event, 'name', '')
    path = getattr(event, 'path', '')

    # Agent final answer → "thought" for the dashboard.
    if path == 'agent.requirement.final_answer' and data is not None:
        text = getattr(data, 'output', '') or ''
        if text:
            return {'type': 'thought', 'text': text}

    if not path.startswith('tool.'):
        return None
    if path.startswith('tool.final_answer.'):
        return None  # synthetic; overlaps with final_answer event above

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


def summarize_for_working(kind: dict[str, Any]) -> str:
    """Render a `classify_event` result as a one-line A2A working update."""
    t = kind.get('type')
    if t == 'tool_call':
        return f'→ {kind.get("name", "?")}({_compact(kind.get("input"))})'
    if t == 'tool_result':
        return f'← {kind.get("name", "?")} → {kind.get("output_summary", "")}'
    if t == 'thought':
        text = (kind.get('text') or '').strip()
        # Final answer is usually a bare filename — short. A long thought
        # would flood the UI, so cap it.
        if text and len(text) < 200:
            return f'💭 {text}'
    return ''


# ---------------------------------------------------------------------------
# Small helpers used by classify_event / summarize_for_working
# ---------------------------------------------------------------------------

def _dump(value: Any) -> Any:
    """Render a Pydantic model (or anything else) as a JSON-safe value."""
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


def _stringify_output(output: Any) -> str:
    if output is None:
        return ''
    if isinstance(output, ToolOutput):
        s = output.get_text_content()
    else:
        s = str(output)
    return s if len(s) <= 200 else s[:197] + '…'


def _compact(value: Any) -> str:
    if value is None:
        return ''
    s = str(value)
    return s if len(s) <= 120 else s[:117] + '…'
