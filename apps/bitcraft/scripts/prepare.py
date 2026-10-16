#!/usr/bin/env python3
"""Build the matched Bitcraft runtime and generate Tauri's bundle configuration."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess


APP = Path(__file__).resolve().parents[1]
REPO = APP.parents[1]
TAURI = APP / "src-tauri"


def run(*command, cwd=REPO):
    subprocess.run(command, cwd=cwd, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", help="Rust target triple; defaults to the native host")
    parser.add_argument("--version", default="0.1.0", help="Bitcraft product version")
    args = parser.parse_args()
    endpoint = os.environ.get("BITCRAFT_UPDATE_ENDPOINT")
    public_key = os.environ.get("BITCRAFT_UPDATE_PUBLIC_KEY")
    if bool(endpoint) != bool(public_key):
        parser.error("Set both BITCRAFT_UPDATE_ENDPOINT and BITCRAFT_UPDATE_PUBLIC_KEY")
    if endpoint and not endpoint.startswith("https://"):
        parser.error("BITCRAFT_UPDATE_ENDPOINT must use HTTPS")
    host = next(
        line.removeprefix("host: ")
        for line in subprocess.check_output(["rustc", "-vV"], text=True).splitlines()
        if line.startswith("host: ")
    )
    target = args.target or host
    suffix = ".exe" if "windows" in target else ""
    run("cargo", "build", "--release", "--locked", "--target", target,
        "-p", "dobjd", "-p", "cli", "-p", "dobj-mcp", "-p", "pexe",
        "--features", "dobjd/bundled-bitcraft-ui,dobj-mcp/proxy",
        "--bin", "dobjd", "--bin", "dobj", "--bin", "dobj-mcp-proxy", "--bin", "pexe")
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=REPO, text=True))
    built = Path(metadata["target_directory"]) / target / "release"
    pexe = [str(built / f"pexe{suffix}")] if target == host else [
        "cargo", "run", "--release", "--locked", "-p", "pexe", "--"]
    run(*pexe, "build", "--check", "--out-dir", str(TAURI / "runtime"), "examples/bitcraft")
    run("cargo", "build", "--manifest-path", str(TAURI / "Cargo.toml"),
        "--release", "--locked", "--target", target, "-p", "bitcraft-launchers", "--bins")

    binaries = TAURI / "binaries"
    binaries.mkdir(exist_ok=True)
    tools = ["dobjd", "dobj", "dobj-mcp-proxy", "pexe"]
    hashes = {}
    for tool in tools:
        source = built / f"{tool}{suffix}"
        packaged_name = f"bitcraft-{tool}"
        if tool in ["dobj", "dobj-mcp-proxy"]:
            packaged_name += "-inner"
        destination = binaries / f"{packaged_name}-{target}{suffix}"
        shutil.copy2(source, destination)
        hashes[packaged_name] = hashlib.sha256(destination.read_bytes()).hexdigest()

    launcher_metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--manifest-path", str(TAURI / "Cargo.toml"),
         "--format-version", "1", "--no-deps"], cwd=REPO, text=True))
    launcher_build = Path(launcher_metadata["target_directory"]) / target / "release"
    for name in ["bitcraft-dobj", "bitcraft-dobj-mcp-proxy"]:
        destination = binaries / f"{name}-{target}{suffix}"
        shutil.copy2(launcher_build / f"{name}{suffix}", destination)
        hashes[name] = hashlib.sha256(destination.read_bytes()).hexdigest()

    resources = TAURI / "runtime"
    plugin_hash = hashlib.sha256((resources / "bitcraft.pexe").read_bytes()).hexdigest()
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip()
    (resources / "release.json").write_text(json.dumps({
        "version": args.version,
        "target": target,
        "sourceCommit": commit,
        "pluginSha256": plugin_hash,
        "binaries": hashes,
    }, indent=2) + "\n")
    (resources / "settings.json").write_text(json.dumps({
        "synchronizerApiUrl": "https://terminus.digiobjects.net:4000",
        "relayerApiUrl": "https://terminus.digiobjects.net:4200",
        "mcpEnabled": False,
    }, indent=2) + "\n")
    config = {
        "version": args.version,
        "bundle": {
            "externalBin": [f"binaries/{name}" for name in hashes],
            "resources": {"runtime/": "runtime/"},
        },
    }
    if endpoint:
        config["bundle"]["createUpdaterArtifacts"] = True
        config["plugins"] = {"updater": {"endpoints": [endpoint], "pubkey": public_key}}
    (TAURI / "tauri.bundle.conf.json").write_text(json.dumps(config, indent=2) + "\n")
    print(f"Prepared Bitcraft {args.version} ({target})")


if __name__ == "__main__":
    main()
