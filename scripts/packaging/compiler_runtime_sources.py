#!/usr/bin/env python3
"""Collect pinned Windows compiler runtime source without executing a compiler.

Archives stay intact, including Unix links and vendored source. Only small,
explicitly selected build evidence files are copied out for inspection. Downloads
are public HTTPS requests and never use GitHub credentials or an OCI image.
"""
from __future__ import annotations

import bz2
import gzip
import hashlib
import json
import lzma
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import tarfile
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
LOCK = ROOT / 'packaging/windows-toolchain-sources.json'
CHUNK = 1024 * 1024
MAX_DOWNLOAD = 512 * 1024 * 1024
MAX_EXPANDED = 12 * 1024 * 1024 * 1024
MAX_MEMBERS = 500_000
MAX_EVIDENCE = 1024 * 1024
REQUIRED_PROVIDERS = {'gcc', 'mingw-w64', 'crosstool-ng', 'btbn-toolchain-recipes',
                      'gcc-rust', 'mingw-w64-rust', 'rust-mingw-build-recipes'}


class SourceError(RuntimeError):
    """Exact runtime source is missing, unsafe, or inconsistent with its lock."""


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def safe_path(name: str) -> str:
    if (not isinstance(name, str) or not name or name.startswith('/')
            or '\\' in name or ':' in name or any(ord(c) < 32 for c in name)
            or any(p in ('', '.', '..') for p in name.split('/'))):
        raise SourceError('unsafe compiler runtime source path')
    return name


def _https(url: str):
    parsed = urllib.parse.urlsplit(url)
    if (parsed.scheme != 'https' or not parsed.hostname or parsed.username
            or parsed.password or parsed.fragment):
        raise SourceError('compiler runtime source requires HTTPS')


def _open(url: str):
    _https(url)
    request = urllib.request.Request(url, headers={'User-Agent': 'jvid-corresponding-source'})
    return urllib.request.build_opener(NoRedirect).open(request, timeout=60)


def file_hashes(path: Path) -> tuple[int, str, str]:
    sha256, sha512, size = hashlib.sha256(), hashlib.sha512(), 0
    with path.open('rb') as stream:
        while block := stream.read(CHUNK):
            size += len(block)
            sha256.update(block)
            sha512.update(block)
    return size, sha256.hexdigest(), sha512.hexdigest()


def verify_file(path: Path, source: dict):
    if not path.is_file() or path.stat().st_size != source['size']:
        raise SourceError('compiler runtime source size/checksum mismatch: ' + source['id'])
    size, sha256, sha512 = file_hashes(path)
    if (size != source['size'] or sha256 != source['sha256']
            or (source.get('sha512') and sha512 != source['sha512'])):
        raise SourceError('compiler runtime source size/checksum mismatch: ' + source['id'])


def fetch(source: dict, destination: Path):
    """Download to disk in bounded chunks; rehash even optional local cache hits."""
    cache = os.environ.get('JVID_WINDOWS_SOURCE_CACHE')
    cached = Path(cache) / source['sha256'] if cache else None
    if cached and cached.is_file():
        verify_file(cached, source)
        shutil.copyfile(cached, destination)
        return
    part = destination.with_name(destination.name + '.part')
    url, start = source['url'], time.monotonic()
    try:
        for _ in range(6):
            try:
                response = _open(url)
                break
            except urllib.error.HTTPError as exc:
                if exc.code not in (301, 302, 303, 307, 308) or not exc.headers.get('Location'):
                    raise SourceError('compiler runtime source download failed: ' + source['id']) from None
                url = urllib.parse.urljoin(url, exc.headers['Location'])
                _https(url)
        else:
            raise SourceError('too many compiler runtime source redirects')
        size = 0
        with response, part.open('wb') as output:
            while block := response.read(CHUNK):
                size += len(block)
                if size > source['size'] or time.monotonic() - start > 3600:
                    raise SourceError('compiler runtime source exceeds size/time limit')
                output.write(block)
        verify_file(part, source)
        part.replace(destination)
    except (urllib.error.URLError, OSError):
        # Network exception strings can contain temporary signed redirect URLs.
        raise SourceError('compiler runtime source download failed: ' + source['id']) from None
    finally:
        part.unlink(missing_ok=True)


def _unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise SourceError('duplicate compiler runtime lock field')
        result[key] = value
    return result


def read_lock() -> dict:
    try:
        lock = json.loads(LOCK.read_text(encoding='utf-8'), object_pairs_hook=_unique)
        if lock['schema_version'] != 1:
            raise SourceError('unsupported compiler runtime source lock')
        sources = lock['sources']
        if not isinstance(sources, list) or not sources:
            raise SourceError('missing compiler runtime source providers')
        ids, filenames = set(), set()
        for source in sources:
            identity = source['id']
            if not re.fullmatch(r'[a-z0-9-]+', identity) or identity in ids:
                raise SourceError('duplicate or invalid compiler runtime provider')
            ids.add(identity)
            name = safe_path(source['filename'])
            if '/' in name or name in filenames:
                raise SourceError('duplicate or invalid compiler runtime archive filename')
            filenames.add(name)
            safe_path(source['archive_root'])
            _https(source['url'])
            if (not re.fullmatch(r'[0-9a-f]{64}', source['sha256'])
                    or (source.get('sha512') and not re.fullmatch(r'[0-9a-f]{128}', source['sha512']))
                    or type(source['size']) is not int or not 0 < source['size'] <= MAX_DOWNLOAD
                    or not source['version'] or not source['provenance']):
                raise SourceError('missing pinned compiler runtime version/size/checksum/provenance')
            required = source['required_files']
            if not isinstance(required, list) or not required or len(set(required)) != len(required):
                raise SourceError('missing or duplicate compiler runtime archive requirements')
            for path in required:
                safe_path(path)
            for path in source.get('retain', []):
                safe_path(path)
                if path not in required:
                    raise SourceError('unvalidated compiler runtime build evidence')
            for path, checksum in source.get('content_sha256', {}).items():
                safe_path(path)
                if path not in required or not re.fullmatch(r'[0-9a-f]{64}', checksum):
                    raise SourceError('unvalidated compiler runtime build evidence checksum')
        if not REQUIRED_PROVIDERS <= ids:
            raise SourceError('missing compiler runtime source providers')
        if lock.get('completeness', {}).get('status') != 'complete':
            raise SourceError('compiler runtime source provenance incomplete: ' +
                              '; '.join(lock.get('completeness', {}).get('missing', [])))
        return lock
    except (KeyError, TypeError, ValueError, OSError):
        raise SourceError('invalid or missing compiler runtime source lock') from None


def _safe_link(name: str, target: str, root: str, hardlink: bool):
    if (not target or target.startswith('/') or '\\' in target or ':' in target
            or any(ord(c) < 32 for c in target)):
        raise SourceError('unsafe compiler runtime archive link')
    parts = [] if hardlink else name.split('/')[:-1]
    for part in target.split('/'):
        if part == '..':
            if len(parts) <= 1:
                raise SourceError('compiler runtime archive link escapes source root')
            parts.pop()
        elif part not in ('', '.'):
            parts.append(part)
    if not parts or parts[0] != root:
        raise SourceError('compiler runtime archive link escapes source root')


def inspect_archive(path: Path, source: dict) -> dict[str, bytes]:
    """Validate required source files and return only small build evidence files.

    Explicit decompression handles concatenated bzip2 streams on Python 3.10.
    Clearing TarInfo history bounds memory even for the full Rust vendor tree.
    No archive member is extracted to a filesystem or executed.
    """
    root = source['archive_root']
    required = set(source['required_files'])
    selected = set(source.get('retain', [])) | set(source.get('content_sha256', {}))
    captured, found, total, count = {}, set(), 0, 0
    opener = bz2.open if path.name.endswith('.bz2') else lzma.open if path.name.endswith('.xz') else gzip.open
    try:
        with opener(path, 'rb') as decompressed, tarfile.open(fileobj=decompressed, mode='r|') as archive:
            for member in archive:
                count += 1
                total += member.size
                if count > MAX_MEMBERS or total > MAX_EXPANDED:
                    raise SourceError('compiler runtime source archive exceeds size/member limit')
                name = safe_path(member.name.rstrip('/'))
                if name != root and not name.startswith(root + '/'):
                    raise SourceError('compiler runtime archive has an unexpected source root')
                if member.issym() or member.islnk():
                    _safe_link(name, member.linkname, root, member.islnk())
                elif not member.isfile() and not member.isdir():
                    raise SourceError('unsupported compiler runtime archive member')
                relative = name[len(root) + 1:]
                if relative in required:
                    if not member.isfile() or relative in found or member.size == 0:
                        raise SourceError('invalid or duplicate required compiler runtime source file')
                    found.add(relative)
                if relative in selected:
                    if not member.isfile() or member.size > MAX_EVIDENCE:
                        raise SourceError('invalid compiler runtime build evidence file')
                    captured[relative] = archive.extractfile(member).read(MAX_EVIDENCE + 1)
                archive.members.clear()
        if found != required:
            raise SourceError('missing compiler runtime source file: ' + ', '.join(sorted(required - found)))
        for name, expected in source.get('content_sha256', {}).items():
            if hashlib.sha256(captured[name]).hexdigest() != expected:
                raise SourceError('compiler runtime build evidence checksum mismatch')
        return captured
    except (tarfile.TarError, EOFError, OSError, lzma.LZMAError):
        raise SourceError('invalid compiler runtime source archive: ' + source['id']) from None


def _validate_checksums(sources: list[dict], evidence: dict[str, dict[str, bytes]]):
    for source in sources:
        origin = source.get('checksum_source')
        if not origin:
            continue
        data = evidence[origin['id']][origin['path']].decode('ascii')
        records = {tuple(line.split()[:2]): line.split()[2]
                   for line in data.splitlines() if len(line.split()) == 3}
        for algorithm in ('sha256', 'sha512'):
            if source.get(algorithm) and records.get((algorithm, source['filename'])) != source[algorithm]:
                raise SourceError('compiler runtime checksum differs from pinned upstream recipe')


def collect(out: Path) -> list[dict]:
    """Collect exact source inputs into fresh output; return relative provider files."""
    if out.is_symlink() or (out.exists() and (not out.is_dir() or any(out.iterdir()))):
        raise SourceError('compiler runtime capture requires a fresh empty output directory')
    lock = read_lock()
    out.mkdir(parents=True, exist_ok=True)
    archives = out / 'sources'
    archives.mkdir()
    evidence, providers = {}, []
    for source in lock['sources']:
        archive = archives / source['filename']
        fetch(source, archive)
        # A fixture/custom download implementation must not bypass integrity.
        verify_file(archive, source)
        evidence[source['id']] = inspect_archive(archive, source)
        files = [archive.relative_to(out).as_posix()]
        for name in source.get('retain', []):
            destination = out / 'build-evidence' / source['id'] / PurePosixPath(name)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(evidence[source['id']][name])
            files.append(destination.relative_to(out).as_posix())
        providers.append({'id': 'windows-' + source['id'], 'version': source['version'],
                          'provenance': {'runtime': lock['runtime'], 'source': source}, 'files': files})
    _validate_checksums(lock['sources'], evidence)
    receipt = out / 'SOURCE-PROVENANCE.json'
    receipt.write_text(json.dumps(lock, indent=2, sort_keys=True) + '\n', encoding='utf-8')
    providers[0]['files'].append(receipt.name)
    return providers
