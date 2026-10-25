#!/usr/bin/env python3
"""Assemble native artifacts from a release binary. No downloads or publishing here.
See docs/packaging.md; run test_packaging.py for format/layout regression checks.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[2]
TARGETS = ("windows-x86_64", "macos-aarch64", "linux-x86_64")


def run(*args, quiet_failure=False):
    try:
        return subprocess.check_output([str(a) for a in args], text=True, stderr=subprocess.STDOUT)
    except subprocess.CalledProcessError as error:
        if error.output and not quiet_failure:
            print(error.output, file=sys.stderr, end="" if error.output.endswith("\n") else "\n")
        raise


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def version():
    return re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M)[1]


def copy_library(source, dest):
    dest.parent.mkdir(parents=True, exist_ok=True)
    if dest.exists() and sha256(dest) != sha256(source):
        raise RuntimeError(f"conflicting runtime library: {source} -> {dest}")
    shutil.copy2(source, dest)


# OS ABI and graphics driver implementations must come from the supported host OS.
LINUX_SYSTEM = re.compile(r"^(ld-linux|lib(c|m|pthread|dl|rt|resolv|nss_[^.]+)\.so|lib(GL|GLX|GLdispatch|EGL|vulkan)\.)")


def linux_libraries(binary, lib):
    pending, seen, sources = [binary], set(), []
    # winit/cpal/font discovery load these at runtime, so ldd(binary) cannot see them.
    cache = run("ldconfig", "-p")
    for name in ("libX11.so.6", "libXcursor.so.1", "libXi.so.6", "libXrandr.so.2",
                 "libxkbcommon.so.0", "libxkbcommon-x11.so.0", "libwayland-client.so.0",
                 "libwayland-cursor.so.0", "libwayland-egl.so.1", "libasound.so.2",
                 "libfontconfig.so.1", "libfreetype.so.6", "libdbus-1.so.3"):
        match = re.search(r"^\s*" + re.escape(name) + r" .*=> (/\S+)$", cache, re.M)
        if not match:
            raise RuntimeError(f"missing dynamically loaded desktop library: {name}")
        dependency = Path(match[1])
        copy_library(dependency, lib / name)
        pending.append(dependency)
        sources.append(dependency)
    while pending:
        source = pending.pop()
        if source in seen:
            continue
        seen.add(source)
        output = run("ldd", source)
        if "not found" in output:
            raise RuntimeError(f"unresolved dependency: {source}\n{output}")
        for name, path in re.findall(r"^\s*(\S+) => (/\S+) \(", output, re.M):
            if LINUX_SYSTEM.match(name):
                continue
            dependency = Path(path)
            dest = lib / name
            if not dest.exists():
                copy_library(dependency, dest)
                pending.append(dependency)
                sources.append(dependency)
    # DT_RPATH (not RUNPATH) also covers libraries loaded by a dependency.
    run("patchelf", "--force-rpath", "--set-rpath", "$ORIGIN/../lib", binary)
    for path in lib.iterdir():
        run("patchelf", "--force-rpath", "--set-rpath", "$ORIGIN", path)
    return sources


def mac_dependencies(path):
    return [line.strip().split(" (", 1)[0] for line in run("otool", "-L", path).splitlines()[1:]]


def mac_libraries(binary, frameworks):
    pending, sources, originals = [(binary, binary)], [], {}
    while pending:
        original, dest = pending.pop()
        for reference in mac_dependencies(original):
            dependency = reference
            if dependency.startswith(("/System/Library/", "/usr/lib/")):
                continue
            # Homebrew's FFmpeg uses absolute install names. Fail closed on a new
            # provider instead of producing an app that works only on the build runner.
            if dependency == f"@rpath/{original.name}":
                continue  # A dylib's own LC_ID_DYLIB, not a dependency.
            if dependency.startswith("@loader_path/"):
                dependency = str(original.parent / dependency.removeprefix("@loader_path/"))
            if dependency.startswith("@rpath/"):
                paths = re.findall(r"cmd LC_RPATH\n\s*cmdsize \d+\n\s*path (.*?) \(offset", run("otool", "-l", original))
                candidates = [original.parent] + [Path(p.replace("@loader_path", str(original.parent))) for p in paths]
                found = next((p / Path(dependency).name for p in candidates if (p / Path(dependency).name).is_file()), None)
                if found is None:
                    raise RuntimeError(f"unresolved rpath: {dependency} in {original}")
                dependency_path = str(found)
            else:
                dependency_path = dependency
            if not dependency_path.startswith("/"):
                raise RuntimeError(f"unresolved Mach-O dependency: {dependency} in {original}")
            source = Path(dependency_path)
            target = frameworks / source.name
            if source.name not in originals:
                copy_library(source, target)
                originals[source.name] = source
                sources.append(source)
                pending.append((source, target))
            elif originals[source.name].resolve() != source.resolve():
                raise RuntimeError(f"conflicting dylib: {source}")
            replacement = f"@executable_path/../Frameworks/{source.name}" if dest == binary else f"@loader_path/{source.name}"
            run("install_name_tool", "-change", reference, replacement, dest)
    for path in frameworks.iterdir():
        run("install_name_tool", "-id", f"@rpath/{path.name}", path)
    return sources


def notices(stage, providers):
    for name in ("LICENSE-MIT", "LICENSE-APACHE"):
        shutil.copy2(ROOT / name, stage / name)
    shutil.copy2(ROOT / "crates/jvid-app/assets/THIRD-PARTY-NOTICES.txt", stage)
    shutil.copy2(ROOT / "docs/packaging.md", stage / "INSTALL.md")
    (stage / "DISTRIBUTION-LICENSE.txt").write_text(
        "jvid source: MIT OR Apache-2.0 (see LICENSE-MIT and LICENSE-APACHE).\n"
        "This release bundles dynamically linked GPL FFmpeg, including x264/x265.\n"
        "The combined distribution follows the GPL terms described in docs/PLAN.md.\n"
        "See runtime-licenses/ and THIRD-PARTY-NOTICES.txt for full notices.\n"
        "Corresponding source must accompany published releases (see SOURCE-INFO.txt).\n",
        encoding="utf-8",
    )
    (stage / "SOURCE-INFO.txt").write_text(
        "Corresponding source and build scripts for a published release are provided as\n"
        "the versioned corresponding-source archive alongside its binary downloads.\n"
        "CI candidates are not approved release distributions. Publication is gated on\n"
        "that archive and its reviewed SHA-256; see docs/packaging.md.\n"
        "jvid source: https://github.com/jarradbanks/jvid\n\n" + "\n".join(providers) + "\n",
        encoding="utf-8",
    )


def is_license(name):
    path = Path(name)
    return (re.match(r"^(COPYING|LICEN[CS]E|NOTICE|COPYRIGHT|leptonica-license)(\.|$|-)", path.name, re.I)
            or any(part.lower() in ("license", "licenses", "licence", "licences") for part in path.parts[:-1]))


def recover_homebrew_licenses(keg, dest):
    """Some bottles omit COPYing symlink targets/REUSE texts. The bottle retains its
    exact formula: verify that formula's source archive, then copy only notices.
    Source archive contents are read, never executed or extracted as filesystem paths.
    """
    recipes = list((keg / ".brew").glob("*.rb"))
    if len(recipes) != 1:
        raise RuntimeError(f"missing unambiguous source formula for {keg}")
    recipe = recipes[0].read_text()
    url = re.search(r'^\s*url "(https://[^"#]+)"', recipe, re.M)
    digest = re.search(r'^\s*sha256 "([0-9a-f]{64})"', recipe, re.M)
    if not url or not digest:
        raise RuntimeError(f"no pinned HTTPS source archive in {recipes[0]}")
    with tempfile.TemporaryDirectory(prefix="jvid-runtime-notices-") as tmp:
        archive = Path(tmp) / "source"
        urllib.request.urlretrieve(url[1], archive)
        if sha256(archive) != digest[1]:
            raise RuntimeError(f"source archive checksum mismatch for {keg}")
        with tarfile.open(archive) as source:
            members = [m for m in source.getmembers() if (m.isfile() or m.issym()) and is_license(m.name)]
            if not members:
                raise RuntimeError(f"source contains no license text for {keg}")
            for index, member in enumerate(members):
                stream = source.extractfile(member)
                if stream is None:
                    raise RuntimeError(f"unreadable source license: {member.name}")
                text = stream.read(2 * 1024 * 1024 + 1)
                if len(text) > 2 * 1024 * 1024:
                    raise RuntimeError(f"unexpectedly large license: {member.name}")
                (dest / f"source-{index}-{Path(member.name).name}").write_bytes(text)
    (dest / "license-source.json").write_text(json.dumps({"url": url[1], "sha256": digest[1], "formula": recipes[0].name}, indent=2) + "\n")


def native_licenses(stage, sources, target, ffmpeg):
    dest = stage / "runtime-licenses"
    dest.mkdir()
    providers = []
    if target.startswith("windows"):
        shutil.copy2(ffmpeg / "LICENSE.txt", dest / "FFmpeg-GPL.txt")
        if (ffmpeg / "doc").is_dir():
            shutil.copytree(ffmpeg / "doc", dest / "ffmpeg-doc")
        providers.append(json.dumps(json.loads((ROOT / "packaging/ffmpeg-windows.json").read_text()), indent=2))
    elif target.startswith("linux"):
        for source in sources:
            # Resolve symlinks as dpkg may own only the versioned filename.
            # Ubuntu's merged /usr has packages registered under either /lib or
            # /usr/lib. Query both spellings of the exact versioned file.
            resolved = source.resolve()
            candidates = [source, resolved]
            if str(resolved).startswith("/usr/lib/"):
                candidates.append(Path(str(resolved).removeprefix("/usr")))
            owner = None
            for candidate in candidates:
                try:
                    owner = run("dpkg-query", "-S", candidate, quiet_failure=True).split(": /", 1)[0].splitlines()[0]
                    break
                except subprocess.CalledProcessError:
                    pass
            if owner is None:
                raise RuntimeError(f"no Debian package provenance for {source}")
            package = owner.split(":", 1)[0]
            doc = Path("/usr/share/doc") / package
            if not (doc / "copyright").is_file():
                raise RuntimeError(f"missing runtime copyright: {source} ({package})")
            (dest / package).mkdir(exist_ok=True)
            shutil.copy2(doc / "copyright", dest / package / "copyright")
            providers.append(run("dpkg-query", "-W", "-f=${binary:Package} ${Version} ${source:Package} ${source:Version}\n", owner).strip())
        for path in Path("/usr/share/common-licenses").iterdir():
            if path.is_file():
                shutil.copy2(path, dest / path.name)
    else:
        for source in sources:
            resolved = source.resolve()
            keg = next((p for p in resolved.parents if (p / "INSTALL_RECEIPT.json").is_file()), None)
            if keg is None:
                raise RuntimeError(f"no Homebrew provenance for {source}")
            name = keg.parent.name
            target_dir = dest / name
            if target_dir.exists():
                continue
            target_dir.mkdir()
            licenses = [p for p in keg.rglob("*") if p.is_file() and is_license(p.relative_to(keg))]
            if not licenses:
                # Some Homebrew bottles omit upstream's nonstandard licence name.
                # Only use explicitly reviewed text for that exact provider version.
                license_root = ROOT / "packaging/licenses"
                fallback = json.loads((license_root / "catalog.json").read_text()).get(f"{name}/{keg.name}")
                if fallback is None:
                    recover_homebrew_licenses(keg, target_dir)
                else:
                    path = license_root / fallback["file"]
                    if sha256(path) != fallback["sha256"]:
                        raise RuntimeError(f"license checksum mismatch: {path}")
                    licenses = [path]
                    (target_dir / "license-source.json").write_text(json.dumps(fallback, indent=2) + "\n")
            for index, path in enumerate(licenses):
                shutil.copy2(path, target_dir / f"{index}-{path.name}")
            for path in (keg / ".brew").glob("*.rb"):
                shutil.copy2(path, target_dir / path.name)
            shutil.copy2(keg / "INSTALL_RECEIPT.json", target_dir)
            providers.append(str(keg))
    return sorted(set(providers))


def marker(bin_dir, value, portable):
    (bin_dir / "jvid-distribution").write_bytes((value + "\n").encode("utf-8"))
    (bin_dir / "jvid.portable").unlink(missing_ok=True)
    if portable:
        (bin_dir / "jvid.portable").write_text("Portable data: see INSTALL.md\n", encoding="utf-8")


def zip_tree(tree, archive):
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as out:
        for path in sorted(tree.rglob("*")):
            if path.is_file():
                out.write(path, path.relative_to(tree.parent))


def inventory(stage, target):
    (stage / "package-files.json").unlink(missing_ok=True)
    files = {p.relative_to(stage).as_posix(): {"sha256": sha256(p), "size": p.stat().st_size}
             for p in sorted(stage.rglob("*")) if p.is_file()}
    (stage / "package-files.json").write_text(json.dumps({"version": version(), "target": target, "files": files}, indent=2) + "\n")


def build(args):
    target, out = args.target, args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    stem = f"jvid-{version()}-{target}"
    work = out / "staging" / target
    # Only a task-specific staging subtree, never the user's install or portable data.
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    if target.startswith("windows"):
        stage = work / stem
        stage.mkdir()
        binary = stage / "jvid-app.exe"
        shutil.copy2(args.binary, binary)
        if args.helper:
            shutil.copy2(args.helper, stage / "jvid-update-helper.exe")
        for dll in args.ffmpeg_dir.joinpath("bin").glob("*.dll"):
            shutil.copy2(dll, stage)
        if not list(stage.glob("avcodec-*.dll")):
            raise RuntimeError("FFmpeg shared DLLs missing")
        # Static MSVC CRT for Rust/whisper; BtbN uses the OS UCRT. Smoke tests catch
        # accidental additional DLL dependencies without development PATH entries.
        sources = []
    elif target.startswith("linux"):
        stage = work / stem
        (stage / "bin").mkdir(parents=True)
        (stage / "lib").mkdir()
        binary = stage / "bin/jvid-app"
        shutil.copy2(args.binary, binary)
        sources = linux_libraries(binary, stage / "lib")
        (stage / "AppRun").write_text('#!/bin/sh\nset -eu\nHERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\nexec "$HERE/bin/jvid-app" "$@"\n')
        (stage / "AppRun").chmod(0o755)
    else:
        stage = work / stem
        bin_dir = stage / "jvid.app/Contents/MacOS"
        bin_dir.mkdir(parents=True)
        frameworks = stage / "jvid.app/Contents/Frameworks"
        frameworks.mkdir()
        binary = bin_dir / "jvid-app"
        shutil.copy2(args.binary, binary)
        plist = (ROOT / "packaging/Info.plist").read_text().replace("@VERSION@", version())
        plistlib.loads(plist.encode())
        (stage / "jvid.app/Contents/Info.plist").write_text(plist)
        sources = mac_libraries(binary, frameworks)
    license_root = binary.parent  # Notices beside the actual executable on every OS.
    providers = native_licenses(license_root, sources, target, args.ffmpeg_dir)
    notices(license_root, providers)
    if target.startswith("windows"):
        marker(binary.parent, "windows-installer", False)
        inventory(stage, target)
        compiler = args.iscc or shutil.which("ISCC.exe") or r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
        print(run(compiler, f"/DAppVersion={version()}", f"/DStageDir={stage}", f"/DOutputDir={out}", ROOT / "packaging/windows.iss"))
        marker(binary.parent, "windows-portable", True)
        inventory(stage, target)
        zip_tree(stage, out / f"{stem}-portable.zip")
    elif target.startswith("linux"):
        marker(binary.parent, "linux-portable", True)
        inventory(stage, target)
        with tarfile.open(out / f"{stem}-portable.tar.gz", "w:gz") as archive:
            archive.add(stage, arcname=stem)
    else:
        marker(binary.parent, "macos-dmg", False)
        sign_app(stage / "jvid.app")
        inventory(stage, target)
        os.symlink("/Applications", stage / "Applications")
        print(run("hdiutil", "create", "-volname", "jvid", "-srcfolder", stage, "-ov", "-format", "UDZO", out / f"{stem}.dmg"))
        # The ZIP has its own signed app/marker and writes outside the .app bundle.
        (stage / "Applications").unlink()
        marker(binary.parent, "macos-portable", True)
        sign_app(stage / "jvid.app")
        inventory(stage, target)
        # ditto preserves macOS signatures, permissions and bundle metadata.
        print(run("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", stage, out / f"{stem}-portable.zip"))


def sign_app(app):
    identity = os.environ.get("JVID_MACOS_SIGN_IDENTITY", "-")
    # Ad hoc candidates have no Team ID for hardened library validation. Developer
    # ID builds enable the hardened runtime and timestamp needed for notarization.
    options = ["--timestamp=none"] if identity == "-" else ["--options", "runtime", "--timestamp"]
    # Sign inside out; all edits and marker writes must precede this.
    for path in (app / "Contents/Frameworks").iterdir():
        run("codesign", "--force", *options, "--sign", identity, path)
    run("codesign", "--force", *options, "--sign", identity, app / "Contents/MacOS/jvid-app")
    run("codesign", "--force", *options, "--sign", identity, app)
    run("codesign", "--verify", "--deep", "--strict", app)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--helper", type=Path)
    parser.add_argument("--ffmpeg-dir", type=Path)
    parser.add_argument("--iscc")
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    if args.target.startswith("windows") and not args.ffmpeg_dir:
        parser.error("Windows requires --ffmpeg-dir")
    build(args)


if __name__ == "__main__":
    main()
