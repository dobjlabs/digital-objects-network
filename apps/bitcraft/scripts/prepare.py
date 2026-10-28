#!/usr/bin/env python3
"""Build the matched Bitcraft runtime and generate Tauri's bundle configuration."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


APP = Path(__file__).resolve().parents[1]
REPO = APP.parents[1]
TAURI = APP / "src-tauri"


def run(*command, cwd=REPO):
    subprocess.run(command, cwd=cwd, check=True)


def build_runtime(target, dev=False):
    selection = [] if dev else [
        "--target", target, "-p", "dobjd", "-p", "cli", "-p", "dobj-mcp", "-p", "pexe"]
    run("cargo", "build", "--release", "--locked", *selection,
        "--features", "dobjd/bundled-bitcraft-ui,dobj-mcp/proxy",
        "--bin", "dobjd", "--bin", "dobj", "--bin", "dobj-mcp-proxy", "--bin", "pexe")


def bitcraft_ui_marker():
    root = APP / "ui"
    excluded = set((REPO / "services" / "dobjd" / "ui-excluded-files.txt").read_text().splitlines())
    assets = []
    for path in root.rglob("*"):
        if path.is_symlink():
            raise RuntimeError(f"UI build must not contain symlinks: {path}")
        if path.is_file() and path.name not in excluded:
            assets.append((path.relative_to(root).as_posix(), path))
    fingerprint = hashlib.sha256(b"dobjd-ui-v1\0")
    for name, path in sorted(assets):
        encoded = name.encode("utf-8")
        fingerprint.update(len(encoded).to_bytes(8, "little"))
        fingerprint.update(encoded)
        fingerprint.update(hashlib.sha256(path.read_bytes()).digest())
    return f"dobjd-BITCRAFT_ASSETS-sha256-v1:{fingerprint.hexdigest()}".encode("ascii")


def ensure_current_bitcraft_ui(binary, target, dev=False):
    marker = bitcraft_ui_marker()
    if marker in binary.read_bytes():
        return
    print("The daemon contains an outdated Bitcraft UI; rebuilding dobjd.", flush=True)
    selection = [] if dev else ["--target", target]
    run("cargo", "clean", "--release", *selection, "-p", "dobjd")
    build_runtime(target, dev=dev)
    if marker not in binary.read_bytes():
        raise RuntimeError(
            f"{binary} still contains a different Bitcraft UI after rebuilding. "
            "Check Cargo's target directory and build-cache configuration. "
            "The outdated daemon will not be staged."
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dev", action="store_true",
                        help="Reuse native workspace builds for local development")
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
    if args.dev and target != host:
        parser.error("--dev requires the native host target; omit it for cross-compilation")
    suffix = ".exe" if "windows" in target else ""
    # The daemon embeds the UI folder, so its presentation packs go in first.
    run(sys.executable, str(APP / "scripts" / "sync_presentation.py"))
    build_runtime(target, dev=args.dev)
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=REPO, text=True))
    built = Path(metadata["target_directory"])
    if not args.dev:
        built /= target
    built /= "release"
    ensure_current_bitcraft_ui(built / f"dobjd{suffix}", target, dev=args.dev)
    pexe = [str(built / f"pexe{suffix}")] if target == host else [
        "cargo", "run", "--release", "--locked", "-p", "pexe", "--"]
    run(*pexe, "build", "--check", "--out-dir", str(TAURI / "runtime"), "examples/bitcraft")
    launcher_selection = ["--workspace", "--bin", "bitcraft-dobj", "--bin", "bitcraft-dobj-mcp-proxy"] if args.dev else [
        "--release", "--target", target, "-p", "bitcraft-launchers", "--bins"]
    run("cargo", "build", "--manifest-path", str(TAURI / "Cargo.toml"),
        "--locked", *launcher_selection)

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
        if tool in ["dobj", "dobj-mcp-proxy"] and target == host:
            # Development prompts use the staged launcher before bundle renaming.
            shutil.copy2(source, binaries / f"{packaged_name}{suffix}")
        hashes[packaged_name] = hashlib.sha256(destination.read_bytes()).hexdigest()

    launcher_metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--manifest-path", str(TAURI / "Cargo.toml"),
         "--format-version", "1", "--no-deps"], cwd=REPO, text=True))
    launcher_build = Path(launcher_metadata["target_directory"])
    launcher_build = launcher_build / "debug" if args.dev else launcher_build / target / "release"
    for name in ["bitcraft-dobj", "bitcraft-dobj-mcp-proxy"]:
        destination = binaries / f"{name}-{target}{suffix}"
        shutil.copy2(launcher_build / f"{name}{suffix}", destination)
        hashes[name] = hashlib.sha256(destination.read_bytes()).hexdigest()

    resources = TAURI / "runtime"
    shutil.copytree(APP / "skills", resources / "skills", dirs_exist_ok=True)
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
