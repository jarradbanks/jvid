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
RUBY = shutil.which("ruby")


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

    def test_archive_failure_identifies_provider_resource_and_both_hashes(self):
        item = dict(self.plan["payloads"][1], url="https://sources.invalid/resource?secret=original",
                    mirrors=["https://mirror.invalid/resource?token=mirror"])
        corrupt = b"upstream returned an HTML challenge instead of the retained resource"
        with mock.patch.object(sources.urllib.request, "urlopen", side_effect=[Response(corrupt), Response(corrupt)]):
            with self.assertRaises(sources.SourceError) as raised:
                sources._archive_input(item, self.out / "resource", provider=self.provider())
        message = str(raised.exception)
        self.assertIn("homebrew:fixture@1.2.3_2", message)
        self.assertIn("resource 'support' ('resource-0')", message)
        self.assertIn("expected SHA-256 " + item["sha256"], message)
        self.assertIn("observed " + hashlib.sha256(corrupt).hexdigest(), message)
        self.assertIn("location 1:", message)
        self.assertIn("location 2:", message)
        self.assertNotIn("https://", message)
        self.assertNotIn("secret", message)
        self.assertNotIn("token", message)
        self.assertFalse((self.out / "resource").exists())

    def test_download_failure_does_not_expose_signed_redirect(self):
        failure = urllib.error.HTTPError("https://redirect.invalid/?secret=signed", 403,
                                         "secret=signed", {}, None)
        with mock.patch.object(sources.urllib.request, "urlopen", side_effect=failure):
            with self.assertRaises(sources.SourceError) as raised:
                sources._download(self.plan["payloads"][0]["url"],
                                  self.plan["payloads"][0]["sha256"], self.out / "resource")
        import traceback
        diagnostic = "".join(traceback.format_exception(raised.exception))
        self.assertIn("source download failed (HTTP 403)", diagnostic)
        self.assertNotIn("secret", diagnostic)
        self.assertNotIn("redirect.invalid", diagnostic)

    def test_videolan_raw_api_recovers_same_pinned_bytes_after_web_challenge(self):
        item = dict(self.plan["payloads"][1],
                    url="https://code.videolan.org/videolan/dav1d-test-data/raw/1.1.0/8-bit/data/00000000.ivf")
        api = ("https://code.videolan.org/api/v4/projects/videolan%2Fdav1d-test-data/"
               "repository/files/8-bit%2Fdata%2F00000000.ivf/raw?ref=1.1.0")
        with mock.patch.object(sources.urllib.request, "urlopen",
                               side_effect=[Response(b"<html>web challenge</html>"), Response(RESOURCE)]):
            self.assertEqual(sources._archive_input(item, self.out / "resource"), api)
        self.assertEqual((self.out / "resource").read_bytes(), RESOURCE)
        with mock.patch.object(sources.urllib.request, "urlopen", return_value=Response(b"wrong")):
            with self.assertRaisesRegex(sources.SourceError, "checksum mismatch"):
                sources._archive_input(item, self.out / "corrupt")
        self.assertFalse((self.out / "corrupt").exists())

    def test_raw_api_transport_preserves_ref_and_is_limited_to_known_public_host(self):
        url = "https://code.videolan.org/owner/repo/-/raw/commit%20id/path%20name/file.ivf"
        self.assertEqual(sources._raw_api_location(url),
                         "https://code.videolan.org/api/v4/projects/owner%2Frepo/"
                         "repository/files/path%20name%2Ffile.ivf/raw?ref=commit+id")
        for url in ("https://other.invalid/owner/repo/raw/ref/file",
                    "http://code.videolan.org/owner/repo/raw/ref/file",
                    "https://code.videolan.org/owner/repo/raw/ref/file?token=private",
                    "https://code.videolan.org/owner/repo/-/archive/ref/repo.tar.gz"):
            self.assertIsNone(sources._raw_api_location(url))

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

    def installed_resource(self):
        keg = self.root / "Cellar/fixture/1.2.3_2"
        (keg / ".brew").mkdir(parents=True)
        shutil.copyfile(self.recipe, keg / ".brew/fixture.rb")
        shutil.copyfile(self.provider_dir / "INSTALL_RECEIPT.json", keg / "INSTALL_RECEIPT.json")
        resource = keg / "share/tessdata/snum.traineddata"
        resource.parent.mkdir(parents=True)
        resource.write_bytes(RESOURCE)
        provider = self.provider()
        # Keep the provenance fixture in macOS form, while using a real temporary
        # keg on each host (including Windows) to exercise resource verification.
        provider["cellar"] = keg
        return provider, resource

    def test_deleted_resource_recovered_only_from_exact_installed_provider(self):
        provider, _ = self.installed_resource()
        item = self.plan["payloads"][1]
        key = (provider["name"], provider["version"], item["role"], item["name"], item["url"], item["sha256"])
        # Exercise a small fixture through the same checked recovery dispatch;
        # production's exact snum bytes are independently verified in replay.
        with mock.patch.object(sources, "SNUM_RECOVERY", key), \
             mock.patch.object(sources, "_providers", return_value=[provider]), \
             mock.patch.object(sources, "_formula_plan", return_value=self.plan), \
             mock.patch.object(sources, "_build_support", side_effect=self.fake_support), \
             mock.patch.object(sources.urllib.request, "urlopen", side_effect=[Response(ARCHIVE), urllib.error.URLError("deleted")]):
            records = sources.collect(self.stage, self.out)
        plan = json.loads((self.out / "providers/fixture/source-plan.json").read_text())
        recovered = plan["sources"][1]
        self.assertEqual(recovered["url"], item["url"])
        self.assertEqual(recovered["sha256"], item["sha256"])
        self.assertEqual(recovered["retrieved_from"], "installed-provider-resource")
        self.assertEqual(recovered["recovery"]["path"], "share/tessdata/snum.traineddata")
        self.assertEqual((self.out / recovered["file"]).read_bytes(), RESOURCE)
        self.assertNotIn(str(self.root), json.dumps(records) + json.dumps(plan))

    def test_installed_resource_rejects_changed_provenance_or_bytes(self):
        provider, resource = self.installed_resource()
        dest = self.out / "resource"
        originals = {path: path.read_bytes() for path in (
            provider["cellar"] / "INSTALL_RECEIPT.json", provider["cellar"] / ".brew/fixture.rb", resource)}
        for path in originals:
            with self.subTest(path=path.name):
                path.write_bytes(b"changed")
                with self.assertRaises(sources.SourceError):
                    sources._installed_resource(self.plan["payloads"][1], dest, provider,
                                                "share/tessdata/snum.traineddata")
                self.assertFalse(dest.exists())
                path.write_bytes(originals[path])

    def test_known_recoveries_never_apply_to_other_inputs(self):
        for key in (sources.SNUM_RECOVERY, sources.SRT_RECOVERY):
            for index in range(len(key)):
                with self.subTest(provider=key[0], changed_field=index):
                    changed = list(key)
                    changed[index] += "changed"
                    name, version, role, resource, url, sha = changed
                    item = {"role": role, "name": resource, "url": url, "sha256": sha}
                    provider = {"name": name, "version": version}
                    self.assertIsNone(sources._recover_archive_input(item, self.out / "resource", provider))

    def test_srt_recovery_mirror_requires_original_hash_and_records_revision(self):
        name, version, role, resource, url, sha = sources.SRT_RECOVERY
        provider = {"name": name, "version": version}
        item = {"role": role, "name": resource, "url": url, "sha256": sha}
        dest = self.out / "srt.tar.gz"
        with mock.patch.object(sources, "_download") as download:
            provenance = sources._recover_archive_input(item, dest, provider)
        download.assert_called_once_with(sources.SRT_SOURCE_MIRROR, sha, dest)
        self.assertEqual(provenance["retrieved_from"], sources.SRT_SOURCE_MIRROR)
        self.assertEqual(provenance["recovery"]["original_revision"], sources.SRT_ORIGINAL_REVISION)
        with mock.patch.object(sources.urllib.request, "urlopen", return_value=Response(b"replacement tag archive")):
            with self.assertRaisesRegex(sources.SourceError, "checksum mismatch.*" + sha):
                sources._recover_archive_input(item, dest, provider)
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

    def run_loader_fixture(self, signature, body, formula_body=""):
        # Replace only the Homebrew entry point, then run the production helper.
        # Exiting in the loader isolates argument compatibility from the DSL.
        library = self.root / "ruby-library"
        library.mkdir(exist_ok=True)
        (library / "formula.rb").write_text("class Formula\n" + formula_body + "\nend\n")
        (library / "formulary.rb").write_text('require "pathname"\n'
            'class Tap\n  def self.fetch(name); name; end\nend\n'
            'module Formulary\n  def self.from_contents(' + signature + ')\n' + body + '\n  end\nend\n')
        request = {"name": "fixture", "recipe": str(self.recipe.resolve()),
                   "version": "1.2.3_2", "receipt": self.receipt}
        return subprocess.run([RUBY, "-I", str(library), str(sources.HELPER)],
                              input=json.dumps(request), capture_output=True, text=True, timeout=30)

    @unittest.skipUnless(RUBY, "Ruby is required for Homebrew loader API fixtures")
    def test_homebrew_loader_keyword_capabilities_preserve_exact_inputs(self):
        signature = "name, path, contents, spec = :stable, tap: nil, flags: []"
        for keyword in ("", ", from_metadata: false", ", from_metadata:"):
            with self.subTest(keyword=keyword):
                result = self.run_loader_fixture(signature + keyword,
                    '$stdout.write(JSON.generate({"name" => name, "path" => path.to_s,\n'
                    '  "contents" => contents, "spec" => spec, "tap" => tap, "flags" => flags,\n'
                    '  "from_metadata" => (defined?(from_metadata) ? from_metadata : nil)}))\nexit')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), {
                    "name": "fixture", "path": str(self.recipe.resolve()), "contents": self.recipe.read_text(),
                    "spec": "stable", "tap": "homebrew/core", "flags": self.receipt["used_options"],
                    "from_metadata": True if keyword else None})

    @unittest.skipUnless(RUBY, "Ruby is required for Homebrew loader API fixtures")
    def test_homebrew_loader_recipe_argument_error_is_not_retried(self):
        result = self.run_loader_fixture(
            "name, path, contents, spec = :stable, tap: nil, flags: [], from_metadata: false",
            '$stdout.write("called\\n")\nraise ArgumentError, "retained recipe failed"')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("retained recipe failed", result.stderr)
        self.assertEqual(result.stdout, "called\n")

    @unittest.skipUnless(RUBY, "Ruby is required for Homebrew loader API fixtures")
    def test_older_homebrew_retains_legacy_autobump_annotation_as_string(self):
        for reason in (":requires_manual_review", ":versioned_formula", '"custom reason"'):
            with self.subTest(reason=reason):
                result = self.run_loader_fixture(
                    "name, path, contents, spec = :stable, tap: nil, flags: []",
                    'Formula.no_autobump!(because: ' + reason + ')\nexit',
                    'def self.no_autobump!(because:)\n'
                    '  raise "obsolete symbol" if because == :requires_manual_review\n'
                    '  $stdout.write(JSON.generate([because, because.class.name]))\nend')
                self.assertEqual(result.returncode, 0, result.stderr)
                expected = {":requires_manual_review": ["requires_manual_review", "String"],
                            ":versioned_formula": ["versioned_formula", "Symbol"],
                            '"custom reason"': ["custom reason", "String"]}
                self.assertEqual(json.loads(result.stdout), expected[reason])

    @unittest.skipUnless(sys.platform == "darwin" and shutil.which("brew"), "native macOS Homebrew is required")
    def test_native_homebrew_resolves_test_resource_and_inline_patch(self):
        # Exercise the real Homebrew API while proving that test/install bodies
        # are not executed, and that current formula lookup is unnecessary.
        self.recipe.write_text('class Fixture < Formula\n  desc "Fixture"\n  homepage "https://example.invalid"\n'
            '  url "https://sources.invalid/fixture-1.2.3.tar.gz"\n  sha256 "' + hashlib.sha256(ARCHIVE).hexdigest() + '"\n'
            '  revision 2\n  no_autobump! because: :requires_manual_review\n'
            '  patch :DATA\n  def install\n    raise "must not run install"\n  end\n'
            '  test do\n    raise "must not run test"\n    resource "support" do\n'
            '      url "https://sources.invalid/support.tar.xz"\n      sha256 "' + hashlib.sha256(RESOURCE).hexdigest() + '"\n'
            '    end\n  end\nend\n__END__\ndiff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n')
        self.receipt["used_options"] = []
        self.write_receipt()
        plan = sources._formula_plan(self.provider())
        self.assertEqual(plan["resource_declarations"], ["support"])
        self.assertEqual([item["kind"] for item in plan["payloads"]], ["archive", "inline", "archive"])

    @unittest.skipUnless(sys.platform == "darwin" and shutil.which("brew"), "native macOS Homebrew is required")
    def test_native_homebrew_resolves_external_local_and_resource_patches(self):
        digest = hashlib.sha256(RESOURCE).hexdigest()
        self.recipe.write_text('class Fixture < Formula\n  desc "Fixture"\n  homepage "https://example.invalid"\n'
            '  url "https://sources.invalid/fixture-1.2.3.tar.gz"\n  sha256 "' + hashlib.sha256(ARCHIVE).hexdigest() + '"\n'
            '  revision 2\n  patch do\n    url "https://sources.invalid/fixes.tar.gz"\n'
            '    sha256 "' + digest + '"\n    apply "fix.patch"\n  end\n'
            '  patch do\n    file "Patches/fixture/local.diff"\n  end\n'
            '  resource "support" do\n    url "https://sources.invalid/support.tar.xz"\n'
            '    sha256 "' + digest + '"\n    patch do\n      url "https://sources.invalid/support.patch"\n'
            '      sha256 "' + digest + '"\n    end\n  end\nend\n')
        self.receipt["used_options"] = []
        self.write_receipt()
        plan = sources._formula_plan(self.provider())
        self.assertEqual([item["kind"] for item in plan["payloads"]],
                         ["archive", "archive", "tap-file", "archive", "archive"])
        self.assertEqual([item["role"] for item in plan["payloads"]],
                         ["source", "patch", "patch", "resource", "patch"])
        self.assertEqual(plan["payloads"][1]["apply"], ["fix.patch"])
        self.assertEqual(plan["payloads"][2]["path"], "Patches/fixture/local.diff")
        self.assertEqual(plan["payloads"][4]["sha256"], digest)


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
