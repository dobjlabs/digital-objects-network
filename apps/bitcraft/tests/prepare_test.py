import importlib.util
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

    def test_current_ui_requires_no_rebuild(self):
        self.binary.write_bytes(b"executable prefix" + self.page + b"executable suffix")
        with patch.object(prepare, "run") as run:
            prepare.ensure_current_bitcraft_ui(self.binary, self.target)
        run.assert_not_called()

    def test_stale_ui_rebuilds_daemon_before_accepting_binary(self):
        def rebuild(*command):
            if command[:2] == ("cargo", "build"):
                self.binary.write_bytes(b"rebuilt executable" + self.page)

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

    def test_build_failure_is_not_accepted(self):
        with patch.object(prepare, "run", side_effect=subprocess.CalledProcessError(1, "cargo")):
            with self.assertRaises(subprocess.CalledProcessError):
                prepare.ensure_current_bitcraft_ui(self.binary, self.target)


if __name__ == "__main__":
    unittest.main()
