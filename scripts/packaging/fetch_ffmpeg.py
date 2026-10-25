#!/usr/bin/env python3
"""Fetch the reviewed Windows GPL shared runtime, rejecting changed upstream bytes."""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[2]


def fetch(dest):
    lock = json.loads((ROOT / "packaging/ffmpeg-windows.json").read_text())
    with tempfile.TemporaryDirectory() as tmp:
        archive = Path(tmp) / "ffmpeg.zip"
        urllib.request.urlretrieve(lock["url"], archive)
        if hashlib.sha256(archive.read_bytes()).hexdigest() != lock["sha256"]:
            raise RuntimeError("FFmpeg SHA-256 mismatch; review the runtime lock before updating it")
        with zipfile.ZipFile(archive) as z:
            z.extractall(Path(tmp) / "unpacked")
        source, = (Path(tmp) / "unpacked").iterdir()
        shutil.copytree(source, dest)


if __name__ == "__main__":
    fetch(Path(sys.argv[1]))
