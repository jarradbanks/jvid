"""Release source completeness and actual multipart/corruption regressions."""
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import sources
from reassemble_sources import PART_LIMIT, digest, read_manifest, verify_parts


class SourceBundleTests(unittest.TestCase):
    revision = {"commit": "a" * 40, "tree": "b" * 40}

    def setUp(self):
        env = patch.dict(os.environ, {"JVID_UPDATE_PUBLIC_KEY": "", "GGML_NATIVE": "OFF",
                                     "GGML_METAL": "OFF", "RUSTFLAGS": "-C target-feature=+crt-static"})
        env.start()
        self.addCleanup(env.stop)

    def write_bundle(self, root, output, target, assets=None):
        root.mkdir()
        (root / "source.txt").write_bytes(b"source\n" * 300)
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
            self.assertTrue(all(0 < p["size"] <= 127 for p in data["parts"]))
            rebuilt = io.BytesIO()
            verify_parts(manifest, data, rebuilt)
            with tarfile.open(fileobj=io.BytesIO(rebuilt.getvalue()), mode="r:gz") as archive:
                self.assertEqual(archive.extractfile("source.txt").read(), b"source\n" * 300)
                inner = json.load(archive.extractfile("SOURCE-MANIFEST.json"))
                self.assertEqual(inner["commit"], self.revision["commit"])
                self.assertNotIn("SOURCE-MANIFEST.json", inner["files"])
            sources.verify_archive(manifest, data)
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
            for path in ("../secret", "/secret", "a/../secret", "a\\secret", "a//secret",
                         "C:secret", "a/", "nul\0file"):
                with self.assertRaisesRegex(RuntimeError, "unsafe source path"):
                    sources.safe_relative(path)
            with self.assertRaisesRegex(RuntimeError, "no retained files"):
                sources.bundle(root / "source", root / "out", "rust", [{"id": "missing"}], {}, self.revision)
            with self.assertRaisesRegex(RuntimeError, "outside the source tree"):
                sources.bundle(root / "source", root / "source/nested", "rust",
                               [{"id": "test", "files": ["BUILD-CONFIG.json"]}], {}, self.revision)

    def rewrite_archive(self, manifest, change):
        """Recompute outer checksums after tampering, so they alone cannot catch it."""
        data = read_manifest(manifest)
        original = io.BytesIO()
        verify_parts(manifest, data, original)
        original.seek(0)
        with tarfile.open(fileobj=original, mode="r:gz") as archive:
            members = [(m, archive.extractfile(m).read() if m.isfile() else None) for m in archive]
        change(members, data)
        rebuilt = io.BytesIO()
        with tarfile.open(fileobj=rebuilt, mode="w:gz") as archive:
            for member, payload in members:
                if payload is not None:
                    member.size = len(payload)
                archive.addfile(member, io.BytesIO(payload) if payload is not None else None)
        for part in data["parts"]:
            (manifest.parent / part["name"]).unlink()
        writer = sources.PartWriter(manifest.parent, manifest.stem, limit=127)
        writer.write(rebuilt.getvalue())
        writer.finish_part()
        data.update(parts=writer.parts, archive_size=writer.total,
                    archive_sha256=writer.archive_hash.hexdigest())
        manifest.write_text(json.dumps(data))
        return data

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

    def test_protected_verify_only_rechecks_staged_bytes_index_and_helper(self):
        with tempfile.TemporaryDirectory() as temp, patch("sources.checkout", return_value=self.revision):
            native, artifacts, dest = self.fixtures(temp)
            sources.collect_release(artifacts, native, dest)
            before = {p.name: digest(p) for p in dest.iterdir()}
            sources.collect_release(dest, native, dest, verify_only=True)
            self.assertEqual(before, {p.name: digest(p) for p in dest.iterdir()})
            part = next(dest.glob("*.part001"))
            original = part.read_bytes()
            part.write_bytes(bytes([original[0] ^ 1]) + original[1:])
            with self.assertRaises(RuntimeError):
                sources.collect_release(dest, native, dest, verify_only=True)
            # Passing pristine original artifacts must not conceal a corrupt staged copy.
            with self.assertRaisesRegex(RuntimeError, "staged source asset mismatch"):
                sources.collect_release(artifacts, native, dest, verify_only=True)
            part.write_bytes(original)
            index = dest / f"jvid-{sources.version()}-source-manifest.json"
            original_index = index.read_bytes()
            data = json.loads(original_index)
            data["bundles"].pop()
            index.write_text(json.dumps(data))
            with self.assertRaisesRegex(RuntimeError, "index mismatch"):
                sources.collect_release(dest, native, dest, verify_only=True)
            index.write_bytes(original_index)
            (dest / "reassemble_sources.py").write_text("modified helper")
            with self.assertRaisesRegex(RuntimeError, "helper mismatch"):
                sources.collect_release(dest, native, dest, verify_only=True)

    def test_wrong_version_duplicate_manifest_and_missing_provider_block_release(self):
        for case in ("version", "duplicate", "provider", "retained-file", "key"):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as temp, patch("sources.checkout", return_value=self.revision):
                native, artifacts, dest = self.fixtures(temp)
                manifest = next(artifacts.rglob("*-sources-rust.json"))
                data = read_manifest(manifest)
                if case == "version":
                    data["version"] = "99.0.0"
                elif case == "duplicate":
                    other = artifacts / "duplicate"
                    other.mkdir()
                    shutil.copy2(manifest, other / manifest.name)
                elif case == "provider":
                    data["providers"] = []
                elif case == "retained-file":
                    data["providers"][0]["files"] = ["missing-provider-source.tar"]
                else:
                    data["build_configuration"]["JVID_UPDATE_PUBLIC_KEY"] = "c" * 64
                manifest.write_text(json.dumps(data))
                with self.assertRaises(RuntimeError):
                    sources.collect_release(artifacts, native, dest)
                self.assertFalse(dest.exists())

    def test_outer_manifest_must_match_embedded_manifest(self):
        with tempfile.TemporaryDirectory() as temp, patch("sources.checkout", return_value=self.revision):
            native, artifacts, dest = self.fixtures(temp)
            manifest = next(artifacts.rglob("*-sources-rust.json"))
            data = read_manifest(manifest)
            data["providers"][0]["id"] = "substituted-provider"
            manifest.write_text(json.dumps(data))
            with self.assertRaisesRegex(RuntimeError, "embedded source manifest mismatch"):
                sources.collect_release(artifacts, native, dest)
            self.assertFalse(dest.exists())

    def test_inventory_rejects_missing_changed_duplicate_or_escaping_archive_members(self):
        for case in ("missing", "changed", "duplicate", "escape", "link"):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as temp, patch("sources.checkout", return_value=self.revision):
                native, artifacts, dest = self.fixtures(temp)
                manifest = next(artifacts.rglob("*-sources-rust.json"))

                def tamper(members, data):
                    index = next(i for i, (m, _) in enumerate(members) if m.name == "source.txt")
                    member, payload = members[index]
                    if case == "missing":
                        members.pop(index)
                    elif case == "changed":
                        members[index] = member, b"X" + payload[1:]
                    elif case == "duplicate":
                        members.append((member, payload))
                    elif case == "escape":
                        members.append((tarfile.TarInfo("../outside"), b"escape"))
                    else:
                        link = tarfile.TarInfo("unsafe-link")
                        link.type, link.linkname = tarfile.SYMTYPE, "../outside"
                        members.append((link, None))

                data = self.rewrite_archive(manifest, tamper)
                verify_parts(manifest, data)  # The transport-level checksums are all valid.
                with self.assertRaises(RuntimeError):
                    sources.collect_release(artifacts, native, dest)
                self.assertFalse(dest.exists())

    def test_provider_identity_and_retained_file_validation(self):
        for providers in ([{"id": "p", "files": ["source.txt"]}] * 2,
                          [{"id": "p", "files": ["missing"]}],
                          [{"id": "p", "files": ["directory"]}],
                          [{"id": "p", "files": ["source.txt", "source.txt"]}]):
            with self.subTest(providers=providers), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                source = root / "input"
                source.mkdir()
                (source / "source.txt").write_bytes(b"source")
                (source / "directory").mkdir()
                with self.assertRaises(RuntimeError):
                    sources.bundle(source, root / "out", "rust", providers, {}, self.revision)

    def test_hardlinked_files_and_internal_symlinks_have_consistent_inventory(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "source"
            source.mkdir()
            (source / "original").write_bytes(b"retained bytes")
            try:
                os.link(source / "original", source / "hardlink")
                (source / "link").symlink_to("original")
            except OSError:
                self.skipTest("host cannot create source hardlinks/symlinks")
            manifest = sources.bundle(source, root / "out", "rust",
                                      [{"id": "test", "files": ["original", "hardlink"]}], {}, self.revision)
            sources.verify_archive(manifest, read_manifest(manifest))
            with self.assertRaisesRegex(RuntimeError, "reserved source manifest"):
                sources.bundle(source, root / "other", "rust", [{"id": "test"}], {}, self.revision)

    def test_symlink_chains_cannot_escape_and_inventory_fields_are_validated(self):
        providers = [{"id": "provider", "files": ["source"]}]
        record = {"size": 0, "sha256": "0" * 64}
        for files in ({"source": record, "dir/link": {"link": ".."}, "escape": {"link": "dir/link/.."}},
                      {"source": record, "cycle": {"link": "cycle"}},
                      {"source": {"size": True, "sha256": "0" * 64}},
                      {"source": {"size": 1, "sha256": "bad"}},
                      {"source": record, "source/child": record}):
            with self.subTest(files=files), self.assertRaises(RuntimeError):
                sources.validate_inventory(files, providers)

    def test_public_build_configuration_excludes_secrets_and_validates_key(self):
        with tempfile.TemporaryDirectory() as temp, patch.dict(os.environ, {
                "JVID_UPDATE_PUBLIC_KEY": "A1" * 32,
                "JVID_UPDATE_SIGNING_KEY": "PRIVATE-SEED-MUST-NOT-APPEAR", "GH_TOKEN": "TOKEN-MUST-NOT-APPEAR"}):
            root = Path(temp)
            manifest = self.write_bundle(root / "input", root / "parts", "rust")
            data = read_manifest(manifest)
            self.assertEqual(data["build_configuration"]["JVID_UPDATE_PUBLIC_KEY"], "a1" * 32)
            sources.verify_archive(manifest, data)
            config = (root / "input/BUILD-CONFIG.json").read_text()
            self.assertNotIn("PRIVATE-SEED", config)
            self.assertNotIn("TOKEN", config)
            self.assertEqual(json.loads(config), data["build_configuration"])
        with patch.dict(os.environ, {"JVID_UPDATE_PUBLIC_KEY": "not-a-public-key"}):
            with self.assertRaisesRegex(RuntimeError, "64 hex"):
                sources.build_configuration()

    def test_standalone_helper_refuses_overwrite_and_removes_failed_output(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            manifest = self.write_bundle(root / "input", root / "parts", "rust")
            helper = root / "reassemble_sources.py"
            shutil.copy2(sources.ROOT / "scripts/packaging/reassemble_sources.py", helper)
            output = root / "sources.tar.gz"
            args = [sys.executable, "-I", str(helper), str(manifest), str(output)]
            result = subprocess.run(args, cwd=root, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            original = output.read_bytes()
            result = subprocess.run(args, cwd=root, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(output.read_bytes(), original)
            output.unlink()
            data = read_manifest(manifest)
            (manifest.parent / data["parts"][-1]["name"]).write_bytes(b"corrupt")
            result = subprocess.run(args, cwd=root, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(output.exists())

    def test_duplicate_json_fields_and_invalid_part_records_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            manifest = self.write_bundle(root / "input", root / "parts", "rust")
            data = read_manifest(manifest)
            for value in (None, {}, [None], [{**data["parts"][0], "size": True}],
                          [{**data["parts"][0], "size": PART_LIMIT + 1}]):
                with self.subTest(parts=value), self.assertRaises(RuntimeError):
                    verify_parts(manifest, {**data, "parts": value})
            manifest.write_text('{"schema":1,"schema":2}')
            with self.assertRaisesRegex(RuntimeError, "duplicate source manifest field"):
                read_manifest(manifest)

    def test_staging_collision_cannot_overwrite_files_or_follow_dangling_links(self):
        for case in ("helper", "dangling-part"):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as temp, patch("sources.checkout", return_value=self.revision):
                native, artifacts, dest = self.fixtures(temp)
                dest.mkdir()
                if case == "helper":
                    occupied = dest / "reassemble_sources.py"
                    occupied.write_bytes(b"user file")
                else:
                    part = next(artifacts.rglob("*.part001"))
                    occupied = dest / part.name
                    try:
                        occupied.symlink_to(Path(temp) / "outside-new-file")
                    except OSError:
                        self.skipTest("host cannot create symlinks")
                with self.assertRaisesRegex(RuntimeError, "source asset already exists"):
                    sources.collect_release(artifacts, native, dest)
                self.assertEqual(list(dest.iterdir()), [occupied])
                self.assertFalse((Path(temp) / "outside-new-file").exists())
                if case == "helper":
                    self.assertEqual(occupied.read_bytes(), b"user file")

    def test_valid_transport_hashes_do_not_hide_corrupt_gzip_footer(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            manifest = self.write_bundle(root / "input", root / "parts", "rust")
            data = read_manifest(manifest)
            rebuilt = io.BytesIO()
            verify_parts(manifest, data, rebuilt)
            payload = bytearray(rebuilt.getvalue())
            payload[-8] ^= 1  # gzip CRC, after the tar terminator and zero padding.
            for part in data["parts"]:
                (manifest.parent / part["name"]).unlink()
            writer = sources.PartWriter(manifest.parent, manifest.stem, limit=127)
            writer.write(payload)
            writer.finish_part()
            data.update(parts=writer.parts, archive_size=writer.total,
                        archive_sha256=writer.archive_hash.hexdigest())
            verify_parts(manifest, data)
            with self.assertRaisesRegex(RuntimeError, "invalid source archive"):
                sources.verify_archive(manifest, data)


if __name__ == "__main__":
    unittest.main()
