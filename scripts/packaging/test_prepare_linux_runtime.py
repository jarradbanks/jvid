#!/usr/bin/env python3
"""Read-only runtime planning, exact-version checks, and packaging closure parity."""
import ast
import contextlib
import inspect
import io
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import build
import prepare_linux_runtime as prepare
import source_linux


def provider(binary, source="systemd", version="249.11-0ubuntu3.22"):
    return dict(zip(prepare._FIELDS, (binary, version, source, version)))


def metadata(source, version, binaries):
    return (f"Package: {source}\nVersion: {version}\nBinary: {', '.join(binaries)}\n"
            f"Checksums-Sha256:\n {'a' * 64} 123 {source}.dsc\n\n")


class RuntimePlanTests(unittest.TestCase):
    def setUp(self):
        self.stderr = io.StringIO()
        self.stdout = io.StringIO()
        stack = contextlib.ExitStack()
        self.addCleanup(stack.close)
        stack.enter_context(contextlib.redirect_stderr(self.stderr))
        stack.enter_context(contextlib.redirect_stdout(self.stdout))
        self.old = [provider("libsystemd0:amd64"), provider("libudev1:amd64")]
        self.new_version = "249.11-0ubuntu3.23"
        self.available = metadata("systemd", self.new_version, ["libsystemd0", "libudev1"])

    def test_old_exact_source_selects_binary_packages_without_newer_source_substitution(self):
        with patch.object(source_linux, "_run", return_value=self.available) as run:
            self.assertEqual(prepare.upgrades(self.old), ["libsystemd0:amd64", "libudev1:amd64"])
        run.assert_called_once_with(["apt-cache", "--only-source", "showsrc", "systemd"])
        self.assertIn("249.11-0ubuntu3.22", self.stderr.getvalue())
        self.assertEqual(self.stdout.getvalue(), "")

    def test_output_is_sorted_deduplicated_binary_names_and_check_refuses_old_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "upgrades.txt"
            with patch.object(prepare, "runtime_providers", return_value=list(reversed(self.old)) + self.old), \
                    patch.object(source_linux, "_run", return_value=self.available):
                self.assertEqual(prepare.main(["--output", str(output)]), 0)
                self.assertEqual(output.read_text(), "libsystemd0:amd64\nlibudev1:amd64\n")
                self.assertEqual(prepare.main(["--check"]), 1)
        self.assertIn("exact runtime sources remain unavailable", self.stderr.getvalue())
        self.assertEqual(self.stdout.getvalue(), "")

    def test_check_succeeds_only_after_installed_provider_versions_are_available(self):
        upgraded = [provider(item["binary_package"], version=self.new_version) for item in self.old]
        with patch.object(prepare, "runtime_providers", return_value=upgraded), \
                patch.object(source_linux, "_run", return_value=self.available):
            self.assertEqual(prepare.main(["--check"]), 0)
            with tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary) / "upgrades.txt"
                self.assertEqual(prepare.main(["--output", str(output)]), 0)
                self.assertEqual(output.read_bytes(), b"")

    def test_metadata_uses_source_version_when_binary_version_has_a_rebuild_suffix(self):
        rebuilt = provider("libsystemd0:amd64", version=self.new_version)
        rebuilt["binary_version"] += "+b1"
        with patch.object(source_linux, "_run", return_value=self.available):
            self.assertEqual(prepare.upgrades([rebuilt]), [])

    def test_all_providers_checked_and_one_absent_source_does_not_hide_another(self):
        other = provider("libother1:amd64", source="other", version="1.0-1")
        available = metadata("other", "1.0-1", ["libother1"])
        with patch.object(source_linux, "_run", side_effect=[available, self.available]) as run:
            self.assertEqual(prepare.upgrades(self.old + [other]), ["libsystemd0:amd64", "libudev1:amd64"])
        self.assertEqual(run.call_count, 2)

    def test_metadata_integrity_errors_are_fatal_and_do_not_write_an_upgrade_file(self):
        exact = metadata("systemd", "249.11-0ubuntu3.22", ["libsystemd0", "libudev1"])
        cases = (
            exact + exact.replace("a" * 64, "b" * 64),
            exact.replace("libsystemd0, libudev1", "unrelated"),
            exact.replace("a" * 64, "invalid-checksum"),
        )
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "upgrades.txt"
            for text in cases:
                with self.subTest(text=text), patch.object(prepare, "runtime_providers", return_value=self.old), \
                        patch.object(source_linux, "_run", return_value=text):
                    self.assertEqual(prepare.main(["--output", str(output)]), 1)
                    self.assertFalse(output.exists())

    def test_apt_errors_and_unresolved_closure_do_not_become_upgrade_candidates(self):
        with patch.object(source_linux, "_metadata", side_effect=RuntimeError("apt-cache failed")):
            with self.assertRaisesRegex(RuntimeError, "apt-cache failed"):
                prepare.upgrades(self.old)
        with patch.object(prepare, "runtime_providers", side_effect=RuntimeError("unresolved runtime dependency")):
            self.assertEqual(prepare.main(["--check"]), 1)

    def test_invalid_package_names_are_never_emitted(self):
        for name in ("--allow-unauthenticated", "libfoo;touch /tmp/escape", "libfoo\nlibbar", "../../private"):
            with self.subTest(name=name), patch.object(source_linux, "_metadata") as check:
                with self.assertRaisesRegex(RuntimeError, "invalid installed"):
                    prepare.upgrades([provider(name)])
                check.assert_not_called()

    def test_symlink_output_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "keep"
            target.write_text("keep")
            output = Path(temporary) / "output"
            output.symlink_to(target)
            with patch.object(prepare, "runtime_providers", return_value=self.old), \
                    patch.object(source_linux, "_run", return_value=self.available):
                self.assertEqual(prepare.main(["--output", str(output)]), 1)
            self.assertEqual(target.read_text(), "keep")


class RuntimeClosureTests(unittest.TestCase):
    def setUp(self):
        stack = contextlib.ExitStack()
        self.addCleanup(stack.close)
        stack.enter_context(contextlib.redirect_stderr(io.StringIO()))

    def test_desktop_roots_and_system_exclusions_match_packager(self):
        function = ast.parse(inspect.getsource(build.linux_libraries))
        tuples = [tuple(item.value for item in node.elts) for node in ast.walk(function)
                  if isinstance(node, ast.Tuple) and all(isinstance(item, ast.Constant)
                                                        and isinstance(item.value, str) for item in node.elts)]
        self.assertIn(prepare.DESKTOP, tuples)
        self.assertIs(prepare.build.LINUX_SYSTEM, build.LINUX_SYSTEM)
        self.assertEqual(set(prepare.FFMPEG), {"libavcodec", "libavformat", "libavutil", "libswscale",
                                              "libswresample", "libavfilter"})

    def test_ffmpeg_uses_pkg_config_linker_symlinks_and_std_cpp_native_cache(self):
        root = Path("/usr/lib/x86_64-linux-gnu")
        aliases = {root / (name + ".so"): root / (name + ".so.58.1.0") for name in prepare.FFMPEG}
        aliases.update({root / (name + ".so.58"): root / (name + ".so.58.1.0") for name in prepare.FFMPEG})
        cache = "".join(f" {name}.so.99 (libc6,x86-64) => {root}/{name}.so.99\n"
                        f" {name}.so.58 (libc6,x86-64) => {root}/{name}.so.58\n" for name in prepare.FFMPEG)
        cache += " libstdc++.so.6 (libc6,x32) => /libx32/libstdc++.so.6\n"
        cache += "".join(f" {name} (libc6,x86-64) => {root}/{name}\n"
                         for name in ("libstdc++.so.6", *prepare.DESKTOP))
        def run(*args):
            if args[0] == "dpkg":
                return "amd64\n"
            if args[0] == "ldconfig":
                return cache
            self.assertEqual(args[:2], ("pkg-config", "--variable=libdir"))
            return str(root) + "\n"
        with patch.object(prepare.sys, "platform", "linux"), patch.dict(prepare.os.environ, {}, clear=True), \
                patch.object(prepare, "_run", side_effect=run), \
                patch.object(prepare, "_library", side_effect=lambda path: aliases.get(Path(path), Path(path))):
            roots = prepare.runtime_roots()
        self.assertEqual(roots[:6], [root / (name + ".so") for name in prepare.FFMPEG])
        self.assertEqual(roots[6], root / "libstdc++.so.6")
        self.assertEqual(roots[7:], [root / name for name in prepare.DESKTOP])

    def test_transitive_non_system_closure_is_complete_and_deduplicated(self):
        seed = Path("/usr/lib/libseed.so.1")
        second = Path("/usr/lib/libsecond.so.1")
        systemd = Path("/usr/lib/libsystemd.so.0")
        graph = {
            seed: f"\tlibsecond.so.1 => {second} (0x123)\n\tlibc.so.6 => /usr/lib/libc.so.6 (0x1)\n"
                  "\tlibGL.so.1 => /usr/lib/libGL.so.1 (0x1)\n",
            second: f"\tlibsystemd.so.0 => {systemd} (0x123)\n\tlibseed.so.1 => {seed} (0x1)\n",
            systemd: "\tlinux-vdso.so.1 (0x1)\n\t/lib/ld-linux-x86-64.so.2 (0x1)\n",
        }
        with patch.object(prepare, "_library", side_effect=Path), \
                patch.object(prepare, "_run", side_effect=lambda command, library: graph[library]) as run:
            self.assertEqual(prepare.runtime_libraries([seed, seed]), sorted([seed, second, systemd]))
        self.assertEqual(run.call_count, 3)

    def test_unresolved_or_unrecognized_ldd_output_is_fatal(self):
        for output in ("libbad.so.1 => not found\n", "statically linked\n", "", "unrecognized dependency\n"):
            with self.subTest(output=output), patch.object(prepare, "_library", side_effect=Path), \
                    patch.object(prepare, "_run", return_value=output):
                with self.assertRaises(RuntimeError):
                    prepare.runtime_libraries([Path("/usr/lib/libseed.so.1")])

    def test_same_soname_cannot_resolve_to_two_providers(self):
        def run(command, library):
            if library.name == "libone.so.1":
                return "libduplicate.so.1 => /usr/lib/a/libduplicate.so.1 (0x1)\n"
            if library.name == "libtwo.so.1":
                return "libduplicate.so.1 => /usr/lib/b/libduplicate.so.1 (0x1)\n"
            return "linux-vdso.so.1 (0x1)\n"
        with patch.object(prepare, "_library", side_effect=Path), patch.object(prepare, "_run", side_effect=run):
            with self.assertRaisesRegex(RuntimeError, "conflicting runtime providers"):
                prepare.runtime_libraries([Path("/usr/lib/libone.so.1"), Path("/usr/lib/libtwo.so.1")])

    def test_dependency_cannot_replace_a_seeded_desktop_provider(self):
        desktop = Path("/usr/lib/libX11.so.6.4.0")
        with patch.object(prepare, "_library", side_effect=Path), \
                patch.object(prepare, "_run", return_value="libX11.so.6 => /usr/lib/other/libX11.so.6 (0x1)\n"):
            with self.assertRaisesRegex(RuntimeError, "conflicting runtime providers: libX11.so.6"):
                prepare.runtime_libraries([desktop, Path("/usr/lib/libseed.so.1")])

    def test_external_library_provider_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            external = Path(temporary) / "libexternal.so.1"
            external.write_bytes(b"external")
            with self.assertRaisesRegex(RuntimeError, "external or invalid"):
                prepare._library(external)

    def test_external_transitive_provider_is_rejected_without_substitution(self):
        with tempfile.TemporaryDirectory() as temporary:
            external = Path(temporary) / "libdrm.so.2"
            external.write_bytes(b"external driver")
            source = Path("/usr/lib/libseed.so.1")
            original = prepare._library
            def library(path):
                return source if Path(path) == source else original(path)
            with patch.object(prepare, "_library", side_effect=library), \
                    patch.object(prepare, "_run", return_value=f"libdrm.so.2 => {external} (0x1)\n"):
                with self.assertRaisesRegex(RuntimeError, "external or invalid"):
                    prepare.runtime_libraries([source])

    def test_excluded_host_graphics_driver_is_not_a_bundled_provider(self):
        source = Path("/usr/lib/libseed.so.1")
        with patch.object(prepare, "_library", side_effect=Path) as library, \
                patch.object(prepare, "_run", return_value="libGL.so.1 => /opt/driver/libGL.so.1 (0x1)\n"):
            self.assertEqual(prepare.runtime_libraries([source]), [source])
        self.assertTrue(all(call.args == (source,) for call in library.call_args_list))

    def test_first_non_native_desktop_cache_entry_cannot_differ_from_packager(self):
        cache = ("libX11.so.6 (libc6,x32) => /libx32/libX11.so.6\n"
                 "libX11.so.6 (libc6,x86-64) => /usr/lib/libX11.so.6\n")
        with self.assertRaisesRegex(RuntimeError, "non-native desktop provider"):
            prepare._cached(cache, "libX11.so.6", packaged=True)

    def test_owner_queries_exact_versioned_file_and_merged_usr_alias(self):
        library = Path("/usr/lib/x86_64-linux-gnu/libsystemd.so.0.32.0")
        alias = Path(str(library).removeprefix("/usr"))
        def run(*args):
            if args[1] == "-S":
                if args[2] == library:
                    raise prepare.CommandError("dpkg-query", 1)
                self.assertEqual(args[2], alias)
                return f"libsystemd0:amd64: {alias}\n"
            return "libsystemd0:amd64 249.11-0ubuntu3.22 systemd 249.11-0ubuntu3.22 installed\n"
        with patch.object(prepare, "_library", return_value=library), patch.object(prepare, "_run", side_effect=run):
            self.assertEqual(prepare._provider(library), provider("libsystemd0:amd64"))

    def test_unowned_ambiguous_and_wrong_file_provider_fail_closed(self):
        library = Path("/usr/lib/libsystemd.so.0.32.0")
        receipts = (f"one:amd64: {library}\ntwo:amd64: {library}\n", "diversion by local\n",
                    "libsystemd0:amd64: /usr/lib/different.so.0\n")
        for receipt in receipts:
            with self.subTest(receipt=receipt), patch.object(prepare, "_library", return_value=library), \
                    patch.object(prepare, "_run", return_value=receipt):
                with self.assertRaises(RuntimeError):
                    prepare._provider(library)
        with patch.object(prepare, "_library", return_value=library), \
                patch.object(prepare, "_run", side_effect=prepare.CommandError("dpkg-query", 1)):
            with self.assertRaisesRegex(RuntimeError, "no Debian package provenance"):
                prepare._provider(library)

    def test_provider_architecture_and_owner_identity_are_required(self):
        for values, owner in ((["libsystemd0:arm64", "1.0", "systemd", "1.0", "installed"], None),
                              (["libsystemd0:amd64", "1.0", "systemd", "1.0", "installed"], "other:amd64"),
                              (["libsystemd0:amd64", "1.0", "systemd", "1.0", "config-files"], None)):
            with self.subTest(values=values), self.assertRaisesRegex(RuntimeError, "invalid installed"):
                prepare._validate_provider(values, owner)

    def test_subprocess_is_read_only_bounded_and_noninteractive(self):
        with patch.object(prepare.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "cache")) as run:
            self.assertEqual(prepare._run("ldconfig", "-p"), "cache")
        self.assertEqual(run.call_args.args[0], ["ldconfig", "-p"])
        self.assertEqual(run.call_args.kwargs["timeout"], 120)
        self.assertEqual(run.call_args.kwargs["stdin"], subprocess.DEVNULL)
        self.assertEqual(run.call_args.kwargs["env"]["LC_ALL"], "C")


if __name__ == "__main__":
    unittest.main()
