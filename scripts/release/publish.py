#!/usr/bin/env python3
"""Publish only a complete, verified draft after the protected release approval."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import urllib.parse

from automation import output, run, stable, version
from github_api import github, pages, release_for_tag, tag_commit

SUFFIXES = (
    ('x86_64-pc-windows-msvc', 'windows-installer', 'windows-x86_64-setup.exe'),
    ('x86_64-pc-windows-msvc', 'windows-portable', 'windows-x86_64-portable.zip'),
    ('aarch64-apple-darwin', 'macos-dmg', 'macos-aarch64.dmg'),
    ('aarch64-apple-darwin', 'macos-portable', 'macos-aarch64-portable.zip'),
    ('x86_64-unknown-linux-gnu', 'linux-portable', 'linux-x86_64-portable.tar.gz'),
)


def publication_requested(event, ref, opt_in, enabled):
    return (enabled == 'true' and ref.startswith('refs/tags/v')
            and (event == 'push' or (event == 'workflow_dispatch' and opt_in == 'true')))


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(block)
    return value.hexdigest()


def preflight(root, repository, commit, tag):
    stable(version(root))
    if tag != 'v' + version(root) or run('git', 'rev-parse', 'HEAD', cwd=root) != commit:
        raise ValueError('Release tag, checkout and workspace version must match')
    if tag_commit(repository, tag, api=github) != commit:
        raise ValueError('Release tag must already identify this exact commit')
    release = release_for_tag(repository, tag, api=github)
    return release


def verify_sources(root, assets):
    subprocess.run([sys.executable, str(root / 'scripts/packaging/assert_assets.py'), str(assets)],
                   cwd=root, check=True)
    subprocess.run([sys.executable, str(root / 'scripts/packaging/sources.py'), 'release',
                    '--sources', str(assets), '--native', str(assets), '--output', str(assets), '--verify-only'],
                   cwd=root, check=True)


def verify_signature(assets, public_key):
    if not re.fullmatch(r'[0-9a-fA-F]{64}', public_key):
        raise ValueError('A pinned public verification key is required')
    # RFC 8410 SubjectPublicKeyInfo for an Ed25519 public key. No private key is
    # read here; the only seed consumer remains the protected metadata step.
    with tempfile.TemporaryDirectory() as temporary:
        public = Path(temporary) / 'public.der'
        public.write_bytes(bytes.fromhex('302a300506032b6570032100' + public_key))
        subprocess.run(['openssl', 'pkeyutl', '-verify', '-pubin', '-keyform', 'DER',
                        '-inkey', str(public), '-rawin', '-in', str(assets / 'jvid-update.json'),
                        '-sigfile', str(assets / 'jvid-update.json.sig')],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def validate_assets(root, assets, public_key):
    verify_sources(root, assets)
    current = version(root)
    prefix = 'jvid-' + current
    native = {prefix + '-' + suffix: (target, distribution) for target, distribution, suffix in SUFFIXES}
    expected = {*native, 'jvid-update.json', 'jvid-update.json.sig', 'SHA256SUMS.txt',
                prefix + '-source-manifest.json', 'SOURCE-README.txt', 'reassemble_sources.py'}
    index = json.loads((assets / (prefix + '-source-manifest.json')).read_text())
    for bundle in index['bundles']:
        expected.add(bundle['manifest'])
        expected.update(part['name'] for part in bundle['parts'])
    entries = list(assets.iterdir())
    if (set(p.name for p in entries) != expected
            or any(p.is_symlink() or not p.is_file() or p.stat().st_size == 0 for p in entries)):
        raise ValueError('Unexpected, empty, linked or missing release asset')
    records = {p.name: {'size': p.stat().st_size, 'sha256': digest(p)} for p in entries}
    sums = {}
    for line in (assets / 'SHA256SUMS.txt').read_text().splitlines():
        match = re.fullmatch(r'([0-9a-f]{64})  ([A-Za-z0-9._-]+)', line)
        if not match or match[2] in sums:
            raise ValueError('Invalid or duplicate checksum entry')
        sums[match[2]] = match[1]
    if sums != {name: r['sha256'] for name, r in records.items() if name != 'SHA256SUMS.txt'}:
        raise ValueError('Release checksums do not match the exact expected assets')
    metadata = json.loads((assets / 'jvid-update.json').read_text())
    notes = (root / 'packaging/release-notes.md').read_text()
    if (metadata.get('schema') != 1 or metadata.get('repository') != 'jarradbanks/jvid'
            or metadata.get('version') != current or metadata.get('notes') != notes):
        raise ValueError('Updater metadata does not match this release')
    observed = {}
    for asset in metadata['assets']:
        name = asset['name']
        if (name in observed or name not in native or (asset['target'], asset['distribution']) != native[name]
                or {'size': asset['size'], 'sha256': asset['sha256']} != records[name]):
            raise ValueError('Updater metadata does not match native asset bytes')
        observed[name] = asset
    if set(observed) != set(native) or records['jvid-update.json.sig']['size'] != 64:
        raise ValueError('Incomplete native metadata or invalid detached signature')
    verify_signature(assets, public_key)
    return records


def upload(repository, release_id, path):
    url = (f'https://uploads.github.com/repos/{repository}/releases/{release_id}/assets?'
           + urllib.parse.urlencode({'name': path.name}))
    # gh streams the file rather than loading multi-GiB source assets into RAM.
    subprocess.run(['gh', 'api', '--method', 'POST', url, '--header',
                    'Content-Type: application/octet-stream', '--input', str(path)],
                   check=True, stdout=subprocess.DEVNULL, timeout=1200)


def matches(asset, record):
    return (asset.get('state') == 'uploaded' and asset.get('size') == record['size']
            and asset.get('digest') == 'sha256:' + record['sha256'])


def publish(root, assets, repository, commit, tag, public_key):
    release = preflight(root, repository, commit, tag)
    if release and not release['draft']:
        output(published='false', reason='Published release is immutable; no changes made')
        return
    records = validate_assets(root, assets, public_key)
    notes = (root / 'packaging/release-notes.md').read_text()
    if release is None:
        release = github(f'repos/{repository}/releases', 'POST', {'tag_name': tag,
            'target_commitish': commit, 'name': 'jvid ' + tag, 'body': notes,
            'draft': True, 'prerelease': False})
    release_id = release['id']
    release_path = f'repos/{repository}/releases/{release_id}'

    def assert_draft():
        current = github(release_path)
        if (not current or not current['draft'] or current.get('immutable')
                or current['tag_name'] != tag or current['target_commitish'] != commit
                or tag_commit(repository, tag, api=github) != commit):
            raise ValueError('Release changed or was published; refusing to modify it')
        return current

    def remote_assets():
        observed = {}
        for asset in pages(release_path + '/assets', api=github):
            name = asset['name']
            if name not in records or name in observed:
                raise ValueError('Unexpected or duplicate asset already in draft')
            observed[name] = asset
        return observed

    assert_draft()
    observed = remote_assets()
    # Never replace successfully uploaded but conflicting bytes, even in a draft.
    # Only GitHub's incomplete starter objects may be removed for a safe retry.
    for name, asset in observed.items():
        if asset['state'] != 'starter' and not matches(asset, records[name]):
            raise ValueError('Existing draft asset has conflicting size/digest/state: ' + name)
    for name, record in sorted(records.items()):
        asset = observed.get(name)
        if asset and matches(asset, record):
            continue
        assert_draft()
        if asset:
            github(f'repos/{repository}/releases/assets/{asset["id"]}', 'DELETE')
        upload(repository, release_id, assets / name)
    assert_draft()
    observed = remote_assets()
    if set(observed) != set(records) or any(not matches(observed[n], r) for n, r in records.items()):
        raise ValueError('Uploaded release asset names/sizes/digests failed verification')
    # Recheck local bytes and signatures too, immediately before publication.
    if validate_assets(root, assets, public_key) != records:
        raise ValueError('Local assets changed while uploading')
    assert_draft()
    observed = remote_assets()
    if set(observed) != set(records) or any(not matches(observed[n], r) for n, r in records.items()):
        raise ValueError('Remote release assets changed before publication')
    result = github(release_path, 'PATCH', {'name': 'jvid ' + tag, 'body': notes,
                    'draft': False, 'prerelease': False, 'make_latest': 'true'})
    if result['draft'] or result['tag_name'] != tag:
        raise RuntimeError('GitHub did not confirm publication; inspect and retry the original job')
    output(published='true', tag=tag)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['status', 'preflight', 'publish'])
    parser.add_argument('--root', type=Path, default=Path.cwd())
    parser.add_argument('--assets', type=Path, default=Path('release-assets'))
    args = parser.parse_args()
    env = os.environ
    requested = publication_requested(env['GITHUB_EVENT_NAME'], env['GITHUB_REF'],
                                      env.get('INPUT_RELEASE', 'false'), env.get('JVID_RELEASE_ENABLED', ''))
    root = args.root.resolve()
    if args.command == 'status':
        release = None
        if env['GITHUB_REF'].startswith('refs/tags/'):
            release = preflight(root, env['GITHUB_REPOSITORY'], env['GITHUB_SHA'], env['GITHUB_REF_NAME'])
        # Duplicate queued runs must not rebuild an existing draft or published
        # release. Recover a draft with the original run's failed jobs instead.
        output(build=str(release is None).lower(), publish=str(requested and release is None).lower())
        return
    if not requested:
        raise ValueError('Publication requires an enabled tag push or explicit tag release dispatch')
    repository, commit, tag = env['GITHUB_REPOSITORY'], env['GITHUB_SHA'], env['GITHUB_REF_NAME']
    if args.command == 'preflight':
        release = preflight(root, repository, commit, tag)
        output(publish=str(not release or release['draft']).lower())
    else:
        publish(root, args.assets.resolve(), repository, commit, tag, env.get('JVID_UPDATE_PUBLIC_KEY', ''))


if __name__ == '__main__':
    main()
