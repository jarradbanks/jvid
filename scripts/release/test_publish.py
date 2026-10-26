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
        self.remote = {}
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
            return {'object': {'type': 'commit', 'sha': self.commit}}
        if path.startswith('releases/tags/'):
            return dict(self.release) if self.release else None
        if path.startswith('releases?'):
            return [dict(self.release)] if self.release else []
        if path == 'releases' and method == 'POST':
            self.release = {'id': 1, **body}
            return dict(self.release)
        if path == 'releases/1':
            if method == 'PATCH':
                self.release.update(body)
            return dict(self.release)
        if path.startswith('releases/1/assets?'):
            return list(self.remote.values())
        if path.startswith('releases/assets/') and method == 'DELETE':
            asset_id = int(path.split('/')[-1])
            self.remote = {n: a for n, a in self.remote.items() if a['id'] != asset_id}
            return None
        raise AssertionError((path, method, body))

    def upload(self, repository, release_id, path):
        self.uploads.append(path.name)
        if self.fail_upload or len(self.uploads) == self.fail_upload_at:
            self.fail_upload = False
            self.remote[path.name] = {'id': 1000, 'name': path.name, 'state': 'starter', 'size': 0, 'digest': None}
            raise RuntimeError('upload interrupted')
        self.remote[path.name] = {'id': len(self.remote) + 1, 'name': path.name, 'state': 'uploaded',
                                 'size': path.stat().st_size, 'digest': 'sha256:' + p.digest(path)}
        if self.bad_upload:
            self.remote[path.name]['digest'] = 'sha256:' + '0' * 64
        if self.publish_during_upload:
            self.release['draft'] = False

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
        self.assertEqual(set(self.remote), set(records))
        self.assertEqual(len(self.uploads), len(records))
        self.assertEqual(self.mutations()[-1][2]['draft'], False)
        self.assertTrue(all(p.matches(self.remote[n], r) for n, r in records.items()))

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
        self.assertTrue(self.release['draft'])
        retained = dict(self.remote)
        self.publish()
        for name in list(retained)[:2]:
            self.assertEqual(self.uploads.count(name), 1)
        self.assertFalse(self.release['draft'])
        self.assertTrue(any(c[1] == 'DELETE' for c in self.calls))
        self.assertEqual(len(self.uploads), len(self.remote) + 1)

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
        self.assertEqual(sum(c[1] == 'POST' and c[0].endswith('/releases') for c in self.calls), 1)
        self.assertFalse(self.release['draft'])

    def test_remote_digest_mismatch_keeps_draft(self):
        self.bad_upload = True
        with self.assertRaisesRegex(ValueError, 'failed verification'):
            self.publish()
        self.assertTrue(self.release['draft'])
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
