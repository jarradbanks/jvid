"""Release source completeness and actual multipart/corruption regressions."""
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import sources
from reassemble_sources import verify_parts


class SourceBundleTests(unittest.TestCase):
    revision = {"commit": "a" * 40, "tree": "b" * 40}

    def write_bundle(self, root, output, target, assets=None):
        root.mkdir()
        (root / "source.txt").write_text("source\n" * 300)
        providers = [{"id": "test-provider", "files": ["source.txt"]}]
        if target == "rust":
            providers[0]["offline_metadata_checked"] = True
        return sources.bundle(root, output, target, providers, assets or {}, self.revision, limit=127)

    def test_actual_multipart_reassembly_and_corrupt_or_missing_part_rejection(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            manifest = self.write_bundle(root / "input", root / "parts", "rust")
            data = json.loads(manifest.read_text())
            self.assertGreater(len(data["parts"]), 1)
            rebuilt = io.BytesIO()
            verify_parts(manifest, data, rebuilt)
            with tarfile.open(fileobj=io.BytesIO(rebuilt.getvalue()), mode="r:gz") as archive:
                self.assertEqual(archive.extractfile("source.txt").read(), b"source\n" * 300)
                inner = json.load(archive.extractfile("SOURCE-MANIFEST.json"))
                self.assertEqual(inner["commit"], self.revision["commit"])
            part = manifest.parent / data["parts"][0]["name"]
            original = part.read_bytes()
            part.write_bytes(bytes([original[0] ^ 1]) + original[1:])
            with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
                verify_parts(manifest, data)
            part.unlink()
            with self.assertRaisesRegex(RuntimeError, "missing source archive part"):
                verify_parts(manifest, data)

    def test_no_path_escape_or_missing_source_provider(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "source").mkdir()
            for path in ("../secret", "/secret", "a/../secret", "a\\secret"):
                with self.assertRaisesRegex(RuntimeError, "unsafe source path"):
                    sources.safe_relative(path)
            with self.assertRaisesRegex(RuntimeError, "no retained files"):
                sources.bundle(root / "source", root / "out", "rust", [{"id": "missing"}], {}, self.revision)

    def fixtures(self, temp):
        root = Path(temp)
        native = root / "native"
        native.mkdir()
        for name in sources.expected_assets():
            (native / name).write_bytes(name.encode())
        artifacts = root / "artifacts"
        for target in sources.BUNDLES:
            output = artifacts / f"sources-{target}"
            assets = {} if target == "rust" else sources.native_assets(native, target)
            self.write_bundle(root / target, output, target, assets)
        return native, artifacts, root / "release"

    def test_full_source_contract_checks_binary_bytes_before_collecting(self):
        with tempfile.TemporaryDirectory() as temp, patch("sources.checkout", return_value=self.revision):
            native, artifacts, dest = self.fixtures(temp)
            binary = native / sorted(sources.expected_assets())[0]
            original = binary.read_bytes()
            binary.write_bytes(b"changed after source capture")
            with self.assertRaisesRegex(RuntimeError, "native package bytes"):
                sources.collect_release(artifacts, native, dest)
            self.assertFalse(dest.exists())
            binary.write_bytes(original)
            sources.collect_release(artifacts, native, dest)
            index = json.loads((dest / f"jvid-{sources.version()}-source-manifest.json").read_text())
            self.assertEqual(len(index["bundles"]), 4)
            self.assertEqual(set(index["binary_assets"]), sources.expected_assets())
            self.assertTrue((dest / "reassemble_sources.py").is_file())

    def test_missing_bundle_or_wrong_commit_prevents_release(self):
        with tempfile.TemporaryDirectory() as temp, patch("sources.checkout", return_value=self.revision):
            native, artifacts, dest = self.fixtures(temp)
            rust = next(artifacts.rglob("*-sources-rust.json"))
            metadata = json.loads(rust.read_text())
            metadata["commit"] = "c" * 40
            rust.write_text(json.dumps(metadata))
            with self.assertRaisesRegex(RuntimeError, "mismatched source manifest"):
                sources.collect_release(artifacts, native, dest)
            rust.unlink()
            with self.assertRaisesRegex(RuntimeError, "missing source bundles"):
                sources.collect_release(artifacts, native, dest)
            self.assertFalse(dest.exists())

    def test_manifest_cannot_reference_external_parts(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            manifest = self.write_bundle(root / "input", root / "parts", "rust")
            data = json.loads(manifest.read_text())
            data["parts"][0]["name"] = "../private-key"
            with self.assertRaisesRegex(RuntimeError, "invalid source archive part"):
                verify_parts(manifest, data)


if __name__ == "__main__":
    unittest.main()
