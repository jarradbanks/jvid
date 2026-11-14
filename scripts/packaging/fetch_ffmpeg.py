#!/usr/bin/env python3
"""Fetch the reviewed Windows GPL shared runtime, rejecting changed upstream bytes."""
import json
from pathlib import Path
import shutil
import sys
import tempfile
import zipfile

from source_windows import download

ROOT = Path(__file__).resolve().parents[2]


def fetch(dest):
    lock = json.loads((ROOT / "packaging/ffmpeg-windows.json").read_text())
    with tempfile.TemporaryDirectory() as tmp:
        archive = Path(tmp) / "ffmpeg.zip"
        download(lock.get("retained_url", lock["url"]), archive, lock["sha256"], 512 * 1024 * 1024)
        with zipfile.ZipFile(archive) as z:
            z.extractall(Path(tmp) / "unpacked")
        source, = (Path(tmp) / "unpacked").iterdir()
        shutil.copytree(source, dest)


if __name__ == "__main__":
    fetch(Path(sys.argv[1]))
