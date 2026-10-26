#!/usr/bin/env python3
"""Capture the exact BtbN Windows sources; refuse an incomplete source release.

Uses only Python's standard library on Windows and Linux. GH_TOKEN is used only
on api.github.com with redirects disabled. The artifact download is a separate,
unauthenticated request. No signed URL or token is saved in provenance/errors.
The source lock deliberately remains blocked until the missing cached-build
inputs identified there have been recovered and reviewed.
"""
from __future__ import annotations

import hashlib
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
        if not token:
            raise SourceError('GH_TOKEN is required for the pinned GitHub source artifact')
        headers['Authorization'] = 'Bearer ' + token
    return urllib.request.Request(url, headers=headers)


def _open(request):
    # NoRedirect prevents credentials crossing hosts and prevents HTTPS downgrades.
    return urllib.request.build_opener(NoRedirect).open(request, timeout=60)


def api_json(endpoint: str) -> dict:
    try:
        with _open(_request('https://api.github.com/' + endpoint, api=True)) as response:
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


def artifact_url(endpoint: str) -> str:
    """Consume the authenticated API redirect without following it."""
    try:
        with _open(_request('https://api.github.com/' + endpoint, api=True)):
            raise SourceError('expected GitHub artifact redirect')
    except urllib.error.HTTPError as exc:
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


def _member_path(name):
    path = PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts or '\\' in name or ':' in name:
        raise SourceError('unsafe path in pinned source archive')
    return path


def read_scripts(archive: Path, revision: str) -> dict[str, bytes]:
    """Read bounded text recipes/patches without filesystem extraction on Windows."""
    files = {}
    prefix = 'FFmpeg-Builds-' + revision
    total = 0
    with tarfile.open(archive, 'r|gz') as tar:
        for member in tar:
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


def validate_cache(path: Path, lock: dict) -> list[dict]:
    """Map all recipes to immutable cache entries; inspect sources without extracting.

    The ZIP stays intact to preserve Unix symlinks, Git metadata and submodules
    which cannot always be extracted faithfully on a native Windows host.
    """
    audit = lock['source']['recipe_audit']['recipes']
    with zipfile.ZipFile(path) as archive:
        entries = {}
        total = 0
        for entry in archive.infolist():
            _member_path(entry.filename)
            if entry.is_dir():
                continue
            if entry.filename != PurePosixPath(entry.filename).name or entry.filename in entries:
                raise SourceError('unexpected or duplicate BtbN cache entry')
            total += entry.file_size
            if total > MAX_CACHE_EXPANDED:
                raise SourceError('BtbN source cache exceeds expanded size limit')
            entries[entry.filename] = entry
        results = []
        for item in audit:
            name = item['download']
            if not name:
                if item['enabled']:
                    results.append({**item, 'coverage': 'recipe only; no download stage'})
                continue
            if name not in entries:
                raise SourceError('missing exact BtbN dependency snapshot: ' + name)
            if not re.fullmatch(r'[A-Za-z0-9_-]+_[0-9a-f]{64}\.tar\.xz', name):
                raise SourceError('invalid pinned BtbN cache filename')
            # Upload-artifact preserves symlinks as files with the target contents.
            # We require the hashed real archive, never the unversioned alias.
            source_files = build_files = 0
            expanded = 0
            with archive.open(entries[name]) as stream, tarfile.open(fileobj=stream, mode='r|xz') as tar:
                for member in tar:
                    parts = _member_path(member.name).parts
                    expanded += member.size
                    if expanded > MAX_CACHE_EXPANDED:
                        raise SourceError('dependency snapshot exceeds expanded size limit')
                    if member.isfile() and '.git' not in parts:
                        suffix = PurePosixPath(member.name).suffix.lower()
                        if suffix in ('.c', '.cc', '.cpp', '.h', '.hpp', '.rs', '.asm', '.s', '.py', '.xml', '.in'):
                            source_files += 1
                        if PurePosixPath(member.name).name in ('configure', 'configure.ac', 'CMakeLists.txt', 'meson.build', 'Makefile.am', 'Cargo.toml', 'Makefile', 'build.xml', 'makefile.am'):
                            build_files += 1
            if not source_files or not build_files:
                raise SourceError('dependency cache lacks source/build payload: ' + name)
            results.append({**item, 'cache_entry': name, 'source_files': source_files,
                            'build_files': build_files, 'coverage': 'source snapshot and recipe'})
    return results


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
        download(spec['url'], out / filename, spec['sha256'], MAX_ARCHIVE_BYTES)
    scripts = read_scripts(out / 'btbn-build-scripts.tar.gz', lock['build_scripts_commit'])
    patches = validate_recipes(scripts, lock)
    cache = source['cache']
    endpoint = f"repos/{source['repository']}/actions/artifacts/{cache['id']}/zip"
    download(artifact_url(endpoint), out / 'dependency-source-cache.zip',
             cache['digest'].removeprefix('sha256:'), cache['size_in_bytes'],
             exact_size=cache['size_in_bytes'])
    coverage = validate_cache(out / 'dependency-source-cache.zip', lock)
    _write_json(out / 'recipe-source-coverage.json', {'recipes': coverage, 'patches': patches,
                'completeness': source['completeness']})
    shared = ['runtime-source-lock.json', 'github-provenance.json', 'SOURCE-INFO.txt',
              'btbn-build-scripts.tar.gz', 'recipe-source-coverage.json']
    records.append({'id': 'windows-ffmpeg', 'source': 'https://github.com/FFmpeg/FFmpeg',
                    'version': source['ffmpeg']['revision'], 'provider': 'BtbN/FFmpeg-Builds',
                    'runtime_url': lock['url'], 'runtime_sha256': lock['sha256'],
                    'provenance': {'run_id': source['run_id'], 'target': source['target'],
                                   'variant': source['variant'], 'addins': source['addins']},
                    'files': ['ffmpeg.tar.gz', *shared]})
    for item in coverage:
        records.append({'id': 'windows-btbn-' + item['recipe'].removeprefix('scripts.d/').removesuffix('.sh').replace('/', '-'),
                        'source': item.get('source_refs', {'recipe': item['recipe']}),
                        'version': lock['build_scripts_commit'],
                        'provenance': {key: item[key] for key in ('recipe', 'enabled', 'coverage')},
                        'cache_entry': item.get('cache_entry'),
                        'files': [*shared, *(['dependency-source-cache.zip'] if item.get('cache_entry') else [])]})
    return records
