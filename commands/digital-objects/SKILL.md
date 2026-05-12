---
name: bitcraft-digital-objects
description: Digital Objects — explain what they are, how they live on disk, and how trading works.
---

# digital-objects

## Output rules

- Markdown is fine — this is teaching content, not MUD command output.
- Echo the fetched resource content verbatim. No preamble, no closing summary, no commentary outside the resource body.
- Do not mention any other command or skill.

## Steps

1. Call the MCP tool `ReadMcpResourceTool` with `server: "bitcraft"` and `uri: "bitcraft://docs/digital-objects"`.

2. Output the returned resource content as the entire reply, exactly as the server returned it. The content is markdown — preserve all headings, lists, and code blocks.

3. On tool error (resource not found, server unreachable, etc.), output the error message verbatim, on one line. Stop.
