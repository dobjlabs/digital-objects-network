"""Extract an AppImage and exercise its bundled runtime, tools, and desktop shell."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request


def wait_for(predicate, label, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            print(f"PASS: {label}", flush=True)
            return result
        time.sleep(0.1)
    raise AssertionError(label)


def port_closed():
    try:
        with socket.create_connection(("127.0.0.1", 7727), timeout=0.5):
            return False
    except ConnectionRefusedError:
        return True


def get(url):
    with urllib.request.urlopen(url, timeout=2) as response:
        return response.read()


def run_tool(command, directory, environment):
    result = subprocess.run(command, cwd=directory, env=environment,
                            capture_output=True, text=True, timeout=15)
    assert result.returncode == 0, f"{command}:\n{result.stdout}{result.stderr}"
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("appimage", type=Path)
    appimage = parser.parse_args().appimage.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="bitcraft-bundle-smoke-") as temporary:
        directory = Path(temporary)
        subprocess.run([str(appimage), "--appimage-extract"], cwd=directory,
                       check=True, stdout=subprocess.DEVNULL, timeout=120)
        appdir = directory / "squashfs-root"
        binaries = appdir / "usr/bin"
        resources = appdir / "usr/lib/Bitcraft/runtime"
        release = json.loads((resources / "release.json").read_text())
        tools = {"bitcraft-dobjd", "bitcraft-dobj", "bitcraft-dobj-inner",
                 "bitcraft-dobj-mcp-proxy", "bitcraft-dobj-mcp-proxy-inner", "bitcraft-pexe"}
        assert set(release["binaries"]) == tools
        assert hashlib.sha256((resources / "bitcraft.pexe").read_bytes()).hexdigest() == release["pluginSha256"]
        assert os.access(binaries / "bitcraft-desktop", os.X_OK)
        environment = {**os.environ, "APPDIR": str(appdir), "WEBKIT_DISABLE_COMPOSITING_MODE": "1"}
        for tool in sorted(tools):
            executable = binaries / tool
            assert os.access(executable, os.X_OK), tool
            assert run_tool([str(executable), "--version"], directory, environment).strip(), tool
        print("PASS: packaged plugin matches its manifest and all six bundled tools execute", flush=True)

        assert port_closed(), "Bitcraft port 7727 is already in use"
        home = directory / "data"
        home.mkdir()
        settings = json.loads((resources / "settings.json").read_text())
        settings.update(synchronizerApiUrl="http://127.0.0.1:1", relayerApiUrl="http://127.0.0.1:1", mcpEnabled=False)
        (home / "settings.json").write_text(json.dumps(settings))
        log_path = directory / "host.log"
        with log_path.open("w") as log:
            host = subprocess.Popen([str(appdir / "AppRun"), "--no-tray", "--home", str(home)],
                                    cwd=directory, env=environment, stdout=log, stderr=log)
        try:
            def ready():
                assert host.poll() is None, "Packaged host exited before readiness"
                try:
                    connection = json.loads((home / "connection.json").read_text())
                    health = json.loads(get(connection["httpUrl"] + "/healthz"))
                    if health.get("ok") and health.get("instance_id") == connection["instanceId"]:
                        return connection
                except (OSError, ValueError):
                    pass
                return None

            connection = wait_for(ready, "packaged host starts its bundled daemon", timeout=330)
            assert Path(connection["stateRoot"]) == home
            assert (home / "actions/bitcraft.pexe").read_bytes() == (resources / "bitcraft.pexe").read_bytes()
            assert json.loads((home / "settings.json").read_text()) == settings
            base = connection["httpUrl"] + "/bitcraft/"
            html = get(base).decode()
            assert '<base href="/bitcraft/">' in html
            assert 'name="dobjd-api-url" content="/"' in html
            assert json.loads(get(base + "class-icons.json"))
            for command in ["objects", "actions"]:
                output = run_tool([str(binaries / "bitcraft-dobj"), "--home", str(home), command],
                                  directory, environment)
                if command == "actions":
                    assert "bitcraft::" in output
            print("PASS: packaged UI and wrapper use the managed daemon and stable data root", flush=True)
        except Exception:
            for path in [log_path, home / "dobjd.log"]:
                if path.exists():
                    print(f"{path.name}:\n{path.read_text()}", flush=True)
            raise
        finally:
            if host.poll() is None:
                host.terminate()
                try:
                    host.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    host.kill()
                    host.wait(timeout=5)
            wait_for(port_closed, "packaged daemon stops when the host exits")

        subprocess.run([sys.executable, str(Path(__file__).with_name("smoke_linux.py").resolve()),
                        "--binary", str(appdir / "AppRun")], cwd=directory,
                       env=environment, check=True, timeout=120)


if __name__ == "__main__":
    main()
