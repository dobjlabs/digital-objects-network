#!/usr/bin/env python3
"""Generate the bitcraft presentation pack: pack.json and the action icons.

The pack gives every bitcraft class and action an icon, a one-line summary
and a longer description, for apps to show. Nothing in dobjd or the .pexe
reads it. Each action's icon is drawn here, once, as a finished SVG, so an
app only has to load the file.

Run it after changing a recipe, adding a class or action, or editing the
art or text below:

    python3 examples/bitcraft/presentation/generate.py

It reads the class and action names from ../manifest.toml and refuses to
write anything if one of them has no entry here. Needs only the Python 3.9
standard library.
"""

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

# ---- Classes: art and text. Text is read by players; keep it in step with
# ../src/main.rs, which generates the recipes.

CLASSES = {
    "Wood": ("wood.png", "A basic building material.",
             "Gathered with a pocketknife or chopped with an axe. Goes into sticks, axes, "
             "quarries and mining drills, and can be stored in a bucket of wood."),
    "Stone": ("stone.png", "Rock for heavier tools and machines.",
              "Gathered with a pocketknife or mined with a quarry. Goes into stone axes, "
              "quarries and mining drills, and can be stored in a bucket of stone."),
    "Iron": ("iron.png", "The rarest raw resource.",
             "Gathered with a pocketknife, which takes the most work of any resource, or "
             "drilled with a mining drill. Goes into iron axes, buckets and mining drills."),
    "Stick": ("stick.png", "Half a piece of wood.",
              "Made by splitting wood. Two sticks and a piece of wood make a wood axe."),
    "BucketOfWood": ("bucket.png", "Holds any amount of wood in one item.",
                     "Keeps your inventory short. Full buckets are spent to level up "
                     "quarries and mining drills."),
    "BucketOfStone": ("bucket.png", "Holds any amount of stone in one item.",
                      "Keeps your inventory short. Full buckets are spent to level up "
                      "quarries and mining drills."),
    "BucketOfIron": ("bucket.png", "Holds any amount of iron in one item.",
                     "Keeps your inventory short. Full buckets are spent to level up "
                     "mining drills."),
    "Pocketknife": ("pocketknife.png", "Your first tool.",
                    "Made from nothing. You need one to gather wood, stone and iron, and "
                    "gathering gives it back, so one lasts forever."),
    "WoodAxe": ("wood-axe.png", "Chops 2 wood per use.",
                "Starts with 100 durability and loses 1 per use; at 0 it can no longer be "
                "used. Can be upgraded to a stone axe."),
    "StoneAxe": ("stone-axe.png", "Chops 3 wood per use.",
                 "Made by upgrading a wood axe, keeping its durability. Loses 1 durability "
                 "per use. Can be upgraded to an iron axe."),
    "IronAxe": ("iron-axe.png", "Chops 4 wood per use.",
                "Made by upgrading a stone axe, keeping its durability. Loses 1 durability "
                "per use."),
    "Quarry": ("quarry.png", "Mines stone without wearing out.",
               "Makes one more stone per use than its level, up to level 6. Level it up with "
               "full buckets of wood and stone."),
    "MiningDrill": ("mining-drill.png", "Drills iron from wood.",
                    "Spends 5 wood per use and makes one more iron than its level, up to "
                    "level 6. Level it up with full buckets of wood, stone and iron."),
    "Bit": ("bit.png", "The game's finest-grained material.",
            "Made from wood, stone or iron. No recipe uses bits yet."),
}

# The art drawn on the rekey icon, which is the same for every class.
REKEY_ART = PACK / "source" / "refresh.svg"

# ---- Actions: the art and badge drawn on each icon, and the text. Each rule
# matches a family of action names; the first match applies.

RESOURCES = {"Wood": "wood", "Stone": "stone", "Iron": "iron"}
GATHER_EFFORT = {"Wood": "a little", "Stone": "a moderate amount of", "Iron": "a lot of"}
AXE_YIELD = {"WoodAxe": 2, "StoneAxe": 3, "IronAxe": 4}
AXE_NAME = {"WoodAxe": "wood axe", "StoneAxe": "stone axe", "IronAxe": "iron axe"}
BITS_YIELD = {"Wood": 1, "Stone": 2, "Iron": 5}


def lower(name):
    return re.sub(r"(?<!^)([A-Z])", r" \1", name).lower()


def quarry_levelup(level):
    return 20 * 2 ** level, 10 * 2 ** level


def drill_levelup(level):
    return 10 * 2 ** level, 50 * 2 ** level, 5 * 2 ** level


ACTION_RULES = [
    (r"CraftPocketknife", lambda m: (
        "Pocketknife", "plus",
        "Make a pocketknife from nothing.",
        "Your first tool. It takes no materials, and you need it to gather wood, stone "
        "and iron.")),
    (r"Gather(Wood|Stone|Iron)", lambda m: (
        m[1], "plus",
        f"Gather 1 {RESOURCES[m[1]]} with your pocketknife.",
        f"Your computer does {GATHER_EFFORT[m[1]]} work first. The pocketknife comes back "
        f"with a fresh key, ready to use again.")),
    (r"Craft(BucketOf(Wood|Stone|Iron))", lambda m: (
        m[1], "plus",
        f"Make an empty bucket for {RESOURCES[m[2]]} from 1 iron.",
        f"A bucket holds any amount of {RESOURCES[m[2]]} in one item. Fill it with the "
        f"Add actions.")),
    (r"(BucketOf(Wood|Stone|Iron))Add(\d+)", lambda m: (
        m[1], "up",
        f"Put {m[3]} {RESOURCES[m[2]]} into a bucket.",
        f"Spends {m[3]} {RESOURCES[m[2]]} and adds them to the bucket's count.")),
    (r"(BucketOf(Wood|Stone|Iron))Take(\d+)", lambda m: (
        m[1], "down",
        f"Take {m[3]} {RESOURCES[m[2]]} out of a bucket.",
        f"Lowers the bucket's count by {m[3]} and gives you {m[3]} {RESOURCES[m[2]]}. The "
        f"bucket must hold at least that many.")),
    (r"CraftStick", lambda m: (
        "Stick", "plus",
        "Split 1 wood into 2 sticks.",
        "Sticks go into wood axes.")),
    (r"CraftWoodAxe", lambda m: (
        "WoodAxe", "plus",
        "Make a wood axe from 1 wood and 2 sticks.",
        "A new axe has 100 durability and loses 1 per use. It chops 2 wood per use.")),
    (r"Craft(StoneAxe|IronAxe)", lambda m: (
        m[1], "plus",
        f"Upgrade your {'wood' if m[1] == 'StoneAxe' else 'stone'} axe with 1 "
        f"{'stone' if m[1] == 'StoneAxe' else 'iron'}.",
        f"Uses up the old axe and keeps its remaining durability. The {AXE_NAME[m[1]]} "
        f"chops {AXE_YIELD[m[1]]} wood per use.")),
    (r"Use(WoodAxe|StoneAxe|IronAxe)", lambda m: (
        m[1], None,
        f"Chop {AXE_YIELD[m[1]]} wood with your {AXE_NAME[m[1]]}.",
        "Takes 1 durability; an axe at 0 can no longer be used. Your computer does a "
        "little work first.")),
    (r"BuildQuarry", lambda m: (
        "Quarry", "plus",
        "Build a level 1 quarry from 20 wood and 10 stone.",
        "Quarries mine stone without wearing out, and make more at higher levels.")),
    (r"BuildMiningDrill", lambda m: (
        "MiningDrill", "plus",
        "Build a level 1 mining drill from 10 wood, 50 stone and 5 iron.",
        "Mining drills turn wood into iron without wearing out, and make more at higher "
        "levels.")),
    (r"UseQuarry(\d+)", lambda m: (
        "Quarry", None,
        f"Mine {int(m[1]) + 1} stone with a level {m[1]} quarry.",
        f"Only works on a quarry at exactly level {m[1]}. Your computer does some work "
        f"first.")),
    (r"UseMiningDrill(\d+)", lambda m: (
        "MiningDrill", None,
        f"Drill {int(m[1]) + 1} iron from 5 wood with a level {m[1]} mining drill.",
        f"Only works on a mining drill at exactly level {m[1]}. Your computer does a lot "
        f"of work first.")),
    (r"LevelUpQuarry(\d+)", lambda m: (
        "Quarry", "up",
        "Raise a quarry's level by 1.",
        "Spends a bucket holding exactly {} wood and a bucket holding exactly {} stone."
        .format(*quarry_levelup(int(m[1]))))),
    (r"LevelUpMiningDrill(\d+)", lambda m: (
        "MiningDrill", "up",
        "Raise a mining drill's level by 1.",
        "Spends buckets holding exactly {} wood, {} stone and {} iron."
        .format(*drill_levelup(int(m[1]))))),
    (r"BitsFrom(Wood|Stone|Iron)", lambda m: (
        "Bit", "plus",
        f"Turn 1 {RESOURCES[m[1]]} into {BITS_YIELD[m[1]]} "
        f"bit{'s' if BITS_YIELD[m[1]] != 1 else ''}.",
        "Takes a short, fixed amount of computing time.")),
]

# Built-in actions every class gets from the SDK, listed in no manifest.
BUILTIN_RULES = [
    (r"Rekey(\w+)", lambda m: (
        REKEY_ART, None,
        f"Give a {lower(m[1])} a fresh key.",
        "Changes nothing else about it. You won't need this for crafting.")),
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
        art_markup = pixel_art_svg(read_rgba_png(CLASS_ART / CLASSES[art][0]), 8, 8)
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


def describe_action(name, classes):
    for pattern, describe in ACTION_RULES:
        match = re.fullmatch(pattern, name)
        if match:
            return describe(match)
    for pattern, describe in BUILTIN_RULES:
        match = re.fullmatch(pattern, name)
        if match and match[1] in classes:
            return describe(match)
    return None


def main():
    plugin, module_hash, names = manifest_names((PLUGIN / "manifest.toml").read_text())
    # Built-in actions are not in the manifest; the SDK adds one per class.
    actions = names["actions"] + [f"Rekey{name}" for name in names["classes"]]

    missing = [f"class {name}" for name in names["classes"] if name not in CLASSES]
    missing += [f"action {name}" for name in actions if describe_action(name, CLASSES) is None]
    missing += [f"art {CLASS_ART / art}" for art, _, _ in CLASSES.values()
                if not (CLASS_ART / art).is_file()]
    if missing:
        sys.exit("No presentation for:\n  " + "\n  ".join(missing))

    pack = {
        "format": 1,
        "plugin": plugin,
        "writtenFor": {"moduleHash": module_hash},
        "classes": {},
        "actions": {},
    }
    for name in names["classes"]:
        art, summary, description = CLASSES[name]
        pack["classes"][name] = {
            "icon": f"assets/classes/{art}", "summary": summary, "description": description,
        }

    ACTION_ICONS.mkdir(parents=True, exist_ok=True)
    written = set()
    for name in actions:
        art, badge, summary, description = describe_action(name, CLASSES)
        icon = ACTION_ICONS / f"{name}.svg"
        icon.write_text(action_icon_svg(art, badge))
        written.add(icon.name)
        pack["actions"][name] = {
            "icon": f"assets/actions/{icon.name}", "summary": summary, "description": description,
        }
    for stale in ACTION_ICONS.glob("*.svg"):
        if stale.name not in written:
            stale.unlink()

    (PACK / "pack.json").write_text(json.dumps(pack, indent=2) + "\n")
    print(f"Wrote {len(pack['classes'])} classes and {len(pack['actions'])} actions to "
          f"{PACK / 'pack.json'}")


if __name__ == "__main__":
    main()
