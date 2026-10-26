#!/usr/bin/env python3
"""Recreate rav1e's vendored Cargo source directory from retained .crate archives.

Run from the source bundle: python source_windows_rav1e.py <rav1e-source-tree>.
The original source/lock are in dependency-source-cache.zip/cache.tar.gz. This
helper installs the separately labeled reconstructed build lock and writes Cargo's
source replacement config. It neither invokes Cargo nor compiles anything.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import sys
import tarfile

CHUNK = 1024 * 1024


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        while block := stream.read(CHUNK):
            digest.update(block)
    return digest.hexdigest()


def prepare(bundle: Path, source: Path):
    manifest = json.loads((bundle / 'source-windows-rav1e.json').read_text(encoding='utf-8'))
    vendor = source / 'vendor'
    vendor.mkdir()
    for item in manifest['crates']:
        if (not re.fullmatch(r'[A-Za-z0-9_-]+', item['name'])
                or not re.fullmatch(r'[A-Za-z0-9.+-]+', item['version'])):
            raise RuntimeError('unsafe rav1e crate identity')
        identity = item['name'] + '-' + item['version']
        archive = bundle / 'rav1e-crates' / (identity + '.crate')
        if sha256(archive) != item['sha256']:
            raise RuntimeError('rav1e crate source SHA-256 mismatch')
        package = vendor / identity
        package.mkdir()
        checksums = {}
        expanded = 0
        with tarfile.open(archive, 'r|gz') as tar:
            for member in tar:
                tar.members.clear()
                path = PurePosixPath(member.name)
                if (path.is_absolute() or '..' in path.parts or '\\' in member.name
                        or ':' in member.name or not path.parts or path.parts[0] != identity):
                    raise RuntimeError('unsafe rav1e crate source path')
                if member.isdir():
                    continue
                expanded += member.size
                if not member.isfile() or expanded > 512 * CHUNK:
                    raise RuntimeError('invalid rav1e crate source entry')
                relative = PurePosixPath(*path.parts[1:]).as_posix()
                if relative in checksums or relative == '.cargo-checksum.json':
                    raise RuntimeError('duplicate rav1e crate source entry')
                dest = package / relative
                dest.parent.mkdir(parents=True, exist_ok=True)
                digest = hashlib.sha256()
                with tar.extractfile(member) as content, dest.open('wb') as output:
                    while block := content.read(CHUNK):
                        digest.update(block)
                        output.write(block)
                checksums[relative] = digest.hexdigest()
        if 'Cargo.toml' not in checksums:
            raise RuntimeError('crate archive lacks source recipe')
        (package / '.cargo-checksum.json').write_text(json.dumps({
            'files': checksums, 'package': item['sha256']}, sort_keys=True) + '\n', encoding='utf-8')
    shutil.copyfile(bundle / 'source-windows-rav1e-Cargo.lock', source / 'Cargo.lock')
    config = source / '.cargo'
    config.mkdir(exist_ok=True)
    (config / 'config.toml').write_text(
        '[source.crates-io]\nreplace-with = "vendored-sources"\n'
        '[source.vendored-sources]\ndirectory = "vendor"\n'
        '[net]\noffline = true\n', encoding='utf-8')


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit('usage: python source_windows_rav1e.py <rav1e-source-tree>')
    prepare(Path(__file__).resolve().parent, Path(sys.argv[1]))
