"""Real multipart source/signature fixtures, mocked uploads; no production writes."""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import automation as a
import publish as p

REAL_UPLOAD = p.upload

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'packaging'))
import sources


class PublicationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.assets = self.root / 'assets'
        self.assets.mkdir()
        (self.root / 'packaging').mkdir()
        self.current = sources.version()
        (self.root / 'Cargo.toml').write_text('[workspace.package]\nversion = "' + self.current + '"\n')
        (self.root / 'packaging/release-notes.md').write_text('Reviewed release notes.\n')
        self.commit = 'a' * 40
        self.tag = 'v' + self.current
        self.revision = {'commit': self.commit, 'tree': 'b' * 40}
        self.repo = 'jarradbanks/jvid'
        self.release = None
        self.source_release = None
        self.remote = {}
        self.source_remote = {}
        self.tags = {self.tag: self.commit}
        self.uploaded_bytes = {}
        self.calls = []
        self.uploads = []
        self.fail_upload = False
        self.fail_upload_at = None
        self.bad_upload = False
        self.publish_during_upload = False
        # Public disposable fixture seed, unrelated to production. DER PKCS#8.
        key = self.root / 'fixture.der'
        key.write_bytes(bytes.fromhex('302e020100300506032b657004220420') + bytes(range(32)))
        public = subprocess.check_output(['openssl', 'pkey', '-inform', 'DER', '-in', str(key),
                                          '-pubout', '-outform', 'DER'], stderr=subprocess.DEVNULL)
        self.public = public[-32:].hex()
        environment = patch.dict(os.environ, {'JVID_UPDATE_PUBLIC_KEY': self.public,
            'GGML_NATIVE': 'OFF', 'GGML_METAL': 'OFF', 'RUSTFLAGS': '-C target-feature=+crt-static'})
        environment.start()
        self.addCleanup(environment.stop)
        revision = patch.object(sources, 'checkout', return_value=self.revision)
        revision.start()
        self.addCleanup(revision.stop)
        self.packages = {}
        for target, distribution, suffix in p.SUFFIXES:
            name = 'jvid-' + self.current + '-' + suffix
            (self.assets / name).write_bytes(('native ' + suffix).encode())
            self.packages[name] = (target, distribution)
        bundles = self.root / 'bundles'
        for bundle in sources.BUNDLES:
            directory = self.root / ('source-' + bundle)
            directory.mkdir()
            (directory / 'source.txt').write_text('Retained exact source and recipe\n')
            provider = {'id': 'fixture', 'files': ['source.txt']}
            native = {}
            if bundle == 'rust':
                provider['offline_metadata_checked'] = True
            else:
                native = sources.native_assets(self.assets, bundle)
            sources.bundle(directory, bundles / bundle, bundle, [provider], native, self.revision, limit=512)
        sources.collect_release(bundles, self.assets, self.assets)
        self.metadata = {'schema': 1, 'repository': self.repo, 'version': self.current,
            'notes': (self.root / 'packaging/release-notes.md').read_text(),
            'assets': [{'target': t, 'distribution': d, 'name': name,
                        'size': (self.assets / name).stat().st_size, 'sha256': p.digest(self.assets / name)}
                       for name, (t, d) in self.packages.items()]}
        (self.assets / 'jvid-update.json').write_text(json.dumps(self.metadata))
        subprocess.run(['openssl', 'pkeyutl', '-sign', '-keyform', 'DER', '-inkey', str(key),
                        '-rawin', '-in', str(self.assets / 'jvid-update.json'),
                        '-out', str(self.assets / 'jvid-update.json.sig')], check=True)
        key.unlink()
        self.checksums()
        replacements = (
            patch.object(p, 'github', self.api),
            patch.object(p, 'run', return_value=self.commit),
            patch.object(p, 'upload', side_effect=self.upload),
            patch.object(p, 'verify_sources', side_effect=lambda root, assets:
                         sources.collect_release(assets, assets, assets, verify_only=True)),
        )
        for replacement in replacements:
            replacement.start()
            self.addCleanup(replacement.stop)
        silence = contextlib.redirect_stdout(io.StringIO())
        silence.__enter__()
        self.addCleanup(silence.__exit__, None, None, None)

    def checksums(self):
        files = sorted(x for x in self.assets.iterdir() if x.name != 'SHA256SUMS.txt')
        (self.assets / 'SHA256SUMS.txt').write_text(''.join(p.digest(x) + '  ' + x.name + '\n' for x in files))

    def api(self, path, method='GET', body=None):
        self.calls.append((path, method, body))
        prefix = f'repos/{self.repo}/'
        if not path.startswith(prefix):
            raise AssertionError(path)
        path = path[len(prefix):]
        if path.startswith('git/ref/tags/'):
            commit = self.tags.get(path.removeprefix('git/ref/tags/'))
            return {'object': {'type': 'commit', 'sha': commit}} if commit else None
        if path == 'git/refs' and method == 'POST':
            self.tags[body['ref'].removeprefix('refs/tags/')] = body['sha']
            return {}
        if path.startswith('releases/tags/'):
            release = self.release if path.removeprefix('releases/tags/') == self.tag else self.source_release
            return dict(release) if release else None
        if path.startswith('releases?'):
            return [dict(r) for r in (self.release, self.source_release) if r]
        if path == 'releases' and method == 'POST':
            if body['tag_name'] == self.tag:
                self.release = {'id': 1, **body}
                return dict(self.release)
            self.source_release = {'id': 2, **body}
            return dict(self.source_release)
        for release_id, release, remote in ((1, self.release, self.remote),
                                            (2, self.source_release, self.source_remote)):
            if path == f'releases/{release_id}':
                if method == 'PATCH':
                    release.update(body)
                return dict(release)
            if path.startswith(f'releases/{release_id}/assets?'):
                return list(remote.values())
        if path.startswith('releases/assets/') and method == 'DELETE':
            asset_id = int(path.split('/')[-1])
            for remote in (self.remote, self.source_remote):
                for name in list(remote):
                    if remote[name]['id'] == asset_id:
                        del remote[name]
            return None
        raise AssertionError((path, method, body))

    def upload(self, repository, release_id, path):
        self.uploads.append(path.name)
        remote = self.remote if release_id == 1 else self.source_remote
        if self.fail_upload or len(self.uploads) == self.fail_upload_at:
            self.fail_upload = False
            remote[path.name] = {'id': 1000, 'name': path.name, 'state': 'starter', 'size': 0, 'digest': None}
            raise RuntimeError('upload interrupted')
        remote[path.name] = {'id': release_id * 100 + len(remote) + 1, 'name': path.name, 'state': 'uploaded',
                            'size': path.stat().st_size, 'digest': 'sha256:' + p.digest(path)}
        self.uploaded_bytes[release_id, path.name] = path.read_bytes()
        if self.bad_upload:
            remote[path.name]['digest'] = 'sha256:' + '0' * 64
        if self.publish_during_upload:
            (self.release if release_id == 1 else self.source_release)['draft'] = False

    def publish(self):
        p.publish(self.root, self.assets, self.repo, self.commit, self.tag, self.public)

    def mutations(self):
        return [c for c in self.calls if c[1] != 'GET']

    def seed_draft(self):
        self.release = {'id': 1, 'tag_name': self.tag, 'target_commitish': self.commit,
                        'draft': True, 'prerelease': False}

    def test_upload_streams_file_through_cli_without_token_arguments(self):
        name = next(iter(self.packages))
        with patch.dict(os.environ, {'GH_TOKEN': 'disposable-fixture-token'}), patch.object(p.subprocess, 'run') as command:
            REAL_UPLOAD(self.repo, 1, self.assets / name)
        arguments = command.call_args.args[0]
        self.assertEqual(arguments[:4], ['gh', 'api', '--method', 'POST'])
        self.assertTrue(arguments[4].startswith('https://uploads.github.com/repos/jarradbanks/jvid/releases/1/assets?name='))
        self.assertIn('--input', arguments)
        self.assertEqual(arguments[-1], str(self.assets / name))
        self.assertNotIn('disposable-fixture-token', ' '.join(arguments))

    def test_exact_artifacts_and_remote_digests_before_publication(self):
        records = p.validate_assets(self.root, self.assets, self.public)
        self.publish()
        self.assertFalse(self.release['draft'])
        self.assertFalse(self.source_release['draft'])
        self.assertEqual(set(self.remote), {*self.packages, 'jvid-update.json', 'jvid-update.json.sig', 'SHA256SUMS.txt'})
        self.assertEqual(set(self.remote) | set(self.source_remote), set(records))
        self.assertEqual(set(self.remote) & set(self.source_remote), {'SHA256SUMS.txt'})
        for release_id, remote in ((1, self.remote), (2, self.source_remote)):
            sums = self.uploaded_bytes[release_id, 'SHA256SUMS.txt'].decode().splitlines()
            self.assertEqual({line.split('  ')[1] for line in sums}, set(remote) - {'SHA256SUMS.txt'})
            for line in sums:
                digest, name = line.split('  ')
                self.assertEqual(remote[name]['digest'], 'sha256:' + digest)
        published = [c for c in self.mutations() if c[1] == 'PATCH']
        self.assertEqual([c[0].split('/')[-1] for c in published], ['2', '1'])
        self.assertEqual([c[2]['make_latest'] for c in published], ['false', 'true'])
        self.assertTrue(all(c[2]['make_latest'] == 'false' for c in self.mutations()
                            if c[1] == 'POST' and c[0].endswith('/releases')))
        self.assertIn('/releases/tag/sources-' + self.tag, self.release['body'])
        self.assertEqual(self.tags['sources-' + self.tag], self.commit)
        # A source-only download must retain every part, manifest and executable
        # reconstruction instruction, without needing any app attachments.
        downloaded = self.root / 'source-download'
        downloaded.mkdir()
        for name in self.source_remote:
            (downloaded / name).write_bytes(self.uploaded_bytes[2, name])
        manifests = sorted(downloaded.glob('jvid-*-sources-*.json'))
        self.assertEqual(len(manifests), len(sources.BUNDLES))
        for manifest in manifests:
            data = json.loads(manifest.read_text())
            archive = downloaded / (data['bundle'] + '.tar.gz')
            subprocess.run([sys.executable, str(downloaded / 'reassemble_sources.py'),
                            str(manifest), str(archive)], check=True, capture_output=True)
            self.assertEqual(p.digest(archive), data['archive_sha256'])

    def test_published_release_is_immutable_on_retry(self):
        self.publish()
        self.calls.clear()
        self.uploads.clear()
        self.publish()
        self.assertEqual(self.mutations(), [])
        self.assertEqual(self.uploads, [])

    def test_interrupted_upload_recovers_starter_and_existing_good_assets(self):
        self.fail_upload_at = 3
        with self.assertRaisesRegex(RuntimeError, 'interrupted'):
            self.publish()
        self.assertTrue(self.source_release['draft'])
        self.assertIsNone(self.release)
        retained = dict(self.source_remote)
        self.publish()
        for name in list(retained)[:2]:
            self.assertEqual(self.uploads.count(name), 2 if name == 'SHA256SUMS.txt' else 1)
        self.assertFalse(self.release['draft'])
        self.assertTrue(any(c[1] == 'DELETE' for c in self.calls))
        self.assertEqual(len(self.uploads), len(self.remote) + len(self.source_remote) + 1)

    def test_publication_during_starter_delete_prevents_the_retry_upload(self):
        self.fail_upload = True
        with self.assertRaises(RuntimeError):
            self.publish()
        original = self.api
        def api(path, method='GET', body=None):
            result = original(path, method, body)
            if method == 'DELETE':
                self.source_release['draft'] = False
            return result
        self.uploads.clear()
        with patch.object(p, 'github', side_effect=api):
            with self.assertRaisesRegex(ValueError, 'published'):
                self.publish()
        self.assertEqual(self.uploads, [])
        self.assertIsNone(self.release)

    def test_starter_that_finishes_during_inspection_is_retained(self):
        self.fail_upload = True
        original_upload = self.upload
        frozen = {}
        def upload(repository, release_id, path):
            frozen[path.name] = path.read_bytes()
            original_upload(repository, release_id, path)
        with patch.object(p, 'upload', side_effect=upload):
            with self.assertRaises(RuntimeError):
                self.publish()
        filename, contents = next(iter(frozen.items()))
        original_api = self.api
        inspections = 0
        def api(path, method='GET', body=None):
            nonlocal inspections
            if '/releases/2/assets?' in path:
                inspections += 1
                if inspections == 3:
                    self.source_remote[filename].update(state='uploaded', size=len(contents),
                        digest='sha256:' + p.hashlib.sha256(contents).hexdigest())
                    self.uploaded_bytes[2, filename] = contents
            return original_api(path, method, body)
        self.calls.clear()
        self.uploads.clear()
        with patch.object(p, 'github', side_effect=api):
            self.publish()
        self.assertFalse(self.release['draft'])
        self.assertFalse(any(c[1] == 'DELETE' for c in self.calls))
        self.assertEqual(self.uploads.count(filename), 1)  # Only the app checksum.

    def test_lost_create_response_resumes_existing_draft(self):
        original = self.api
        def api(path, method='GET', body=None):
            response = original(path, method, body)
            if method == 'POST' and path.endswith('/releases'):
                raise RuntimeError('lost create response')
            return response
        with patch.object(p, 'github', side_effect=api):
            with self.assertRaisesRegex(RuntimeError, 'lost create'):
                self.publish()
        self.publish()
        self.assertEqual(sum(c[1] == 'POST' and c[0].endswith('/releases') for c in self.calls), 2)
        self.assertFalse(self.release['draft'])

    def test_retry_rejects_changed_valid_candidate_even_before_first_upload(self):
        with patch.object(p, 'upload', side_effect=RuntimeError('interrupted before upload')):
            with self.assertRaises(RuntimeError):
                self.publish()
        self.assertTrue(self.source_release['draft'])
        self.assertEqual(self.source_remote, {})
        # Different, still valid signed metadata must not be substituted on retry.
        (self.assets / 'jvid-update.json').write_text(json.dumps(self.metadata, indent=2))
        key = self.root / 'fixture.der'
        key.write_bytes(bytes.fromhex('302e020100300506032b657004220420') + bytes(range(32)))
        subprocess.run(['openssl', 'pkeyutl', '-sign', '-keyform', 'DER', '-inkey', str(key),
                        '-rawin', '-in', str(self.assets / 'jvid-update.json'),
                        '-out', str(self.assets / 'jvid-update.json.sig')], check=True)
        key.unlink()
        self.checksums()
        p.validate_assets(self.root, self.assets, self.public)
        self.calls.clear()
        with self.assertRaisesRegex(ValueError, 'fingerprint.*frozen'):
            self.publish()
        self.assertEqual(self.mutations(), [])

    def test_lost_source_tag_create_response_reuses_existing_tag(self):
        original = self.api
        def api(path, method='GET', body=None):
            result = original(path, method, body)
            if method == 'POST' and path.endswith('/git/refs'):
                raise RuntimeError('lost tag response')
            return result
        with patch.object(p, 'github', side_effect=api):
            with self.assertRaisesRegex(RuntimeError, 'lost tag response'):
                self.publish()
        self.assertIsNone(self.source_release)
        self.calls.clear()
        self.publish()
        self.assertFalse(any(c[1] != 'GET' and '/git/' in c[0] for c in self.calls))

    def test_lost_source_publication_response_never_republishes_sources(self):
        original = self.api
        def api(path, method='GET', body=None):
            result = original(path, method, body)
            if method == 'PATCH' and path.endswith('/releases/2'):
                raise RuntimeError('lost publication response')
            return result
        with patch.object(p, 'github', side_effect=api):
            with self.assertRaisesRegex(RuntimeError, 'lost publication response'):
                self.publish()
        self.assertFalse(self.source_release['draft'])
        self.assertIsNone(self.release)
        self.calls.clear()
        self.publish()
        self.assertFalse(self.release['draft'])
        self.assertFalse(any(c[1] != 'GET' and '/releases/2' in c[0] for c in self.calls))

    def test_remote_digest_mismatch_keeps_draft(self):
        self.bad_upload = True
        with self.assertRaisesRegex(ValueError, 'conflicting|failed verification'):
            self.publish()
        self.assertTrue(self.source_release['draft'])
        self.assertIsNone(self.release)
        self.assertFalse(any(c[1] == 'PATCH' for c in self.calls))

    def test_conflicting_uploaded_asset_never_overwritten(self):
        self.seed_draft()
        name = next(iter(self.packages))
        self.remote[name] = {'name': name, 'id': 2, 'state': 'uploaded', 'size': 9, 'digest': 'sha256:wrong'}
        with self.assertRaisesRegex(ValueError, 'conflicting'):
            self.publish()
        self.assertEqual(self.mutations(), [])
        self.assertEqual(self.uploads, [])

    def test_unexpected_remote_asset_blocks_publication(self):
        self.seed_draft()
        self.remote['unreviewed.exe'] = {'name': 'unreviewed.exe', 'id': 2, 'state': 'uploaded'}
        with self.assertRaisesRegex(ValueError, 'Unexpected'):
            self.publish()
        self.assertEqual(self.mutations(), [])

    def test_publication_during_upload_stops_all_further_mutations(self):
        self.publish_during_upload = True
        with self.assertRaisesRegex(ValueError, 'published'):
            self.publish()
        self.assertEqual(len(self.uploads), 1)
        self.assertFalse(any(c[1] == 'PATCH' for c in self.calls))

    def test_failed_source_validation_prevents_draft_and_publication(self):
        part = next(self.assets.glob('*.part*'))
        part.write_bytes(b'corrupt source')
        self.checksums()
        with self.assertRaises(RuntimeError):
            self.publish()
        self.assertEqual(self.mutations(), [])

    def test_failed_native_validation_prevents_publication(self):
        name = next(iter(self.packages))
        (self.assets / name).write_bytes(b'corrupt native')
        self.checksums()
        with self.assertRaises(RuntimeError):
            self.publish()
        self.assertEqual(self.mutations(), [])

    def test_invalid_checksums_prevent_publication(self):
        (self.assets / 'SHA256SUMS.txt').write_text('0' * 64 + '  missing.zip\n')
        with self.assertRaisesRegex(ValueError, 'checksums'):
            self.publish()
        self.assertEqual(self.mutations(), [])

    def test_invalid_signature_prevents_publication_even_with_updated_checksum(self):
        (self.assets / 'jvid-update.json.sig').write_bytes(b'x' * 64)
        self.checksums()
        with self.assertRaises(subprocess.CalledProcessError):
            self.publish()
        self.assertEqual(self.mutations(), [])

    def test_unknown_file_or_symlink_prevents_publication(self):
        extra = self.assets / 'secret.txt'
        extra.write_bytes(b'not allowed')
        self.checksums()
        with self.assertRaisesRegex(ValueError, 'Unexpected'):
            self.publish()
        extra.unlink()
        name = next(iter(self.packages))
        contents = (self.assets / name).read_bytes()
        target = self.root / 'outside.exe'
        target.write_bytes(contents)
        (self.assets / name).unlink()
        (self.assets / name).symlink_to(target)
        self.checksums()
        with self.assertRaises((RuntimeError, ValueError)):
            self.publish()
        self.assertEqual(self.mutations(), [])

    def test_tag_conflict_prevents_any_mutation(self):
        with patch.object(p, 'tag_commit', return_value='f' * 40):
            with self.assertRaisesRegex(ValueError, 'exact commit'):
                self.publish()
        self.assertEqual(self.mutations(), [])

    def test_source_tag_conflict_prevents_any_mutation(self):
        self.tags['sources-' + self.tag] = 'f' * 40
        with self.assertRaisesRegex(ValueError, 'different commit'):
            self.publish()
        self.assertEqual(self.mutations(), [])

    def test_app_upload_retry_reuses_published_sources(self):
        original = self.upload
        fail = True
        def upload(repository, release_id, path):
            nonlocal fail
            if release_id == 1 and fail:
                fail = False
                raise RuntimeError('app upload interrupted')
            original(repository, release_id, path)
        with patch.object(p, 'upload', side_effect=upload):
            with self.assertRaisesRegex(RuntimeError, 'app upload interrupted'):
                self.publish()
            self.assertFalse(self.source_release['draft'])
            self.assertTrue(self.release['draft'])
            retained = dict(self.source_remote)
            self.calls.clear()
            self.publish()
        self.assertEqual(self.source_remote, retained)
        self.assertFalse(self.release['draft'])
        self.assertFalse(any(c[0].endswith('/releases/2') and c[1] == 'PATCH' for c in self.calls))

    def test_changed_published_sources_block_app_retry(self):
        original = self.upload
        def upload(repository, release_id, path):
            if release_id == 1:
                raise RuntimeError('app interrupted')
            original(repository, release_id, path)
        with patch.object(p, 'upload', side_effect=upload):
            with self.assertRaises(RuntimeError):
                self.publish()
        original_remote = {n: dict(asset) for n, asset in self.source_remote.items()}
        name = next(n for n in self.source_remote if n != 'SHA256SUMS.txt')
        for change in ('missing', 'size', 'digest', 'state', 'extra'):
            with self.subTest(change=change):
                self.source_remote = {n: dict(asset) for n, asset in original_remote.items()}
                if change == 'missing':
                    del self.source_remote[name]
                elif change == 'extra':
                    self.source_remote['unexpected.txt'] = {'name': 'unexpected.txt'}
                else:
                    self.source_remote[name][change] = {'size': 0, 'digest': 'sha256:bad', 'state': 'starter'}[change]
                self.calls.clear()
                with self.assertRaises(ValueError):
                    self.publish()
                self.assertEqual(self.mutations(), [])

    def test_source_publication_must_still_be_public_before_app_publication(self):
        original = self.upload
        def upload(repository, release_id, path):
            original(repository, release_id, path)
            if release_id == 1:
                self.source_release['draft'] = True
        with patch.object(p, 'upload', side_effect=upload):
            with self.assertRaisesRegex(ValueError, 'source must be published'):
                self.publish()
        self.assertTrue(self.release['draft'])
        self.assertFalse(any(c[1] == 'PATCH' and c[0].endswith('/releases/1') for c in self.calls))

    def test_status_skips_source_tag_or_release_without_app_to_preserve_frozen_bytes(self):
        for source_state in ('tag', 'draft', 'published'):
            with self.subTest(source_state=source_state):
                self.tags = {self.tag: self.commit}
                self.source_release = None
                if source_state == 'tag':
                    self.tags['sources-' + self.tag] = self.commit
                else:
                    self.source_release = {'id': 2, 'tag_name': 'sources-' + self.tag,
                                           'draft': source_state == 'draft'}
                environment = {'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_REF': 'refs/tags/' + self.tag,
                               'GITHUB_REF_NAME': self.tag, 'GITHUB_SHA': self.commit,
                               'GITHUB_REPOSITORY': self.repo, 'INPUT_RELEASE': 'true', 'JVID_RELEASE_ENABLED': 'true'}
                with patch.dict(os.environ, environment), patch.object(sys, 'argv', ['publish.py', 'status', '--root', str(self.root)]), patch.object(p, 'output') as output:
                    p.main()
                    output.assert_called_once_with(build='false', publish='false')
        self.assertEqual(self.mutations(), [])

    def test_dispatch_does_not_rebuild_a_source_only_publication(self):
        (self.root / a.PLAN).parent.mkdir()
        (self.root / a.PLAN).write_text('{}')
        plan = {'main_commit': 'b' * 40, 'previous_version': '0.0.0'}
        for source_state in ('tag', 'release'):
            with self.subTest(source_state=source_state):
                self.tags = {'sources-' + self.tag: self.commit} if source_state == 'tag' else {}
                self.source_release = {'tag_name': 'sources-' + self.tag} if source_state == 'release' else None
                self.calls.clear()
                with patch.object(a, 'github', side_effect=self.api), patch.object(a, 'read_plan', return_value=plan), patch.object(a, 'version_at', return_value='0.0.0'), patch.object(a, 'run', side_effect=[self.commit, 'b' * 40]), patch.object(a, 'output') as output:
                    a.dispatch(self.root, self.repo, self.commit, 'b' * 40)
                    self.assertEqual(output.call_args.kwargs['release'], 'false')
                    self.assertIn('original Package', output.call_args.kwargs['reason'])
                self.assertEqual(self.mutations(), [])

    def test_status_skips_existing_draft_and_published_even_for_manual_opt_out(self):
        for draft in (True, False):
            self.seed_draft()
            self.release['draft'] = draft
            for opt_in in ('true', 'false'):
                environment = {'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_REF': 'refs/tags/' + self.tag,
                               'GITHUB_REF_NAME': self.tag, 'GITHUB_SHA': self.commit,
                               'GITHUB_REPOSITORY': self.repo, 'INPUT_RELEASE': opt_in, 'JVID_RELEASE_ENABLED': 'true'}
                with patch.dict(os.environ, environment), patch.object(sys, 'argv', ['publish.py', 'status', '--root', str(self.root)]), patch.object(p, 'output') as output:
                    p.main()
                    output.assert_called_once_with(build='false', publish='false')
        self.assertEqual(self.mutations(), [])

    def test_branch_status_builds_without_any_github_request_and_cannot_publish(self):
        environment = {'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_REF': 'refs/heads/release/' + self.tag,
                       'INPUT_RELEASE': 'true', 'JVID_RELEASE_ENABLED': 'true'}
        with patch.dict(os.environ, environment), patch.object(sys, 'argv', ['publish.py', 'status', '--root', str(self.root)]), patch.object(p, 'output') as output:
            p.main()
            output.assert_called_once_with(build='true', publish='false')
        self.assertEqual(self.calls, [])
        with patch.dict(os.environ, environment), patch.object(sys, 'argv', ['publish.py', 'publish', '--root', str(self.root)]):
            with self.assertRaisesRegex(ValueError, 'explicit tag release dispatch'):
                p.main()
        self.assertEqual(self.calls, [])

    def test_publication_gate_requires_enabled_tag_and_explicit_dispatch_opt_in(self):
        for event in ('push', 'pull_request', 'workflow_dispatch'):
            for ref in ('refs/heads/main', 'refs/heads/release/v0.2.0', 'refs/tags/v0.2.0'):
                for opt_in in ('true', 'false', ''):
                    for enabled in ('true', 'false', ''):
                        expected = (event in ('push', 'workflow_dispatch') and ref.startswith('refs/tags/')
                                    and enabled == 'true' and (event == 'push' or opt_in == 'true'))
                        self.assertEqual(p.publication_requested(event, ref, opt_in, enabled), expected)


if __name__ == '__main__':
    unittest.main()
