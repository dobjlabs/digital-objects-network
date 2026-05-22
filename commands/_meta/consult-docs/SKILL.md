---
name: bitcraft-consult-docs
description: Consult the bitcraft docs to answer a question.
argument-hint: [question]
arguments: question
---

# consult-docs

Answer the user's question using ONLY text from the bitcraft doc resources. Quote verbatim. Never paraphrase, summarize, or generate new sentences.

## Output rules

- Output is a quoted excerpt from one or more doc resources, plus an optional source header per excerpt. Nothing else.
- You MUST NOT generate prose of your own. Every sentence in your reply must be present, character-for-character, in a fetched resource.
- You MUST NOT paraphrase. If you find yourself rewriting a sentence "in plain English," stop — quote the original instead.
- You MUST NOT summarize across sections. Pick the smallest set of full sections that answers the question and quote them.
- You MUST NOT output the full doc unless the user's question explicitly asks for "everything", "the full doc", "show me all of [topic]", or similar all-or-nothing phrasing.
- If no doc section meaningfully addresses the question, output EXACTLY this one line and stop:

  ```
  not covered in docs — ask on the bitcraft Discord: <discord-invite-url>
  ```

  (Substitute `<discord-invite-url>` with the project's Discord invite when published; otherwise leave the literal placeholder.)

- No preamble. No closing summary. No commentary about what you found. The quoted excerpts speak for themselves.

## Steps

1. If `$question` is empty (the user invoked `consult-docs` with no argument), output `usage: consult-docs <question>` and stop.

2. Read the player-facing doc resources via the MCP tool `ReadMcpResourceTool`:
   - `server: "bitcraft", uri: "bitcraft://docs/game-rules"`
   - `server: "bitcraft", uri: "bitcraft://docs/digital-objects"`

3. If the question is about protocol internals (ZK predicates, podlang, transaction model, nullifier mechanics in detail), ALSO read:
   - `server: "bitcraft", uri: "bitcraft://docs/object-lifecycle"`
   - `server: "bitcraft", uri: "bitcraft://docs/podlang-reference"`

4. Scan the fetched markdown. A "section" is a markdown heading (e.g. `## Live vs. spent`) plus all the body content under it (up to the next heading of equal or shallower depth).

5. Identify the smallest set of sections that contains the answer to the user's question. Prefer one section. If two non-adjacent sections are both required, include both.

6. Output the matched section(s) verbatim — including the heading line. If you matched sections from more than one resource, put a tiny source header before each excerpt:

   ```
   # from bitcraft://docs/<resource>

   <section content verbatim>
   ```

   If only one resource matched, omit the source header.

7. If after scanning, no section contains text that answers the question — including no section that the answer could be drawn from without re-wording — output the fallback line from "Output rules" and stop.

8. On tool error (resource not found, server unreachable), output the error message verbatim, on one line. Stop.

## What counts as a "matching section"

A section matches the question if it contains the literal phrases or specific concepts the user is asking about. Examples:

- Question: "what's a Pick?" → match the section under `## Actions` in game-rules (it lists picks) OR the relevant paragraph in digital-objects that mentions picks.
- Question: "how do nullifiers work?" → match `## Live vs. spent` in digital-objects.
- Question: "what's the tech tree?" → match `## The tech tree` in game-rules.
- Question: "what's a podlang predicate?" → match a heading in podlang-reference.

If the user's question is too vague to map to a section (e.g. "tell me about bitcraft"), pick the top-level intro of game-rules or digital-objects — whichever resource's first paragraph most directly answers it.

If the user's question is about something the docs simply don't cover (e.g. "what's the price of stone in USD?", "how do I make a sword?", "is there multiplayer?"), output the fallback line. Do NOT invent answers from general knowledge.
