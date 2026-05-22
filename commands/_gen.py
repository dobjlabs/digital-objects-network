#!/usr/bin/env python3
"""Generate per-class command SKILL.md files for an episode plugin.

Reads two files from `plugins/<EPISODE>/`:
- `manifest.toml`  → class + action metadata (name, emoji, description, hidden)
- `plugin.rhai`    → per-action input/output classes, parsed by regex

For every class that has at least one non-hidden action producing it, writes
`commands/<EPISODE>/<verb>-<class-kebab>/SKILL.md`, where `<verb>` is `mine`
if every producing action begins with `Mine`, `farm` if every one begins with
`Farm`, otherwise `craft`. The SKILL.md prompts the user to pick a recipe
variant, then picks each input from inventory, then runs the action.

Usage:
    python3 commands/_gen.py <episode-name>

Idempotent: wipes `commands/<EPISODE>/` first so renamed/deleted classes don't
linger. The generated files are checked into the source tree so reviewers can
inspect them; re-run after editing manifest.toml or plugin.rhai.
"""

import argparse
import re
import shutil
import sys
from pathlib import Path

# tomllib is stdlib only on Python 3.11+. Fall back to the `tomli` package
# (preinstalled on the user's machine; pip-installable elsewhere). All we need
# is `loads(str) -> dict`, which both expose identically.
try:
    import tomllib  # type: ignore[import-not-found]
except ModuleNotFoundError:  # pragma: no cover — exercised on Python < 3.11
    import tomli as tomllib  # type: ignore[import-not-found, no-redef]


REPO = Path(__file__).resolve().parent.parent


# ── parsing ─────────────────────────────────────────────────────────────────


def parse_manifest(path: Path) -> tuple[dict[str, dict], dict[str, dict]]:
    """Return (classes, actions) keyed by name. Both maps include `emoji`,
    `description`, and (for actions) `hidden`."""
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    classes = {c["name"]: c for c in data.get("classes", [])}
    actions = {
        a["name"]: a
        for a in data.get("actions", [])
    }
    return classes, actions


# Regex for `fn ActionName(action) { … body … }` blocks. Greedy enough to
# capture nested braces in practice because the Rhai functions in this repo
# don't use anonymous closures with braces at the top scope.
_FN_RE = re.compile(r"fn\s+(\w+)\s*\(\s*action\s*\)\s*\{(.*?)\n\}", re.DOTALL)
_INPUT_RE = re.compile(r'action\.input\(\s*"(\w+)"\s*\)')
_OUTPUT_RE = re.compile(r'action\.output\(\s*"(\w+)"\s*\)')
_MUTATE_RE = re.compile(r'action\.mutate\(\s*"(\w+)"\s*\)')
_SUB_RE = re.compile(r'action\.subaction\(\s*"(\w+)"\s*\)')


def parse_rhai(path: Path) -> dict[str, dict]:
    """For each `fn ActionName(action) { … }` block, return:
        {
            "inputs":     [class, …]   # ordered, with duplicates
            "outputs":    [class, …]   # ordered, with duplicates
            "mutates":    [class, …]   # consumed-and-rewritten same class
            "subactions": [name, …]    # other actions called as sub-procedures
        }
    """
    text = path.read_text(encoding="utf-8")
    out: dict[str, dict] = {}
    for m in _FN_RE.finditer(text):
        name = m.group(1)
        body = m.group(2)
        out[name] = {
            "inputs":     _INPUT_RE.findall(body),
            "outputs":    _OUTPUT_RE.findall(body),
            "mutates":    _MUTATE_RE.findall(body),
            "subactions": _SUB_RE.findall(body),
        }
    return out


# ── command shape ───────────────────────────────────────────────────────────


def kebab(camel: str) -> str:
    """`CamelCase` → `camel-case`, `MachineII` → `machine-ii`,
    `DrillBit` → `drill-bit`."""
    s = re.sub(r"(.)([A-Z][a-z]+)", r"\1-\2", camel)
    s = re.sub(r"([a-z0-9])([A-Z])", r"\1-\2", s)
    return s.lower()


def verb_for(actions: list[str]) -> str:
    """Pick the verb prefix based on what produces the class.

    Precedence: Mine > Farm > Craft. So if a class is produced both by a
    Mine* action (its primary producer) AND a Craft* action that recovers
    it as a byproduct (e.g. `CraftRefineryCrude` recovers Oil), we still
    name the command `mine-<class>` — the player's mental model is "I mine
    oil," not "I craft oil." Same logic for Farm.
    """
    if any(a.startswith("Mine") for a in actions):
        return "mine"
    if any(a.startswith("Farm") for a in actions):
        return "farm"
    return "craft"


def group_inputs(inputs: list[str]) -> list[tuple[str, int]]:
    """Order-preserving group: `["Iron", "Iron", "Flux"]` → `[("Iron", 2), ("Flux", 1)]`."""
    out: list[tuple[str, int]] = []
    for cls in inputs:
        if out and out[-1][0] == cls:
            out[-1] = (cls, out[-1][1] + 1)
        else:
            out.append((cls, 1))
    return out


def group_outputs(outputs: list[str]) -> list[tuple[str, int]]:
    return group_inputs(outputs)  # same logic, different name for clarity


def producer_command_name(
    class_name: str,
    producers_by_class: dict[str, list[str]],
) -> str | None:
    """Return the kebab command name that produces `class_name`, or None if
    nothing produces it. Used to render `no <X> available — run <cmd>` hints."""
    actions = producers_by_class.get(class_name, [])
    if not actions:
        return None
    return f"{verb_for(actions)}-{kebab(class_name)}"


# ── rendering ───────────────────────────────────────────────────────────────


def render_skill_md(
    class_name: str,
    cmd_name: str,
    producing_actions: list[str],
    actions_meta: dict[str, dict],
    rhai_io: dict[str, dict],
    producers_by_class: dict[str, list[str]],
    classes_meta: dict[str, dict],
) -> str:
    """Build the full SKILL.md text for `<cmd_name>`."""
    emoji = classes_meta.get(class_name, {}).get("emoji", "")
    n_recipes = len(producing_actions)
    verb = verb_for(producing_actions)
    verb_title = verb.capitalize()

    if n_recipes == 1:
        desc = actions_meta[producing_actions[0]]["description"]
    else:
        desc = f"{verb_title} {class_name} ({n_recipes} recipe variants)."

    lines: list[str] = []
    lines.append("---")
    lines.append(f"name: bitcraft-{cmd_name}")
    lines.append(f"description: {desc}")
    lines.append("---")
    lines.append("")
    lines.append(f"# {cmd_name}")
    lines.append("")
    lines.append("## Output rules")
    lines.append("")
    lines.append("- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.")
    lines.append("- No preamble. No closing summary. No suggestions. No commentary.")
    lines.append("- Do not mention any other command, skill, or capability.")
    lines.append("")
    lines.append("## Steps")
    lines.append("")

    step = 1

    # ── recipe pick (only if >1 variant) ────────────────────────────────────
    if n_recipes > 1:
        lines.append(f"{step}. Output exactly the following recipe menu, then end the turn and wait for the user's reply:")
        lines.append("")
        lines.append("   ```")
        for i, a in enumerate(producing_actions, start=1):
            lines.append(f"   {i}) {a} — {actions_meta[a]['description']}")
        lines.append("   pick recipe:")
        lines.append("   ```")
        lines.append("")
        step += 1

        lines.append(f"{step}. First check for exit words. If the reply (case-insensitive, trimmed) is `cancel`, `quit`, `exit`, `q`, or `nevermind`, output exactly `cancelled` and stop. Otherwise parse as an integer in the range 1..{n_recipes}. If invalid, output exactly `invalid choice` and stop.")
        lines.append("")
        step += 1

        lines.append(f"{step}. Branch on the chosen recipe number:")
        lines.append("")
        for i, a in enumerate(producing_actions, start=1):
            inputs = rhai_io.get(a, {}).get("inputs", [])
            grouped = group_inputs(inputs)
            outputs = group_outputs(rhai_io.get(a, {}).get("outputs", []))
            if grouped:
                input_str = ", ".join(f"{cnt} {c}" for c, cnt in grouped)
            else:
                input_str = "no input objects"
            out_str = ", ".join(f"{cnt} {c}" for c, cnt in outputs)
            lines.append(f"   - **{i}** → `action_id=\"{a}\"`, inputs: {input_str}, outputs: {out_str}.")
        lines.append("")
        step += 1
    else:
        # Single recipe: no menu, no branch.
        pass

    # ── input picking ───────────────────────────────────────────────────────
    if n_recipes == 1:
        a = producing_actions[0]
        inputs = rhai_io.get(a, {}).get("inputs", [])
        grouped = group_inputs(inputs)
        outputs = group_outputs(rhai_io.get(a, {}).get("outputs", []))
        if not grouped:
            # Zero-input recipe (mining + farming). Just run it.
            lines.append(f"{step}. Call `run_action` with `action_id=\"{a}\"` and `input_object_paths=[]`.")
            step += 1
            lines.append("")
            lines.append(f"{step}. On success, for each entry in the tool result's `outputs` array, output one line:")
            lines.append("")
            lines.append("   `<class_name> → <output_path>`")
            lines.append("")
            lines.append(f"   The class names you should see, in order: {', '.join(f'{cnt}× {c}' for c, cnt in outputs) or 'none'}.")
            lines.append("")
            step += 1
            lines.append(f"{step}. On tool error, output the tool's error message verbatim, on one line. Stop.")
            return "\n".join(lines) + "\n"

        # Has inputs. Pick each one from inventory.
        for c, cnt in grouped:
            hint = producer_command_name(c, producers_by_class)
            hint_suffix = f" — run {hint}" if hint else ""
            lines.append(f"{step}. Call `list_inventory`. Filter to live objects with `class_name == \"{c}\"`. If fewer than {cnt}, output exactly `no {c} available{hint_suffix}` and stop.")
            step += 1
            if cnt == 1:
                lines.append("")
                lines.append(f"{step}. Output candidates and prompt — exactly:")
                lines.append("")
                lines.append("   ```")
                lines.append(f"   1) <file_name of first live {c}>")
                lines.append(f"   2) <file_name of second live {c}>")
                lines.append("   ...")
                lines.append(f"   pick {c}:")
                lines.append("   ```")
                lines.append("")
                lines.append("   End the turn and wait for the user's reply.")
                lines.append("")
                step += 1
                lines.append(f"{step}. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<{c.lower()}_path>`.")
                step += 1
            else:
                lines.append("")
                lines.append(f"{step}. Output candidates and prompt — exactly:")
                lines.append("")
                lines.append("   ```")
                lines.append(f"   1) <file_name of first live {c}>")
                lines.append(f"   2) <file_name of second live {c}>")
                lines.append("   ...")
                lines.append(f"   pick {cnt} {c} (comma-separated, e.g. 1,2):")
                lines.append("   ```")
                lines.append("")
                lines.append("   End the turn and wait for the user's reply.")
                lines.append("")
                step += 1
                lines.append(f"{step}. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly {cnt} comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<{c.lower()}_paths>`.")
                step += 1
            lines.append("")

        # Run the action.
        path_args = []
        for c, cnt in grouped:
            if cnt == 1:
                path_args.append(f"<{c.lower()}_path>")
            else:
                path_args.append(f"...<{c.lower()}_paths>")
        path_list = ", ".join(path_args)
        lines.append(f"{step}. Call `run_action` with `action_id=\"{a}\"` and `input_object_paths=[{path_list}]` (flatten into a single list in the order shown).")
        step += 1
        lines.append("")
        lines.append(f"{step}. On success, for each entry in the tool result's `outputs` array, output one line:")
        lines.append("")
        lines.append("   `<class_name> → <output_path>`")
        lines.append("")
        lines.append(f"   The class names you should see, in order: {', '.join(f'{cnt}× {c}' for c, cnt in outputs) or 'none'}.")
        lines.append("")
        step += 1
        lines.append(f"{step}. On tool error, output the tool's error message verbatim, on one line. Stop.")
        return "\n".join(lines) + "\n"

    # ── multi-recipe path ──────────────────────────────────────────────────
    # For multi-recipe commands, drive input picking off the chosen recipe's
    # `inputs` slot list. Rather than emit a per-recipe full procedure (would
    # explode the file size), tell the agent to follow the slot list of the
    # chosen recipe.
    lines.append(f"{step}. For each input slot of the chosen recipe (looked up in step 3), in order:")
    lines.append(f"   - Call `list_inventory`. Filter to live objects matching the slot's class.")
    lines.append(f"   - If fewer than the slot's required count are available, output `no <class> available — run <producer>` and stop. `<producer>` is the bitcraft command that produces that class (e.g. `mine-iron` for `Iron`, `farm-water` for `Water`, `craft-flux` for `Flux`).")
    lines.append(f"   - Output candidates and prompt:")
    lines.append("")
    lines.append("     ```")
    lines.append(f"     1) <file_name of first candidate>")
    lines.append(f"     2) <file_name of second candidate>")
    lines.append("     ...")
    lines.append(f"     pick <class>:")
    lines.append("     ```")
    lines.append("")
    lines.append(f"   - If the slot's count is >1, prompt `pick <count> <class> (comma-separated, e.g. 1,2):` instead and parse as that many distinct integers.")
    lines.append(f"   - Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`).")
    lines.append(f"   - Parse choice(s). Invalid → `invalid choice` and stop.")
    lines.append(f"   - Append the chosen `file_path` value(s) to the running `input_object_paths` list, in order.")
    lines.append("")
    step += 1

    lines.append(f"{step}. Call `run_action` with the chosen recipe's `action_id` and the accumulated `input_object_paths`.")
    step += 1
    lines.append("")
    lines.append(f"{step}. On success, for each entry in the tool result's `outputs` array, output one line:")
    lines.append("")
    lines.append("   `<class_name> → <output_path>`")
    lines.append("")
    step += 1
    lines.append(f"{step}. On tool error, output the tool's error message verbatim, on one line. Stop.")
    return "\n".join(lines) + "\n"


# ── main ────────────────────────────────────────────────────────────────────


def generate(episode: str) -> None:
    plugin_dir = REPO / "plugins" / episode
    cmds_dir = REPO / "commands" / episode
    if not plugin_dir.is_dir():
        sys.exit(f"plugins/{episode}/ not found")

    classes_meta, actions_meta = parse_manifest(plugin_dir / "manifest.toml")
    rhai_io = parse_rhai(plugin_dir / "plugin.rhai")

    # Build producers_by_class from non-hidden actions only.
    producers_by_class: dict[str, list[str]] = {}
    for action_name, meta in actions_meta.items():
        if meta.get("hidden", False):
            continue
        outputs = rhai_io.get(action_name, {}).get("outputs", [])
        for cls in dict.fromkeys(outputs):  # dedupe, preserve order
            producers_by_class.setdefault(cls, []).append(action_name)

    # Wipe target dir so renamed/removed classes don't linger.
    if cmds_dir.exists():
        # Be careful: only delete subdirs we'd have generated (i.e., everything).
        for child in cmds_dir.iterdir():
            if child.is_dir():
                shutil.rmtree(child)
    cmds_dir.mkdir(parents=True, exist_ok=True)

    written: list[str] = []
    for class_name in sorted(producers_by_class.keys()):
        actions = producers_by_class[class_name]
        verb = verb_for(actions)
        cmd_name = f"{verb}-{kebab(class_name)}"
        target_dir = cmds_dir / cmd_name
        target_dir.mkdir(parents=True, exist_ok=True)
        skill = render_skill_md(
            class_name=class_name,
            cmd_name=cmd_name,
            producing_actions=actions,
            actions_meta=actions_meta,
            rhai_io=rhai_io,
            producers_by_class=producers_by_class,
            classes_meta=classes_meta,
        )
        (target_dir / "SKILL.md").write_text(skill, encoding="utf-8")
        written.append(cmd_name)

    print(f"generated {len(written)} command(s) under commands/{episode}/")
    for name in written:
        print(f"  {name}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("episode", help="Plugin/episode name (e.g. episode-1)")
    args = parser.parse_args()
    generate(args.episode)
    return 0


if __name__ == "__main__":
    sys.exit(main())
