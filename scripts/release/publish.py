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

from automation import output, run, source_publication_started, stable, version
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


def release_assets(repository, release, commit, tag, records, *, published=False, body=None):
    """Inspect a draft or the already-published source companion without modifying it."""
    path = f'repos/{repository}/releases/{release["id"]}'
    current = github(path)
    if (not current or (not published and not current['draft'])
            or (current['draft'] and current.get('immutable'))
            or current['tag_name'] != tag or current['target_commitish'] != commit
            or tag_commit(repository, tag, api=github) != commit):
        raise ValueError('Release changed or was published; refusing to modify it')
    if body is not None and current.get('body') != body:
        raise ValueError('Source notes or asset fingerprint differ from the frozen publication')
    observed = {}
    for asset in pages(path + '/assets', api=github):
        name = asset['name']
        if name not in records or name in observed:
            raise ValueError('Unexpected or duplicate asset already in release')
        if asset['state'] != 'starter' and not matches(asset, records[name]):
            raise ValueError('Existing release asset has conflicting size/digest/state: ' + name)
        observed[name] = asset
    if not current['draft'] and (set(observed) != set(records)
            or any(not matches(observed[n], r) for n, r in records.items())):
        raise ValueError('Published source assets failed verification')
    return current, observed


def split_assets(assets, records, temporary, current):
    """Separate upload sets; retain the combined candidate for complete verification.

    Only the tiny checksum files are written. Multi-GiB source archives stay in place.
    Each release's checksum list describes exactly its own attachments.
    """
    app = {'jvid-' + current + '-' + suffix for _, _, suffix in SUFFIXES}
    app.update({'jvid-update.json', 'jvid-update.json.sig'})
    scopes = []
    for label, names in (('app', app), ('sources', set(records) - app - {'SHA256SUMS.txt'})):
        paths = {name: assets / name for name in names}
        sums = temporary / label / 'SHA256SUMS.txt'
        sums.parent.mkdir()
        sums.write_text(''.join(records[n]['sha256'] + '  ' + n + '\n' for n in sorted(names)))
        paths[sums.name] = sums
        scoped = {n: records[n] for n in names}
        scoped[sums.name] = {'size': sums.stat().st_size, 'sha256': digest(sums)}
        scopes.append((scoped, paths))
    return scopes


def publish_one(repository, commit, tag, name, notes, records, paths, release, revalidate, *, latest):
    if release is None:
        release = github(f'repos/{repository}/releases', 'POST', {'tag_name': tag,
            'target_commitish': commit, 'name': name, 'body': notes,
            'draft': True, 'prerelease': False, 'make_latest': 'false'})
    source_body = notes if not latest else None
    current, observed = release_assets(repository, release, commit, tag, records,
                                       published=not latest, body=source_body)
    if not current['draft']:
        return release  # A verified published source release is reusable on an app retry.
    for filename, record in sorted(records.items()):
        asset = observed.get(filename)
        if asset and matches(asset, record):
            continue
        _, refreshed = release_assets(repository, release, commit, tag, records, body=source_body)
        asset = refreshed.get(filename)
        if asset and matches(asset, record):
            continue  # An earlier starter may have completed since inspection.
        if asset:
            github(f'repos/{repository}/releases/assets/{asset["id"]}', 'DELETE')
            release_assets(repository, release, commit, tag, records, body=source_body)
        upload(repository, release['id'], paths[filename])
    _, observed = release_assets(repository, release, commit, tag, records, body=source_body)
    if set(observed) != set(records) or any(not matches(observed[n], r) for n, r in records.items()):
        raise ValueError('Uploaded release asset names/sizes/digests failed verification')
    revalidate()
    _, observed = release_assets(repository, release, commit, tag, records, body=source_body)
    if set(observed) != set(records) or any(not matches(observed[n], r) for n, r in records.items()):
        raise ValueError('Remote release assets changed before publication')
    result = github(f'repos/{repository}/releases/{release["id"]}', 'PATCH',
        {'name': name, 'body': notes, 'draft': False, 'prerelease': False,
         'make_latest': 'true' if latest else 'false'})
    if (result['draft'] or result['tag_name'] != tag or result['target_commitish'] != commit
            or result['prerelease']):
        raise RuntimeError('GitHub did not confirm publication; inspect and retry the original job')
    return result


def publish(root, assets, repository, commit, tag, public_key):
    release = preflight(root, repository, commit, tag)
    if release and not release['draft']:
        output(published='false', reason='Published release is immutable; no changes made')
        return
    records = validate_assets(root, assets, public_key)
    source_tag = 'sources-' + tag  # Avoid v* package triggers and app updater version selection.
    source_commit = tag_commit(repository, source_tag, api=github)
    if source_commit is not None and source_commit != commit:
        raise ValueError('Source release tag belongs to a different commit; refusing to retag')
    source_release = release_for_tag(repository, source_tag, api=github)
    source_url = f'https://github.com/{repository}/releases/tag/{source_tag}'
    notes = (root / 'packaging/release-notes.md').read_text().rstrip() + '\n'
    if source_url not in notes:
        notes += '\n[Source code](' + source_url + ')\n'
    source_notes = (f'Corresponding source and build scripts for [jvid {tag}]'
                    f'(https://github.com/{repository}/releases/tag/{tag}).\n\n'
                    'Download SOURCE-README.txt for verification and reconstruction instructions.\n')
    # Freeze the entire signed candidate before the first upload. A draft whose
    # create response or first upload was lost must reject freshly rebuilt bytes,
    # including valid but different metadata/signatures or source archives.
    fingerprint = hashlib.sha256(json.dumps(records, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
    source_notes += '\n<!-- jvid-release-assets-sha256: ' + fingerprint + ' -->\n'

    def revalidate():
        if validate_assets(root, assets, public_key) != records:
            raise ValueError('Local assets changed while uploading')

    with tempfile.TemporaryDirectory() as directory:
        (app_records, app_paths), (source_records, source_paths) = split_assets(
            assets, records, Path(directory), version(root))
        # Detect existing conflicts in either release before creating tags or uploading anything.
        if release:
            release_assets(repository, release, commit, tag, app_records)
        if source_release:
            release_assets(repository, source_release, commit, source_tag, source_records,
                           published=True, body=source_notes)
        if source_commit is None:
            github(f'repos/{repository}/git/refs', 'POST',
                   {'ref': 'refs/tags/' + source_tag, 'sha': commit})
        source_release = publish_one(repository, commit, source_tag, 'jvid ' + tag + ' sources',
            source_notes, source_records, source_paths, source_release, revalidate, latest=False)

        def before_app_publication():
            revalidate()
            current, _ = release_assets(repository, source_release, commit, source_tag,
                                        source_records, published=True, body=source_notes)
            if current['draft']:
                raise ValueError('Corresponding source must be published before the app')

        before_app_publication()
        publish_one(repository, commit, tag, 'jvid ' + tag, notes,
                    app_records, app_paths, release, before_app_publication, latest=True)
    output(published='true', tag=tag, sources=source_tag)


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
        started = False
        if env['GITHUB_REF'].startswith('refs/tags/'):
            release = preflight(root, env['GITHUB_REPOSITORY'], env['GITHUB_SHA'], env['GITHUB_REF_NAME'])
            started = release is not None or source_publication_started(
                env['GITHUB_REPOSITORY'], env['GITHUB_REF_NAME'], api=github)
        # Duplicate queued runs must not rebuild an existing draft or published
        # release. Recover a draft with the original run's failed jobs instead.
        output(build=str(not started).lower(), publish=str(requested and not started).lower())
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
