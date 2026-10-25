#!/usr/bin/env python3
"""Collect only native payloads from CI artifacts, excluding smoke fixtures/evidence."""
from pathlib import Path
import shutil
import sys

source, dest = map(Path, sys.argv[1:])
dest.mkdir(parents=True, exist_ok=True)
for package in sorted(source.glob('packages-*')):
    for path in sorted(package.iterdir()):
        if path.is_file() and path.name.endswith(('.exe', '.zip', '.dmg', '.tar.gz')):
            if (dest / path.name).exists():
                raise RuntimeError(f'duplicate release asset: {path.name}')
            shutil.copy2(path, dest)
