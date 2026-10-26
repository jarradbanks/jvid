#!/usr/bin/env python3
"""Retain exact Debian corresponding sources using authenticated APT metadata.

The build host must already have up-to-date, authenticated deb-src indexes.
This module neither updates system configuration nor installs/builds packages.
Original source archives and Debian recipe archives/diffs remain intact; no
downloaded source, maintainer script, or archive path is executed or extracted.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


_NAME = re.compile(r"[a-z0-9][a-z0-9+.-]*\Z")
_BINARY = re.compile(r"[a-z0-9][a-z0-9+.-]*(?::[a-z0-9][a-z0-9-]*)?\Z")
_VERSION = re.compile(r"[0-9][A-Za-z0-9.+:~\-]*\Z")
_FILENAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9.+_~\-]*\Z")
_PREAMBLE = {
    "Corresponding source and build scripts for a published release are provided as",
    "the versioned corresponding-source archive alongside its binary downloads.",
    "CI candidates are not approved release distributions. Publication is gated on",
    "that archive and its reviewed SHA-256; see docs/packaging.md.",
    "jvid source: https://github.com/jarradbanks/jvid",
}
_APT_OPTIONS = [
    "-o", "APT::Get::AllowUnauthenticated=false",
    "-o", "APT::Get::force-yes=false",
    "-o", "Acquire::AllowInsecureRepositories=false",
    "-o", "Acquire::AllowDowngradeToInsecureRepositories=false",
    "-o", "Acquire::https::Verify-Peer=true",
    "-o", "Acquire::https::Verify-Host=true",
    "-o", "Acquire::Retries=2",
    "-o", "Acquire::http::Timeout=60",
    "-o", "Acquire::https::Timeout=60",
    "-o", "APT::Get::Only-Source=true",
    "-o", "APT::Get::Download-Only=true",
    "-o", "APT::Get::Diff-Only=false",
    "-o", "APT::Get::Tar-Only=false",
    "-o", "APT::Get::Dsc-Only=false",
    "-o", "APT::Get::Print-URIs=false",
]


def _regular(path: Path) -> bool:
    return path.is_file() and not path.is_symlink()


def parse_providers(stage: Path) -> list[dict]:
    """Require exact four-column dpkg-query receipts and notice coverage."""
    info = stage / "SOURCE-INFO.txt"
    if not _regular(info):
        raise RuntimeError("missing regular SOURCE-INFO.txt")
    providers = {}
    started = False
    for number, line in enumerate(info.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        if not started and line in _PREAMBLE:
            continue
        columns = line.split()
        if (len(columns) != 4 or not _BINARY.fullmatch(columns[0])
                or not _VERSION.fullmatch(columns[1])
                or not _NAME.fullmatch(columns[2])
                or not _VERSION.fullmatch(columns[3])):
            raise RuntimeError(f"invalid Debian provider at SOURCE-INFO.txt line {number}")
        started = True
        provider = dict(zip(("binary_package", "binary_version", "source_package", "source_version"), columns))
        previous = providers.setdefault(columns[0], provider)
        if previous != provider:
            raise RuntimeError(f"conflicting Debian provider: {columns[0]}")
    if not providers:
        raise RuntimeError("SOURCE-INFO.txt records no Debian providers")
    licenses = stage / "runtime-licenses"
    if not licenses.is_dir() or licenses.is_symlink():
        raise RuntimeError("missing regular runtime-licenses directory")
    expected = {name.split(":", 1)[0] for name in providers}
    observed = set()
    for entry in licenses.iterdir():
        if entry.is_symlink():
            raise RuntimeError("symlink in runtime-licenses")
        if entry.is_dir():
            if not _NAME.fullmatch(entry.name) or not _regular(entry / "copyright"):
                raise RuntimeError("missing regular Debian copyright receipt")
            observed.add(entry.name)
        elif not _regular(entry):
            raise RuntimeError("invalid runtime-licenses entry")
    if observed != expected:
        missing = sorted(expected - observed)
        extra = sorted(observed - expected)
        raise RuntimeError(f"Debian provider coverage mismatch: missing notices={missing}, unrecorded providers={extra}")
    return [providers[name] for name in sorted(providers)]


def _run(arguments: list[str], cwd: Path | None = None) -> str:
    """Bound subprocesses and keep transient/private paths out of receipts/errors."""
    try:
        result = subprocess.run(arguments, cwd=cwd, check=True, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                stdin=subprocess.DEVNULL, timeout=1800,
                                env={**os.environ, "LC_ALL": "C", "DEBIAN_FRONTEND": "noninteractive"})
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"{arguments[0]} timed out collecting exact Debian sources") from error
    except subprocess.CalledProcessError as error:
        raise RuntimeError(f"{arguments[0]} failed collecting exact Debian sources (exit {error.returncode}); "
                           "check authenticated deb-src indexes and the recorded version") from error
    return result.stdout


def _paragraphs(text: str) -> list[dict[str, str]]:
    """Read Debian control stanzas without silently accepting duplicate fields."""
    records = []
    fields = {}
    key = None
    for line in text.splitlines() + [""]:
        if not line:
            if fields:
                records.append(fields)
            fields, key = {}, None
        elif line[0].isspace():
            if key is None:
                raise RuntimeError("invalid Debian control continuation")
            fields[key] += "\n" + line[1:]
        else:
            match = re.fullmatch(r"([A-Za-z0-9-]+):\s*(.*)", line)
            if not match or match[1].lower() in fields:
                raise RuntimeError("invalid or duplicate Debian control field")
            key = match[1].lower()
            fields[key] = match[2]
    return records


def _dsc_fields(text: str) -> dict[str, str]:
    # APT authenticates the .dsc hash through its signed repository metadata;
    # uploader keys need not be installed. Retain the complete signature file.
    lines = text.splitlines()
    if lines and lines[0] == "-----BEGIN PGP SIGNED MESSAGE-----":
        try:
            start = lines.index("") + 1
            end = lines.index("-----BEGIN PGP SIGNATURE-----", start)
        except ValueError as error:
            raise RuntimeError("invalid clearsigned .dsc") from error
        if not lines or lines[-1] != "-----END PGP SIGNATURE-----":
            raise RuntimeError("incomplete clearsigned .dsc")
        lines = [line[2:] if line.startswith("- ") else line for line in lines[start:end]]
    records = _paragraphs("\n".join(lines))
    if len(records) != 1:
        raise RuntimeError("expected one Debian source control record")
    return records[0]


def _checksums(fields: dict[str, str], key: str, digits: int) -> dict[str, tuple[str, int]]:
    result = {}
    for line in fields.get(key, "").splitlines():
        if not line.strip():
            continue
        values = line.split()
        if (len(values) != 3 or not re.fullmatch(rf"[0-9a-fA-F]{{{digits}}}", values[0])
                or not re.fullmatch(r"[0-9]+", values[1])
                or not _FILENAME.fullmatch(values[2]) or values[2] in result):
            raise RuntimeError(f"invalid {key} source file entry")
        result[values[2]] = (values[0].lower(), int(values[1]))
    if not result:
        raise RuntimeError(f"missing {key} source checksums")
    return result


def _check_binary_coverage(fields: dict[str, str], binaries: list[dict]) -> None:
    names = {name.strip() for name in fields.get("binary", "").replace("\n", " ").split(",")}
    if any(binary["binary_package"].split(":", 1)[0] not in names for binary in binaries):
        raise RuntimeError("Debian source Binary field does not cover recorded binary providers")


def _metadata(name: str, version: str, binaries: list[dict]) -> tuple[dict, dict]:
    records = _paragraphs(_run(["apt-cache", "--only-source", "showsrc", name]))
    candidates = [record for record in records
                  if record.get("package") == name and record.get("version") == version]
    if not candidates:
        raise RuntimeError(f"exact APT source metadata unavailable: {name}={version}")
    chosen, checksums = candidates[0], _checksums(candidates[0], "checksums-sha256", 64)
    if {filename.casefold() for filename in checksums} & {"build.txt", "provenance.json"}:
        raise RuntimeError("APT source filename conflicts with collector receipts")
    for candidate in candidates:
        if _checksums(candidate, "checksums-sha256", 64) != checksums:
            raise RuntimeError(f"conflicting APT source metadata: {name}={version}")
        _check_binary_coverage(candidate, binaries)
    return chosen, checksums


def _verify_file(path: Path, expected: tuple[str, int], algorithm: str = "sha256") -> None:
    if not _regular(path):
        raise RuntimeError(f"missing regular Debian source file: {path.name}")
    digest = hashlib.new(algorithm)
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    if path.stat().st_size != expected[1] or digest.hexdigest() != expected[0]:
        raise RuntimeError(f"Debian source checksum mismatch: {path.name} ({algorithm})")


def _verify_download(directory: Path, name: str, version: str, binaries: list[dict],
                     expected: dict[str, tuple[str, int]]) -> tuple[str, dict]:
    if {entry.name for entry in directory.iterdir()} != set(expected):
        raise RuntimeError("downloaded Debian source file set differs from exact APT metadata")
    for filename, checksum in expected.items():
        _verify_file(directory / filename, checksum)
    dscs = [filename for filename in expected if filename.endswith(".dsc")]
    if len(dscs) != 1:
        raise RuntimeError("exact APT metadata must contain one .dsc")
    dsc = _dsc_fields((directory / dscs[0]).read_text(encoding="utf-8"))
    if dsc.get("source") != name or dsc.get("version") != version:
        raise RuntimeError("Debian .dsc source package/version mismatch")
    _check_binary_coverage(dsc, binaries)
    referenced = _checksums(dsc, "checksums-sha256", 64)
    if set(referenced) != set(expected) - set(dscs):
        raise RuntimeError("Debian .dsc file coverage differs from exact APT metadata")
    if any(checksum != expected[filename] for filename, checksum in referenced.items()):
        raise RuntimeError("Debian .dsc checksums differ from exact APT metadata")
    # Verify every listed checksum, including legacy Files/Checksums-Sha1, and
    # reject attempts to omit a referenced archive from the SHA-256 list.
    for field, digits, algorithm in (("checksums-sha256", 64, "sha256"),
                                     ("files", 32, "md5"),
                                     ("checksums-sha1", 40, "sha1"),
                                     ("checksums-sha512", 128, "sha512")):
        if field in ("checksums-sha1", "checksums-sha512") and field not in dsc:
            continue
        entries = _checksums(dsc, field, digits)
        if set(entries) != set(referenced):
            raise RuntimeError(f"Debian .dsc {field} file coverage mismatch")
        for filename, checksum in entries.items():
            _verify_file(directory / filename, checksum, algorithm)
    return dscs[0], dsc


def collect(stage: Path, out: Path) -> list[dict]:
    """Collect all exact source providers or raise without returning partial coverage."""
    stage, out = Path(stage), Path(out)
    binaries = parse_providers(stage)
    if out.is_symlink() or (out.exists() and (not out.is_dir() or any(out.iterdir()))):
        raise RuntimeError("Linux source output must be a fresh empty directory")
    out.mkdir(parents=True, exist_ok=True)
    grouped = {}
    for binary in binaries:
        grouped.setdefault((binary["source_package"], binary["source_version"]), []).append(binary)
    records = []
    for (name, version), provided in sorted(grouped.items()):
        metadata, checksums = _metadata(name, version, provided)
        # Manifest values are validated above; filenames are independently
        # validated before joining any paths. Epoch ':' is not portable.
        identifier = f"debian-{name}-{version.replace(':', '_')}"
        target = out / identifier
        with tempfile.TemporaryDirectory(prefix="jvid-linux-source-") as temporary:
            download = Path(temporary)
            _run(["apt-get", *_APT_OPTIONS, "--assume-yes", "source", "--download-only",
                  "--only-source", f"{name}={version}"], cwd=download)
            dsc_name, dsc = _verify_download(download, name, version, provided, checksums)
            target.mkdir()
            for filename in sorted(checksums):
                shutil.copyfile(download / filename, target / filename)
        # Retain only portable authenticated fields, never apt-cache's local
        # source-index paths or raw command output containing private paths.
        receipt = {
            "source_package": name, "source_version": version,
            "binary_packages": provided, "format": dsc.get("format", ""),
            "authentication": "APT authenticated source download; signed repository metadata SHA-256",
            "apt_source": {key: metadata[key] for key in ("package", "version", "binary", "checksums-sha256")},
            "download": ["apt-get", "source", "--download-only", "--only-source", f"{name}={version}"],
            "files": [{"name": filename, "sha256": checksum[0], "size": checksum[1]}
                      for filename, checksum in sorted(checksums.items())],
        }
        (target / "provenance.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        (target / "BUILD.txt").write_text(
            f"Exact Debian source: {name} {version}\n"
            "All original source payloads and Debian build recipes/patches are retained.\n"
            "Packaging lives in the .debian.tar archive, .diff.gz, or native source archive.\n"
            f"Reconstruct the complete source and debian/ recipes with: dpkg-source -x {dsc_name}\n"
            "Read debian/control and debian/rules in that source tree for build dependencies and steps.\n"
            "A native package build normally uses dpkg-buildpackage -us -uc -b after installing its build dependencies.\n",
            encoding="utf-8")
        records.append({
            "id": identifier, "source": name, "version": version,
            "source_package": name, "source_version": version,
            "binary_packages": provided,
            "provenance": "Exact Debian source from authenticated APT source metadata and download",
            "files": [path.relative_to(out).as_posix() for path in sorted(target.iterdir())],
        })
    covered = [binary for record in records for binary in record["binary_packages"]]
    if sorted(covered, key=lambda item: item["binary_package"]) != binaries:
        raise RuntimeError("incomplete Debian source provider coverage")
    return records
