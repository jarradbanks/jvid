"""Run with python3 -m unittest discover -s scripts/packaging -p test_source_macos.py."""

import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import urllib.error

import source_macos as sources


ARCHIVE = b"fixture exact source archive\n"
RESOURCE = b"fixture source resource\n"
TAP_REVISION = "a" * 40


class Response(io.BytesIO):
    def geturl(self):
        return "https://sources.invalid/fixture.tar.gz"


class MacSourceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="jvid-test-macos-sources-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.stage = self.root / "stage"
        self.stage.mkdir()
        self.out = self.root / "out"
        self.out.mkdir()
        self.provider_dir = self.stage / "runtime-licenses/fixture"
        self.provider_dir.mkdir(parents=True)
        (self.stage / "SOURCE-INFO.txt").write_text("jvid source: https://example.invalid/jvid\n/opt/homebrew/Cellar/fixture/1.2.3_2\n")
        self.recipe = self.provider_dir / "fixture.rb"
        self.recipe.write_text('class Fixture < Formula\n  url "https://sources.invalid/fixture-1.2.3.tar.gz"\n'
                               '  sha256 "' + hashlib.sha256(ARCHIVE).hexdigest() + '"\n'
                               '  revision 2\n  resource "support" do\n'
                               '    url "https://sources.invalid/support.tar.xz"\n'
                               '    sha256 "' + hashlib.sha256(RESOURCE).hexdigest() + '"\n  end\nend\n')
        self.receipt = {"used_options": ["--with-feature"], "unused_options": [],
                        "homebrew_version": "6.0.0", "arch": "arm64", "compiler": "clang",
                        "source": {"spec": "stable", "versions": {"stable": "1.2.3"},
                                   "tap": "homebrew/core", "tap_git_head": TAP_REVISION}}
        self.write_receipt()
        self.plan = {"name": "fixture", "version": "1.2.3_2", "recipe_sha256": sources._hash(self.recipe),
                     "resource_declarations": ["support"], "dependencies": [{"name": "cmake", "tags": ["build"]}],
                     "payloads": [
                         {"id": "source", "role": "source", "kind": "archive", "url": "https://sources.invalid/fixture.tar.gz",
                          "sha256": hashlib.sha256(ARCHIVE).hexdigest()},
                         {"id": "resource-0", "name": "support", "role": "resource", "kind": "archive",
                          "url": "https://sources.invalid/support.tar.xz", "sha256": hashlib.sha256(RESOURCE).hexdigest()},
                     ]}

    def write_receipt(self):
        (self.provider_dir / "INSTALL_RECEIPT.json").write_text(json.dumps(self.receipt))

    def provider(self):
        return sources._providers(self.stage)[0]

    def fake_support(self, out, cache, providers):
        path = out / "build-support/exact-tap.tar"
        path.parent.mkdir()
        path.write_bytes(b"dependency recipes")
        return [{"id": "homebrew-build-support@" + TAP_REVISION, "files": ["build-support/exact-tap.tar"]}], {}

    def test_collect_all_sources_and_original_receipts(self):
        original_receipt = (self.provider_dir / "INSTALL_RECEIPT.json").read_bytes()
        inline = {"id": "source-patch-0", "role": "patch", "kind": "inline", "strip": "p1", "contents": "diff --git a/a b/a\n"}
        self.recipe.write_text(self.recipe.read_text().replace("  revision 2\n", "  revision 2\n  patch :DATA\n"))
        self.plan["recipe_sha256"] = sources._hash(self.recipe)
        self.plan["payloads"].insert(1, inline)
        with mock.patch.object(sources, "_formula_plan", return_value=self.plan), \
             mock.patch.object(sources, "_build_support", side_effect=self.fake_support), \
             mock.patch.object(sources.urllib.request, "urlopen", side_effect=[Response(ARCHIVE), Response(RESOURCE)]):
            records = sources.collect(self.stage, self.out)
        record = records[0]
        self.assertEqual(record["id"], "homebrew:fixture@1.2.3_2")
        self.assertEqual(record["provenance"]["used_options"], ["--with-feature"])
        self.assertEqual((self.out / "providers/fixture/INSTALL_RECEIPT.json").read_bytes(), original_receipt)
        self.assertEqual((self.out / "providers/fixture/fixture.rb").read_bytes(), self.recipe.read_bytes())
        payloads = [self.out / p for p in record["files"]]
        self.assertTrue(any(p.read_bytes() == ARCHIVE for p in payloads))
        self.assertTrue(any(p.read_bytes() == RESOURCE for p in payloads))
        self.assertTrue(any(p.read_text() == inline["contents"] for p in payloads))
        self.assertNotIn(str(self.root), json.dumps(records))
        self.assertTrue(all(not Path(path).is_absolute() and (self.out / path).is_file() for r in records for path in r["files"]))

    def test_exact_retained_recipe_is_passed_to_homebrew(self):
        current = self.root / "current/fixture.rb"
        current.parent.mkdir()
        current.write_text('class Fixture < Formula\nurl "https://wrong.invalid/current"\nend\n')
        with mock.patch.object(sources, "_run", return_value=json.dumps(self.plan).encode()) as run:
            result = sources._formula_plan(self.provider())
        self.assertEqual(result, self.plan)
        args, kwargs = run.call_args
        self.assertEqual(args[:2], ("brew", "ruby"))
        self.assertEqual(json.loads(kwargs["input"])["recipe"], str(self.recipe.resolve()))
        self.assertEqual(json.loads(kwargs["input"])["receipt"], self.receipt)

    def test_substituted_recipe_is_rejected(self):
        self.plan["recipe_sha256"] = "0" * 64
        with self.assertRaisesRegex(sources.SourceError, "different retained recipe"):
            sources._validate_plan(self.provider(), self.plan)

    def test_corrupted_archive_is_removed(self):
        dest = self.out / "source.tar.gz"
        with mock.patch.object(sources.urllib.request, "urlopen", return_value=Response(b"corrupt")):
            with self.assertRaisesRegex(sources.SourceError, "checksum mismatch"):
                sources._download("https://sources.invalid/source", hashlib.sha256(ARCHIVE).hexdigest(), dest)
        self.assertFalse(dest.exists())

    def test_unavailable_resource_fails_collection(self):
        with mock.patch.object(sources, "_formula_plan", return_value=self.plan), \
             mock.patch.object(sources, "_build_support", side_effect=self.fake_support), \
             mock.patch.object(sources.urllib.request, "urlopen", side_effect=[Response(ARCHIVE), urllib.error.URLError("missing resource")]):
            with self.assertRaisesRegex(sources.SourceError, "recorded source locations failed"):
                sources.collect(self.stage, self.out)
        self.assertFalse((self.out / "providers/fixture/source-plan.json").exists())

    def test_recorded_mirror_still_requires_exact_digest(self):
        item = dict(self.plan["payloads"][0], mirrors=["https://mirror.invalid/exact.tar.gz"])
        dest = self.out / "download"
        with mock.patch.object(sources.urllib.request, "urlopen", side_effect=[urllib.error.URLError("offline"), Response(ARCHIVE)]):
            selected = sources._archive_input(item, dest)
        self.assertEqual(selected, item["mirrors"][0])
        self.assertEqual(dest.read_bytes(), ARCHIVE)
        with mock.patch.object(sources.urllib.request, "urlopen", side_effect=[Response(b"bad primary"), Response(b"bad mirror")]):
            with self.assertRaisesRegex(sources.SourceError, "recorded source locations failed"):
                sources._archive_input(item, dest)
        self.assertFalse(dest.exists())

    def test_collect_external_and_tap_relative_patches(self):
        patch_data = b"--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n"
        self.recipe.write_text(self.recipe.read_text().replace("  revision 2\n", "  revision 2\n"
            '  patch do\n    url "https://sources.invalid/fix.patch"\n  end\n'
            '  patch do\n    file "Patches/fixture/local.diff"\n  end\n'))
        self.plan["recipe_sha256"] = sources._hash(self.recipe)
        self.plan["payloads"].insert(1, {"id": "source-patch-0", "role": "patch", "kind": "archive",
            "url": "https://sources.invalid/fix.patch", "sha256": hashlib.sha256(patch_data).hexdigest(), "strip": "p1"})
        self.plan["payloads"].insert(2, {"id": "source-patch-1", "role": "patch", "kind": "tap-file",
            "path": "Patches/fixture/local.diff", "strip": "p1"})
        sources._validate_plan(self.provider(), self.plan)
        def support(out, cache, providers):
            records, _ = self.fake_support(out, cache, providers)
            return records, {("homebrew/core", TAP_REVISION): self.root / "tap.git"}
        with mock.patch.object(sources, "_formula_plan", return_value=self.plan), \
             mock.patch.object(sources, "_build_support", side_effect=support), \
             mock.patch.object(sources, "_git_blob", return_value=patch_data) as blob, \
             mock.patch.object(sources.urllib.request, "urlopen", side_effect=[Response(ARCHIVE), Response(patch_data), Response(RESOURCE)]):
            records = sources.collect(self.stage, self.out)
        blob.assert_called_once_with(self.root / "tap.git", TAP_REVISION, "Patches/fixture/local.diff")
        self.assertEqual(sum((self.out / path).read_bytes() == patch_data for path in records[0]["files"]), 2)

    def test_omitted_resource_payload_is_rejected(self):
        self.plan["payloads"].pop()
        with self.assertRaisesRegex(sources.SourceError, "resource source payload missing"):
            sources._validate_plan(self.provider(), self.plan)

    def test_omitted_resource_declaration_audit_is_rejected(self):
        self.plan["resource_declarations"] = []
        self.plan["payloads"].pop()
        with self.assertRaisesRegex(sources.SourceError, "audit omitted"):
            sources._validate_plan(self.provider(), self.plan)

    def test_missing_recorded_provider_is_rejected(self):
        with (self.stage / "SOURCE-INFO.txt").open("a") as stream:
            stream.write("/opt/homebrew/Cellar/missing/1.0\n")
        with self.assertRaisesRegex(sources.SourceError, "providers differ"):
            self.provider()

    def test_unrecorded_retained_provider_is_rejected(self):
        (self.stage / "runtime-licenses/extra").mkdir()
        with self.assertRaisesRegex(sources.SourceError, "providers differ"):
            self.provider()

    def test_missing_and_ambiguous_recipe_fail_closed(self):
        self.recipe.unlink()
        with self.assertRaisesRegex(sources.SourceError, "ambiguous retained recipe"):
            self.provider()
        self.recipe.write_text("class Fixture < Formula; end\n")
        (self.provider_dir / "other.rb").write_text("other")
        with self.assertRaisesRegex(sources.SourceError, "ambiguous retained recipe"):
            self.provider()

    def test_receipt_corruption_and_version_mismatch_are_rejected(self):
        (self.provider_dir / "INSTALL_RECEIPT.json").write_text("{broken")
        with self.assertRaisesRegex(sources.SourceError, "invalid or missing JSON"):
            self.provider()
        self.receipt["source"]["versions"]["stable"] = "9.9.9"
        self.write_receipt()
        with self.assertRaisesRegex(sources.SourceError, "receipt version differs"):
            self.provider()

    def test_missing_options_and_mutable_tap_are_rejected(self):
        del self.receipt["used_options"]
        self.write_receipt()
        with self.assertRaisesRegex(sources.SourceError, "build options"):
            self.provider()
        self.receipt["used_options"] = []
        self.receipt["source"]["tap_git_head"] = "main"
        self.write_receipt()
        with self.assertRaisesRegex(sources.SourceError, "immutable tap revision"):
            self.provider()

    def test_floating_git_source_is_rejected(self):
        self.plan["payloads"][0] = {"id": "source", "role": "source", "kind": "git", "url": "https://example.invalid/repo.git", "revision": "main"}
        with self.assertRaisesRegex(sources.SourceError, "immutable revision"):
            sources._validate_plan(self.provider(), self.plan)

    def test_escaping_tap_patch_is_rejected(self):
        self.recipe.write_text(self.recipe.read_text().replace("  revision 2\n", "  revision 2\n  patch :DATA\n"))
        self.plan["recipe_sha256"] = sources._hash(self.recipe)
        self.plan["payloads"].append({"id": "patch", "role": "patch", "kind": "tap-file", "path": "../private"})
        with self.assertRaisesRegex(sources.SourceError, "escaping provider path"):
            sources._validate_plan(self.provider(), self.plan)

    def test_omitted_patch_is_rejected(self):
        self.recipe.write_text(self.recipe.read_text().replace("  revision 2\n", "  revision 2\n  patch :DATA\n"))
        self.plan["recipe_sha256"] = sources._hash(self.recipe)
        with self.assertRaisesRegex(sources.SourceError, "patch source payload missing"):
            sources._validate_plan(self.provider(), self.plan)

    @unittest.skipUnless(sys.platform == "darwin" and shutil.which("brew"), "native macOS Homebrew is required")
    def test_native_homebrew_resolves_test_resource_and_inline_patch(self):
        # Exercise the real Homebrew API while proving that test/install bodies
        # are not executed, and that current formula lookup is unnecessary.
        self.recipe.write_text('class Fixture < Formula\n  desc "Fixture"\n  homepage "https://example.invalid"\n'
            '  url "https://sources.invalid/fixture-1.2.3.tar.gz"\n  sha256 "' + hashlib.sha256(ARCHIVE).hexdigest() + '"\n'
            '  revision 2\n  patch :DATA\n  def install\n    raise "must not run install"\n  end\n'
            '  test do\n    raise "must not run test"\n    resource "support" do\n'
            '      url "https://sources.invalid/support.tar.xz"\n      sha256 "' + hashlib.sha256(RESOURCE).hexdigest() + '"\n'
            '    end\n  end\nend\n__END__\ndiff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n')
        self.receipt["used_options"] = []
        self.write_receipt()
        plan = sources._formula_plan(self.provider())
        self.assertEqual(plan["resource_declarations"], ["support"])
        self.assertEqual([item["kind"] for item in plan["payloads"]], ["archive", "inline", "archive"])


class GitSnapshotTests(unittest.TestCase):
    def test_exact_git_snapshot_and_submodule_revision(self):
        with tempfile.TemporaryDirectory(prefix="jvid-test-macos-git-") as temporary:
            root = Path(temporary)
            def git(repo, *args):
                return subprocess.check_output(["git", "-C", str(repo), *args], stderr=subprocess.DEVNULL).decode().strip()
            def repository(name):
                repo = root / name
                subprocess.run(["git", "init", "-q", str(repo)], check=True)
                git(repo, "config", "user.email", "fixture@example.invalid")
                git(repo, "config", "user.name", "Fixture")
                (repo / "source.txt").write_text(name)
                (repo / ".gitattributes").write_text("source.txt export-ignore\n")
                git(repo, "add", ".")
                git(repo, "commit", "-qm", "fixture")
                return repo
            child = repository("child")
            child_revision = git(child, "rev-parse", "HEAD")
            parent = repository("parent")
            git(parent, "-c", "protocol.file.allow=always", "submodule", "add", str(child), "vendor/child")
            git(parent, "commit", "-qam", "pin submodule")
            revision = git(parent, "rev-parse", "HEAD")
            (child / "source.txt").write_text("later source must not replace pin")
            git(child, "commit", "-qam", "later")
            cache = root / "cache"
            cache.mkdir()
            # File transport exists only inside this local Git fixture; the
            # production URL validator always requires HTTPS.
            with mock.patch.object(sources, "_url", side_effect=lambda value, **kw: value):
                records = sources._git_snapshot(str(parent), revision, root / "snapshot.tar", cache)
            self.assertEqual(records[0]["revision"], revision)
            self.assertEqual(records[1]["revision"], child_revision)
            self.assertEqual(records[1]["path"], "vendor/child")
            import tarfile
            with tarfile.open(root / records[1]["file"]) as archive:
                self.assertEqual(archive.extractfile("source.txt").read(), b"child")


if __name__ == "__main__":
    unittest.main()
