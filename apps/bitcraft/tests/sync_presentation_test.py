import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "sync_presentation",
    Path(__file__).resolve().parents[1] / "scripts" / "sync_presentation.py",
)
sync = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sync)


class PresentationSyncTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        self.source = root / "source"
        self.destination = root / "ui" / "presentation"
        (self.source / "assets" / "actions").mkdir(parents=True)
        (self.source / "pack.json").write_text(json.dumps({"plugin": "bitcraft"}))
        self.icon = self.source / "assets" / "actions" / "GatherWood.svg"
        self.icon.write_bytes(b"<svg>old</svg>")
        for name, value in [("DESTINATION", self.destination), ("PACKS", [self.source])]:
            setting = patch.object(sync, name, value)
            setting.start()
            self.addCleanup(setting.stop)

    def test_initial_sync_copies_pack_assets_and_index(self):
        sync.main()
        self.assertEqual(json.loads((self.destination / "index.json").read_text()),
                         {"packs": ["bitcraft/pack.json"]})
        for relative in ["pack.json", "assets/actions/GatherWood.svg"]:
            self.assertEqual((self.destination / "bitcraft" / relative).read_bytes(),
                             (self.source / relative).read_bytes())

    def test_unchanged_sync_preserves_all_files_and_directory_metadata(self):
        sync.main()
        paths = [self.destination.parent, self.destination, *self.destination.rglob("*")]
        for path in paths:
            os.utime(path, ns=(1_600_000_000_000_000_000, 1_600_000_000_000_000_000))
        before = {path: path.stat() for path in paths}
        os.utime(self.icon, ns=(1_700_000_000_000_000_000, 1_700_000_000_000_000_000))

        sync.main()

        self.assertEqual(set(paths),
                         {self.destination.parent, self.destination, *self.destination.rglob("*")})
        for path, previous in before.items():
            current = path.stat()
            self.assertEqual((current.st_ino, current.st_mtime_ns, current.st_ctime_ns),
                             (previous.st_ino, previous.st_mtime_ns, previous.st_ctime_ns))

    def test_changed_content_with_same_size_and_timestamp_is_synced(self):
        sync.main()
        previous = self.icon.stat()
        self.icon.write_bytes(b"<svg>new</svg>")
        os.utime(self.icon, ns=(previous.st_atime_ns, previous.st_mtime_ns))

        sync.main()

        self.assertEqual(
            (self.destination / "bitcraft" / "assets" / "actions" / self.icon.name).read_bytes(),
            b"<svg>new</svg>")

    def test_sync_adds_assets_and_removes_stale_files_and_packs(self):
        sync.main()
        self.icon.unlink()
        (self.source / "assets" / "actions" / "GatherStone.svg").write_bytes(b"<svg/>")
        (self.destination / "obsolete").mkdir()
        (self.destination / "obsolete" / "pack.json").write_text("{}")
        (self.destination / ".stale").write_text("stale")

        sync.main()

        self.assertFalse((self.destination / "obsolete").exists())
        self.assertFalse((self.destination / ".stale").exists())
        assets = self.destination / "bitcraft" / "assets" / "actions"
        self.assertEqual({path.name for path in assets.iterdir()}, {"GatherStone.svg"})

    def test_removing_pack_updates_index_and_removes_assets(self):
        sync.main()

        with patch.object(sync, "PACKS", []):
            sync.main()

        self.assertEqual(json.loads((self.destination / "index.json").read_text()),
                         {"packs": []})
        self.assertEqual({path.name for path in self.destination.iterdir()}, {"index.json"})


if __name__ == "__main__":
    unittest.main()
