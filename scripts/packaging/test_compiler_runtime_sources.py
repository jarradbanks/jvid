#!/usr/bin/env python3
"""Compiler runtime capture: integrity, complete source inputs and path safety."""
import bz2
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import urllib.error

import compiler_runtime_sources as runtime


def digest(data):
    return hashlib.sha256(data).hexdigest()


def source_tar(files, *, links=None, mode='w:gz'):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode=mode) as archive:
        for name, data in files.items():
            member = tarfile.TarInfo(name)
            member.size = len(data)
            archive.addfile(member, io.BytesIO(data))
        for name, target in (links or {}).items():
            member = tarfile.TarInfo(name)
            member.type = tarfile.SYMTYPE
            member.linkname = target
            archive.addfile(member)
    return output.getvalue()


class CompilerRuntimeSourcesTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.out = self.root / 'out'
        self.lock_path = self.root / 'lock.json'
        self.payloads = {}
        self.lock = {'schema_version': 1, 'runtime': {'build_scripts_commit': 'a' * 40},
                     'completeness': {'status': 'complete'}, 'sources': []}
        for identity in sorted(runtime.REQUIRED_PROVIDERS):
            payload = source_tar({'source/src/runtime.c': b'int runtime;',
                                  'source/recipe': b'--enable-runtime'})
            self.payloads[identity] = payload
            self.lock['sources'].append({'id': identity, 'version': '1.2.3',
                'filename': identity + '.tar.gz', 'archive_root': 'source',
                'url': 'https://sources.invalid/' + identity,
                'size': len(payload), 'sha256': digest(payload),
                'sha512': hashlib.sha512(payload).hexdigest(),
                'provenance': {'description': 'Immutable fixture input'},
                'required_files': ['src/runtime.c', 'recipe'], 'retain': ['recipe'],
                'content_sha256': {'recipe': digest(b'--enable-runtime')}})
        self.save_lock()

    def save_lock(self):
        self.lock_path.write_text(json.dumps(self.lock), encoding='utf-8')

    def fetch(self, source, destination):
        destination.write_bytes(self.payloads[source['id']])

    def collect(self):
        with patch.object(runtime, 'LOCK', self.lock_path), patch.object(runtime, 'fetch', side_effect=self.fetch):
            return runtime.collect(self.out)

    def test_complete_sources_recipes_and_relative_provider_inventory(self):
        records = self.collect()
        self.assertEqual(len(records), len(runtime.REQUIRED_PROVIDERS))
        self.assertEqual({p['id'] for p in records}, {'windows-' + p for p in runtime.REQUIRED_PROVIDERS})
        for provider in records:
            self.assertEqual(provider['version'], '1.2.3')
            self.assertEqual(provider['provenance']['runtime'], self.lock['runtime'])
            for name in provider['files']:
                self.assertTrue((self.out / name).is_file())
                self.assertFalse(Path(name).is_absolute())
        self.assertEqual(json.loads((self.out / 'SOURCE-PROVENANCE.json').read_text()), self.lock)
        self.assertEqual((self.out / 'build-evidence/gcc/recipe').read_bytes(), b'--enable-runtime')

    def test_missing_provider_or_unresolved_provenance_fails_before_download(self):
        for change in ('missing', 'unresolved'):
            with self.subTest(change=change):
                lock = copy.deepcopy(self.lock)
                if change == 'missing':
                    lock['sources'].pop()
                else:
                    lock['completeness'] = {'status': 'blocked', 'missing': ['unknown builder source']}
                self.lock_path.write_text(json.dumps(lock))
                with patch.object(runtime, 'LOCK', self.lock_path), patch.object(runtime, 'fetch') as fetch:
                    with self.assertRaisesRegex(runtime.SourceError, 'missing|incomplete'):
                        runtime.collect(self.out)
                fetch.assert_not_called()
                self.assertFalse(self.out.exists())

    def test_reject_existing_nonempty_output(self):
        self.out.mkdir()
        (self.out / 'previous').write_text('keep this')
        with self.assertRaisesRegex(runtime.SourceError, 'fresh empty'):
            self.collect()
        self.assertEqual((self.out / 'previous').read_text(), 'keep this')

    def test_empty_existing_output_is_supported(self):
        self.out.mkdir()
        self.collect()

    def test_missing_runtime_source_and_wrong_archive_root(self):
        source = self.lock['sources'][0]
        for files in ({'source/recipe': b'recipe'}, {'other/src/runtime.c': b'source'}):
            archive = self.root / 'test.tar.gz'
            archive.write_bytes(source_tar(files))
            with self.assertRaisesRegex(runtime.SourceError, 'missing|root'):
                runtime.inspect_archive(archive, source)

    def test_archive_traversal_and_absolute_links_are_rejected(self):
        source = self.lock['sources'][0]
        bad_files = [{'source/../../escape': b'bad'}, {'/absolute': b'bad'},
                     {'source/back\\slash': b'bad'}, {'source/C:drive': b'bad'}]
        for files in bad_files:
            archive = self.root / 'unsafe.tar.gz'
            archive.write_bytes(source_tar(files))
            with self.assertRaisesRegex(runtime.SourceError, 'unsafe'):
                runtime.inspect_archive(archive, source)
        for target in ('/etc/passwd', '../../escape', 'C:\\escape'):
            archive.write_bytes(source_tar({}, links={'source/lib/link': target}))
            with self.assertRaisesRegex(runtime.SourceError, 'unsafe|escapes'):
                runtime.inspect_archive(archive, source)
        self.assertFalse((self.root / 'escape').exists())

    def test_safe_unix_links_stay_inside_intact_archive(self):
        source = self.lock['sources'][0]
        archive = self.root / 'links.tar.gz'
        archive.write_bytes(source_tar({'source/src/runtime.c': b'int runtime;',
                                       'source/recipe': b'--enable-runtime'},
                                      links={'source/lib/runtime.c': '../src/runtime.c'}))
        self.assertEqual(runtime.inspect_archive(archive, source)['recipe'], b'--enable-runtime')
        self.assertFalse((self.root / 'source').exists())

    def test_duplicate_or_linked_required_source_is_not_coverage(self):
        source = self.lock['sources'][0]
        archive = self.root / 'bad.tar.gz'
        archive.write_bytes(source_tar({'source/recipe': b'recipe'}, links={'source/src/runtime.c': '../recipe'}))
        with self.assertRaisesRegex(runtime.SourceError, 'required'):
            runtime.inspect_archive(archive, source)
        data = io.BytesIO()
        with tarfile.open(fileobj=data, mode='w:gz') as duplicate:
            for _ in range(2):
                member = tarfile.TarInfo('source/src/runtime.c')
                member.size = 1
                duplicate.addfile(member, io.BytesIO(b'x'))
        archive.write_bytes(data.getvalue())
        with self.assertRaisesRegex(runtime.SourceError, 'duplicate'):
            runtime.inspect_archive(archive, source)

    def test_concatenated_bzip2_source_archive_on_python_310(self):
        source = self.lock['sources'][0]
        raw = source_tar({'source/src/runtime.c': b'int runtime;',
                          'source/recipe': b'--enable-runtime'}, mode='w')
        archive = self.root / 'source.tar.bz2'
        archive.write_bytes(bz2.compress(raw[:1024]) + bz2.compress(raw[1024:]))
        self.assertEqual(runtime.inspect_archive(archive, source)['recipe'], b'--enable-runtime')

    def test_member_and_expanded_size_limits(self):
        source = self.lock['sources'][0]
        archive = self.root / 'source.tar.gz'
        archive.write_bytes(self.payloads[source['id']])
        for field in ('MAX_MEMBERS', 'MAX_EXPANDED'):
            with self.subTest(field=field), patch.object(runtime, field, 1):
                with self.assertRaisesRegex(runtime.SourceError, 'limit'):
                    runtime.inspect_archive(archive, source)

    def test_lock_rejects_unsafe_filename_missing_digest_and_duplicate_provider(self):
        for change in ('path', 'digest', 'duplicate', 'evidence'):
            lock = copy.deepcopy(self.lock)
            if change == 'path': lock['sources'][0]['filename'] = '../outside.tar.gz'
            if change == 'digest': lock['sources'][0]['sha256'] = ''
            if change == 'duplicate': lock['sources'].append(lock['sources'][0])
            if change == 'evidence': lock['sources'][0]['retain'] = ['not-validated']
            self.lock_path.write_text(json.dumps(lock))
            with self.subTest(change=change), patch.object(runtime, 'LOCK', self.lock_path):
                with self.assertRaises(runtime.SourceError):
                    runtime.read_lock()

    def test_corrupted_payload_is_rejected_even_with_custom_fetch(self):
        self.payloads['gcc'] = b'corrupted download'
        with self.assertRaisesRegex(runtime.SourceError, 'checksum mismatch'):
            self.collect()

    def test_stream_download_checks_sha512_size_and_cleans_partial(self):
        source = self.lock['sources'][0]
        for change in ('valid', 'sha512', 'size'):
            sample = copy.deepcopy(source)
            if change == 'sha512': sample['sha512'] = '0' * 128
            if change == 'size': sample['size'] += 1
            destination = self.root / (change + '.tar.gz')
            with patch.dict(os.environ, {'JVID_WINDOWS_SOURCE_CACHE': ''}), patch.object(runtime, '_open', return_value=io.BytesIO(self.payloads[source['id']])):
                if change == 'valid':
                    with patch.object(Path, 'read_bytes', side_effect=AssertionError('whole-file read')):
                        runtime.fetch(sample, destination)
                else:
                    with self.assertRaisesRegex(runtime.SourceError, 'checksum mismatch'):
                        runtime.fetch(sample, destination)
                    self.assertFalse(destination.exists())
            self.assertFalse(destination.with_name(destination.name + '.part').exists())

    def test_cache_is_reverified(self):
        source = self.lock['sources'][0]
        cache = self.root / 'cache'
        cache.mkdir()
        (cache / source['sha256']).write_bytes(b'corrupt cached source')
        with patch.dict(os.environ, {'JVID_WINDOWS_SOURCE_CACHE': str(cache)}), patch.object(runtime, '_open') as request:
            with self.assertRaisesRegex(runtime.SourceError, 'checksum mismatch'):
                runtime.fetch(source, self.root / 'cache-result')
        request.assert_not_called()

    def test_upstream_recipe_checksum_mismatch(self):
        gcc = self.lock['sources'][0]
        gcc['checksum_source'] = {'id': 'recipe', 'path': 'chksum'}
        with self.assertRaisesRegex(runtime.SourceError, 'upstream recipe'):
            runtime._validate_checksums([gcc], {'recipe': {'chksum': b'sha256 wrong.tar.gz 012345'}})

    def test_errors_do_not_expose_redirect_or_environment_secrets(self):
        source = self.lock['sources'][0]
        signed = 'https://sources.invalid/file?secret-signed-url'
        with patch.dict(os.environ, {'GH_TOKEN': 'secret-token', 'JVID_WINDOWS_SOURCE_CACHE': ''}), patch.object(runtime, '_open', side_effect=urllib.error.URLError(signed)):
            with self.assertRaises(runtime.SourceError) as caught:
                runtime.fetch(source, self.root / 'failed')
        self.assertNotIn('secret', str(caught.exception))
        redirect = urllib.error.HTTPError(source['url'], 302, '', {'Location': 'http://unsafe.invalid'}, None)
        with patch.dict(os.environ, {'JVID_WINDOWS_SOURCE_CACHE': ''}), patch.object(runtime, '_open', side_effect=redirect):
            with self.assertRaisesRegex(runtime.SourceError, 'HTTPS'):
                runtime.fetch(source, self.root / 'failed')

    def test_retained_build_evidence_hash_is_checked(self):
        source = self.lock['sources'][0]
        source['content_sha256']['recipe'] = '0' * 64
        archive = self.root / 'source.tar.gz'
        archive.write_bytes(self.payloads[source['id']])
        with self.assertRaisesRegex(runtime.SourceError, 'evidence checksum'):
            runtime.inspect_archive(archive, source)

    def test_reviewed_lock_preserves_runtime_patch_equivalence_and_original_patch_lists(self):
        lock = runtime.read_lock()
        sources = {source['id']: source for source in lock['sources']}
        origin = sources['crosstool-ng']['content_sha256']
        later = sources['crosstool-ng-build-window']['content_sha256']
        self.assertTrue(origin)
        self.assertTrue(all(later.get(name) == checksum for name, checksum in origin.items()))
        additions = set(later) - set(origin)
        self.assertEqual(len(additions), 2)
        self.assertTrue(any('LoongArch' in name for name in additions))
        self.assertTrue(any('mips' in name for name in additions))
        original = lock['source_resolution']['rust_prebuilt_runtime']
        patches = original['gcc']['patches'] + original['mingw_crt_patches']
        recipe = sources['rust-mingw-build-recipes']
        self.assertEqual(len(patches), 17)
        self.assertTrue(all('patches/' + name in recipe['content_sha256'] for name in patches))
        self.assertIn('not attested', lock['completeness']['provenance_limits'][0])


if __name__ == '__main__':
    unittest.main()
