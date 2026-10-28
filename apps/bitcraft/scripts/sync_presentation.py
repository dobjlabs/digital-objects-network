#!/usr/bin/env python3
"""Copy the presentation packs the Bitcraft UI ships into ui/presentation/.

Each pack's pack.json and assets/ are copied to
ui/presentation/<plugin>/, and ui/presentation/index.json lists them. The
copy is generated and ignored by Git; unchanged copies are left untouched.
To ship another plugin's pack, add its presentation folder to PACKS.
"""

import filecmp
import json
from pathlib import Path
import shutil
import tempfile


APP = Path(__file__).resolve().parents[1]
REPO = APP.parents[1]
DESTINATION = APP / "ui" / "presentation"
PACKS = [REPO / "examples" / "bitcraft" / "presentation"]


def same_tree(source, target):
    if not target.is_dir() or target.is_symlink():
        return False
    comparison = filecmp.dircmp(source, target, ignore=[])
    if comparison.left_only or comparison.right_only or comparison.common_funny:
        return False
    if any((target / name).is_symlink() for name in comparison.common):
        return False
    _, different, errors = filecmp.cmpfiles(
        source, target, comparison.common_files, shallow=False)
    return not different and not errors and all(
        same_tree(source / name, target / name) for name in comparison.common_dirs)


def copy_packs(destination):
    listed = []
    for source in PACKS:
        pack = json.loads((source / "pack.json").read_text())
        plugin = pack["plugin"]
        target = destination / plugin
        target.mkdir()
        shutil.copy2(source / "pack.json", target / "pack.json")
        if (source / "assets").is_dir():
            shutil.copytree(source / "assets", target / "assets")
        listed.append(f"{plugin}/pack.json")
    (destination / "index.json").write_text(json.dumps({"packs": listed}, indent=2) + "\n")
    return len(listed)


def main():
    with tempfile.TemporaryDirectory(prefix="bitcraft-presentation-") as temporary:
        staged = Path(temporary)
        count = copy_packs(staged)
        if same_tree(staged, DESTINATION):
            print(f"Presentation pack(s) unchanged ({count}) at {DESTINATION}")
            return
        if DESTINATION.exists():
            shutil.rmtree(DESTINATION)
        shutil.copytree(staged, DESTINATION)
    print(f"Synced {count} presentation pack(s) to {DESTINATION}")


if __name__ == "__main__":
    main()
