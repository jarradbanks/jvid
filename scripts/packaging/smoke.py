#!/usr/bin/env python3
"""Extract and exercise shipped artifacts with a development-free PATH.
On disposable Windows CI users only, --windows-install checks setup, upgrade and uninstall.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile

from build import sha256


def extract(archive, dest):
    if archive.name.endswith(".zip"):
        with zipfile.ZipFile(archive) as source:
            for member in source.infolist():
                path = dest / member.filename
                if not path.resolve().is_relative_to(dest.resolve()):
                    raise RuntimeError("archive path escapes destination")
            source.extractall(dest)
    else:
        with tarfile.open(archive) as source:
            for member in source.getmembers():
                path = dest / member.name
                if not path.resolve().is_relative_to(dest.resolve()) or member.issym() or member.islnk():
                    raise RuntimeError("unsafe archive member")
            source.extractall(dest)


def verify_inventory(root):
    inventory = next(root.rglob("package-files.json"))
    entries = json.loads(inventory.read_text())["files"]
    for name, expected in entries.items():
        path = inventory.parent / name
        if not path.resolve().is_relative_to(inventory.parent.resolve()):
            raise RuntimeError("inventory path escapes package")
        if path.stat().st_size != expected["size"] or sha256(path) != expected["sha256"]:
            raise RuntimeError(f"package inventory mismatch: {name}")
    if any(p.name == "jvid-data" for p in root.rglob("*")):
        raise RuntimeError("package must not include user data")


def clean_env(tmp):
    env = os.environ.copy()
    # Windows Known Folders use the real profile: --windows-install must run only on
    # a disposable CI user. Unix profiles are isolated from the developer's account.
    env.update(HOME=str(tmp / "home"), XDG_CONFIG_HOME=str(tmp / "config"),
               XDG_DATA_HOME=str(tmp / "data"), XDG_CACHE_HOME=str(tmp / "cache"))
    env["PATH"] = (str(Path(os.environ.get("SystemRoot", "C:/Windows")) / "System32")
                   if sys.platform == "win32" else "/usr/bin:/bin")
    for key in ("LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "FFMPEG_DIR", "LIBCLANG_PATH"):
        env.pop(key, None)
    return env


def exercise(binary, scratch, env):
    subprocess.run([str(binary), "--smoke-test", str(scratch)], env=env, check=True, timeout=120)
    report = (scratch / "smoke-report.txt").read_text()
    if "import_decode_export=passed" not in report:
        raise RuntimeError("missing media smoke evidence")
    return report


def windows_install(installer, tmp, env):
    install = tmp / "installed path with spaces"
    args = [str(installer), "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", f"/DIR={install}"]
    subprocess.run(args, env=env, check=True, timeout=120)
    report = exercise(install / "jvid-app.exe", tmp / "installed-smoke", env)
    if "portable=false" not in report:
        raise RuntimeError("installer selected portable mode")
    paths = dict(line.split("=", 1) for line in report.splitlines() if "=" in line)
    settings = Path(paths["config"])
    sentinel = Path(paths["models"]) / "upgrade-preserved.txt"
    settings.parent.mkdir(parents=True, exist_ok=True)
    settings.write_text('theme = "light"\n# preserved across upgrades\n')
    sentinel.write_bytes(b"existing downloaded model sentinel")
    # Also keep a user file at the install root. No wildcard uninstall deletion allowed.
    user_file = install / "my-project.jvid"
    user_file.write_bytes(b"user project sentinel")
    # Build a numerically newer installer from identical payload to exercise real Inno
    # upgrade registration, directory reuse, overwrite, and cumulative uninstall logs.
    from build import ROOT, run, version
    newer = version().split(".")
    newer[-1] = str(int(newer[-1]) + 1)
    newer = ".".join(newer)
    stage = next((installer.parent / "staging/windows-x86_64").iterdir())
    # Staging ends as portable; give the upgrade installer an installed marker.
    from build import marker
    marker(stage, "windows-installer", False)
    compiler = r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
    run(compiler, f"/DAppVersion={newer}", f"/DStageDir={stage}", f"/DOutputDir={tmp}", ROOT / "packaging/windows.iss")
    upgrade = tmp / f"jvid-{newer}-windows-x86_64-setup.exe"
    subprocess.run([str(upgrade), "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"], env=env, check=True, timeout=120)
    exercise(install / "jvid-app.exe", tmp / "upgrade-smoke", env)
    subprocess.run([str(install / "unins000.exe"), "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"], env=env, check=True, timeout=120)
    if (install / "jvid-app.exe").exists():
        raise RuntimeError("uninstall left program executable")
    assert settings.read_text() == 'theme = "light"\n# preserved across upgrades\n'
    assert sentinel.read_bytes() == b"existing downloaded model sentinel"
    assert user_file.read_bytes() == b"user project sentinel"
    print("Windows install, media, upgrade, uninstall and user-data preservation passed")


def smoke(archive, evidence, installer=None):
    evidence.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="jvid-package-smoke-") as temp:
        tmp = Path(temp)
        env = clean_env(tmp)
        extracted = tmp / "extracted path with spaces"
        extract(archive, extracted)
        verify_inventory(extracted)
        binaries = list(extracted.rglob("jvid-app.exe" if sys.platform == "win32" else "jvid-app"))
        binary, = binaries
        if sys.platform != "win32":
            binary.chmod(0o755)
        report = exercise(binary, evidence / "portable", env)
        package_root = binary.parent
        if package_root.name == "MacOS":
            package_root = package_root.parents[2]
        elif package_root.name == "bin":
            package_root = package_root.parent
        data = package_root / "jvid-data"
        assert "portable=true" in report
        assert str(data / "config/settings.toml") in report
        assert str(data / "data/models") in report
        # Settings survive repeated launches, and two copies remain independent.
        settings = data / "config/settings.toml"
        settings.write_text('theme = "light"\n')
        model = data / "data/models/kept.bin"
        model.write_bytes(b"existing model sentinel")
        exercise(binary, evidence / "portable-again", env)
        assert settings.read_text() == 'theme = "light"\n'
        assert model.read_bytes() == b"existing model sentinel"
        other = tmp / "second portable copy"
        extract(archive, other)
        other_binary, = other.rglob(binary.name)
        if sys.platform != "win32":
            other_binary.chmod(0o755)
        other_report = exercise(other_binary, evidence / "portable-independent", env)
        assert str(data) not in other_report, "separate copies shared portable data"
        # A file blocks the data directory on every OS, even a privileged CI account.
        shutil.rmtree(data)
        data.write_bytes(b"blocked")
        failed = subprocess.run([str(binary), "--smoke-test", str(tmp / "blocked-smoke")], env=env, timeout=30)
        assert failed.returncode != 0, "portable copy silently fell back to the profile"
        if installer:
            windows_install(installer, tmp, env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--windows-install", type=Path)
    args = parser.parse_args()
    smoke(args.archive.resolve(), args.evidence.resolve(), args.windows_install.resolve() if args.windows_install else None)


if __name__ == "__main__":
    main()
