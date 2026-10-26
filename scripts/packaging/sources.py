#!/usr/bin/env python3
"""Capture exact source inputs with each package; verify them before release signing."""
import argparse
import gzip
import hashlib
import importlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import tempfile
import zlib

from build import ROOT, TARGETS, version
from assert_assets import expected_assets
from reassemble_sources import CHUNK, PART_LIMIT, digest, part_blocks, read_manifest, unique_object

BUNDLES = ("rust", *TARGETS)


def build_configuration(target=None):
    """Only public, release-relevant inputs; never serialize the process environment."""
    key = os.environ.get("JVID_UPDATE_PUBLIC_KEY", "").strip()
    if key and not re.fullmatch(r"[0-9a-fA-F]{64}", key):
        raise RuntimeError("JVID_UPDATE_PUBLIC_KEY must be empty or 64 hex characters")
    for name in ("GGML_NATIVE", "GGML_METAL"):
        if os.environ.get(name, "OFF") != "OFF":
            raise RuntimeError(f"{name} does not match the release build configuration")
    windows_flags = "-C target-feature=+crt-static"
    if target and target.startswith("windows") and os.environ.get("RUSTFLAGS", windows_flags) != windows_flags:
        raise RuntimeError("Windows RUSTFLAGS do not match the release CRT configuration")
    return {"workspace_version": version(), "JVID_UPDATE_PUBLIC_KEY": key.lower(),
            "GGML_NATIVE": "OFF", "GGML_METAL": "OFF",
            "windows_RUSTFLAGS": windows_flags, "windows_CARGO_BUILD_TARGET": "x86_64-pc-windows-msvc",
            "provenance": [".github/workflows/package.yml", ".cargo/config.toml"]}


def run(*args, cwd=ROOT, **kwargs):
    return subprocess.check_output([str(a) for a in args], cwd=cwd, **kwargs)


def checkout():
    # Only tracked release code is exported; don't accidentally archive local edits
    # or an owner's keys, caches, untracked projects or audit artifacts.
    subprocess.run(["git", "diff", "--exit-code", "HEAD", "--"], cwd=ROOT, check=True,
                   stdout=subprocess.DEVNULL)
    return {"commit": run("git", "rev-parse", "HEAD", text=True).strip(),
            "tree": run("git", "rev-parse", "HEAD^{tree}", text=True).strip()}


def file_record(path):
    return {"size": path.stat().st_size, "sha256": digest(path)}


def safe_relative(name):
    if (not isinstance(name, str) or not name or "\\" in name or ":" in name
            or any(ord(c) < 32 for c in name)
            or any(p in ("", ".", "..") for p in name.split("/"))):
        raise RuntimeError(f"unsafe source path: {name}")
    path = PurePosixPath(name)
    if path.is_absolute():
        raise RuntimeError(f"unsafe source path: {name}")
    return path


def safe_link(name, target):
    if (not isinstance(target, str) or not target or "\\" in target or ":" in target
            or any(ord(c) < 32 for c in target) or PurePosixPath(target).is_absolute()):
        raise RuntimeError(f"source link escapes archive: {name}")
    parent = list(safe_relative(name).parts[:-1])
    for part in target.split("/"):
        if part == "..":
            if not parent:
                raise RuntimeError(f"source link escapes archive: {name}")
            parent.pop()
        elif part not in ("", "."):
            parent.append(part)


def validate_inventory(files, providers):
    if not isinstance(files, dict) or not files:
        raise RuntimeError("source tree is empty")
    for name, record in files.items():
        safe_relative(name)
        if any(parent.as_posix() in files for parent in PurePosixPath(name).parents):
            raise RuntimeError(f"source inventory path traverses a file or link: {name}")
        if name == "SOURCE-MANIFEST.json" or not isinstance(record, dict):
            raise RuntimeError("invalid source inventory record")
        if set(record) == {"link"}:
            safe_link(name, record["link"])
        elif (set(record) != {"size", "sha256"} or type(record.get("size")) is not int
              or record["size"] < 0 or not isinstance(record.get("sha256"), str)
              or not re.fullmatch(r"[0-9a-f]{64}", record["sha256"])):
            raise RuntimeError("invalid source inventory record")
    for name, record in files.items():
        if "link" not in record:
            continue
        # Resolve chains as an extractor would: a/../ can escape after following a.
        pending = list(PurePosixPath(name).parts[:-1]) + record["link"].split("/")
        resolved, followed = [], 0
        while pending:
            part = pending.pop(0)
            if part in ("", "."):
                continue
            if part == "..":
                if not resolved:
                    raise RuntimeError(f"source link escapes archive: {name}")
                resolved.pop()
                continue
            resolved.append(part)
            linked = files.get("/".join(resolved), {})
            if "link" in linked:
                followed += 1
                if followed > 40:
                    raise RuntimeError(f"cyclic or excessive source links: {name}")
                resolved.pop()
                pending = linked["link"].split("/") + pending
    if not isinstance(providers, list) or not providers:
        raise RuntimeError("missing source bundle providers")
    identities = set()
    for provider in providers:
        if (not isinstance(provider, dict) or not isinstance(provider.get("id"), str)
                or not provider["id"] or provider["id"] in identities):
            raise RuntimeError("missing or duplicate source provider identity")
        identities.add(provider["id"])
        names = provider.get("files")
        if not isinstance(names, list) or not names:
            raise RuntimeError(f"source provider has no retained files: {provider['id']}")
        retained = set()
        for name in names:
            safe_relative(name)
            if name in retained or name not in files or "link" in files[name]:
                raise RuntimeError(f"missing or invalid source provider input: {name}")
            retained.add(name)


def source_inventory(root):
    result = {}
    for path in sorted(root.rglob("*")):
        name = path.relative_to(root).as_posix()
        safe_relative(name)
        if path.is_symlink():
            safe_link(name, path.readlink().as_posix())
            if not path.resolve().is_relative_to(root.resolve()):
                raise RuntimeError(f"source link escapes archive: {name}")
            result[name] = {"link": path.readlink().as_posix()}
        elif path.is_file():
            result[name] = file_record(path)
        elif not path.is_dir():
            raise RuntimeError(f"unsupported source file: {name}")
    if not result:
        raise RuntimeError("source tree is empty")
    return result


class PartReader:
    """Expose verified multipart chunks to gzip/tar without joining them on disk."""
    def __init__(self, path, data):
        self.blocks = part_blocks(path, data)
        self.pending = memoryview(b"")

    def read(self, size):
        result = bytearray()
        while len(result) < size:
            if not self.pending:
                self.pending = memoryview(next(self.blocks, b""))
                if not self.pending:
                    break
            count = min(size - len(result), len(self.pending))
            result.extend(self.pending[:count])
            self.pending = self.pending[count:]
        return bytes(result)

    def close(self):
        self.blocks.close()


def verify_archive(path, data):
    """Bind the outer manifest to the embedded inventory and the actual tar bytes."""
    validate_inventory(data.get("files"), data.get("providers"))
    inner_expected = {k: v for k, v in data.items()
                      if k not in ("parts", "archive_size", "archive_sha256")}
    reader = PartReader(path, data)
    seen, observed = set(), {}
    try:
        with gzip.GzipFile(fileobj=reader, mode="rb") as compressed:
            # One tar block avoids consuming bytes beyond the archive terminator.
            with tarfile.open(fileobj=compressed, mode="r|", bufsize=512) as archive:
                for member in archive:
                    name = member.name
                    safe_relative(name)
                    if name in seen:
                        raise RuntimeError(f"duplicate source archive member: {name}")
                    seen.add(name)
                    if not (member.isfile() or member.isdir() or member.issym()):
                        raise RuntimeError(f"unsupported source archive member: {name}")
                    if name == "SOURCE-MANIFEST.json":
                        if (len(seen) != 1 or not member.isfile()
                                or member.size > 64 * 1024 * 1024):
                            raise RuntimeError("invalid embedded source manifest")
                        embedded = json.load(archive.extractfile(member), object_pairs_hook=unique_object)
                        if embedded != inner_expected:
                            raise RuntimeError("embedded source manifest mismatch")
                        continue
                    if "SOURCE-MANIFEST.json" not in seen:
                        raise RuntimeError("missing embedded source manifest")
                    # A source symlink must never become a parent directory.
                    if any("link" in data["files"].get(p.as_posix(), {})
                           for p in PurePosixPath(name).parents if p != PurePosixPath(".")):
                        raise RuntimeError(f"source archive member traverses a link: {name}")
                    if member.isdir():
                        if name in data["files"]:
                            raise RuntimeError(f"source inventory type mismatch: {name}")
                        continue
                    expected = data["files"].get(name)
                    if member.issym():
                        safe_link(name, member.linkname)
                        record = {"link": member.linkname}
                    else:
                        if not expected or expected.get("size") != member.size:
                            raise RuntimeError(f"source inventory size mismatch: {name}")
                        value = hashlib.sha256()
                        stream = archive.extractfile(member)
                        if name == "BUILD-CONFIG.json":
                            if member.size > CHUNK:
                                raise RuntimeError("invalid public build configuration")
                            configuration = stream.read()
                            if json.loads(configuration, object_pairs_hook=unique_object) != data.get("build_configuration"):
                                raise RuntimeError("public build configuration mismatch")
                            value.update(configuration)
                        else:
                            for block in iter(lambda: stream.read(CHUNK), b""):
                                value.update(block)
                        record = {"size": member.size, "sha256": value.hexdigest()}
                    if record != expected:
                        raise RuntimeError(f"source inventory checksum or link mismatch: {name}")
                    observed[name] = record
            # Check gzip CRC/footer and reject a hidden second tar after the terminator.
            for block in iter(lambda: compressed.read(CHUNK), b""):
                if block.strip(b"\0"):
                    raise RuntimeError("unexpected data after source archive")
        while reader.read(CHUNK):
            pass
    except (tarfile.TarError, gzip.BadGzipFile, EOFError, zlib.error, ValueError) as error:
        raise RuntimeError("invalid source archive or embedded manifest") from error
    finally:
        reader.close()
    if "SOURCE-MANIFEST.json" not in seen or observed != data["files"]:
        raise RuntimeError("source archive inventory mismatch")


def native_assets(dist, target):
    names = {n for n in expected_assets() if f"-{target}" in n}
    if not names:
        raise RuntimeError(f"unknown native source target: {target}")
    result = {}
    for name in sorted(names):
        path = dist / name
        if not path.is_file() or path.is_symlink() or path.stat().st_size == 0:
            raise RuntimeError(f"missing native payload for source binding: {name}")
        result[name] = file_record(path)
    return result


class PartWriter:
    """Streaming tar/gzip sink, with each uploaded asset kept below GitHub's limit."""
    def __init__(self, output, prefix, limit=PART_LIMIT):
        if not 0 < limit <= PART_LIMIT:
            raise ValueError("invalid archive part limit")
        self.output, self.prefix, self.limit = output, prefix, limit
        self.parts, self.file = [], None
        self.size = self.total = 0
        self.part_hash, self.archive_hash = hashlib.sha256(), hashlib.sha256()

    def write(self, data):
        original = len(data)
        data = memoryview(data)
        while data:
            if self.file is None:
                if len(self.parts) >= 999:
                    raise RuntimeError("source archive requires too many parts")
                name = f"{self.prefix}.tar.gz.part{len(self.parts) + 1:03}"
                self.file = (self.output / name).open("xb")
                self.name, self.size, self.part_hash = name, 0, hashlib.sha256()
            block = data[:self.limit - self.size]
            self.file.write(block)
            self.part_hash.update(block)
            self.archive_hash.update(block)
            self.size += len(block)
            self.total += len(block)
            data = data[len(block):]
            if self.size == self.limit:
                self.finish_part()
        return original

    def flush(self):
        if self.file is not None:
            self.file.flush()

    def finish_part(self):
        if self.file is not None:
            self.file.close()
            self.parts.append({"name": self.name, "size": self.size,
                               "sha256": self.part_hash.hexdigest()})
            self.file = None


def bundle(root, output, target, providers, assets, revision, limit=PART_LIMIT):
    if target not in BUNDLES or not providers:
        raise RuntimeError("missing source bundle providers or unknown target")
    if output.resolve().is_relative_to(root.resolve()):
        raise RuntimeError("source bundle output must be outside the source tree")
    if (root / "SOURCE-MANIFEST.json").exists() or (root / "SOURCE-MANIFEST.json").is_symlink():
        raise RuntimeError("reserved source manifest already exists")
    configuration = build_configuration(target)
    with (root / "BUILD-CONFIG.json").open("x", encoding="utf-8") as stream:
        json.dump(configuration, stream, indent=2)
        stream.write("\n")
    files = source_inventory(root)
    validate_inventory(files, providers)
    metadata = {"schema": 1, "repository": "jarradbanks/jvid", "version": version(),
                "bundle": target, **revision, "providers": providers,
                "build_configuration": configuration, "binary_assets": assets, "files": files}
    (root / "SOURCE-MANIFEST.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    output.mkdir(parents=True, exist_ok=True)
    prefix = f"jvid-{version()}-sources-{target}"
    writer = PartWriter(output, prefix, limit)
    try:
        with gzip.GzipFile(filename="", mode="wb", fileobj=writer, mtime=0, compresslevel=1) as compressed:
            with tarfile.open(mode="w|", fileobj=compressed) as archive:
                # Put metadata first so readers can inspect it without unpacking sources.
                paths = [root / "SOURCE-MANIFEST.json"] + [p for p in sorted(root.rglob("*"))
                                                          if p != root / "SOURCE-MANIFEST.json"]
                for path in paths:
                    info = archive.gettarinfo(str(path), arcname=path.relative_to(root).as_posix())
                    info.uid = info.gid = info.mtime = 0
                    info.uname = info.gname = ""
                    # Materialize hardlinked files so inventory entries always bind bytes.
                    if info.islnk():
                        info.type, info.linkname = tarfile.REGTYPE, ""
                        info.size = path.stat().st_size
                    if info.isfile():
                        with path.open("rb") as stream:
                            archive.addfile(info, stream)
                    else:
                        archive.addfile(info)
        writer.finish_part()
    finally:
        if writer.file is not None:
            writer.file.close()
    metadata.update(parts=writer.parts, archive_size=writer.total,
                    archive_sha256=writer.archive_hash.hexdigest())
    manifest = output / f"{prefix}.json"
    with manifest.open("x", encoding="utf-8") as stream:
        json.dump(metadata, stream, indent=2)
        stream.write("\n")
    return manifest


def collect_native(target, dist, output):
    revision = checkout()
    stage = dist / "staging" / target / f"jvid-{version()}-{target}"
    if target.startswith("linux"):
        stage /= "bin"
    elif target.startswith("macos"):
        stage /= "jvid.app/Contents/Resources"
    if not (stage / "SOURCE-INFO.txt").is_file():
        raise RuntimeError(f"missing staged package provenance: {stage}")
    module = importlib.import_module("source_" + target.split("-")[0])
    with tempfile.TemporaryDirectory(prefix="jvid-native-sources-") as temp:
        source = Path(temp)
        providers = module.collect(stage, source)
        shutil.copy2(stage / "SOURCE-INFO.txt", source / "PACKAGE-SOURCE-INFO.txt")
        shutil.copytree(stage / "runtime-licenses", source / "package-provenance", dirs_exist_ok=True)
        bundle(source, output, target, providers, native_assets(dist, target), revision)


def collect_rust(output):
    revision = checkout()
    with tempfile.TemporaryDirectory(prefix="jvid-workspace-sources-") as temp:
        source = Path(temp)
        workspace = source / "workspace"
        workspace.mkdir()
        archive_path = source / "checkout.tar"
        with archive_path.open("wb") as out:
            subprocess.run(["git", "archive", "--format=tar", revision["commit"]],
                           cwd=ROOT, stdout=out, check=True)
        with tarfile.open(archive_path) as archive:
            for member in archive:
                safe_relative(member.name.rstrip("/"))
                if member.issym():
                    resolved = (workspace / member.name).parent / member.linkname
                    if not resolved.resolve().is_relative_to(workspace.resolve()):
                        raise RuntimeError("Git source link escapes checkout")
                if not (member.isfile() or member.isdir() or member.issym()):
                    raise RuntimeError("unsupported Git archive member")
            archive.extractall(workspace, filter="data")
        archive_path.unlink()
        config = run("cargo", "vendor", "--locked", "--versioned-dirs", "vendor",
                     cwd=workspace, text=True)
        config_path = workspace / ".cargo/config.toml"
        config_path.parent.mkdir(exist_ok=True)
        with config_path.open("a", encoding="utf-8") as out:
            out.write("\n" + config)
        # An empty Cargo home proves this export doesn't depend on the runner's registry cache.
        env = os.environ.copy()
        env["CARGO_HOME"] = str(source / "offline-cargo-home")
        subprocess.run(["cargo", "metadata", "--locked", "--offline", "--format-version=1"],
                       cwd=workspace, env=env, stdout=subprocess.DEVNULL, check=True)
        shutil.rmtree(source / "offline-cargo-home", ignore_errors=True)
        (source / "REBUILD.txt").write_text(
            "workspace/ contains the exact jvid release checkout, Cargo.lock, assets, scripts,\n"
            "and vendored crates (including their embedded native sources). Its Cargo config\n"
            "uses these local sources; offline cargo metadata was verified with an empty\n"
            "Cargo home. Install the pinned Rust toolchain and native build tools described\n"
            "in workspace/docs/packaging.md. Native source archives accompany this bundle.\n"
            "Native toolchains/OS SDKs remain external build prerequisites.\n", encoding="utf-8")
        providers = [{"id": "jvid-workspace-and-cargo", "offline_metadata_checked": True,
                      "files": ["workspace/Cargo.lock", "workspace/.cargo/config.toml", "REBUILD.txt"]}]
        bundle(source, output, "rust", providers, {}, revision)


def collect_release(source_artifacts, native, destination, verify_only=False):
    """Fail before metadata signing unless all four source bundles match this checkout/payload."""
    revision = checkout()
    configuration = build_configuration()
    manifests = sorted(source_artifacts.rglob("jvid-*-sources-*.json"))
    found, assets, to_copy, summaries = set(), {}, {}, []
    for path in manifests:
        if path.is_symlink():
            raise RuntimeError("source manifest must be a regular file")
        data = read_manifest(path)
        target = data.get("bundle")
        if (data.get("schema") != 1 or data.get("repository") != "jarradbanks/jvid"
                or data.get("version") != version() or data.get("commit") != revision["commit"]
                or data.get("tree") != revision["tree"] or target not in BUNDLES or target in found
                or data.get("build_configuration") != configuration
                or path.name != f"jvid-{version()}-sources-{target}.json"
                or not data.get("providers") or not data.get("files")):
            raise RuntimeError(f"missing, duplicate or mismatched source manifest: {path.name}")
        found.add(target)
        verify_archive(path, data)
        if target == "rust":
            if len(data["providers"]) != 1:
                raise RuntimeError("Rust source bundle must have one offline-verified provider")
            provider = data["providers"][0]
            if (provider.get("offline_metadata_checked") is not True
                    or data.get("binary_assets") != {}):
                raise RuntimeError("Rust source bundle was not verified offline")
        else:
            expected = native_assets(native, target)
            if data.get("binary_assets") != expected:
                raise RuntimeError(f"source bundle does not match native package bytes: {target}")
            assets.update(expected)
        for file in [path] + [path.parent / part["name"] for part in data["parts"]]:
            if file.name in to_copy:
                raise RuntimeError(f"duplicate source asset: {file.name}")
            to_copy[file.name] = file
        summaries.append({"bundle": target, "manifest": path.name, "sha256": digest(path),
                          "archive_sha256": data["archive_sha256"], "parts": data["parts"]})
    if found != set(BUNDLES) or set(assets) != expected_assets():
        raise RuntimeError("missing source bundles for the complete native release contract")
    index = {"schema": 1, "repository": "jarradbanks/jvid", "version": version(), **revision,
             "build_configuration": configuration, "binary_assets": assets,
             "bundles": sorted(summaries, key=lambda b: b["bundle"])}
    if verify_only:
        recorded = read_manifest(destination / f"jvid-{version()}-source-manifest.json")
        if recorded != index:
            raise RuntimeError("release source index mismatch")
        helper = destination / "reassemble_sources.py"
        if helper.is_symlink() or digest(helper) != digest(ROOT / "scripts/packaging/reassemble_sources.py"):
            raise RuntimeError("source reconstruction helper mismatch")
        # Also check the published copies when --sources points at original artifacts.
        for name, path in to_copy.items():
            staged = destination / name
            if staged.is_symlink() or not staged.is_file():
                raise RuntimeError(f"missing staged source asset: {name}")
            if staged.resolve() != path.resolve() and file_record(staged) != file_record(path):
                raise RuntimeError(f"staged source asset mismatch: {name}")
        return
    destination.mkdir(parents=True, exist_ok=True)
    index_name = f"jvid-{version()}-source-manifest.json"
    for name in {*to_copy, "reassemble_sources.py", index_name, "SOURCE-README.txt"}:
        if (destination / name).exists() or (destination / name).is_symlink():
            raise RuntimeError(f"source asset already exists: {name}")
    to_copy["reassemble_sources.py"] = ROOT / "scripts/packaging/reassemble_sources.py"
    for name, path in sorted(to_copy.items()):
        with path.open("rb") as source, (destination / name).open("xb") as output:
            shutil.copyfileobj(source, output, length=CHUNK)
    with (destination / index_name).open("x", encoding="utf-8") as stream:
        stream.write(json.dumps(index, indent=2) + "\n")
    with (destination / "SOURCE-README.txt").open("x", encoding="utf-8") as stream:
        stream.write(
            "The source-manifest records the exact release commit, binary checksums and four\n"
            "source bundles. Keep each bundle's .json and all .tar.gz.partNNN files together.\n"
            "Verify SHA256SUMS.txt, then join any bundle using Python 3:\n"
            "  python reassemble_sources.py jvid-VERSION-sources-BUNDLE.json sources.tar.gz\n"
            "This checks every part and the full archive checksum without overwriting files.\n"
            "Extract the resulting sources.tar.gz with your archive utility. The Rust bundle\n"
            "contains workspace/, Cargo vendor configuration and build/release documentation.\n"
            "BUILD-CONFIG.json retains public compile-time configuration, including the updater\n"
            "public key and CPU/CRT flags; private keys and tokens are not part of that file.\n"
            "Native bundles contain provider sources, patches, recipes and provenance.\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    native = sub.add_parser("native")
    native.add_argument("--target", choices=TARGETS, required=True)
    native.add_argument("--dist", type=Path, default=ROOT / "dist")
    native.add_argument("--output", type=Path, required=True)
    rust = sub.add_parser("rust")
    rust.add_argument("--output", type=Path, required=True)
    release = sub.add_parser("release")
    release.add_argument("--sources", type=Path, required=True)
    release.add_argument("--native", type=Path, required=True)
    release.add_argument("--output", type=Path, required=True)
    release.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    if args.command == "native":
        collect_native(args.target, args.dist.resolve(), args.output.resolve())
    elif args.command == "rust":
        collect_rust(args.output.resolve())
    else:
        collect_release(args.sources.resolve(), args.native.resolve(), args.output.resolve(), args.verify_only)


if __name__ == "__main__":
    main()
