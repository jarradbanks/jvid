#!/usr/bin/env python3
"""Verify retained Windows inputs after upstream Actions artifacts expire."""
from datetime import datetime
import json
from pathlib import Path, PurePosixPath
import tempfile

from reassemble_sources import unique_object
import source_windows as windows


def metadata(lock):
    spec = lock['source']['provenance']
    name = spec['file']
    if PurePosixPath(name).name != name or '\\' in name or ':' in name:
        raise windows.SourceError('invalid retained provenance path')
    path = windows.ROOT / 'packaging' / name
    if not path.is_file() or windows.sha256(path) != spec['sha256']:
        raise windows.SourceError('retained provenance SHA-256 mismatch')
    value = json.loads(path.read_text(encoding='utf-8'), object_pairs_hook=unique_object)
    if value.get('schema') != 1 or not isinstance(value.get('records'), dict):
        raise windows.SourceError('invalid retained GitHub provenance')
    try:
        captured = datetime.fromisoformat(value['captured_at'].replace('Z', '+00:00'))
        cache = lock['source']['cache']
        endpoint = f"repos/{lock['source']['repository']}/actions/artifacts/{cache['id']}"
        artifact = value['records'][endpoint]
        created = datetime.fromisoformat(artifact['created_at'].replace('Z', '+00:00'))
        expires = datetime.fromisoformat(artifact['expires_at'].replace('Z', '+00:00'))
        if captured.tzinfo is None or not created <= captured < expires:
            raise ValueError('capture outside original artifact lifetime')
    except (KeyError, TypeError, ValueError):
        raise windows.SourceError('invalid retained provenance capture time') from None

    def read(endpoint):
        record = value['records'].get(endpoint)
        if not isinstance(record, dict):
            raise windows.SourceError('missing retained GitHub provenance record')
        return record

    return read


def fetch_cache(cache, destination):
    parts = cache['retained_parts']
    if (not isinstance(parts, list) or not 1 <= len(parts) <= 40
            or any(type(part.get('size')) is not int or not 0 < part['size'] <= 1024**3 for part in parts)
            or sum(part['size'] for part in parts) != cache['size_in_bytes']):
        raise windows.SourceError('invalid retained source cache parts')
    partial = destination.with_name(destination.name + '.part')
    try:
        with tempfile.TemporaryDirectory(dir=destination.parent) as temporary, partial.open('xb') as output:
            for index, part in enumerate(parts):
                path = Path(temporary) / str(index)
                windows.fetch(part['url'], path, part['sha256'], part['size'], exact_size=part['size'])
                with path.open('rb') as source:
                    while block := source.read(windows.CHUNK):
                        output.write(block)
                path.unlink()
        if windows.sha256(partial) != cache['digest'].removeprefix('sha256:'):
            raise windows.SourceError('retained source cache SHA-256 mismatch')
        partial.replace(destination)
    finally:
        partial.unlink(missing_ok=True)
