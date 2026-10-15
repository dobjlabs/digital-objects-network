"""Initialize a local-network Bitcraft home without changing existing settings."""

import argparse
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("home", type=Path)
    home = parser.parse_args().home.expanduser().resolve()
    home.mkdir(parents=True, exist_ok=True)
    settings = home / "settings.json"
    try:
        output = settings.open("x", encoding="utf-8")
    except FileExistsError:
        print(f"Keeping existing Bitcraft settings at {settings}")
        return
    with output:
        json.dump({
            "synchronizerApiUrl": "http://127.0.0.1:3000",
            "relayerApiUrl": "http://127.0.0.1:3200",
            "mcpEnabled": False,
        }, output, indent=2)
        output.write("\n")
    print(f"Initialized local Bitcraft settings at {settings}")


if __name__ == "__main__":
    main()
