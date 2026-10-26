#!/usr/bin/env python3
"""Plan exact-source runtime upgrades before compiling on Ubuntu 22.04 amd64.

Run with --output upgrades.txt, install only the emitted binary package names
from the same authenticated official APT repositories, then run with --check.
This program never installs packages, changes APT configuration, downloads
sources, copies libraries, or patches ELF files. APT_CONFIG is inherited.
"""
import argparse
import os
from pathlib import Path
import re
import subprocess
import sys

import build
import source_linux


FFMPEG = ("libavcodec", "libavformat", "libavutil", "libswscale", "libswresample", "libavfilter")
# Keep in step with build.linux_libraries; the regression test checks these seeds.
DESKTOP = ("libX11.so.6", "libXcursor.so.1", "libXi.so.6", "libXrandr.so.2",
           "libxkbcommon.so.0", "libxkbcommon-x11.so.0", "libwayland-client.so.0",
           "libwayland-cursor.so.0", "libwayland-egl.so.1", "libasound.so.2",
           "libfontconfig.so.1", "libfreetype.so.6", "libdbus-1.so.3")
_FIELDS = ("binary_package", "binary_version", "source_package", "source_version")


class CommandError(RuntimeError):
    def __init__(self, command, returncode):
        self.returncode = returncode
        super().__init__(f"{command} failed inspecting native runtime (exit {returncode})")


def _run(*arguments):
    try:
        return subprocess.run([str(value) for value in arguments], check=True, text=True,
                              stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              timeout=120, env={**os.environ, "LC_ALL": "C"}).stdout
    except subprocess.CalledProcessError as error:
        raise CommandError(arguments[0], error.returncode) from error
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"{arguments[0]} timed out inspecting native runtime") from error


def _library(path):
    """Allow only actual system library files, never a local/external provider."""
    path = Path(path)
    if not path.is_absolute() or ".." in path.parts:
        raise RuntimeError(f"invalid runtime library path: {path}")
    resolved = path.resolve(strict=True)
    if not resolved.is_file() or not any(resolved.is_relative_to(root) for root in (Path("/usr/lib"), Path("/lib"))):
        raise RuntimeError(f"external or invalid runtime library provider: {path}")
    return resolved


def _cached(cache, name, packaged=False):
    # Select the native architecture, not e.g. the runner's x32 libstdc++ entry.
    matches = re.findall(r"^\s*" + re.escape(name) + r" \(([^)]+)\) => (/\S+)$", cache, re.M)
    native = [Path(path) for flags, path in matches if "x86-64" in flags.split(",")]
    if not native:
        raise RuntimeError(f"missing native runtime library: {name}")
    if packaged and matches[0][1] != str(native[0]):
        raise RuntimeError(f"packager would select a non-native desktop provider: {name}")
    _library(native[0])
    return native[0]


def runtime_roots():
    """Seed the linker-selected FFmpeg ABI, whisper's C++ runtime, and dlopen libs."""
    if sys.platform != "linux" or _run("dpkg", "--print-architecture").strip() != "amd64":
        raise RuntimeError("Linux runtime preparation requires native Debian/Ubuntu amd64")
    for name in ("LD_LIBRARY_PATH", "LD_PRELOAD", "LD_AUDIT", "LIBRARY_PATH"):
        if os.environ.get(name):
            raise RuntimeError(f"external runtime/linker search override is unsupported: {name}")
    cache = _run("ldconfig", "-p")
    roots = []
    for name in FFMPEG:
        libdir = _run("pkg-config", "--variable=libdir", name).strip()
        if not libdir or "\n" in libdir:
            raise RuntimeError(f"missing unambiguous pkg-config library directory: {name}")
        linked = Path(libdir) / f"{name}.so"
        resolved = _library(linked)
        abi = re.fullmatch(re.escape(name) + r"\.so\.(\d+)(?:\.\d+)*", resolved.name)
        if not abi or _library(_cached(cache, f"{name}.so.{abi[1]}")) != resolved:
            raise RuntimeError(f"FFmpeg linker/runtime provider mismatch: {name}")
        roots.append(linked)
    roots.append(_cached(cache, "libstdc++.so.6"))
    roots.extend(_cached(cache, name, packaged=True) for name in DESKTOP)
    return roots


def runtime_libraries(roots):
    """Read-only equivalent of build.linux_libraries' non-system ldd traversal."""
    pending = list(roots)
    by_name = {}
    for original in pending:
        source = _library(original)
        name = re.sub(r"(\.so\.\d+)(?:\.\d+)*$", r"\1", source.name)
        previous = by_name.setdefault(name, source)
        if previous != source:
            raise RuntimeError(f"conflicting runtime providers: {name}")
    seen = set()
    while pending:
        original = pending.pop()
        source = _library(original)
        if source in seen:
            continue
        seen.add(source)
        output = _run("ldd", original)
        if "not found" in output or not output.strip():
            raise RuntimeError(f"unresolved runtime dependency: {source.name}")
        for line in output.splitlines():
            match = re.fullmatch(r"\s*(\S+) => (/\S+) \(0x[0-9a-fA-F]+\)\s*", line)
            if match:
                name, path = match.groups()
                if not build.LINUX_SYSTEM.match(name):
                    dependency = _library(path)
                    previous = by_name.setdefault(name, dependency)
                    if previous != dependency:
                        raise RuntimeError(f"conflicting runtime providers: {name}")
                    pending.append(Path(path))
            elif re.fullmatch(r"\s*linux-vdso\.so\.\d+ \(0x[0-9a-fA-F]+\)\s*", line):
                continue
            else:
                loader = re.fullmatch(r"\s*(/\S+) \(0x[0-9a-fA-F]+\)\s*", line)
                if not loader or not build.LINUX_SYSTEM.match(Path(loader[1]).name):
                    raise RuntimeError(f"unrecognized ldd dependency for {source.name}")
                _library(loader[1])
    if not seen:
        raise RuntimeError("no native runtime libraries resolved")
    return sorted(seen)


def _provider(library):
    """Query the owned versioned file, including Ubuntu merged-/usr aliases."""
    library = _library(library)
    candidates = [library]
    if str(library).startswith("/usr/lib/"):
        candidates.append(Path(str(library).removeprefix("/usr")))
    elif str(library).startswith("/lib/"):
        candidates.append(Path("/usr" + str(library)))
    owner = None
    for candidate in candidates:
        try:
            receipt = _run("dpkg-query", "-S", candidate)
        except CommandError as error:
            if error.returncode != 1:
                raise
            continue
        rows = receipt.splitlines()
        if len(rows) != 1:
            raise RuntimeError(f"ambiguous Debian owner for {library.name}")
        package, separator, filename = rows[0].rpartition(": ")
        if not separator or filename != str(candidate) or not source_linux._BINARY.fullmatch(package):
            raise RuntimeError(f"invalid Debian owner for {library.name}")
        owner = package
        break
    if owner is None:
        raise RuntimeError(f"no Debian package provenance for {library.name}")
    values = _run("dpkg-query", "-W",
                  "-f=${binary:Package} ${Version} ${source:Package} ${source:Version} ${db:Status-Status}\n",
                  owner).split()
    return _validate_provider(values, owner)


def _validate_provider(values, owner=None):
    if (len(values) != 5 or values[4] != "installed"
            or not source_linux._BINARY.fullmatch(values[0])
            or not source_linux._VERSION.fullmatch(values[1])
            or not source_linux._NAME.fullmatch(values[2])
            or not source_linux._VERSION.fullmatch(values[3])
            or (":" in values[0] and not values[0].endswith(":amd64"))
            or (owner is not None and ((":" in owner and owner != values[0])
                                      or owner.split(":")[0] != values[0].split(":")[0]))):
        raise RuntimeError("invalid installed Debian runtime provider")
    return dict(zip(_FIELDS, values[:4]))


def runtime_providers():
    print("Resolving native FFmpeg, C++, desktop/audio runtime dependencies", file=sys.stderr)
    roots = runtime_roots()
    libraries = runtime_libraries(roots)
    print(f"Resolved {len(libraries)} native runtime libraries", file=sys.stderr)
    providers = {}
    by_library = {}
    for library in libraries:
        provider = _provider(library)
        by_library[library] = provider
        previous = providers.setdefault(provider["binary_package"], provider)
        if previous != provider:
            raise RuntimeError(f"conflicting installed Debian provider: {provider['binary_package']}")
    for name, root in zip(FFMPEG, roots):
        library = _library(root)
        provider = by_library[library]
        major = library.name.split(".so.")[1].split(".")[0]
        if provider["source_package"] != "ffmpeg" or provider["binary_package"].split(":")[0] != name + major:
            raise RuntimeError(f"unexpected FFmpeg ABI/package provider: {name}")
    if by_library[_library(roots[len(FFMPEG)])]["binary_package"].split(":")[0] != "libstdc++6":
        raise RuntimeError("unexpected libstdc++ runtime provider")
    return [providers[name] for name in sorted(providers)]


def upgrades(providers):
    """Return binary names only for absent exact sources; integrity errors propagate."""
    grouped = {}
    for provider in providers:
        _validate_provider([provider[key] for key in _FIELDS] + ["installed"])
        grouped.setdefault((provider["source_package"], provider["source_version"]), []).append(provider)
    if not grouped:
        raise RuntimeError("no Debian runtime providers resolved")
    result = set()
    for (name, version), binaries in sorted(grouped.items()):
        print(f"Checking exact source {name}={version}", file=sys.stderr)
        try:
            source_linux._metadata(name, version, binaries)
        except RuntimeError as error:
            if str(error) != f"exact APT source metadata unavailable: {name}={version}":
                raise
            names = sorted(binary["binary_package"] for binary in binaries)
            result.update(names)
            print(f"Exact source unavailable; upgrade providers: {' '.join(names)}", file=sys.stderr)
    return sorted(result)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--output", type=Path, help="write sorted binary upgrade names, one per line (may be empty)")
    mode.add_argument("--check", action="store_true", help="require every installed provider's exact source metadata")
    args = parser.parse_args(argv)
    try:
        names = upgrades(runtime_providers())
        if args.check:
            if names:
                raise RuntimeError("exact runtime sources remain unavailable for: " + " ".join(names))
            print("Every native runtime provider has exact source metadata", file=sys.stderr)
        else:
            if args.output.is_symlink() or (args.output.exists() and not args.output.is_file()):
                raise RuntimeError("upgrade output must be a regular file")
            args.output.write_text("".join(name + "\n" for name in names), encoding="utf-8")
            print(f"Planned {len(names)} binary package upgrades", file=sys.stderr)
        return 0
    except (RuntimeError, OSError) as error:
        print(f"Linux runtime preparation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
