from contextlib import ExitStack
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "bitcraft_prepare", Path(__file__).resolve().parents[1] / "scripts" / "prepare.py"
)
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class BundledUiTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.app = Path(self.temporary.name)
        (self.app / "ui").mkdir()
        self.page = b'<html><section id="startup-screen">Current UI</section></html>'
        (self.app / "ui" / "index.html").write_bytes(self.page)
        self.binary = self.app / "dobjd"
        self.binary.write_bytes(b"binary with the previous checkout's UI")
        self.target = "aarch64-apple-darwin"
        self.app_patch = patch.object(prepare, "APP", self.app)
        self.app_patch.start()
        self.addCleanup(self.app_patch.stop)
        self.marker = prepare.bitcraft_ui_marker()

    def test_current_ui_requires_no_rebuild(self):
        self.binary.write_bytes(b"executable prefix" + self.marker + b"executable suffix")
        with patch.object(prepare, "run") as run:
            prepare.ensure_current_bitcraft_ui(self.binary, self.target)
        run.assert_not_called()

    def test_stale_ui_rebuilds_daemon_before_accepting_binary(self):
        def rebuild(*command):
            if command[:2] == ("cargo", "build"):
                self.binary.write_bytes(b"rebuilt executable" + self.page + self.marker)

        with patch.object(prepare, "run", side_effect=rebuild) as run:
            prepare.ensure_current_bitcraft_ui(self.binary, self.target)
        self.assertEqual(run.call_args_list[0].args,
                         ("cargo", "clean", "--release", "--target", self.target, "-p", "dobjd"))
        self.assertEqual(run.call_args_list[1].args[:2], ("cargo", "build"))
        self.assertIn(self.page, self.binary.read_bytes())

    def test_stale_ui_after_rebuild_prevents_staging(self):
        with patch.object(prepare, "run"):
            with self.assertRaisesRegex(RuntimeError, "outdated daemon will not be staged"):
                prepare.ensure_current_bitcraft_ui(self.binary, self.target)

    def test_native_dev_rebuild_uses_native_cache(self):
        def rebuild(*command):
            if command[:2] == ("cargo", "build"):
                self.binary.write_bytes(self.marker)

        with patch.object(prepare, "run", side_effect=rebuild) as run:
            prepare.ensure_current_bitcraft_ui(self.binary, self.target, dev=True)
        self.assertEqual(run.call_args_list[0].args,
                         ("cargo", "clean", "--release", "-p", "dobjd"))
        self.assertNotIn("--target", run.call_args_list[1].args)
        self.assertNotIn("-p", run.call_args_list[1].args)

    def test_build_failure_is_not_accepted(self):
        with patch.object(prepare, "run", side_effect=subprocess.CalledProcessError(1, "cargo")):
            with self.assertRaises(subprocess.CalledProcessError):
                prepare.ensure_current_bitcraft_ui(self.binary, self.target)

    def test_matching_index_without_bundle_marker_is_rejected(self):
        self.binary.write_bytes(self.page)
        with patch.object(prepare, "run") as run:
            with self.assertRaisesRegex(RuntimeError, "outdated daemon will not be staged"):
                prepare.ensure_current_bitcraft_ui(self.binary, self.target)
        self.assertEqual(run.call_count, 2)

    def test_supporting_asset_changes_rebuild_without_changing_index(self):
        asset = self.app / "ui" / "inventory-values.json"
        renamed = self.app / "ui" / "renamed-values.json"
        changes = [
            ("added", lambda: asset.write_bytes(b'{"Wood":1}')),
            ("changed", lambda: asset.write_bytes(b'{"Wood":2}')),
            ("renamed", lambda: asset.rename(renamed)),
            ("removed", lambda: renamed.unlink()),
        ]
        for name, change in changes:
            with self.subTest(name=name):
                self.binary.write_bytes(self.page + prepare.bitcraft_ui_marker())
                change()

                def rebuild(*command):
                    if command[:2] == ("cargo", "build"):
                        self.binary.write_bytes(self.page + prepare.bitcraft_ui_marker())

                with patch.object(prepare, "run", side_effect=rebuild) as run:
                    prepare.ensure_current_bitcraft_ui(self.binary, self.target)
                self.assertEqual(run.call_count, 2)
                self.assertEqual((self.app / "ui" / "index.html").read_bytes(), self.page)

    def test_nested_assets_are_included_and_contributor_docs_are_excluded(self):
        nested = self.app / "ui" / "presentation" / "bitcraft"
        nested.mkdir(parents=True)
        for name in ["AGENTS.md", "CLAUDE.md", "BITCRAFT_SPECIFIC_RULES.md"]:
            (nested / name).write_text("Contributor instructions")
        self.assertEqual(prepare.bitcraft_ui_marker(), self.marker)
        (nested / "pack.json").write_text('{"plugin":"bitcraft"}')
        self.assertNotEqual(prepare.bitcraft_ui_marker(), self.marker)

    def test_fingerprint_ignores_timestamps_and_tracks_runtime_markdown(self):
        index = self.app / "ui" / "index.html"
        index.touch()
        self.assertEqual(prepare.bitcraft_ui_marker(), self.marker)
        (self.app / "ui" / "guide.md").write_text("Player instructions")
        self.assertNotEqual(prepare.bitcraft_ui_marker(), self.marker)


class PreparationTests(unittest.TestCase):
    def prepare_runtime(self, *, dev, target="aarch64-apple-darwin"):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        app = root / "app"
        tauri = app / "src-tauri"
        runtime_cache = root / "runtime-cache"
        desktop_cache = root / "desktop-cache"
        page = b"<html>Current UI</html>"
        (app / "ui").mkdir(parents=True)
        (app / "ui" / "index.html").write_bytes(page)
        policy = root / "services" / "dobjd" / "ui-excluded-files.txt"
        policy.parent.mkdir(parents=True)
        policy.write_bytes((prepare.REPO / "services" / "dobjd" / policy.name).read_bytes())
        (app / "skills" / "bitcraft-play").mkdir(parents=True)
        (app / "skills" / "bitcraft-play" / "SKILL.md").write_text("Play Bitcraft")
        tauri.mkdir()
        commands = []

        def output(command, **kwargs):
            if command == ["rustc", "-vV"]:
                return "host: aarch64-apple-darwin\n"
            if command[:2] == ["git", "rev-parse"]:
                return "test-commit\n"
            self.assertEqual(command[:2], ["cargo", "metadata"])
            return json.dumps({"target_directory": str(
                desktop_cache if "--manifest-path" in command else runtime_cache)})

        def run(*command):
            commands.append(command)
            if command[:2] == ("cargo", "build"):
                launcher = "--manifest-path" in command
                directory = desktop_cache if launcher else runtime_cache
                if "--target" in command:
                    directory /= command[command.index("--target") + 1]
                directory /= "release" if "--release" in command else "debug"
                directory.mkdir(parents=True, exist_ok=True)
                names = ["bitcraft-dobj", "bitcraft-dobj-mcp-proxy"] if launcher else [
                    "dobjd", "dobj", "dobj-mcp-proxy", "pexe"]
                for name in names:
                    (directory / name).write_bytes(
                        str(directory / name).encode() + page + prepare.bitcraft_ui_marker())
            elif "--out-dir" in command:
                destination = Path(command[command.index("--out-dir") + 1])
                destination.mkdir(parents=True)
                (destination / "bitcraft.pexe").write_bytes(b"checked plugin")

        arguments = ["prepare.py", "--target", target]
        if dev:
            arguments.append("--dev")
        with ExitStack() as stack:
            for name, value in [("APP", app), ("TAURI", tauri), ("REPO", root)]:
                stack.enter_context(patch.object(prepare, name, value))
            stack.enter_context(patch.object(prepare.sys, "argv", arguments))
            stack.enter_context(patch.object(prepare.subprocess, "check_output", side_effect=output))
            stack.enter_context(patch.object(prepare, "run", side_effect=run))
            stack.enter_context(patch.dict(prepare.os.environ, {
                "BITCRAFT_UPDATE_ENDPOINT": "", "BITCRAFT_UPDATE_PUBLIC_KEY": ""}))
            prepare.main()

        release = json.loads((tauri / "runtime" / "release.json").read_text())
        self.assertEqual(release["target"], target)
        for name, digest in release["binaries"].items():
            staged = tauri / "binaries" / f"{name}-{target}"
            self.assertEqual(hashlib.sha256(staged.read_bytes()).hexdigest(), digest)
            cache = desktop_cache if name in ["bitcraft-dobj", "bitcraft-dobj-mcp-proxy"] else runtime_cache
            profile = "debug" if dev and cache == desktop_cache else "release"
            expected = cache / profile if dev else cache / target / profile
            self.assertIn(str(expected).encode(), staged.read_bytes())
        self.assertEqual((tauri / "runtime" / "skills" / "bitcraft-play" / "SKILL.md").read_text(),
                         "Play Bitcraft")
        return commands, tauri

    def test_dev_reuses_workspace_native_artifacts_and_stages_launchers(self):
        commands, tauri = self.prepare_runtime(dev=True)
        builds = [command for command in commands if command[:2] == ("cargo", "build")]
        for command in builds:
            self.assertNotIn("--target", command)
            self.assertNotIn("-p", command)
        self.assertIn("--release", builds[0])
        self.assertNotIn("--release", builds[1])
        self.assertIn("--workspace", builds[1])
        self.assertTrue((tauri / "binaries" / "bitcraft-dobj-inner").is_file())

    def test_packaging_keeps_target_specific_release_artifacts(self):
        commands, _ = self.prepare_runtime(dev=False)
        for command in commands:
            if command[:2] == ("cargo", "build"):
                self.assertIn("--target", command)
                self.assertIn("--release", command)
                self.assertIn("-p", command)

    def test_cross_packaging_builds_plugin_on_host(self):
        commands, tauri = self.prepare_runtime(dev=False, target="x86_64-unknown-linux-gnu")
        plugin_build = next(command for command in commands if "--out-dir" in command)
        self.assertEqual(plugin_build[:7],
                         ("cargo", "run", "--release", "--locked", "-p", "pexe", "--"))
        self.assertFalse((tauri / "binaries" / "bitcraft-dobj-inner").exists())

    def test_dev_rejects_cross_compilation_before_building(self):
        with patch.object(prepare.sys, "argv", ["prepare.py", "--dev", "--target", "other-target"]), \
                patch.object(prepare.subprocess, "check_output", return_value="host: native-target\n"), \
                patch.object(prepare, "run") as run:
            with self.assertRaises(SystemExit) as error:
                prepare.main()
        self.assertEqual(error.exception.code, 2)
        run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
