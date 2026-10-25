#!/usr/bin/env python3
"""Contract, archive containment, and corruption-detection regressions."""
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile

import build
import smoke


class PackagingTests(unittest.TestCase):
    def test_marker_inventory_and_detecting_corruption(self):
        with tempfile.TemporaryDirectory() as tmp:
            stage = Path(tmp)
            binary = stage / "jvid-app.exe"
            binary.write_bytes(b"payload")
            build.marker(stage, "windows-installer", False)
            build.inventory(stage, "windows-x86_64")
            smoke.verify_inventory(stage)
            build.marker(stage, "windows-portable", True)
            build.inventory(stage, "windows-x86_64")
            smoke.verify_inventory(stage)
            binary.write_bytes(b"damaged")
            with self.assertRaisesRegex(RuntimeError, "mismatch"):
                smoke.verify_inventory(stage)

    def test_archives_cannot_escape_extraction_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = root / "bad.zip"
            with zipfile.ZipFile(archive, "w") as z:
                z.writestr("../escape", b"bad")
            with self.assertRaisesRegex(RuntimeError, "escapes"):
                smoke.extract(archive, root / "output")
            archive = root / "bad.tar.gz"
            with tarfile.open(archive, "w:gz") as t:
                member = tarfile.TarInfo("escape")
                member.type = tarfile.SYMTYPE
                member.linkname = "../../outside"
                t.addfile(member)
            with self.assertRaisesRegex(RuntimeError, "unsafe"):
                smoke.extract(archive, root / "output")

    def test_versioned_zip_preserves_explicit_payload_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            tree = root / f"jvid-{build.version()}-windows-x86_64"
            tree.mkdir()
            (tree / "jvid-app.exe").write_bytes(b"payload")
            build.marker(tree, "windows-portable", True)
            archive = root / "portable.zip"
            build.zip_tree(tree, archive)
            smoke.extract(archive, root / "output")
            self.assertEqual((root / "output" / tree.name / "jvid-distribution").read_text(), "windows-portable\n")
            self.assertFalse((root / "output" / tree.name / "jvid-data").exists())


if __name__ == "__main__":
    unittest.main()
