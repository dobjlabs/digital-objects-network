#!/usr/bin/env python3
"""Generate the bitcraft presentation pack: pack.json and the action icons.

The pack gives every bitcraft class and action an icon, a one-line summary
and a longer description, for apps to show. Nothing in dobjd or the .pexe
reads it. Each action's icon is drawn here, once, as a finished SVG, so an
app only has to load the file. The summaries and descriptions are written
by hand in copy/classes.md and copy/actions.md (see README.md).

Run it after editing the copy or the art, or changing the plugin's classes
or actions:

    python3 examples/bitcraft/presentation/generate.py

With --check it writes nothing, and fails if pack.json or an action icon is
not what it would write. It reads the class and action names from
../manifest.toml and refuses to write anything if one of them has no art or
copy, or the copy is malformed. Needs only the Python 3.9 standard library.
"""

import argparse
import ast
import json
import re
import struct
import sys
import zlib
from pathlib import Path

PACK = Path(__file__).resolve().parent
PLUGIN = PACK.parent
CLASS_ART = PACK / "assets" / "classes"
ACTION_ICONS = PACK / "assets" / "actions"
COPY = PACK / "copy"

# ---- Classes: the art for each, also drawn on its actions' icons.

CLASS_ART_FILES = {
    "Wood": "wood.png",
    "Stone": "stone.png",
    "Iron": "iron.png",
    "Stick": "stick.png",
    "BucketOfWood": "bucket.png",
    "BucketOfStone": "bucket.png",
    "BucketOfIron": "bucket.png",
    "Pocketknife": "pocketknife.png",
    "WoodAxe": "wood-axe.png",
    "StoneAxe": "stone-axe.png",
    "IronAxe": "iron-axe.png",
    "Quarry": "quarry.png",
    "MiningDrill": "mining-drill.png",
    "Bit": "bit.png",
}

# The art drawn on the rekey icon, which is the same for every class.
REKEY_ART = PACK / "source" / "refresh.svg"

# ---- Actions: the art and badge drawn on each icon. Each rule matches a
# family of action names; the first match applies. The art is a class name,
# whose art is used, or a path to other art.

ACTION_ART_RULES = [
    (r"CraftPocketknife", lambda m: ("Pocketknife", "plus")),
    (r"Gather(Wood|Stone|Iron)", lambda m: (m[1], "plus")),
    (r"Craft(BucketOf(Wood|Stone|Iron))", lambda m: (m[1], "plus")),
    (r"(BucketOf(Wood|Stone|Iron))Add(\d+)", lambda m: (m[1], "up")),
    (r"(BucketOf(Wood|Stone|Iron))Take(\d+)", lambda m: (m[1], "down")),
    (r"CraftStick", lambda m: ("Stick", "plus")),
    (r"Craft(WoodAxe|StoneAxe|IronAxe)", lambda m: (m[1], "plus")),
    (r"Use(WoodAxe|StoneAxe|IronAxe)", lambda m: (m[1], None)),
    (r"BuildQuarry", lambda m: ("Quarry", "plus")),
    (r"BuildMiningDrill", lambda m: ("MiningDrill", "plus")),
    (r"UseQuarry(\d+)", lambda m: ("Quarry", None)),
    (r"UseMiningDrill(\d+)", lambda m: ("MiningDrill", None)),
    (r"LevelUpQuarry(\d+)", lambda m: ("Quarry", "up")),
    (r"LevelUpMiningDrill(\d+)", lambda m: ("MiningDrill", "up")),
    (r"BitsFrom(Wood|Stone|Iron)", lambda m: ("Bit", "plus")),
]

# Built-in actions every class gets from the SDK, listed in no manifest.
BUILTIN_ART_RULES = [
    (r"Rekey(\w+)", lambda m: (REKEY_ART, None)),
]

# ---- Badges, drawn on the 32-unit icon grid in the top-right corner, with a
# white outline so they read over any art.

BADGES = {
    "plus": '<path d="M24 0h5v3h3v5h-3v3h-5v-3h-3v-5h3z" fill="#fff"/>'
            '<path d="M25 1h3v3h3v3h-3v3h-3v-3h-3v-3h3z" fill="#16a34a"/>',
    "up": '<path d="M22.5 8.5l4-4 4 4" fill="none" stroke="#fff" stroke-width="5" '
          'stroke-linecap="round" stroke-linejoin="round"/>'
          '<path d="M22.5 8.5l4-4 4 4" fill="none" stroke="#16a34a" stroke-width="2.5" '
          'stroke-linecap="round" stroke-linejoin="round"/>',
    "down": '<path d="M22.5 4.5l4 4 4-4" fill="none" stroke="#fff" stroke-width="5" '
            'stroke-linecap="round" stroke-linejoin="round"/>'
            '<path d="M22.5 4.5l4 4 4-4" fill="none" stroke="#dc2626" stroke-width="2.5" '
            'stroke-linecap="round" stroke-linejoin="round"/>',
}


# ---- Copy: the hand-written summaries and descriptions.


class Copy:
    """One copy file's entries. An entry's heading is a name, or a family
    pattern with {placeholders} that covers every name it fits; an exact
    name overrides a family."""

    HEADING = re.compile(r"(?:[A-Za-z0-9]+|\{[a-z_]+\})+")

    def __init__(self, path, errors):
        self.path = path
        self.errors = errors
        # Heading -> (line number, summary, description or None).
        self.entries = {}
        self.families = []
        self.used = set()
        self.read()

    def error(self, line, message):
        self.errors.append(f"copy/{self.path.name}:{line}: {message}")

    def read(self):
        heading, heading_line, body = None, 0, []
        for number, line in enumerate(self.path.read_text().splitlines() + ["## "], 1):
            if line.startswith("## "):
                if heading is not None:
                    self.add(heading, heading_line, body)
                heading, heading_line, body = line[3:].strip(), number, []
            elif heading is None:
                continue
            elif line.startswith("#"):
                self.error(number, "only '## Name' headings are allowed after the first entry")
            else:
                body.append((number, line))

    def add(self, heading, line, body):
        if not self.HEADING.fullmatch(heading):
            self.error(line, f"'{heading}' is not a name or a family like UseQuarry{{level}}")
            return
        if heading in self.entries:
            self.error(line, f"'{heading}' already has an entry, at line {self.entries[heading][0]}")
            return
        paragraphs, current = [], []
        for _, text in body + [(0, "")]:
            if text.strip():
                current.append(text.strip())
            elif current:
                paragraphs.append(" ".join(current))
                current = []
        if not paragraphs:
            self.error(line, f"'{heading}' has no summary")
            return
        description = "\n\n".join(paragraphs[1:]) or None
        self.entries[heading] = (line, paragraphs[0], description)
        if "{" in heading:
            placeholders = re.findall(r"\{([a-z_]+)\}", heading)
            if len(placeholders) != len(set(placeholders)):
                self.error(line, f"'{heading}' uses a placeholder twice")
                return
            pattern = re.sub(r"\\\{([a-z_]+)\\\}", r"(?P<\1>[A-Za-z0-9]+?)", re.escape(heading))
            self.families.append((heading, re.compile(pattern)))

    def text_for(self, name):
        """The summary and description for `name`, or None when no entry fits."""
        if name in self.entries:
            self.used.add(name)
            _, summary, description = self.entries[name]
            return summary, description
        fits = [(heading, match) for heading, pattern in self.families
                for match in [pattern.fullmatch(name)] if match]
        if len(fits) > 1:
            self.error(self.entries[fits[0][0]][0],
                       f"'{name}' fits more than one family: "
                       + ", ".join(heading for heading, _ in fits)
                       + "; give it an entry of its own")
            return None
        if not fits:
            return None
        heading, match = fits[0]
        self.used.add(heading)
        line, summary, description = self.entries[heading]
        values = match.groupdict()
        render = lambda text: fill_in(text, values, lambda message: self.error(line, message))
        return render(summary), None if description is None else render(description)

    def report_unused(self):
        for heading, (line, _, _) in self.entries.items():
            if heading not in self.used:
                self.error(line, f"'{heading}' matches nothing in ../manifest.toml")


# Ways to write a placeholder's value: {resource|lower} gives "wood" for Wood,
# {class|words} gives "bucket of wood" for BucketOfWood.
FILTERS = {
    "lower": str.lower,
    "words": lambda value: re.sub(r"(?<!^)([A-Z])", r" \1", value).lower(),
}


def fill_in(text, values, error):
    """Replaces each {field} in `text`: a placeholder, a placeholder|filter, or
    whole-number arithmetic on placeholders, like {level + 1} or
    {20 * 2 ** level}. {{ and }} stand for literal braces."""
    def field(match):
        if match[0] in ("{{", "}}"):
            return match[0][0]
        inner = match[1].strip()
        if "|" in inner:
            name, filter_name = (part.strip() for part in inner.split("|", 1))
            if name not in values:
                error(f"{{{inner}}}: no placeholder '{name}' in the heading")
            elif filter_name not in FILTERS:
                error(f"{{{inner}}}: no filter '{filter_name}'; use " + " or ".join(FILTERS))
            else:
                return FILTERS[filter_name](values[name])
            return match[0]
        if inner in values:
            return values[inner]
        try:
            return str(evaluate(ast.parse(inner, mode="eval").body, values))
        except (SyntaxError, ValueError) as problem:
            error(f"{{{inner}}}: {problem}")
            return match[0]
    return re.sub(r"\{\{|\}\}|\{([^{}]*)\}", field, text)


ARITHMETIC = {ast.Add: lambda a, b: a + b, ast.Sub: lambda a, b: a - b,
              ast.Mult: lambda a, b: a * b, ast.Pow: lambda a, b: a ** b,
              ast.FloorDiv: lambda a, b: a // b}


def evaluate(node, values):
    if isinstance(node, ast.Constant) and type(node.value) is int:
        return node.value
    if isinstance(node, ast.Name):
        if node.id not in values:
            raise ValueError(f"no placeholder '{node.id}' in the heading")
        if not values[node.id].isdigit():
            raise ValueError(f"'{node.id}' is '{values[node.id]}', not a whole number")
        return int(values[node.id])
    if isinstance(node, ast.BinOp) and type(node.op) in ARITHMETIC:
        return ARITHMETIC[type(node.op)](evaluate(node.left, values), evaluate(node.right, values))
    raise ValueError("only placeholders, whole numbers and + - * ** // are allowed")


def manifest_names(manifest_text):
    """The plugin name and module hash, and the class and action names in order."""
    plugin = re.search(r'^\[plugin\][^\[]*?^name = "([^"]+)"', manifest_text, re.M | re.S)[1]
    module_hash = re.search(r'^module_hash = "([0-9a-f]+)"', manifest_text, re.M)[1]
    names = {"classes": [], "actions": []}
    for table, body in re.findall(r"^\[\[(classes|actions)\]\]\n(.*?)(?=^\[|\Z)",
                                  manifest_text, re.M | re.S):
        names[table].append(re.search(r'^name = "([^"]+)"', body, re.M)[1])
    return plugin, module_hash, names


def read_rgba_png(path):
    """The pixels of an 8-bit RGBA PNG, as rows of (r, g, b, a)."""
    data = path.read_bytes()
    position, compressed, width, height = 8, b"", 0, 0
    while position < len(data):
        length, kind = struct.unpack(">I4s", data[position:position + 8])
        body = data[position + 8:position + 8 + length]
        if kind == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
            if (depth, color, interlace) != (8, 6, 0):
                sys.exit(f"{path}: only 8-bit RGBA, non-interlaced PNGs are supported")
        elif kind == b"IDAT":
            compressed += body
        position += 12 + length
    raw, stride, rows, previous = zlib.decompress(compressed), width * 4, [], bytearray(width * 4)
    for y in range(height):
        start = y * (stride + 1)
        kind, line = raw[start], bytearray(raw[start + 1:start + 1 + stride])
        for x in range(stride):
            left = line[x - 4] if x >= 4 else 0
            up = previous[x]
            up_left = previous[x - 4] if x >= 4 else 0
            if kind == 1:
                line[x] = (line[x] + left) & 255
            elif kind == 2:
                line[x] = (line[x] + up) & 255
            elif kind == 3:
                line[x] = (line[x] + (left + up) // 2) & 255
            elif kind == 4:
                estimate = left + up - up_left
                distances = (abs(estimate - left), abs(estimate - up), abs(estimate - up_left))
                nearest = (left, up, up_left)[distances.index(min(distances))]
                line[x] = (line[x] + nearest) & 255
        rows.append([tuple(line[x:x + 4]) for x in range(0, stride, 4)])
        previous = line
    return rows


def pixel_art_svg(rows, left, top):
    """Draws pixel art as unit squares at (left, top), merging runs of equal
    pixels, so the icon stays sharp at any size in any browser."""
    shapes = []
    for y, row in enumerate(rows):
        x = 0
        while x < len(row):
            pixel, run = row[x], 1
            while x + run < len(row) and row[x + run] == pixel:
                run += 1
            red, green, blue, alpha = pixel
            if alpha:
                opacity = "" if alpha == 255 else f' fill-opacity="{alpha / 255:.3f}"'
                shapes.append(f'<rect x="{left + x}" y="{top + y}" width="{run}" height="1" '
                              f'fill="#{red:02x}{green:02x}{blue:02x}"{opacity}/>')
            x += run
    return "".join(shapes)


def action_icon_svg(art, badge):
    """A 32-unit app-style icon: a white rounded tile, the art at 16 units in
    its center, and an optional badge."""
    if isinstance(art, Path):
        inner = re.search(r"<svg[^>]*>(.*)</svg>", art.read_text(), re.S)[1].strip()
        art_markup = f'<svg x="8" y="8" width="16" height="16" viewBox="0 0 16 16">{inner}</svg>'
    else:
        art_markup = pixel_art_svg(read_rgba_png(CLASS_ART / CLASS_ART_FILES[art]), 8, 8)
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 32 32">'
        '<defs><filter id="shadow" x="-20%" y="-20%" width="140%" height="140%">'
        '<feDropShadow dx="0" dy="0.5" stdDeviation="0.5" flood-color="#000" flood-opacity="0.3"/>'
        '</filter></defs>'
        '<rect x="2" y="2" width="28" height="28" rx="6.25" fill="#fff" filter="url(#shadow)"/>'
        '<rect x="2.125" y="2.125" width="27.75" height="27.75" rx="6.125" fill="none" '
        'stroke="#000" stroke-opacity="0.08" stroke-width="0.25"/>'
        f'{art_markup}{BADGES.get(badge, "")}</svg>\n'
    )


def action_art(name, classes):
    """The art and badge for the action's icon, or None when no rule fits."""
    for pattern, art in ACTION_ART_RULES:
        match = re.fullmatch(pattern, name)
        if match:
            return art(match)
    for pattern, art in BUILTIN_ART_RULES:
        match = re.fullmatch(pattern, name)
        if match and match[1] in classes:
            return art(match)
    return None


def entry(icon, text):
    summary, description = text
    fields = {"icon": icon, "summary": summary}
    if description is not None:
        fields["description"] = description
    return fields


def main():
    parser = argparse.ArgumentParser(description="Generate the bitcraft presentation pack.")
    parser.add_argument("--check", action="store_true",
                        help="write nothing; fail if pack.json or an icon is out of date")
    check = parser.parse_args().check

    plugin, module_hash, names = manifest_names((PLUGIN / "manifest.toml").read_text())
    # Built-in actions are not in the manifest; the SDK adds one per class.
    actions = names["actions"] + [f"Rekey{name}" for name in names["classes"]]

    errors = []
    class_copy = Copy(COPY / "classes.md", errors)
    action_copy = Copy(COPY / "actions.md", errors)
    class_text = {name: class_copy.text_for(name) for name in names["classes"]}
    action_text = {name: action_copy.text_for(name) for name in actions}
    class_copy.report_unused()
    action_copy.report_unused()

    missing = [f"copy for class {name} in copy/classes.md"
               for name, text in class_text.items() if text is None]
    missing += [f"copy for action {name} in copy/actions.md"
                for name, text in action_text.items() if text is None]
    missing += [f"art for class {name} in CLASS_ART_FILES"
                for name in names["classes"] if name not in CLASS_ART_FILES]
    missing += [f"art {CLASS_ART / art}" for art in set(CLASS_ART_FILES.values())
                if not (CLASS_ART / art).is_file()]
    missing += [f"an icon rule for action {name}"
                for name in actions if action_art(name, CLASS_ART_FILES) is None]
    if errors or missing:
        sys.exit("\n".join(["The pack was not written."] + errors
                           + [f"No {item}." for item in missing]))

    pack = {
        "format": 1,
        "plugin": plugin,
        "writtenFor": {"moduleHash": module_hash},
        "classes": {name: entry(f"assets/classes/{CLASS_ART_FILES[name]}", class_text[name])
                    for name in names["classes"]},
        "actions": {},
    }
    icons = {}
    for name in actions:
        icons[f"{name}.svg"] = action_icon_svg(*action_art(name, CLASS_ART_FILES))
        pack["actions"][name] = entry(f"assets/actions/{name}.svg", action_text[name])
    files = {PACK / "pack.json": json.dumps(pack, indent=2) + "\n"}
    files.update({ACTION_ICONS / icon: svg for icon, svg in icons.items()})
    stale = [path for path in ACTION_ICONS.glob("*.svg") if path.name not in icons]

    if check:
        changed = [path for path, text in files.items()
                   if not path.is_file() or path.read_text() != text]
        if changed or stale:
            sys.exit("The presentation pack is out of date; run "
                     "`just bitcraft-presentation` and commit the result:\n  "
                     + "\n  ".join(str(path.relative_to(PACK)) for path in changed + stale))
        print("The presentation pack is up to date.")
        return

    ACTION_ICONS.mkdir(parents=True, exist_ok=True)
    for path, text in files.items():
        path.write_text(text)
    for path in stale:
        path.unlink()
    print(f"Wrote {len(pack['classes'])} classes and {len(pack['actions'])} actions to "
          f"{PACK / 'pack.json'}")


if __name__ == "__main__":
    main()
