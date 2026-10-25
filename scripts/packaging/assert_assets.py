#!/usr/bin/env python3
"""Fail before signing or publishing an incomplete native release set."""
from pathlib import Path
import sys
from build import version


def expected_assets():
    prefix = f'jvid-{version()}-'
    return {prefix + name for name in (
        'windows-x86_64-setup.exe', 'windows-x86_64-portable.zip',
        'macos-aarch64.dmg', 'macos-aarch64-portable.zip',
        'linux-x86_64-portable.tar.gz')}


def check(root):
    found = {p.name for p in root.iterdir() if p.is_file()}
    missing = expected_assets() - found
    if missing:
        raise RuntimeError('missing native artifacts: ' + ', '.join(sorted(missing)))
    for name in expected_assets():
        if (root / name).stat().st_size == 0:
            raise RuntimeError('empty native artifact: ' + name)


if __name__ == '__main__':
    check(Path(sys.argv[1]))
