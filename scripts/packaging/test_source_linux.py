#!/usr/bin/env python3
"""Exact Debian receipt coverage, authenticated downloads, and fail-closed checks."""
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import source_linux as linux


HEADER = "jvid source: https://github.com/jarradbanks/jvid\n\n"
PROVIDERS = [
    "libexample1:amd64 2:1.2.3-4+b1 example 2:1.2.3-4",
    "libexample-dev:amd64 2:1.2.3-4+b1 example 2:1.2.3-4",
]


def tar_bytes(files):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w:gz") as archive:
        for name, payload in files.items():
            entry = tarfile.TarInfo(name)
            entry.size = len(payload)
            archive.addfile(entry, io.BytesIO(payload))
    return stream.getvalue()


class LinuxSourceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="private-linux-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.stage = self.root / "stage"
        self.stage.mkdir()
        self.output = self.root / "sources"
        self.write_stage(PROVIDERS)
        self.commands = []
        self.payloads = {
            "example_1.2.3.orig.tar.gz": tar_bytes({"example-1.2.3/source.c": b"int main(void) { return 0; }\n"}),
            "example_1.2.3-4.debian.tar.gz": tar_bytes({
                "debian/control": b"Source: example\n",
                "debian/rules": b"#!/usr/bin/make -f\n%:\n\tdh $@\n",
                "debian/patches/fix.patch": b"recorded patch\n",
            }),
        }
        self.dsc_source = "example"
        self.dsc_version = "2:1.2.3-4"
        self.dsc_binary = "libexample1, libexample-dev"
        self.signed = True
        self.format = "3.0 (quilt)"
        self.download_transform = None
        self.metadata_transform = None
        self.dsc_transform = None

    def write_stage(self, lines):
        (self.stage / "SOURCE-INFO.txt").write_text(HEADER + "\n".join(lines) + "\n")
        for line in lines:
            name = line.split()[0].split(":")[0]
            directory = self.stage / "runtime-licenses" / name
            directory.mkdir(parents=True, exist_ok=True)
            (directory / "copyright").write_text("Debian copyright receipt\n")

    @staticmethod
    def checksums(payloads, algorithm):
        return "\n".join(f" {hashlib.new(algorithm, payload).hexdigest()} {len(payload)} {name}"
                         for name, payload in sorted(payloads.items()))

    def fixture(self):
        dsc = (f"Format: {self.format}\nSource: {self.dsc_source}\n"
               f"Binary: {self.dsc_binary}\nVersion: {self.dsc_version}\n"
               f"Files:\n{self.checksums(self.payloads, 'md5')}\n"
               f"Checksums-Sha1:\n{self.checksums(self.payloads, 'sha1')}\n"
               f"Checksums-Sha256:\n{self.checksums(self.payloads, 'sha256')}\n")
        if self.dsc_transform:
            dsc = self.dsc_transform(dsc)
        if self.signed:
            dsc = ("-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\n" + dsc + "\n"
                   "-----BEGIN PGP SIGNATURE-----\nretained-signature\n-----END PGP SIGNATURE-----\n")
        payloads = {**self.payloads, "example_1.2.3-4.dsc": dsc.encode()}
        metadata = ("Package: example\nVersion: 2:1.2.3-4\nBinary: libexample1, libexample-dev\n"
                    f"Checksums-Sha256:\n{self.checksums(payloads, 'sha256')}\n"
                    f"Directory: pool/main/e/example\nFiles:\n{self.checksums(payloads, 'md5')}\n"
                    f"X-Local-Index: {self.root}/private/apt/Sources\n\n")
        if self.metadata_transform:
            metadata = self.metadata_transform(metadata)
        return metadata, payloads

    def fake_run(self, arguments, cwd=None):
        self.commands.append(arguments)
        metadata, payloads = self.fixture()
        if arguments[0] == "apt-cache":
            self.assertEqual(arguments, ["apt-cache", "--only-source", "showsrc", "example"])
            return metadata
        self.assertEqual(arguments[0], "apt-get")
        self.assertIn("example=2:1.2.3-4", arguments)
        self.assertIn("--download-only", arguments)
        self.assertIn("--only-source", arguments)
        self.assertIn("APT::Get::AllowUnauthenticated=false", arguments)
        self.assertIn("Acquire::AllowInsecureRepositories=false", arguments)
        self.assertNotIn("--allow-unauthenticated", arguments)
        self.assertEqual(arguments.count("source"), 1)
        for name, payload in payloads.items():
            (cwd / name).write_bytes(payload)
        if self.download_transform:
            self.download_transform(cwd)
        return "Downloaded (private output is never persisted)"

    def collect(self):
        with patch.object(linux, "_run", side_effect=self.fake_run):
            return linux.collect(self.stage, self.output)

    def test_collects_exact_epoch_source_once_and_retains_build_recipes(self):
        records = self.collect()
        self.assertEqual(len(records), 1)
        record = records[0]
        self.assertEqual(record["source"], "example")
        self.assertEqual(record["version"], "2:1.2.3-4")
        self.assertEqual(record["id"], "debian-example-2_1.2.3-4")
        self.assertEqual(len(record["binary_packages"]), 2)
        self.assertEqual(len(self.commands), 2)
        paths = [self.output / name for name in record["files"]]
        self.assertTrue(all(path.is_file() and path.is_relative_to(self.output) for path in paths))
        recipe = next(path for path in paths if ".debian.tar." in path.name)
        with tarfile.open(recipe) as source:
            self.assertEqual(source.extractfile("debian/rules").read(), b"#!/usr/bin/make -f\n%:\n\tdh $@\n")
            self.assertIn("debian/patches/fix.patch", source.getnames())
        receipt = json.loads(next(path for path in paths if path.name == "provenance.json").read_text())
        self.assertEqual(receipt["binary_packages"][0]["binary_version"], "2:1.2.3-4+b1")
        self.assertEqual(receipt["download"][-1], "example=2:1.2.3-4")
        text = json.dumps(records) + json.dumps(receipt)
        self.assertNotIn(str(self.root), text)
        self.assertNotIn("X-Local-Index", text)
        self.assertNotIn("/tmp/", text)

    def test_retains_native_source_with_embedded_debian_recipes(self):
        self.signed = False
        self.format = "3.0 (native)"
        self.payloads = {"example_1.2.3-4.tar.gz": tar_bytes({"example/debian/rules": b"native build recipe"})}
        record = self.collect()[0]
        self.assertTrue(any(name.endswith(".tar.gz") for name in record["files"]))

    def test_retains_legacy_debian_diff(self):
        self.format = "1.0"
        self.payloads.pop("example_1.2.3-4.debian.tar.gz")
        self.payloads["example_1.2.3-4.diff.gz"] = b"original Debian packaging diff"
        self.assertTrue(any(name.endswith(".diff.gz") for name in self.collect()[0]["files"]))

    def test_binary_receipt_duplicates_are_deduplicated(self):
        self.write_stage(PROVIDERS + PROVIDERS)
        self.assertEqual(len(self.collect()[0]["binary_packages"]), 2)

    def test_missing_provider_receipt_fails_closed(self):
        (self.stage / "SOURCE-INFO.txt").write_text(HEADER + PROVIDERS[0] + "\n")
        with self.assertRaisesRegex(RuntimeError, "unrecorded providers"):
            self.collect()
        self.assertEqual(self.commands, [])

    def test_missing_provider_notice_fails_closed(self):
        (self.stage / "runtime-licenses/libexample-dev/copyright").unlink()
        with self.assertRaisesRegex(RuntimeError, "copyright"):
            self.collect()

    def test_empty_and_malformed_receipts_fail_closed(self):
        for text in (HEADER, HEADER + "libexample1:amd64 1.2 example\n",
                     HEADER + "../../private 1.0 example 1.0\n",
                     HEADER + "libexample1 1.0 ../../private 1.0\n",
                     HEADER + "libexample1 1.0 example 1/../../private\n",
                     HEADER + "unrecognized provider format\n"):
            with self.subTest(text=text):
                (self.stage / "SOURCE-INFO.txt").write_text(text)
                with self.assertRaises(RuntimeError):
                    self.collect()
        self.assertEqual(self.commands, [])

    def test_conflicting_duplicate_binary_version_fails(self):
        self.write_stage(PROVIDERS + [PROVIDERS[0].replace("+b1", "+b2")])
        with self.assertRaisesRegex(RuntimeError, "conflicting Debian provider"):
            self.collect()

    def test_missing_exact_apt_version_does_not_fall_back(self):
        self.metadata_transform = lambda text: text.replace("Version: 2:1.2.3-4", "Version: 2:9.9-1")
        with self.assertRaisesRegex(RuntimeError, "exact APT source metadata unavailable"):
            self.collect()
        self.assertEqual(len(self.commands), 1)

    def test_dsc_source_and_version_mismatch_fail_closed(self):
        for attribute, value in (("dsc_source", "other"), ("dsc_version", "2:1.2.3-5")):
            with self.subTest(attribute=attribute):
                setattr(self, attribute, value)
                with self.assertRaisesRegex(RuntimeError, "source package/version mismatch"):
                    self.collect()
                setattr(self, attribute, "example" if attribute == "dsc_source" else "2:1.2.3-4")

    def test_dsc_binary_coverage_mismatch_fails_closed(self):
        self.dsc_binary = "libunrelated1"
        with self.assertRaisesRegex(RuntimeError, "does not cover"):
            self.collect()

    def test_source_archive_corruption_fails_closed(self):
        self.download_transform = lambda directory: (directory / "example_1.2.3.orig.tar.gz").write_bytes(b"corrupt")
        with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
            self.collect()

    def test_dsc_corruption_fails_apt_checksum_check(self):
        self.download_transform = lambda directory: (directory / "example_1.2.3-4.dsc").write_text("Source: injected\n")
        with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
            self.collect()

    def test_missing_source_payload_fails_closed(self):
        self.download_transform = lambda directory: (directory / "example_1.2.3.orig.tar.gz").unlink()
        with self.assertRaisesRegex(RuntimeError, "file set differs"):
            self.collect()

    def test_symlink_download_is_rejected(self):
        def substitute(directory):
            path = directory / "example_1.2.3.orig.tar.gz"
            external = self.root / "private-archive"
            external.write_bytes(path.read_bytes())
            path.unlink()
            path.symlink_to(external)
        self.download_transform = substitute
        with self.assertRaisesRegex(RuntimeError, "missing regular"):
            self.collect()

    def test_unsanitized_metadata_and_dsc_paths_are_rejected(self):
        for filename in ("../escape.tar.gz", "/private/source.tar.gz", "dir/source.tar.gz", "..\\escape.tar.gz"):
            with self.subTest(filename=filename):
                self.metadata_transform = lambda text: text.replace("example_1.2.3.orig.tar.gz", filename)
                with self.assertRaisesRegex(RuntimeError, "invalid checksums-sha256"):
                    self.collect()
                self.metadata_transform = None
                self.dsc_transform = lambda text: text.replace("example_1.2.3.orig.tar.gz", filename)
                with self.assertRaisesRegex(RuntimeError, "invalid checksums-sha256"):
                    self.collect()
                self.dsc_transform = None
        self.assertFalse((self.root / "escape.tar.gz").exists())

    def test_dsc_reference_coverage_cannot_omit_an_archive(self):
        def omit(text):
            before, after = text.split("Checksums-Sha256:\n", 1)
            return before + "Checksums-Sha256:\n" + "\n".join(line for line in after.splitlines() if ".orig.tar.gz" not in line) + "\n"
        self.dsc_transform = omit
        with self.assertRaisesRegex(RuntimeError, "file coverage differs"):
            self.collect()

    def test_legacy_checksum_mismatch_is_also_rejected(self):
        original = hashlib.md5(self.payloads["example_1.2.3.orig.tar.gz"]).hexdigest()
        self.dsc_transform = lambda text: text.replace(original, "0" * 32)
        with self.assertRaisesRegex(RuntimeError, "checksum mismatch.*md5"):
            self.collect()

    def test_conflicting_apt_metadata_fails_closed(self):
        def duplicate(text):
            return text + text.replace("example_1.2.3.orig.tar.gz", "example_other.orig.tar.gz")
        self.metadata_transform = duplicate
        with self.assertRaisesRegex(RuntimeError, "conflicting APT"):
            self.collect()

    def test_source_payload_cannot_overwrite_provenance_receipt(self):
        self.metadata_transform = lambda text: text.replace("example_1.2.3.orig.tar.gz", "provenance.json")
        with self.assertRaisesRegex(RuntimeError, "conflicts with collector receipts"):
            self.collect()
        self.assertEqual(len(self.commands), 1)

    def test_every_provider_is_collected_and_missing_second_source_fails(self):
        extra = "libzother1:amd64 1.0-1 zother 1.0-1"
        self.write_stage(PROVIDERS + [extra])
        original = self.fake_run
        def missing_second(arguments, cwd=None):
            if arguments[0] == "apt-cache" and arguments[-1] == "zother":
                return "Package: zother\nVersion: 2.0-1\n\n"
            return original(arguments, cwd)
        with patch.object(linux, "_run", side_effect=missing_second):
            with self.assertRaisesRegex(RuntimeError, "exact APT source metadata unavailable: zother=1.0-1"):
                linux.collect(self.stage, self.output)
        # Partial first-provider data cannot turn the missing second source
        # into a successful (incomplete) list of provider records.
        self.assertTrue((self.output / "debian-example-2_1.2.3-4").is_dir())

    def test_stage_receipts_cannot_be_symlinks(self):
        info = self.stage / "SOURCE-INFO.txt"
        external = self.root / "outside-info"
        info.rename(external)
        info.symlink_to(external)
        with self.assertRaisesRegex(RuntimeError, "regular SOURCE-INFO"):
            self.collect()

    def test_dsc_sha512_is_verified_when_present(self):
        extra = self.checksums(self.payloads, "sha512")
        self.dsc_transform = lambda text: text + "Checksums-Sha512:\n" + extra.replace(extra.split()[0], "0" * 128) + "\n"
        with self.assertRaisesRegex(RuntimeError, "checksum mismatch.*sha512"):
            self.collect()

    def test_output_must_be_fresh(self):
        self.output.mkdir()
        (self.output / "old-source").write_text("old")
        with self.assertRaisesRegex(RuntimeError, "fresh empty"):
            self.collect()
        self.assertEqual(self.commands, [])

    def test_unauthenticated_or_failed_apt_download_propagates(self):
        def fail_download(arguments, cwd=None):
            if arguments[0] == "apt-get":
                raise RuntimeError("apt-get failed: unauthenticated source")
            return self.fake_run(arguments, cwd)
        with patch.object(linux, "_run", side_effect=fail_download):
            with self.assertRaisesRegex(RuntimeError, "unauthenticated"):
                linux.collect(self.stage, self.output)
        self.assertFalse(any(self.output.iterdir()))

    def test_subprocess_is_bounded_noninteractive_and_errors_do_not_leak_paths(self):
        private = str(self.root / "private")
        failure = subprocess.CalledProcessError(100, ["apt-get"], stderr=private)
        with patch.object(linux.subprocess, "run", side_effect=failure) as run:
            with self.assertRaisesRegex(RuntimeError, "exit 100") as raised:
                linux._run(["apt-get", "source"], self.root)
        self.assertNotIn(private, str(raised.exception))
        self.assertEqual(run.call_args.kwargs["timeout"], 1800)
        self.assertEqual(run.call_args.kwargs["stdin"], subprocess.DEVNULL)


if __name__ == "__main__":
    unittest.main()
