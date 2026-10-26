#!/usr/bin/env python3
"""Capture the exact BtbN Windows sources; refuse an incomplete source release.

Uses only Python's standard library on Windows and Linux. GH_TOKEN is used only
on api.github.com with redirects disabled. The artifact download is a separate,
unauthenticated request. No signed URL or token is saved in provenance/errors.
All network inputs are bound to the reviewed runtime lock. The BtbN dependency
artifact is a ZIP containing a gzip tar containing individual xz source tars;
each level is streamed, without extracting Unix links on a Windows filesystem.
"""
from __future__ import annotations

import hashlib
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import tarfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[2]
LOCK = ROOT / 'packaging/ffmpeg-windows.json'
CHUNK = 1024 * 1024
API_LIMIT = 8 * 1024 * 1024
MAX_DOWNLOAD_SECONDS = 2 * 60 * 60
MAX_ARCHIVE_BYTES = 512 * 1024 * 1024
MAX_CACHE_EXPANDED = 40 * 1024 * 1024 * 1024
MAX_MEMBERS = 2_000_000


class SourceError(RuntimeError):
    """Missing, mismatched, or incomplete exact source input."""


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        while block := stream.read(CHUNK):
            digest.update(block)
    return digest.hexdigest()


def _https(url: str, host: str | None = None) -> None:
    parsed = urllib.parse.urlsplit(url)
    if (parsed.scheme != 'https' or not parsed.hostname or parsed.username or
            parsed.password or (host and parsed.hostname != host)):
        raise SourceError('source URL is not an approved HTTPS endpoint')


def _request(url: str, *, api=False):
    _https(url, 'api.github.com' if api else None)
    headers = {'User-Agent': 'jvid-corresponding-source'}
    if api:
        headers.update({'Accept': 'application/vnd.github+json',
                        'X-GitHub-Api-Version': '2022-11-28'})
        token = os.environ.get('GH_TOKEN')
        if token:
            headers['Authorization'] = 'Bearer ' + token
    return urllib.request.Request(url, headers=headers)


def _open(request):
    # NoRedirect prevents credentials crossing hosts and prevents HTTPS downgrades.
    return urllib.request.build_opener(NoRedirect).open(request, timeout=60)


def api_json(endpoint: str) -> dict:
    try:
        url = 'https://api.github.com/' + endpoint
        try:
            response = _open(_request(url, api=True))
        except urllib.error.HTTPError as exc:
            if exc.code not in (401, 403, 404):
                raise
            # Provenance is public; a restricted job token must not prevent
            # the same immutable metadata being read anonymously.
            response = _open(_request(url))
        with response:
            data = response.read(API_LIMIT + 1)
        if len(data) > API_LIMIT:
            raise SourceError('GitHub source metadata exceeds size limit')
        value = json.loads(data)
        if not isinstance(value, dict):
            raise SourceError('GitHub source metadata must be an object')
        return value
    except (urllib.error.URLError, OSError, ValueError):
        # Network exception strings may contain signed URLs, so do not echo them.
        raise SourceError('could not read pinned GitHub source metadata') from None


def download(url: str, dest: Path, digest: str, max_bytes: int, *, exact_size=None):
    """Bounded streaming download; each redirect is explicitly unauthenticated."""
    if not re.fullmatch(r'[0-9a-f]{64}', digest):
        raise SourceError('missing pinned source SHA-256')
    part = dest.with_name(dest.name + '.part')
    start = time.monotonic()
    try:
        for _ in range(6):
            try:
                response = _open(_request(url))
                break
            except urllib.error.HTTPError as exc:
                if exc.code not in (301, 302, 303, 307, 308) or not exc.headers.get('Location'):
                    raise SourceError('pinned source download failed') from None
                url = urllib.parse.urljoin(url, exc.headers['Location'])
                _https(url)
        else:
            raise SourceError('too many source download redirects')
        hasher = hashlib.sha256()
        size = 0
        with response, part.open('wb') as output:
            while block := response.read(CHUNK):
                size += len(block)
                if size > max_bytes or time.monotonic() - start > MAX_DOWNLOAD_SECONDS:
                    raise SourceError('source download exceeds size/time limit')
                hasher.update(block)
                output.write(block)
        if exact_size is not None and size != exact_size:
            raise SourceError('source artifact size mismatch')
        if hasher.hexdigest() != digest:
            raise SourceError('source SHA-256 mismatch')
        part.replace(dest)
    except (urllib.error.URLError, OSError):
        raise SourceError('pinned source download failed') from None
    finally:
        part.unlink(missing_ok=True)


def fetch(url: str, dest: Path, digest: str, max_bytes: int, *, exact_size=None):
    """Optional local provider cache, still rehashed before every collection."""
    if not re.fullmatch(r'[0-9a-f]{64}', digest):
        raise SourceError('missing pinned source SHA-256')
    cache = os.environ.get('JVID_WINDOWS_SOURCE_CACHE')
    cached = Path(cache) / digest if cache else None
    if cached and cached.is_file():
        size = cached.stat().st_size
        if (size > max_bytes or (exact_size is not None and size != exact_size)
                or sha256(cached) != digest):
            raise SourceError('cached Windows source size/SHA-256 mismatch')
        shutil.copyfile(cached, dest)
        return
    download(url, dest, digest, max_bytes, exact_size=exact_size)


def artifact_url(endpoint: str, public_url: str | None = None) -> str:
    """Consume the authenticated API redirect without following it."""
    try:
        with _open(_request('https://api.github.com/' + endpoint, api=True)):
            raise SourceError('expected GitHub artifact redirect')
    except urllib.error.HTTPError as exc:
        if exc.code in (401, 403, 404) and public_url:
            # The job's repository-scoped github.token may not read another
            # repository's artifacts. nightly.link accesses the same immutable
            # public artifact ID; the GitHub metadata and ZIP digest are still
            # independently checked. No token goes to that transport.
            _https(public_url, 'nightly.link')
            return public_url
        if exc.code != 302 or not exc.headers.get('Location'):
            raise SourceError('pinned source artifact is unavailable') from None
        location = exc.headers['Location']
        _https(location)
        return location
    except (urllib.error.URLError, OSError):
        raise SourceError('could not request pinned source artifact') from None


def _match(actual, expected, label):
    if actual != expected:
        raise SourceError(label + ' does not match pinned source provenance')


def _write_json(path: Path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + '\n', encoding='utf-8')


def validate_stage(stage: Path, lock: dict):
    """Every recorded Windows provider must match this exact runtime lock."""
    info = stage / 'SOURCE-INFO.txt'
    if not info.is_file():
        raise SourceError('missing Windows SOURCE-INFO.txt provider record')
    text = info.read_text(encoding='utf-8-sig')
    # The package format is a prose preamble followed by JSON provider records.
    marker = text.find('\n{')
    if marker < 0:
        raise SourceError('missing Windows runtime provider JSON')
    remaining = text[marker:].strip()
    try:
        recorded, end = json.JSONDecoder().raw_decode(remaining)
    except ValueError:
        raise SourceError('invalid Windows runtime provider JSON') from None
    if remaining[end:].strip():
        raise SourceError('unrepresented additional Windows runtime provider')
    for key in ('url', 'sha256', 'build_scripts_commit', 'policy'):
        _match(recorded.get(key), lock.get(key), 'recorded Windows ' + key)
    if 'source' in recorded:
        _match(recorded['source'], lock['source'], 'recorded Windows source lock')
    notices = stage / 'runtime-licenses'
    if not (notices / 'FFmpeg-GPL.txt').is_file():
        raise SourceError('missing recorded Windows FFmpeg GPL provider')
    for path in notices.iterdir():
        if path.name not in ('FFmpeg-GPL.txt', 'ffmpeg-doc'):
            raise SourceError('unrepresented Windows runtime-licenses provider: ' + path.name)
    source = lock['source']
    revision = source['ffmpeg']['revision']
    if not re.fullmatch(r'[0-9a-f]{40}', revision):
        raise SourceError('FFmpeg source revision must be a full Git commit')
    suffix = f"-g{revision[:10]}-{source['target']}-{source['variant']}-{'-'.join(source['addins'])}.zip"
    if not urllib.parse.urlsplit(lock['url']).path.endswith(suffix):
        raise SourceError('runtime filename does not match pinned source revision/variant')
    _match((source['target'], source['variant'], source['addins']),
           ('win64', 'gpl-shared', ['8.1']), 'reviewed GPL shared variant')
    for item in ('ffmpeg', 'build_scripts'):
        if not re.fullmatch(r'[0-9a-f]{64}', source[item]['sha256']):
            raise SourceError('missing pinned source archive checksum')
    if not source['ffmpeg']['url'].endswith('/' + revision):
        raise SourceError('FFmpeg archive does not use the exact revision')
    if not source['build_scripts']['url'].endswith('/' + lock['build_scripts_commit']):
        raise SourceError('build scripts archive does not use the exact revision')
    cache = source['cache']
    if (not re.fullmatch(r'sha256:[0-9a-f]{64}', cache.get('digest', ''))
            or not isinstance(cache.get('size_in_bytes'), int) or cache['size_in_bytes'] <= 0):
        raise SourceError('missing pinned source artifact digest/size')
    _match(cache.get('public_url'),
           f"https://nightly.link/{source['repository']}/actions/artifacts/{cache['id']}.zip",
           'immutable public source artifact transport')


def validate_provenance(lock: dict) -> dict:
    source = lock['source']
    repo = source['repository']
    prefix = f'repos/{repo}/actions'
    run = api_json(f"{prefix}/runs/{source['run_id']}")
    expected = {'id': source['run_id'], 'head_sha': lock['build_scripts_commit'],
                'run_attempt': source['run_attempt'], 'path': source['workflow'],
                'event': source['event'], 'status': 'completed', 'conclusion': 'success'}
    for key, value in expected.items():
        _match(run.get(key), value, 'BtbN run ' + key)
    _match(run.get('repository', {}).get('full_name'), repo, 'BtbN run repository')
    _match(run.get('repository', {}).get('id'), source['repository_id'], 'BtbN repository id')
    artifact = api_json(f"{prefix}/artifacts/{source['cache']['id']}")
    for key in ('id', 'name', 'size_in_bytes', 'digest'):
        _match(artifact.get(key), source['cache'][key], 'BtbN cache ' + key)
    _match(artifact.get('expired'), False, 'BtbN cache expiration')
    for key, value in {'id': source['run_id'], 'head_sha': lock['build_scripts_commit'],
                       'repository_id': source['repository_id']}.items():
        _match(artifact.get('workflow_run', {}).get(key), value, 'BtbN cache run ' + key)
    jobs = {}
    for field in ('build_job', 'variant_job'):
        expected = source[field]
        job = api_json(f"{prefix}/jobs/{expected['id']}")
        for key, value in {**expected, 'run_id': source['run_id'],
                           'conclusion': 'success', 'status': 'completed'}.items():
            _match(job.get(key), value, 'BtbN ' + field + ' ' + key)
        jobs[field] = {key: job[key] for key in ('id', 'name', 'run_id', 'status', 'conclusion')}
    # Retain a deliberate allowlist. Do not store archive_download_url or signed URLs.
    return {'run': {**expected_run(source, lock), 'repository': repo},
            'cache': {key: artifact[key] for key in ('id', 'name', 'size_in_bytes', 'digest', 'expired')},
            **jobs}


def expected_run(source, lock):
    return {'id': source['run_id'], 'head_sha': lock['build_scripts_commit'],
            'run_attempt': source['run_attempt'], 'path': source['workflow'],
            'event': source['event'], 'status': 'completed', 'conclusion': 'success'}


def _member_path(name, *, portable=True):
    path = PurePosixPath(name)
    if (path.is_absolute() or '..' in path.parts or '\0' in name
            or (portable and ('\\' in name or ':' in name))):
        raise SourceError('unsafe path in pinned source archive')
    return path


def validate_ffmpeg(archive: Path, revision: str):
    required = {'configure', 'Makefile', 'COPYING.GPLv3', 'libavcodec/avcodec.h',
                'libavformat/avformat.h', 'libavutil/avutil.h'}
    prefix = 'FFmpeg-' + revision
    expanded = 0
    with tarfile.open(archive, 'r|gz') as tar:
        for index, member in enumerate(tar):
            tar.members.clear()  # Bound TarInfo retention as well as payload reads.
            parts = _member_path(member.name, portable=False).parts
            if not parts or parts[0] != prefix:
                raise SourceError('FFmpeg source archive root mismatch')
            expanded += member.size
            if index > MAX_MEMBERS or expanded > MAX_ARCHIVE_BYTES:
                raise SourceError('FFmpeg source archive exceeds size limit')
            if member.isfile() and member.size:
                required.discard(PurePosixPath(*parts[1:]).as_posix())
    if required:
        raise SourceError('FFmpeg archive lacks required source/build payload')


def read_scripts(archive: Path, revision: str) -> dict[str, bytes]:
    """Read bounded text recipes/patches without filesystem extraction on Windows."""
    files = {}
    prefix = 'FFmpeg-Builds-' + revision
    total = 0
    with tarfile.open(archive, 'r|gz') as tar:
        for member in tar:
            tar.members.clear()
            parts = _member_path(member.name).parts
            if not parts or parts[0] != prefix:
                raise SourceError('build scripts archive root mismatch')
            if member.isdir():
                continue
            if not member.isfile() or member.size > 8 * CHUNK:
                raise SourceError('invalid build recipe archive entry')
            total += member.size
            if total > 64 * CHUNK:
                raise SourceError('build recipes exceed size limit')
            name = PurePosixPath(*parts[1:]).as_posix()
            if name in files:
                raise SourceError('duplicate build recipe entry')
            with tar.extractfile(member) as stream:
                files[name] = stream.read(member.size + 1)
    return files


def validate_recipes(files: dict, lock: dict):
    source = lock['source']
    audit = source['recipe_audit']['recipes']
    recipes = {item['recipe'] for item in audit}
    if len(recipes) != len(audit):
        raise SourceError('duplicate recipe in source coverage audit')
    _match(recipes, {name for name in files if name.startswith('scripts.d/') and name.endswith('.sh')},
           'complete BtbN recipe coverage')
    for name in ('build.sh', 'download.sh', 'generate.sh', 'util/dl_functions.sh',
                 'util/run_stage.sh', 'util/vars.sh', '.github/workflows/build.yml',
                 'images/base/Dockerfile', 'images/base/git-mini-clone.sh',
                 'images/base-win64/Dockerfile', 'images/base-win64/ct-ng-config',
                 'images/base-win64/toolchain.cmake', 'images/base-win64/cross.meson',
                 'variants/win64-gpl-shared.sh', 'variants/defaults-gpl-shared.sh',
                 'variants/defaults-gpl.sh', 'variants/windows-install-shared.sh', 'addins/8.1.sh'):
        if not files.get(name):
            raise SourceError('missing BtbN build script: ' + name)
    for codec in ('x264', 'x265'):
        if not any(item['recipe'] == f'scripts.d/50-{codec}.sh' and item['enabled'] and item['download'] for item in audit):
            raise SourceError('missing GPL codec source coverage: ' + codec)
    patches = set()
    for content in files.values():
        for match in re.findall(rb'patches/[A-Za-z0-9_./-]+', content):
            name = match.decode().rstrip('/')
            if name not in files and not any(p.startswith(name + '/') for p in files):
                raise SourceError('missing referenced BtbN patch: ' + name)
            patches.add(name)
    return sorted(patches)


def _inspect_snapshot(stream, item, lock):
    source_files = build_files = expanded = 0
    original_lock = None
    with tarfile.open(fileobj=stream, mode='r|xz') as tar:
        for index, member in enumerate(tar):
            tar.members.clear()  # Bound TarInfo retention as well as payload reads.
            # Unix source test fixtures may contain ':' or '\\'. They remain
            # inside the original tar, never native Windows output paths.
            parts = _member_path(member.name, portable=False).parts
            expanded += member.size
            if index > MAX_MEMBERS or expanded > MAX_CACHE_EXPANDED:
                raise SourceError('dependency snapshot exceeds expanded size/member limit')
            # Links stay inside the retained archive; nothing is extracted.
            if not member.isfile() or '.git' in parts:
                continue
            suffix = PurePosixPath(member.name).suffix.lower()
            if suffix in ('.c', '.cc', '.cpp', '.h', '.hpp', '.rs', '.asm', '.s', '.py', '.xml', '.in'):
                source_files += 1
            if PurePosixPath(member.name).name in ('configure', 'configure.ac', 'CMakeLists.txt', 'meson.build', 'Makefile.am', 'Cargo.toml', 'Makefile', 'build.xml', 'makefile.am'):
                build_files += 1
            if item['recipe'] == 'scripts.d/50-rav1e.sh' and parts == ('Cargo.lock',):
                with tar.extractfile(member) as source:
                    original_lock = hashlib.sha256(source.read(1024 * 1024)).hexdigest()
    if not source_files or not build_files:
        raise SourceError('dependency cache lacks source/build payload: ' + item['download'])
    if item['recipe'] == 'scripts.d/50-rav1e.sh':
        _match(original_lock, lock['source']['rav1e']['original_lock_sha256'], 'rav1e original Cargo.lock')
    return {**item, 'cache_entry': '.cache/downloads/' + item['download'],
            'source_files': source_files, 'build_files': build_files,
            'coverage': 'source snapshot and recipe'}


def validate_cache(path: Path, lock: dict) -> list[dict]:
    """Stream ZIP -> cache.tar.gz -> downloads/*.tar.xz; preserve all links."""
    audit = lock['source']['recipe_audit']['recipes']
    expected = {item['download']: item for item in audit if item['download']}
    if len(expected) != sum(bool(item['download']) for item in audit):
        raise SourceError('duplicate dependency source cache recipe')
    found, names, aliases = {}, set(), {}
    expanded = 0
    with zipfile.ZipFile(path) as archive:
        entries = archive.infolist()
        if len(entries) != 1 or entries[0].filename != 'cache.tar.gz':
            raise SourceError('expected single cache.tar.gz in BtbN source artifact')
        if entries[0].file_size > MAX_CACHE_EXPANDED:
            raise SourceError('BtbN source cache exceeds expanded size limit')
        with archive.open(entries[0]) as stream, tarfile.open(fileobj=stream, mode='r|gz') as tar:
            for index, member in enumerate(tar):
                tar.members.clear()  # Bound TarInfo retention as well as payload reads.
                path = _member_path(member.name)
                if index > MAX_MEMBERS:
                    raise SourceError('BtbN source cache exceeds member limit')
                if member.isdir() and path.as_posix() in ('.cache', '.cache/downloads'):
                    continue
                if path.parent.as_posix() != '.cache/downloads' or path.name in names:
                    raise SourceError('unexpected or duplicate BtbN source cache entry')
                names.add(path.name)
                if member.issym():
                    target = _member_path(member.linkname)
                    if len(target.parts) != 1 or not re.fullmatch(r'[A-Za-z0-9_-]+_[0-9a-f]{64}\.tar\.xz', target.name):
                        raise SourceError('unsafe source cache alias')
                    aliases[path.name] = target.name
                    continue
                if not member.isfile() or not re.fullmatch(r'[A-Za-z0-9_-]+_[0-9a-f]{64}\.tar\.xz', path.name):
                    raise SourceError('invalid BtbN source snapshot entry')
                expanded += member.size
                if expanded > MAX_CACHE_EXPANDED:
                    raise SourceError('BtbN source cache exceeds expanded size limit')
                if path.name in expected:
                    with tar.extractfile(member) as snapshot:
                        found[path.name] = _inspect_snapshot(snapshot, expected[path.name], lock)
            # Reading through ZIP EOF also checks its CRC; tar readers stop at
            # tar EOF and otherwise leave the ZIP trailer unchecked.
            while stream.read(CHUNK):
                pass
    if set(expected) - set(found):
        raise SourceError('missing exact BtbN dependency snapshot: ' + sorted(set(expected) - set(found))[0])
    if set(aliases.values()) - names:
        raise SourceError('missing BtbN source cache alias target')
    return [found[item['download']] if item['download'] else
            {**item, 'coverage': 'recipe only; no download stage'}
            for item in audit if item['download'] or item['enabled']]


def cargo_packages(text: str) -> dict[str, str]:
    """Read the fixed Cargo.lock package fields, without requiring third-party TOML."""
    packages = {}
    for block in text.split('[[package]]')[1:]:
        fields = dict(re.findall(r'^(name|version|source|checksum) = "([^"\n]+)"$', block, re.M))
        if fields.get('source', '').startswith('registry+'):
            if fields['source'] != 'registry+https://github.com/rust-lang/crates.io-index':
                raise SourceError('unreviewed rav1e package registry')
            key = fields['name'] + '-' + fields['version']
            if key in packages or not re.fullmatch(r'[0-9a-f]{64}', fields.get('checksum', '')):
                raise SourceError('invalid rav1e package checksum/identity')
            packages[key] = fields['checksum']
        elif 'source' in fields:
            raise SourceError('unrepresented rav1e Git source')
    if not packages:
        raise SourceError('missing rav1e crate source inventory')
    return packages


def collect_rav1e(out: Path, lock: dict) -> list[str]:
    spec = lock['source']['rav1e']
    retained = []
    for field in ('manifest', 'build_lock'):
        name = spec[field]['file']
        if PurePosixPath(name).name != name:
            raise SourceError('invalid rav1e evidence file path')
        path = ROOT / 'packaging' / name
        if not path.is_file() or sha256(path) != spec[field]['sha256']:
            raise SourceError('rav1e source inventory/lock checksum mismatch')
        shutil.copyfile(path, out / name)
        retained.append(name)
    manifest = json.loads((out / retained[0]).read_text(encoding='utf-8'))
    crates = manifest['crates']
    packages = {item['name'] + '-' + item['version']: item['sha256'] for item in crates}
    if len(packages) != len(crates):
        raise SourceError('duplicate rav1e crate source')
    _match(packages, cargo_packages((out / retained[1]).read_text(encoding='utf-8')),
           'rav1e build lock crate coverage')
    recipe = next((r for r in lock['source']['recipe_audit']['recipes']
                   if r['recipe'] == 'scripts.d/50-rav1e.sh'), {})
    _match(manifest['revision'], recipe.get('source_refs', {}).get('COMMIT'), 'rav1e source revision')
    _match(manifest['provenance']['original_lock_sha256'], spec['original_lock_sha256'],
           'rav1e original lock provenance')
    directory = out / 'rav1e-crates'
    directory.mkdir()
    def capture(item):
        name, version, digest = item['name'], item['version'], item['sha256']
        if not re.fullmatch(r'[A-Za-z0-9_-]+', name) or not re.fullmatch(r'[A-Za-z0-9.+-]+', version):
            raise SourceError('unsafe rav1e crate identity')
        filename = name + '-' + version + '.crate'
        fetch(f'https://static.crates.io/crates/{name}/{filename}', directory / filename,
              digest, 64 * CHUNK)
        return 'rav1e-crates/' + filename
    with ThreadPoolExecutor(max_workers=6) as pool:
        retained.extend(pool.map(capture, crates))
    helper = 'source_windows_rav1e.py'
    shutil.copyfile(Path(__file__).with_name(helper), out / helper)
    retained.append(helper)
    return retained


def collect_rust_std(out: Path, lock: dict) -> dict:
    spec = lock['source']['rust_std']
    path = out / 'rust-runtime-source.tar.xz'
    fetch(spec['url'], path, spec['sha256'], MAX_ARCHIVE_BYTES)
    required = set(spec['required_files'])
    expanded = 0
    with tarfile.open(path, 'r|xz') as archive:
        for index, member in enumerate(archive):
            archive.members.clear()
            parts = _member_path(member.name, portable=False).parts
            if not parts or parts[0] != spec['archive_root']:
                raise SourceError('Rust runtime source archive root mismatch')
            expanded += member.size
            if index > MAX_MEMBERS or expanded > MAX_CACHE_EXPANDED:
                raise SourceError('Rust runtime source archive exceeds size limit')
            if member.isfile() and member.size:
                required.discard(PurePosixPath(*parts[1:]).as_posix())
    if required:
        raise SourceError('Rust runtime archive lacks required source/build/vendor payload')
    return {'id': 'windows-rust-standard-library', 'source': spec['url'],
            'version': spec['version'], 'revision': spec['revision'],
            'provenance': spec['provenance'],
            'files': ['rust-runtime-source.tar.xz', 'runtime-source-lock.json']}


def collect_compiler_runtimes(out: Path, lock: dict) -> list[dict]:
    import compiler_runtime_sources as compiler
    expected = lock['source']['compiler_runtime_lock']
    if compiler.LOCK.name != expected['file'] or sha256(compiler.LOCK) != expected['sha256']:
        raise SourceError('compiler runtime source lock checksum mismatch')
    runtime_lock = json.loads(compiler.LOCK.read_text(encoding='utf-8')).get('runtime', {})
    source = lock['source']
    for key, value in {'repository': source['repository'],
                       'build_scripts_commit': lock['build_scripts_commit'],
                       'run_id': source['run_id'], 'target': source['target'],
                       'variant': source['variant'], 'addin': ' '.join(source['addins'])}.items():
        _match(runtime_lock.get(key), value, 'compiler runtime ' + key)
    runtime = out / 'compiler-runtime'
    runtime.mkdir()
    records = compiler.collect(runtime)
    if not records:
        raise SourceError('missing linked compiler runtime source providers')
    return [{**record, 'files': ['compiler-runtime/' + name for name in record['files']]}
            for record in records]


def _require_complete(source):
    completeness = source.get('completeness', {})
    if completeness.get('status') != 'complete' or completeness.get('missing'):
        missing = completeness.get('missing') or ['missing reviewed source completeness evidence']
        raise SourceError('Windows corresponding source is incomplete: ' + '; '.join(missing))


def collect(stage: Path, out: Path) -> list[dict]:
    """Shared native collector entry point. Never returns success for partial source."""
    lock = json.loads(LOCK.read_text(encoding='utf-8'))
    validate_stage(stage, lock)
    if not out.is_dir() or any(out.iterdir()):
        raise SourceError('Windows source output must be a fresh empty directory')
    # Fail before a multi-GB fetch when the reviewed lock already records a gap.
    _require_complete(lock['source'])
    return collect_pinned_inputs(stage, out, lock)


def collect_pinned_inputs(stage: Path, out: Path, lock: dict) -> list[dict]:
    """Capture reviewed inputs for auditing; this alone is NOT a completeness claim.

    Exposed separately for investigation and integration tests. Production uses
    collect(), which also enforces the reviewed completeness gate above.
    """
    validate_stage(stage, lock)
    if not out.is_dir() or any(out.iterdir()):
        raise SourceError('Windows source output must be a fresh empty directory')
    source = lock['source']
    provenance = validate_provenance(lock)
    _write_json(out / 'runtime-source-lock.json', lock)
    _write_json(out / 'github-provenance.json', provenance)
    shutil.copy2(stage / 'SOURCE-INFO.txt', out / 'SOURCE-INFO.txt')
    records = []
    for key, filename in (('ffmpeg', 'ffmpeg.tar.gz'), ('build_scripts', 'btbn-build-scripts.tar.gz')):
        spec = source[key]
        fetch(spec['url'], out / filename, spec['sha256'], MAX_ARCHIVE_BYTES)
    validate_ffmpeg(out / 'ffmpeg.tar.gz', source['ffmpeg']['revision'])
    scripts = read_scripts(out / 'btbn-build-scripts.tar.gz', lock['build_scripts_commit'])
    patches = validate_recipes(scripts, lock)
    records.extend(collect_compiler_runtimes(out, lock))
    records.append(collect_rust_std(out, lock))
    cache = source['cache']
    endpoint = f"repos/{source['repository']}/actions/artifacts/{cache['id']}/zip"
    # Resolve the API redirect even with a local cache, so artifact access and
    # retention are revalidated on every actual collection.
    fetch(artifact_url(endpoint, cache['public_url']), out / 'dependency-source-cache.zip',
             cache['digest'].removeprefix('sha256:'), cache['size_in_bytes'],
             exact_size=cache['size_in_bytes'])
    coverage = validate_cache(out / 'dependency-source-cache.zip', lock)
    rav1e_files = collect_rav1e(out, lock)
    _write_json(out / 'recipe-source-coverage.json', {'recipes': coverage, 'patches': patches,
                'completeness': source['completeness']})
    (out / 'REBUILD-SOURCES.txt').write_text(
        'Windows corresponding-source inputs\n\n'
        'The runtime-source-lock.json and github-provenance.json bind these inputs '
        'to the exact BtbN run and win64 gpl-shared 8.1 variant.\n'
        'btbn-build-scripts.tar.gz retains the original recipes, patches, image '
        'Dockerfiles, toolchain configuration, addins and build/package scripts.\n'
        'dependency-source-cache.zip contains cache.tar.gz, which contains '
        '.cache/downloads/<recipe>_<recipe-hash>.tar.xz source snapshots. '
        'Extract on a Unix filesystem to preserve source/Git/submodule symlinks.\n'
        'ffmpeg.tar.gz is the exact FFmpeg revision in the lock. The upstream '
        'build.sh normally clones a branch: use this retained tree when rebuilding, '
        'rather than resolving the branch again.\n'
        'rav1e uses the original Cargo.lock inside its source snapshot, followed '
        'by cargo update cc in the original recipe. source-windows-rav1e-Cargo.lock '
        'is an explicitly reconstructed post-update build lock, supported by the '
        'immutable OCI build-layer evidence in source-windows-rav1e.json. '
        'After extracting the rav1e source snapshot, run '
        'python source_windows_rav1e.py <rav1e-source-tree> to install that lock '
        'and vendor the retained .crate sources for offline Cargo use.\n'
        'rust-runtime-source.tar.xz retains exact Rust standard-library and '
        'compiler-builtins source, submodules and library vendor dependencies.\n'
        'compiler-runtime/ retains GCC/MinGW runtime source and the cross-toolchain '
        'build scripts/patches; its provenance describes external compiler and '
        'OS build-tool prerequisites.\n', encoding='utf-8')
    shared = ['runtime-source-lock.json', 'github-provenance.json', 'SOURCE-INFO.txt',
              'btbn-build-scripts.tar.gz', 'recipe-source-coverage.json', 'REBUILD-SOURCES.txt']
    records.append({'id': 'windows-ffmpeg', 'source': 'https://github.com/FFmpeg/FFmpeg',
                    'version': source['ffmpeg']['revision'], 'provider': 'BtbN/FFmpeg-Builds',
                    'runtime_url': lock['url'], 'runtime_sha256': lock['sha256'],
                    'provenance': {'run_id': source['run_id'], 'target': source['target'],
                                   'variant': source['variant'], 'addins': source['addins']},
                    'files': ['ffmpeg.tar.gz', *shared]})
    records.append({'id': 'windows-rav1e-crates', 'source': 'https://crates.io',
                    'version': source['rav1e']['manifest']['sha256'],
                    'provenance': 'Exact registry archives recovered from the pinned rav1e build layer; build Cargo.lock is explicitly reconstructed.',
                    'files': [*rav1e_files, *shared]})
    for item in coverage:
        records.append({'id': 'windows-btbn-' + item['recipe'].removeprefix('scripts.d/').removesuffix('.sh').replace('/', '-'),
                        'source': item.get('source_refs', {'recipe': item['recipe']}),
                        'version': lock['build_scripts_commit'],
                        'provenance': {key: item[key] for key in ('recipe', 'enabled', 'coverage')},
                        'cache_entry': item.get('cache_entry'),
                        'files': [*shared, *(['dependency-source-cache.zip'] if item.get('cache_entry') else [])]})
    return records
