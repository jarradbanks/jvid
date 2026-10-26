#!/usr/bin/env python3
"""Verify and join one generated source bundle. Uses only the Python standard library."""
import argparse
import hashlib
import json
from pathlib import Path
import re

PART_LIMIT = 1024 * 1024 * 1024
CHUNK = 1024 * 1024


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise RuntimeError(f"duplicate source manifest field: {key}")
        result[key] = value
    return result


def read_manifest(path):
    if path.is_symlink() or not path.is_file():
        raise RuntimeError("source manifest must be a regular file")
    data = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique_object)
    if not isinstance(data, dict):
        raise RuntimeError("invalid source manifest")
    return data


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(CHUNK), b""):
            value.update(block)
    return value.hexdigest()


def part_paths(manifest_path, manifest):
    if not isinstance(manifest, dict):
        raise RuntimeError("invalid source manifest")
    prefix = manifest_path.name.removesuffix(".json") + ".tar.gz.part"
    parts = manifest.get("parts", [])
    if not isinstance(parts, list) or not parts or len(parts) > 999:
        raise RuntimeError("missing or excessive source archive parts")
    paths = []
    for index, entry in enumerate(parts, 1):
        if (not isinstance(entry, dict) or entry.get("name") != f"{prefix}{index:03}"
                or type(entry.get("size")) is not int
                or not 0 < entry["size"] <= PART_LIMIT
                or not isinstance(entry.get("sha256"), str)
                or not re.fullmatch(r"[0-9a-f]{64}", entry["sha256"])):
            raise RuntimeError("invalid source archive part record")
        path = manifest_path.parent / entry["name"]
        if path.is_symlink() or not path.is_file():
            raise RuntimeError(f"missing source archive part: {entry['name']}")
        paths.append((path, entry))
    return paths


def part_blocks(manifest_path, manifest):
    """Read bounded chunks and verify every byte, including the final gzip trailer."""
    paths = part_paths(manifest_path, manifest)
    if (type(manifest.get("archive_size")) is not int or manifest["archive_size"] <= 0
            or not isinstance(manifest.get("archive_sha256"), str)
            or not re.fullmatch(r"[0-9a-f]{64}", manifest["archive_sha256"])):
        raise RuntimeError("invalid combined source archive record")
    archive_hash = hashlib.sha256()
    total = 0
    for path, entry in paths:
        part_hash = hashlib.sha256()
        size = 0
        with path.open("rb") as source:
            for block in iter(lambda: source.read(CHUNK), b""):
                part_hash.update(block)
                archive_hash.update(block)
                size += len(block)
                if size > entry["size"]:
                    raise RuntimeError(f"source part size mismatch: {path.name}")
                yield block
        if size != entry["size"] or part_hash.hexdigest() != entry["sha256"]:
            raise RuntimeError(f"source part checksum mismatch: {path.name}")
        total += size
    if (archive_hash.hexdigest() != manifest.get("archive_sha256")
            or total != manifest.get("archive_size")):
        raise RuntimeError("combined source archive checksum mismatch")


def verify_parts(manifest_path, manifest, output=None):
    for block in part_blocks(manifest_path, manifest):
        if output is not None:
            output.write(block)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("output", type=Path, help="new .tar.gz file to create")
    args = parser.parse_args()
    manifest = read_manifest(args.manifest)
    # Exclusive create: never truncate a source part or an existing user file.
    with args.output.open("xb") as output:
        try:
            verify_parts(args.manifest, manifest, output)
        except BaseException:
            output.close()
            args.output.unlink()
            raise
    print(f"Verified source archive: {args.output}")


if __name__ == "__main__":
    main()
