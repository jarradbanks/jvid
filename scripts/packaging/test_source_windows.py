#!/usr/bin/env python3
"""Windows source collector fixtures, corruption, coverage and credential safety."""
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
import urllib.request
import zipfile

import source_windows as windows


def digest(data):
    return hashlib.sha256(data).hexdigest()


def tar_bytes(files, mode='w:xz'):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode=mode) as archive:
        for name, data in files.items():
            entry = tarfile.TarInfo(name)
            entry.size = len(data)
            archive.addfile(entry, io.BytesIO(data))
    return stream.getvalue()


class Response(io.BytesIO):
    pass


class WindowsSourcesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.stage = self.root / 'stage'
        self.stage.mkdir()
        (self.stage / 'runtime-licenses').mkdir()
        (self.stage / 'runtime-licenses/FFmpeg-GPL.txt').write_text('GNU GPL')
        self.out = self.root / 'out'
        self.out.mkdir()
        self.lock = json.loads(windows.LOCK.read_text())
        self.write_stage()

    def write_stage(self, extra=''):
        # Legacy binary stages record the original runtime keys only. New stages
        # include the complete source lock. Both must name exactly this runtime.
        record = {key: value for key, value in self.lock.items() if key != 'source'}
        (self.stage / 'SOURCE-INFO.txt').write_text('Corresponding source\njvid source: https://github.com/jarradbanks/jvid\n\n' + json.dumps(record) + '\n' + extra)

    def metadata(self, endpoint):
        source = self.lock['source']
        if '/runs/' in endpoint:
            return {**windows.expected_run(source, self.lock),
                    'repository': {'full_name': source['repository'], 'id': source['repository_id']}}
        if '/artifacts/' in endpoint:
            return {**source['cache'], 'expired': False,
                    'workflow_run': {'id': source['run_id'], 'head_sha': self.lock['build_scripts_commit'],
                                     'repository_id': source['repository_id']}}
        for name in ('build_job', 'variant_job'):
            if endpoint.endswith('/' + str(source[name]['id'])):
                return {**source[name], 'run_id': source['run_id'],
                        'conclusion': 'success', 'status': 'completed'}
        raise AssertionError(endpoint)

    def cache(self, payload=None, missing=None):
        # Fixture retains all four cases: enabled source, disabled source,
        # no-download stage and required GPL codecs.
        recipes = []
        path = self.root / 'cache.zip'
        with zipfile.ZipFile(path, 'w') as archive:
            for index, name in enumerate(('50-x264', '50-x265', '50-extra')):
                entry = name + '_' + str(index) * 64 + '.tar.xz'
                recipes.append({'recipe': 'scripts.d/' + name + '.sh',
                                'enabled': index != 2, 'download': entry})
                if name != missing:
                    archive.writestr(entry, payload if payload is not None else tar_bytes({'./source.c': b'int source;', './CMakeLists.txt': b'project(source)'}))
            recipes.append({'recipe': 'scripts.d/15-base.sh', 'enabled': True, 'download': None})
        self.lock['source']['recipe_audit']['recipes'] = recipes
        return path

    def test_stage_matches_exact_runtime_and_all_providers(self):
        windows.validate_stage(self.stage, self.lock)
        (self.stage / 'runtime-licenses/unrecorded').mkdir()
        with self.assertRaisesRegex(windows.SourceError, 'unrepresented'):
            windows.validate_stage(self.stage, self.lock)

    def test_missing_provider_and_runtime_drift_fail(self):
        (self.stage / 'runtime-licenses/FFmpeg-GPL.txt').unlink()
        with self.assertRaisesRegex(windows.SourceError, 'missing recorded'):
            windows.validate_stage(self.stage, self.lock)
        (self.stage / 'runtime-licenses/FFmpeg-GPL.txt').write_text('GPL')
        self.lock['sha256'] = '0' * 64
        with self.assertRaisesRegex(windows.SourceError, 'recorded Windows sha256'):
            windows.validate_stage(self.stage, self.lock)

    def test_additional_source_info_provider_is_not_ignored(self):
        self.write_stage('Unknown dependency 1.0\n')
        with self.assertRaisesRegex(windows.SourceError, 'additional'):
            windows.validate_stage(self.stage, self.lock)
        self.write_stage('{}\n')
        with self.assertRaisesRegex(windows.SourceError, 'additional'):
            windows.validate_stage(self.stage, self.lock)

    def test_revision_and_gpl_variant_must_match_runtime(self):
        for change in ('revision', 'variant'):
            lock = copy.deepcopy(self.lock)
            if change == 'revision':
                lock['source']['ffmpeg']['revision'] = '0' * 40
            else:
                lock['source']['variant'] = 'nonfree'
            with self.assertRaisesRegex(windows.SourceError, 'filename|variant'):
                windows.validate_stage(self.stage, lock)

    def test_current_pin_fails_closed_before_downloading(self):
        with patch.object(windows, 'download') as fetch:
            with self.assertRaisesRegex(windows.SourceError, 'incomplete:.*rav1e'):
                windows.collect(self.stage, self.out)
        fetch.assert_not_called()
        self.assertEqual(list(self.out.iterdir()), [])

    def test_reject_nonempty_output(self):
        (self.out / 'previous').write_text('previous source')
        with self.assertRaisesRegex(windows.SourceError, 'fresh empty'):
            windows.collect(self.stage, self.out)

    def test_streaming_download_and_corruption_cleanup(self):
        data = b'payload' * 1000
        original_read_bytes = Path.read_bytes
        with patch.object(windows, '_open', return_value=Response(data)), patch.object(Path, 'read_bytes', side_effect=AssertionError('whole-file read')):
            windows.download('https://sources.invalid/source', self.out / 'source', digest(data), len(data))
            self.assertEqual(windows.sha256(self.out / 'source'), digest(data))
        self.assertEqual(original_read_bytes(self.out / 'source'), data)
        for expected, limit, exact in (('0' * 64, len(data), None), (digest(data), 1, None), (digest(data), len(data), len(data) + 1)):
            with patch.object(windows, '_open', return_value=Response(data)):
                with self.assertRaisesRegex(windows.SourceError, 'mismatch|limit'):
                    windows.download('https://sources.invalid/source', self.out / 'bad', expected, limit, exact_size=exact)
            self.assertFalse((self.out / 'bad').exists())
            self.assertFalse((self.out / 'bad.part').exists())

    def test_api_token_never_reaches_download_host_or_errors(self):
        seen = []
        location = 'https://download.invalid/archive?secret-signed-url'
        def open_request(req):
            seen.append(req)
            if req.host == 'api.github.com':
                raise urllib.error.HTTPError(req.full_url, 302, 'redirect', {'Location': location}, None)
            return Response(b'source')
        with patch.dict(os.environ, {'GH_TOKEN': 'secret-token'}), patch.object(windows, '_open', side_effect=open_request):
            url = windows.artifact_url('repos/org/repo/actions/artifacts/1/zip')
            windows.download(url, self.out / 'download', digest(b'source'), 6)
        self.assertEqual(seen[0].get_header('Authorization'), 'Bearer secret-token')
        self.assertIsNone(seen[1].get_header('Authorization'))
        self.assertEqual(seen[1].full_url, location)
        with patch.object(windows, '_open', side_effect=urllib.error.URLError(location)):
            with self.assertRaises(windows.SourceError) as error:
                windows.download(location, self.out / 'failed', '0' * 64, 6)
        self.assertNotIn('secret', str(error.exception))
        self.assertIsNone(windows.NoRedirect().redirect_request(seen[0], None, 302, '', {}, location))

    def test_reject_insecure_redirect(self):
        failure = urllib.error.HTTPError('https://source.invalid', 302, 'redirect', {'Location': 'http://insecure.invalid'}, None)
        with patch.object(windows, '_open', side_effect=failure):
            with self.assertRaisesRegex(windows.SourceError, 'HTTPS'):
                windows.download('https://source.invalid', self.out / 'bad', '0' * 64, 10)

    def test_provenance_revalidates_digest_run_repository_and_variant(self):
        with patch.object(windows, 'api_json', side_effect=self.metadata):
            receipt = windows.validate_provenance(self.lock)
        self.assertEqual(receipt['cache']['digest'], self.lock['source']['cache']['digest'])
        self.assertNotIn('archive_download_url', json.dumps(receipt))
        for field in ('digest', 'expired', 'run', 'repository', 'job'):
            def changed(endpoint):
                result = self.metadata(endpoint)
                if '/artifacts/' in endpoint:
                    if field == 'digest': result['digest'] = 'sha256:' + '0' * 64
                    if field == 'expired': result['expired'] = True
                    if field == 'run': result['workflow_run']['id'] += 1
                if '/runs/' in endpoint and field == 'repository': result['repository']['id'] += 1
                if '/jobs/' in endpoint and field == 'job': result['name'] = 'Build ffmpeg (win64, nonfree)'
                return result
            with self.subTest(field=field), patch.object(windows, 'api_json', side_effect=changed):
                with self.assertRaisesRegex(windows.SourceError, 'provenance'):
                    windows.validate_provenance(self.lock)

    def test_cache_contains_exact_sources_and_disabled_superset(self):
        records = windows.validate_cache(self.cache(), self.lock)
        self.assertEqual(len(records), 4)
        self.assertEqual(len([record for record in records if record.get('source_files')]), 3)
        self.assertFalse(records[2]['enabled'])
        self.assertEqual(records[3]['coverage'], 'recipe only; no download stage')

    def test_cache_missing_provider_corruption_and_binary_only_fail(self):
        with self.assertRaisesRegex(windows.SourceError, 'missing exact'):
            windows.validate_cache(self.cache(missing='50-x265'), self.lock)
        with self.assertRaises((tarfile.TarError, windows.SourceError)):
            windows.validate_cache(self.cache(payload=b'corruption'), self.lock)
        with self.assertRaisesRegex(windows.SourceError, 'lacks source'):
            windows.validate_cache(self.cache(payload=tar_bytes({'./lib.dll': b'ELF', './LICENSE': b'GPL'})), self.lock)

    def test_cache_archive_path_escape_fails(self):
        path = self.cache()
        with zipfile.ZipFile(path, 'a') as archive:
            archive.writestr('../escape', b'bad')
        with self.assertRaisesRegex(windows.SourceError, 'unsafe path'):
            windows.validate_cache(path, self.lock)
        with self.assertRaisesRegex(windows.SourceError, 'unsafe path'):
            windows.validate_cache(self.cache(payload=tar_bytes({'../escape.c': b'bad'})), self.lock)

    def test_script_archive_is_read_without_extraction(self):
        prefix = 'FFmpeg-Builds-' + self.lock['build_scripts_commit']
        path = self.root / 'scripts.tar.gz'
        path.write_bytes(tar_bytes({prefix + '/build.sh': b'build', prefix + '/patches/a.patch': b'patch'}, 'w:gz'))
        files = windows.read_scripts(path, self.lock['build_scripts_commit'])
        self.assertEqual(files['build.sh'], b'build')
        path.write_bytes(tar_bytes({'wrong/build.sh': b'build'}, 'w:gz'))
        with self.assertRaisesRegex(windows.SourceError, 'root mismatch'):
            windows.read_scripts(path, self.lock['build_scripts_commit'])

    def test_recipe_audit_covers_every_script_and_referenced_patch(self):
        # Full pinned tree is checksum-bound; fixture exercises missing coverage
        # before checking the platform's mandatory recipe files.
        self.lock['source']['recipe_audit']['recipes'] = []
        with self.assertRaisesRegex(windows.SourceError, 'recipe coverage'):
            windows.validate_recipes({'scripts.d/new-provider.sh': b'source'}, self.lock)
        self.lock['source']['recipe_audit']['recipes'] = [{'recipe': 'scripts.d/new-provider.sh'}]
        with self.assertRaisesRegex(windows.SourceError, 'missing BtbN build script'):
            windows.validate_recipes({'scripts.d/new-provider.sh': b'source'}, self.lock)


if __name__ == '__main__':
    unittest.main()
