#!/usr/bin/env python3
"""Copy the presentation packs the Bitcraft UI ships into ui/presentation/.

Each pack's pack.json and assets/ are copied to
ui/presentation/<plugin>/, and ui/presentation/index.json lists them. The
copy is generated and ignored by Git; this replaces it entirely each run.
To ship another plugin's pack, add its presentation folder to PACKS.
"""

import json
from pathlib import Path
import shutil


APP = Path(__file__).resolve().parents[1]
REPO = APP.parents[1]
DESTINATION = APP / "ui" / "presentation"
PACKS = [REPO / "examples" / "bitcraft" / "presentation"]


def main():
    if DESTINATION.exists():
        shutil.rmtree(DESTINATION)
    DESTINATION.mkdir(parents=True)
    listed = []
    for source in PACKS:
        pack = json.loads((source / "pack.json").read_text())
        plugin = pack["plugin"]
        target = DESTINATION / plugin
        target.mkdir()
        shutil.copy2(source / "pack.json", target / "pack.json")
        if (source / "assets").is_dir():
            shutil.copytree(source / "assets", target / "assets")
        listed.append(f"{plugin}/pack.json")
    (DESTINATION / "index.json").write_text(json.dumps({"packs": listed}, indent=2) + "\n")
    print(f"Synced {len(listed)} presentation pack(s) to {DESTINATION}")


if __name__ == "__main__":
    main()
