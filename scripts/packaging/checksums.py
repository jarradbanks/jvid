#!/usr/bin/env python3
from pathlib import Path
import hashlib
import sys

root = Path(sys.argv[1])
files = sorted(p for p in root.iterdir() if p.is_file() and p.name != 'SHA256SUMS.txt')
(root / 'SHA256SUMS.txt').write_text(''.join(hashlib.sha256(p.read_bytes()).hexdigest() + '  ' + p.name + '\n' for p in files))
