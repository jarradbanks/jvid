"""Corresponding sources for the exact Homebrew providers retained in a package.

Native host requirements: Python 3, git, and Homebrew (including its portable Ruby).
``collect`` invokes ``brew ruby source_macos_formula.rb`` with a JSON request on
stdin; it never asks Homebrew to look up a formula by name or run install/test.
The trusted retained recipes are executable Homebrew DSL, not untrusted input.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request


class SourceError(RuntimeError):
    pass


SHA256 = re.compile(r"[0-9a-f]{64}\Z")
REVISION = re.compile(r"[0-9a-f]{40}(?:[0-9a-f]{24})?\Z")
NAME = re.compile(r"[a-z0-9][a-z0-9+@_.-]*\Z")
TAP = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\Z")
MAX_DOWNLOAD_BYTES = 4 * 1024**3
NETWORK_SECONDS = 900
HELPER = Path(__file__).with_name("source_macos_formula.rb")
# These recoveries describe the exact historical inputs, not replacement
# versions. See Homebrew/core commits 0619ed8265b992a3e08faff4afad026f05b3b4d8
# (SRT's moved tag) and 78dd0b36788851f1c12293c1623c39ec073e09f7 (deleted snum).
SRT_RECOVERY = (
    "srt", "1.5.7", "source", "srt",
    "https://github.com/Haivision/srt/archive/refs/tags/v1.5.7.tar.gz",
    "fee6aee6b4933f01ba8b7e18d5d9e4896ad604053fdad2ac55df4a4f1561f30a",
)
SRT_ORIGINAL_REVISION = "d99d2e1a3b1a213b03c7dfbea5133898935fdeea"
SRT_SOURCE_MIRROR = (
    "https://src.fedoraproject.org/repo/pkgs/srt/srt-1.5.7.tar.gz/sha512/"
    "877d69f8323a3a2964a76a97f3cb51ab43f762f6e8cc8f00f72c4d5e93ffb3045c092ce64a0a5f853522086d1f4f57080fead17113061f22657d653fa27899c5/"
    "srt-1.5.7.tar.gz"
)
SNUM_RECOVERY = (
    "tesseract", "5.5.3", "resource", "snum",
    "https://github.com/USCDataScience/counterfeit-electronics-tesseract/raw/"
    "319a6eeacff181dad5c02f3e7a3aff804eaadeca/Training%20Tesseract/snum.traineddata",
    "36f772980ff17c66a767f584a0d80bf2302a1afa585c01a226c1863afcea1392",
)


def _json(path: Path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise SourceError(f"invalid or missing JSON: {path.name}") from exc


def _hash(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _verify_archive(path: Path, digest: str) -> None:
    observed = _hash(path)
    if observed != digest:
        raise SourceError(f"source archive checksum mismatch (expected SHA-256 {digest}, observed {observed})")


def _run(*args, input=None, cwd=None, stdout=None, timeout=NETWORK_SECONDS):
    env = dict(os.environ, HOMEBREW_NO_AUTO_UPDATE="1", HOMEBREW_NO_INSTALL_FROM_API="1",
               HOMEBREW_NO_ANALYTICS="1", GIT_TERMINAL_PROMPT="0")
    try:
        result = subprocess.run([str(arg) for arg in args], input=input, cwd=cwd,
                                stdout=stdout or subprocess.PIPE, stderr=subprocess.PIPE,
                                timeout=timeout, check=True, env=env)
    except (OSError, subprocess.SubprocessError) as exc:
        detail = ""
        if isinstance(exc, subprocess.CalledProcessError) and exc.stderr:
            detail = ": " + exc.stderr.decode("utf-8", errors="replace").strip()[-2000:]
        raise SourceError(f"source collection command failed: {Path(str(args[0])).name}{detail}") from exc
    return result.stdout


def _relative(path: Path, out: Path) -> str:
    return path.relative_to(out).as_posix()


def _safe_path(value: str) -> str:
    path = PurePosixPath(value)
    if not value or "\\" in value or path.is_absolute() or any(x in ("", ".", "..") for x in value.split("/")):
        raise SourceError("nonportable or escaping provider path")
    return value


def _url(value: str, *, git=False) -> str:
    parsed = urllib.parse.urlsplit(value)
    # Git/file/SSH/custom download strategies must never bypass our verifier.
    schemes = ("https",) if git else ("https", "http")
    if parsed.scheme not in schemes or not parsed.hostname or parsed.username or parsed.password or parsed.fragment:
        raise SourceError("source URL uses unsupported transport or credentials")
    return value


def _download(url: str, digest: str, dest: Path) -> None:
    _url(url)
    if not isinstance(digest, str) or not SHA256.fullmatch(digest):
        raise SourceError("source archive has no pinned SHA-256")
    dest.parent.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    size = 0
    try:
        request = urllib.request.Request(url, headers={"User-Agent": "jvid-corresponding-source/1"})
        with urllib.request.urlopen(request, timeout=60) as response, dest.open("wb") as stream:
            _url(response.geturl())
            while chunk := response.read(1024 * 1024):
                size += len(chunk)
                if size > MAX_DOWNLOAD_BYTES or time.monotonic() - started > NETWORK_SECONDS:
                    raise SourceError("source download exceeded its size or time limit")
                stream.write(chunk)
        _verify_archive(dest, digest)
    except Exception as exc:
        dest.unlink(missing_ok=True)
        if isinstance(exc, SourceError):
            raise
        # Exception text and redirect URLs can contain credentials or signed
        # query strings. Keep the diagnostic to the safe status code, if any.
        status = f" (HTTP {exc.code})" if isinstance(exc, urllib.error.HTTPError) else ""
        raise SourceError(f"source download failed{status}") from None


def _raw_api_location(url: str) -> str | None:
    """Serve VideoLAN raw blobs through its public GitLab API at the same ref."""
    parsed = urllib.parse.urlsplit(url)
    if (parsed.scheme != "https" or parsed.netloc != "code.videolan.org"
            or parsed.query or parsed.fragment):
        return None
    match = re.fullmatch(r"/([^/]+)/([^/]+)/(?:-/)?raw/([^/]+)/(.+)", parsed.path)
    if not match:
        return None
    owner, repository, revision, path = (urllib.parse.unquote(value) for value in match.groups())
    project = urllib.parse.quote(owner + "/" + repository, safe="")
    path = urllib.parse.quote(path, safe="")
    query = urllib.parse.urlencode({"ref": revision})
    return f"https://code.videolan.org/api/v4/projects/{project}/repository/files/{path}/raw?{query}"


def _archive_input(item: dict, dest: Path, *, provider: dict | None = None) -> str:
    """Verify retained mirrors and equivalent raw-file transport against one digest."""
    failures = []
    locations = [item["url"], *item.get("mirrors", [])]
    api = _raw_api_location(item["url"])
    if api is not None:
        locations.append(api)
    for index, url in enumerate(dict.fromkeys(locations), 1):
        try:
            _download(url, item["sha256"], dest)
            return url
        except SourceError as exc:
            failures.append(f"location {index}: {exc}")
    # Identity comes from the retained recipe, never a potentially signed URL.
    # Bound and escape labels so one bad resource cannot flood a CI log.
    def label(value):
        return ascii(str(value)[:120])
    identity = f"{item.get('role', 'payload')} {label(item.get('name', item['id']))} ({label(item['id'])})"
    if provider is not None:
        identity = f"homebrew:{provider['name'][:120]}@{provider['version'][:120]} {identity}"
    raise SourceError(f"all recorded source locations failed verification or download: {identity}; "
                      + "; ".join(failures)) from None


def _recover_archive_input(item: dict, dest: Path, provider: dict) -> dict | None:
    key = (provider["name"], provider["version"], item.get("role"), item.get("name"),
           item.get("url"), item.get("sha256"))
    if key == SRT_RECOVERY:
        # Fedora's immutable lookaside retains the original archive, including
        # its original Git pax commit and compression. The moved GitHub tag
        # has different source contents; never adopt its replacement checksum.
        _download(SRT_SOURCE_MIRROR, item["sha256"], dest)
        return {"retrieved_from": SRT_SOURCE_MIRROR, "recovery": {
            "kind": "exact-hash-source-mirror", "original_revision": SRT_ORIGINAL_REVISION,
            "evidence": "https://github.com/Haivision/srt/issues/3378",
        }}
    if key == SNUM_RECOVERY:
        return _installed_resource(item, dest, provider, "share/tessdata/snum.traineddata")
    return None


def _installed_resource(item: dict, dest: Path, provider: dict, relative: str) -> dict:
    # The retained recipe installs this data verbatim. Recover from only that
    # exact recorded keg, and require its receipt, recipe AND resource to match.
    keg = provider["cellar"]
    try:
        if (_hash(keg / "INSTALL_RECEIPT.json") != _hash(provider["receipt_path"])
                or _hash(keg / ".brew" / f"{provider['name']}.rb") != _hash(provider["recipe"])):
            raise SourceError("installed resource provider differs from retained provenance")
        original = keg / relative
        if original.is_symlink() or not original.is_file():
            raise SourceError("original installed resource is unavailable")
        if original.stat().st_size > MAX_DOWNLOAD_BYTES:
            raise SourceError("installed resource exceeded its size limit")
        _verify_archive(original, item["sha256"])
        shutil.copyfile(original, dest)
        _verify_archive(dest, item["sha256"])
    except (OSError, SourceError) as exc:
        dest.unlink(missing_ok=True)
        if isinstance(exc, SourceError):
            raise
        raise SourceError("original installed resource is unavailable") from None
    return {"retrieved_from": "installed-provider-resource", "recovery": {
        "kind": "installed-resource", "provider": f"homebrew:{provider['name']}@{provider['version']}", "path": relative,
        "verification": "retained receipt, recipe and original resource SHA-256",
    }}


def _providers(stage: Path) -> list[dict]:
    """Match every recorded Cellar path to its retained recipe and receipt."""
    try:
        info = (stage / "SOURCE-INFO.txt").read_text(encoding="utf-8")
    except OSError as exc:
        raise SourceError("missing SOURCE-INFO.txt") from exc
    expected = {}
    cellars = {}
    for line in info.splitlines():
        if "/Cellar/" not in line:
            continue
        match = re.fullmatch(r"/[^\r\n]*/Cellar/([^/]+)/([^/]+)", line.strip())
        if not match or not NAME.fullmatch(match[1]) or not NAME.fullmatch(match[2]):
            raise SourceError("invalid Homebrew provider in SOURCE-INFO.txt")
        if match[1] in expected:
            raise SourceError(f"duplicate Homebrew provider: {match[1]}")
        expected[match[1]] = match[2]
        cellars[match[1]] = Path(line.strip())
    if not expected:
        raise SourceError("SOURCE-INFO.txt records no Homebrew providers")
    licenses = stage / "runtime-licenses"
    if not licenses.is_dir() or licenses.is_symlink():
        raise SourceError("missing retained Homebrew provenance")
    retained = {p.name for p in licenses.iterdir() if p.is_dir()}
    if retained != set(expected):
        raise SourceError("recorded and retained Homebrew providers differ: " +
                          ", ".join(sorted(retained ^ set(expected))))
    providers = []
    for name, version in sorted(expected.items()):
        root = licenses / name
        recipe = root / f"{name}.rb"
        receipt_path = root / "INSTALL_RECEIPT.json"
        recipes = list(root.glob("*.rb")) + list((root / ".brew").glob("*.rb"))
        if recipe not in recipes:
            recipe = root / ".brew" / f"{name}.rb"
        if recipes != [recipe] or recipe.is_symlink() or receipt_path.is_symlink() or root.is_symlink():
            raise SourceError(f"missing or ambiguous retained recipe: {name}")
        receipt = _json(receipt_path)
        if not isinstance(receipt, dict) or not isinstance(receipt.get("source"), dict):
            raise SourceError(f"missing receipt source metadata: {name}")
        source = receipt["source"]
        if source.get("spec") != "stable":
            raise SourceError(f"unsupported nonstable installed provider: {name}")
        versions = source.get("versions")
        stable = versions.get("stable") if isinstance(versions, dict) else None
        # The evaluated recipe checks the numeric formula revision as well.
        if not isinstance(stable, str) or not (version == stable or re.fullmatch(re.escape(stable) + r"_[0-9]+", version)):
            raise SourceError(f"receipt version differs from recorded provider: {name}")
        if not isinstance(source.get("tap"), str) or not TAP.fullmatch(source["tap"]):
            raise SourceError(f"missing receipt tap: {name}")
        if not isinstance(source.get("tap_git_head"), str) or not REVISION.fullmatch(source["tap_git_head"]):
            raise SourceError(f"missing immutable tap revision: {name}")
        for option_key in ("used_options", "unused_options"):
            if not isinstance(receipt.get(option_key), list) or not all(isinstance(x, str) for x in receipt[option_key]):
                raise SourceError(f"missing receipt build options: {name}")
        providers.append({"name": name, "version": version, "recipe": recipe,
                          "receipt_path": receipt_path, "receipt": receipt, "cellar": cellars[name]})
    return providers


def _formula_plan(provider: dict) -> dict:
    request = {"recipe": str(provider["recipe"].resolve()), "name": provider["name"],
               "version": provider["version"], "receipt": provider["receipt"]}
    data = _run("brew", "ruby", HELPER, input=json.dumps(request).encode(), timeout=180)
    try:
        plan = json.loads(data)
    except (ValueError, UnicodeError) as exc:
        raise SourceError("Homebrew helper did not return a source plan") from exc
    _validate_plan(provider, plan)
    return plan


def _validate_plan(provider: dict, plan: dict) -> None:
    if not isinstance(plan, dict) or plan.get("name") != provider["name"] or plan.get("version") != provider["version"]:
        raise SourceError("evaluated retained recipe does not match the installed provider")
    if plan.get("recipe_sha256") != _hash(provider["recipe"]):
        raise SourceError("Homebrew evaluated a different retained recipe")
    payloads = plan.get("payloads")
    if not isinstance(payloads, list) or not payloads:
        raise SourceError("retained recipe has no sources")
    ids = [x.get("id") for x in payloads if isinstance(x, dict)]
    if (len(ids) != len(payloads) or not all(isinstance(x, str) for x in ids)
            or len(set(ids)) != len(ids) or ids[0] != "source"
            or payloads[0].get("role") != "source"):
        raise SourceError("missing or duplicate source payloads")
    declarations = plan.get("resource_declarations")
    if not isinstance(declarations, list) or not all(isinstance(x, str) for x in declarations):
        raise SourceError("missing resource declaration audit")
    # Independent completeness check: catches a broken helper that emits only the
    # main archive (including the historically missed resource blocks in tests).
    text = provider["recipe"].read_text(encoding="utf-8").split("\n__END__", 1)[0]
    literal_resources = re.findall(r'^\s*resource\s*(?:\(\s*)?["\']([^"\']+)["\']\s*\)?\s*(?:,\s*:[a-z_]+\s*\)?)?\s+do\b', text, re.M)
    if set(literal_resources) != set(declarations) or len(literal_resources) != len(declarations):
        raise SourceError("resource declaration audit omitted or duplicated a resource")
    resource_names = [x.get("name") for x in payloads if x.get("role") == "resource"]
    if sorted(resource_names) != sorted(declarations):
        raise SourceError("resource source payload missing from retained recipe")
    patch_declarations = re.findall(r"^\s*patch(?:\s|\()", text, re.M)
    if len(patch_declarations) != sum(item.get("role") == "patch" for item in payloads):
        raise SourceError("patch source payload missing from retained recipe")
    for item in payloads:
        if not isinstance(item.get("id"), str) or not NAME.fullmatch(item["id"]):
            raise SourceError("invalid source payload id")
        kind = item.get("kind")
        if kind == "archive":
            _url(item.get("url", ""))
            if not isinstance(item.get("sha256"), str) or not SHA256.fullmatch(item["sha256"]):
                raise SourceError("source archive has no pinned SHA-256")
            mirrors = item.get("mirrors", [])
            if not isinstance(mirrors, list) or len(mirrors) > 16 or not all(isinstance(x, str) for x in mirrors):
                raise SourceError("unsupported source mirror list")
            for url in mirrors:
                _url(url)
        elif kind == "git":
            _url(item.get("url", ""), git=True)
            if not isinstance(item.get("revision"), str) or not REVISION.fullmatch(item["revision"]):
                raise SourceError("Git source has no immutable revision")
        elif kind == "tap-file":
            _safe_path(item.get("path", ""))
        elif kind == "inline":
            if not isinstance(item.get("contents"), str) or not item["contents"]:
                raise SourceError("empty inline patch")
        else:
            raise SourceError("unsupported retained source payload")


def _git_repository(url: str, revision: str, parent: Path) -> Path:
    _url(url, git=True)
    if not REVISION.fullmatch(revision):
        raise SourceError("Git source has no immutable revision")
    repo = parent / hashlib.sha256((url + revision).encode()).hexdigest()
    if not repo.exists():
        _run("git", "init", "--bare", repo)
        _run("git", "-C", repo, "-c", "fetch.fsckObjects=true", "-c", "transfer.fsckObjects=true",
             "fetch", "--depth=1", "--no-tags", url, revision)
    # A source snapshot includes every tracked file, including files upstream
    # excludes from release archives, and preserves committed bytes verbatim.
    (repo / "info" / "attributes").write_text("* -export-ignore -export-subst\n", encoding="utf-8")
    resolved = _run("git", "-C", repo, "rev-parse", "FETCH_HEAD^{commit}").decode().strip()
    if resolved != revision:
        raise SourceError("fetched Git source differs from its pinned revision")
    _run("git", "-C", repo, "fsck", "--no-reflogs")
    return repo


def _git_blob(repo: Path, revision: str, path: str) -> bytes:
    _safe_path(path)
    # `show` resolves a Git object at the verified commit, never a working file.
    tree_entry = _run("git", "-C", repo, "ls-tree", revision, "--", path)
    if not tree_entry.startswith((b"100644 blob ", b"100755 blob ")):
        raise SourceError("missing or unsupported tap/submodule metadata file")
    contents = _run("git", "-C", repo, "show", f"{revision}:{path}")
    if not contents:
        raise SourceError("empty tap/submodule metadata file")
    return contents


def _git_snapshot(url: str, revision: str, dest: Path, cache: Path, *, depth=0) -> list[dict]:
    """Keep one archive per repo; submodules are separately pinned source trees."""
    if depth > 16:
        raise SourceError("Git submodule recursion limit exceeded")
    repo = _git_repository(url, revision, cache)
    dest.parent.mkdir(parents=True, exist_ok=True)
    with dest.open("wb") as stream:
        _run("git", "-C", repo, "archive", "--format=tar", revision, stdout=stream)
    records = [{"url": url, "revision": revision, "file": dest.name}]
    links = []
    for row in _run("git", "-C", repo, "ls-tree", "-rz", revision).split(b"\0"):
        if row.startswith(b"160000 "):
            meta, path = row.split(b"\t", 1)
            links.append((path.decode(), meta.split()[2].decode()))
    if not links:
        return records
    modules_file = cache / "submodules.config"
    modules_file.write_bytes(_git_blob(repo, revision, ".gitmodules"))
    raw = _run("git", "config", "--file", modules_file, "--null", "--get-regexp", r"^submodule\..*\.(path|url)$")
    modules = {}
    for entry in raw.split(b"\0"):
        if not entry:
            continue
        key, value = entry.decode().split("\n", 1)
        stem, field = key.rsplit(".", 1)
        modules.setdefault(stem, {})[field] = value
    by_path = {v.get("path"): v.get("url") for v in modules.values()}
    if len(by_path) != len(modules) or set(by_path) != {p for p, _ in links}:
        raise SourceError("Git submodule declarations do not match pinned gitlinks")
    for index, (path, pinned) in enumerate(links):
        _safe_path(path)
        child_url = by_path[path]
        if not isinstance(child_url, str):
            raise SourceError("missing Git submodule URL")
        if child_url.startswith(("../", "./")):
            child_url = urllib.parse.urljoin(url.rstrip("/") + "/", child_url)
        _url(child_url, git=True)
        child = dest.with_name(f"{dest.stem}-submodule-{index:03d}.tar")
        children = _git_snapshot(child_url, pinned, child, cache, depth=depth + 1)
        for item in children:
            item["path"] = path + ("/" + item["path"] if item.get("path") else "")
        records.extend(children)
    return records


def _build_support(out: Path, cache: Path, providers: list[dict]) -> tuple[list[dict], dict]:
    """Archive exact taps (dependency recipes and local patches) and brew DSL."""
    records = []
    repositories = {}
    taps = sorted({(p["receipt"]["source"]["tap"], p["receipt"]["source"]["tap_git_head"]) for p in providers})
    for tap, revision in taps:
        owner, name = tap.split("/")
        url = f"https://github.com/{owner}/homebrew-{name}.git"
        dest = out / "build-support" / f"{owner}-{name}-{revision}.tar"
        snapshots = _git_snapshot(url, revision, dest, cache)
        repositories[(tap, revision)] = _git_repository(url, revision, cache)
        files = [_relative(dest.with_name(x["file"]), out) for x in snapshots]
        records.append({"id": f"homebrew-tap:{tap}@{revision}", "source": url, "version": revision,
                        "provenance": "Git-verified receipt tap snapshot including dependency recipes and patches",
                        "files": files})
    brew_repo = Path(_run("brew", "--repository", timeout=60).decode().strip())
    revision = _run("git", "-C", brew_repo, "rev-parse", "HEAD^{commit}").decode().strip()
    if not REVISION.fullmatch(revision):
        raise SourceError("installed Homebrew has no immutable build-support revision")
    dest = out / "build-support" / f"homebrew-brew-{revision}.tar"
    with dest.open("wb") as stream:
        _run("git", "-C", brew_repo, "archive", "--format=tar", revision, stdout=stream)
    records.append({"id": f"homebrew-build-support@{revision}", "version": revision,
                    "source": "https://github.com/Homebrew/brew", "files": [_relative(dest, out)],
                    "provenance": "Installed Homebrew DSL/build support; original bottle tool versions remain in receipts"})
    return records, repositories


def collect(stage: Path, out: Path) -> list[dict]:
    stage, out = Path(stage), Path(out)
    providers = _providers(stage)
    if not out.is_dir() or any(out.iterdir()):
        raise SourceError("macOS source output must be a fresh empty directory")
    # Resolve and validate *all* recipes before the first network request.
    plans = [_formula_plan(provider) for provider in providers]
    shutil.copyfile(stage / "SOURCE-INFO.txt", out / "SOURCE-INFO.txt")
    with tempfile.TemporaryDirectory(prefix="jvid-macos-sources-") as temporary:
        cache = Path(temporary)
        support, taps = _build_support(out, cache, providers)
        records = []
        for provider, plan in zip(providers, plans):
            name = provider["name"]
            dest = out / "providers" / name
            dest.mkdir(parents=True)
            shutil.copyfile(provider["recipe"], dest / f"{name}.rb")
            shutil.copyfile(provider["receipt_path"], dest / "INSTALL_RECEIPT.json")
            source = provider["receipt"]["source"]
            provenance = []
            for index, item in enumerate(plan["payloads"]):
                stem = f"{index:03d}-{item['id']}"
                if item["kind"] == "archive":
                    filename = Path(urllib.parse.unquote(urllib.parse.urlsplit(item["url"]).path)).name
                    # Keep the original suffix for archives, without trusting URL paths.
                    suffix = "".join(Path(filename).suffixes)[-32:]
                    suffix = suffix if re.fullmatch(r"(?:\.[A-Za-z0-9_-]+)*", suffix) else ""
                    target = dest / (stem + suffix)
                    try:
                        retrieved = {"retrieved_from": _archive_input(item, target, provider=provider)}
                    except SourceError as download_failure:
                        try:
                            retrieved = _recover_archive_input(item, target, provider)
                        except SourceError as recovery_failure:
                            raise SourceError(f"{download_failure}; exact source recovery failed: {recovery_failure}") from None
                        if retrieved is None:
                            raise
                    item = dict(item, **retrieved)
                elif item["kind"] == "git":
                    target = dest / (stem + ".tar")
                    item = dict(item, repositories=_git_snapshot(item["url"], item["revision"], target, cache))
                elif item["kind"] == "tap-file":
                    target = dest / (stem + ".patch")
                    target.write_bytes(_git_blob(taps[(source["tap"], source["tap_git_head"])], source["tap_git_head"], item["path"]))
                    item = dict(item, tap=source["tap"], revision=source["tap_git_head"])
                elif item["kind"] == "inline":
                    target = dest / (stem + ".patch")
                    target.write_text(item["contents"], encoding="utf-8")
                    item = {k: v for k, v in item.items() if k != "contents"}
                provenance.append(dict(item, file=_relative(target, out)))
            (dest / "source-plan.json").write_text(json.dumps({"name": name, "version": provider["version"],
                "sources": provenance, "dependencies": plan.get("dependencies", []),
                "resource_declarations": plan["resource_declarations"]}, indent=2) + "\n", encoding="utf-8")
            records.append({"id": f"homebrew:{name}@{provider['version']}", "name": name,
                "version": provider["version"], "source": provenance[0].get("url"),
                "provenance": {"formula": f"{name}.rb", "tap": source["tap"],
                    "tap_revision": source["tap_git_head"], "spec": source["spec"],
                    "used_options": provider["receipt"]["used_options"],
                    "unused_options": provider["receipt"]["unused_options"]},
                "files": sorted(_relative(p, out) for p in dest.iterdir() if p.is_file())})
        instructions = out / "BUILD-SOURCES.txt"
        instructions.write_text(
            "Each providers/ directory retains the installed bottle's exact formula, unmodified\n"
            "INSTALL_RECEIPT.json (including options/compiler), source inputs, and source-plan.json.\n"
            "Archive inputs match their original retained SHA-256; exact historical recovery\n"
            "provenance is in source-plan.json. Git tar snapshots correspond to verified\n"
            "commits. For submodules, extract each separately at its recorded repository path.\n"
            "The build-support tap snapshots contain dependency recipes and tap-local patches\n"
            "at the receipt's tap revision. Use those recipes, not current Homebrew formulae.\n"
            "Homebrew's installed DSL/build implementation is retained as a separate snapshot.\n"
            "The receipts preserve the original bottle tool versions and build host information.\n"
            "Rebuild on macOS with Xcode Command Line Tools and Homebrew; restore the selected\n"
            "tap snapshot and source inputs to Homebrew's cache, then brew install --build-from-source\n"
            "the retained formula with the receipt's used_options. Source archives may require\n"
            "additional build tools/dependencies declared by that exact formula. Test-only and\n"
            "inactive-platform resource declarations are also retained for completeness.\n",
            encoding="utf-8")
        support[-1]["files"].append(_relative(instructions, out))
        support[-1]["files"].append("SOURCE-INFO.txt")
        return records + support
