#!/usr/bin/env python3
"""Contract, archive containment, and corruption-detection regressions."""
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
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

    def test_macos_preserves_leptonica_nonstandard_license_and_provenance(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            keg = root / "Cellar/leptonica/1.87.0"
            (keg / "lib").mkdir(parents=True)
            (keg / "INSTALL_RECEIPT.json").write_text('{}')
            (keg / "leptonica-license.txt").write_text('Copyright and distribution terms')
            source = keg / "lib/liblept.dylib"
            source.write_bytes(b'dylib')
            stage = root / "package"
            stage.mkdir()
            providers = build.native_licenses(stage, [source], "macos-aarch64", None)
            self.assertEqual(providers, [str(keg.resolve())])
            copied = stage / "runtime-licenses/leptonica"
            self.assertTrue((copied / "INSTALL_RECEIPT.json").is_file())
            self.assertIn('Copyright and distribution terms', '\n'.join(p.read_text() for p in copied.iterdir()))
            # Actual Homebrew bottle omits this file: use the reviewed version-pinned
            # upstream text and retain its source/checksum, never silently omit it.
            import shutil
            shutil.rmtree(stage / "runtime-licenses")
            (keg / "leptonica-license.txt").unlink()
            build.native_licenses(stage, [source], "macos-aarch64", None)
            self.assertTrue((copied / "license-source.json").is_file())
            self.assertTrue(list(copied.glob('*-leptonica-1.87.0.txt')))

    def test_macos_recovers_reuse_text_from_verified_source_and_rejects_corruption(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            keg = root / "glib/2.88.3"
            (keg / ".brew").mkdir(parents=True)
            archive = root / "glib.tar.gz"
            with tarfile.open(archive, "w:gz") as out:
                data = b"GNU LGPL source license text"
                member = tarfile.TarInfo("glib/LICENSES/LGPL-2.1-or-later.txt")
                member.size = len(data)
                out.addfile(member, io.BytesIO(data))
                link = tarfile.TarInfo("glib/COPYING")
                link.type = tarfile.SYMTYPE
                link.linkname = "LICENSES/LGPL-2.1-or-later.txt"
                out.addfile(link)
            (keg / ".brew/glib.rb").write_text('url "https://example.invalid/glib.tar.gz"\nsha256 "' + build.sha256(archive) + '"\n')
            dest = root / "notices"
            dest.mkdir()
            import shutil
            with patch('build.urllib.request.urlretrieve', side_effect=lambda url, path: shutil.copy2(archive, path)):
                build.recover_homebrew_licenses(keg, dest)
                self.assertIn('GNU LGPL source license text', (dest / 'source-1-COPYING').read_text())
                self.assertTrue((dest / 'license-source.json').is_file())
                archive.write_bytes(b'corrupted source download')
                with self.assertRaisesRegex(RuntimeError, 'checksum mismatch'):
                    build.recover_homebrew_licenses(keg, dest)

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
