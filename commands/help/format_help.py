#!/usr/bin/env python3
"""Scan ~/.claude/skills/bitcraft-* and print a formatted help block.

The output is the FULL reply the agent will send to the user — no other
text, no markdown decorations. Tweak the layout here, not in any
MCP-server instructions.
"""

from pathlib import Path

SKILLS_DIR = Path.home() / ".claude" / "skills"
PREFIX = "bitcraft-"
INDENT = "  "             # leading spaces on each row
GAP = "  "                # spaces between name column and description
HEADER = "Commands:"


def parse_frontmatter(skill_md: Path) -> dict | None:
    """Read a SKILL.md and return the YAML-frontmatter fields as a dict.

    Returns None if the file is missing or has no `---`-delimited
    frontmatter block at the top.
    """
    try:
        text = skill_md.read_text(encoding="utf-8")
    except OSError:
        return None
    if not text.startswith("---\n"):
        return None
    end = text.find("\n---", 4)
    if end == -1:
        return None
    fields: dict[str, str] = {}
    for line in text[4:end].splitlines():
        if ":" in line:
            k, _, v = line.partition(":")
            fields[k.strip()] = v.strip()
    return fields


def collect_commands() -> list[tuple[str, str]]:
    """Return [(short_name, description), ...] for all visible bitcraft-* skills,
    sorted alphabetically by short_name.
    """
    out: list[tuple[str, str]] = []
    if not SKILLS_DIR.is_dir():
        return out
    for child in sorted(SKILLS_DIR.iterdir()):
        if not child.name.startswith(PREFIX):
            continue
        fm = parse_frontmatter(child / "SKILL.md")
        if not fm:
            continue
        full_name = fm.get("name", "")
        if not full_name.startswith(PREFIX):
            continue
        short_name = full_name[len(PREFIX):]
        description = fm.get("description", "")
        hidden = fm.get("hidden", "").lower() in ("true", "yes", "1")
        if hidden or not short_name or not description:
            continue
        out.append((short_name, description))
    return out


def render(commands: list[tuple[str, str]]) -> str:
    """Wrap the help block in a fenced code block so the chat UI
    preserves the column alignment (markdown collapses spaces in
    plain prose)."""
    if not commands:
        body = (
            f"{HEADER}\n"
            f"{INDENT}(no bitcraft commands installed — type create-command to define one)"
        )
    else:
        name_width = max(len(name) for name, _ in commands)
        lines = [HEADER]
        for name, desc in commands:
            lines.append(f"{INDENT}{name.ljust(name_width)}{GAP}{desc}")
        body = "\n".join(lines)
    return f"```\n{body}\n```"


def main() -> None:
    print(render(collect_commands()))


if __name__ == "__main__":
    main()
