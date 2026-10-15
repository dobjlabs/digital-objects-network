"""Verify a Bitcraft installer and stage it for a GitHub release."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


APP = Path(__file__).resolve().parents[1]
TOOLS = {"bitcraft-dobjd", "bitcraft-dobj", "bitcraft-dobj-inner",
         "bitcraft-dobj-mcp-proxy", "bitcraft-dobj-mcp-proxy-inner", "bitcraft-pexe"}
STAMPED_TOOLS = {"bitcraft-dobjd", "bitcraft-dobj", "bitcraft-dobj-inner", "bitcraft-pexe"}


def verify_payload(binaries, resources, args):
    release = json.loads((resources / "release.json").read_text())
    for field, expected in [("version", args.version), ("target", args.target), ("sourceCommit", args.commit)]:
        if release.get(field) != expected:
            raise ValueError(f"Packaged {field} is {release.get(field)!r}, expected {expected!r}")
    if set(release["binaries"]) != TOOLS:
        raise ValueError("Packaged release manifest does not list the complete Bitcraft tool set")
    if hashlib.sha256((resources / "bitcraft.pexe").read_bytes()).hexdigest() != release["pluginSha256"]:
        raise ValueError("Packaged Bitcraft plugin does not match its release manifest")
    json.loads((resources / "settings.json").read_text())
    suffix = ".exe" if "windows" in args.target else ""
    for name in sorted(TOOLS | {"bitcraft-desktop"}):
        executable = binaries / f"{name}{suffix}"
        result = subprocess.run([str(executable), "--version"], capture_output=True,
                                text=True, timeout=30)
        if result.returncode or not result.stdout.strip():
            raise ValueError(f"{name} could not execute:\n{result.stdout}{result.stderr}")
        if name in STAMPED_TOOLS and f"v{args.version}" not in result.stdout:
            raise ValueError(f"{name} reports the wrong release: {result.stdout.strip()}")
        if args.signed:
            subprocess.run(["codesign", "--verify", "--strict", str(executable)], check=True)
    print("PASS: installed payload matches the source commit, version, target, and plugin; all tools execute", flush=True)


def verify_dmg(installer, args):
    if args.signed:
        subprocess.run(["codesign", "--verify", "--strict", str(installer)], check=True)
        result = subprocess.run(["xcrun", "notarytool", "submit", str(installer),
                                 "--key", os.environ["APPLE_API_KEY_PATH"],
                                 "--key-id", os.environ["APPLE_API_KEY"],
                                 "--issuer", os.environ["APPLE_API_ISSUER"],
                                 "--wait", "--timeout", "30m", "--output-format", "json"],
                                capture_output=True, text=True)
        if result.returncode:
            raise ValueError(f"DMG notarization command failed:\n{result.stdout}{result.stderr}")
        submission = json.loads(result.stdout)
        if submission.get("status") != "Accepted":
            raise ValueError(f"DMG notarization failed: {submission}")
        subprocess.run(["xcrun", "stapler", "staple", str(installer)], check=True)
        subprocess.run(["xcrun", "stapler", "validate", str(installer)], check=True)
    with tempfile.TemporaryDirectory(prefix="bitcraft-dmg-") as temporary:
        mount = Path(temporary) / "mount"
        mount.mkdir()
        subprocess.run(["hdiutil", "attach", "-nobrowse", "-readonly",
                        "-mountpoint", str(mount), str(installer)], check=True)
        try:
            app = mount / "Bitcraft.app"
            verify_payload(app / "Contents/MacOS", app / "Contents/Resources/runtime", args)
            if args.signed:
                subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
                subprocess.run(["xcrun", "stapler", "validate", str(app)], check=True)
        finally:
            subprocess.run(["hdiutil", "detach", str(mount)], check=True)


def verify_nsis(installer, args):
    with tempfile.TemporaryDirectory(prefix="bitcraft-nsis-") as temporary:
        destination = Path(temporary) / "install"
        subprocess.run(f'"{installer}" /S /D={destination}', check=True, timeout=180)
        verify_payload(destination, destination / "runtime", args)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--bundle-dir", required=True, type=Path)
    parser.add_argument("--output", type=Path, default=APP / "src-tauri/target/release-artifacts")
    parser.add_argument("--signed", action="store_true", help="Require macOS signing and notarization")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-(?:rc|alpha|beta)\.[0-9]+)?", args.version):
        parser.error("Invalid release version")
    if not re.fullmatch(r"[a-zA-Z0-9_-]+", args.target):
        parser.error("Invalid target triple")
    if "apple-darwin" in args.target:
        pattern = "dmg/*.dmg"
    elif "windows" in args.target:
        pattern = "nsis/*.exe"
    elif "linux" in args.target:
        pattern = "appimage/*.AppImage"
    else:
        parser.error("Unsupported installer target")
    if args.signed and "apple-darwin" not in args.target:
        parser.error("--signed requires a macOS target")
    installers = list(args.bundle_dir.resolve().glob(pattern))
    if len(installers) != 1:
        parser.error(f"Expected one installer matching {pattern}, found {len(installers)}")
    installer = installers[0]
    if installer.suffix == ".dmg":
        verify_dmg(installer, args)
    elif installer.suffix == ".exe":
        verify_nsis(installer, args)
    else:
        subprocess.run([sys.executable, str(APP / "scripts/smoke_bundle_linux.py"), str(installer),
                        "--version", args.version, "--target", args.target, "--commit", args.commit], check=True)
    args.output.mkdir(parents=True, exist_ok=True)
    destination = args.output / f"bitcraft-{args.version}-{args.target}{installer.suffix}"
    shutil.copy2(installer, destination)
    print(f"Staged {destination}", flush=True)


if __name__ == "__main__":
    main()
