#!/usr/bin/env python3
"""Windows source collector fixtures, corruption, coverage and credential safety."""
import copy
import hashlib
import io
import json
import os
import re
from pathlib import Path
import tarfile
import tempfile
import types
import unittest
from unittest.mock import patch
import urllib.error
import urllib.request
import zipfile

import source_windows as windows
import source_windows_rav1e as rav1e


def digest(data):
    return hashlib.sha256(data).hexdigest()


def tar_bytes(files, mode='w:xz', links=None):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode=mode) as archive:
        for name, data in files.items():
            entry = tarfile.TarInfo(name)
            entry.size = len(data)
            archive.addfile(entry, io.BytesIO(data))
        for name, target in (links or {}).items():
            entry = tarfile.TarInfo(name)
            entry.type = tarfile.SYMTYPE
            entry.linkname = target
            archive.addfile(entry)
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

    def cache(self, payload=None, missing=None, extra=None):
        # Match the real upload-artifact nesting, including Unix alias links.
        recipes, snapshots = [], {}
        path = self.root / 'cache.zip'
        for index, name in enumerate(('50-x264', '50-x265', '50-extra')):
            entry = name + '_' + str(index) * 64 + '.tar.xz'
            recipes.append({'recipe': 'scripts.d/' + name + '.sh',
                            'enabled': index != 2, 'download': entry})
            if name != missing:
                snapshots['.cache/downloads/' + entry] = (payload if payload is not None else
                    tar_bytes({'./source.c': b'int source;', './CMakeLists.txt': b'project(source)'}))
        if extra:
            snapshots.update(extra)
        recipes.append({'recipe': 'scripts.d/15-base.sh', 'enabled': True, 'download': None})
        with zipfile.ZipFile(path, 'w') as archive:
            aliases = {name.rsplit('_', 1)[0] + '.tar.xz': name.rsplit('/', 1)[1] for name in snapshots if name.endswith('.tar.xz')}
            archive.writestr('cache.tar.gz', tar_bytes(snapshots, 'w:gz', links=aliases))
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

    def test_incomplete_review_fails_closed_before_downloading(self):
        self.lock['source']['completeness'] = {'status': 'blocked', 'missing': ['rav1e source missing']}
        lock_path = self.root / 'lock.json'
        lock_path.write_text(json.dumps(self.lock))
        with patch.object(windows, 'LOCK', lock_path), patch.object(windows, 'download') as fetch:
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
        with self.assertRaisesRegex(windows.SourceError, 'single cache.tar.gz'):
            windows.validate_cache(path, self.lock)
        with self.assertRaisesRegex(windows.SourceError, 'unsafe path'):
            windows.validate_cache(self.cache(payload=tar_bytes({'../escape.c': b'bad'})), self.lock)
        with self.assertRaisesRegex(windows.SourceError, 'unsafe path'):
            windows.validate_cache(self.cache(extra={'../escape': b'bad'}), self.lock)

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

    def test_public_artifact_fallback_keeps_credentials_on_api_host(self):
        public = self.lock['source']['cache']['public_url']
        for code in (401, 403, 404):
            error = urllib.error.HTTPError('https://api.github.com/pinned', code, '', {}, None)
            with patch.object(windows, '_open', side_effect=error):
                self.assertEqual(windows.artifact_url('pinned', public), public)
        error = urllib.error.HTTPError('https://api.github.com/pinned', 410, '', {}, None)
        with patch.object(windows, '_open', side_effect=error):
            with self.assertRaisesRegex(windows.SourceError, 'unavailable'):
                windows.artifact_url('pinned', public)

    def test_public_provenance_survives_repository_scoped_token(self):
        seen = []
        def read(req):
            seen.append(req)
            if req.get_header('Authorization'):
                raise urllib.error.HTTPError(req.full_url, 403, '', {}, None)
            return Response(b'{"id": 123}')
        with patch.dict(os.environ, {'GH_TOKEN': 'private-token'}), patch.object(windows, '_open', side_effect=read):
            self.assertEqual(windows.api_json('repos/public/repo/actions/runs/123'), {'id': 123})
        self.assertEqual(len(seen), 2)
        self.assertIsNone(seen[1].get_header('Authorization'))
        self.assertTrue(all(req.host == 'api.github.com' for req in seen))

    def test_source_cache_is_rehashed_and_bounded(self):
        cache = self.root / 'provider-cache'
        cache.mkdir()
        data = b'actual source'
        entry = cache / digest(data)
        entry.write_bytes(data)
        with patch.dict(os.environ, {'JVID_WINDOWS_SOURCE_CACHE': str(cache)}), patch.object(windows, 'download') as network:
            windows.fetch('https://source.invalid', self.out / 'source', digest(data), len(data))
            network.assert_not_called()
            entry.write_bytes(b'corrupt cache')
            with self.assertRaisesRegex(windows.SourceError, 'SHA-256 mismatch'):
                windows.fetch('https://source.invalid', self.out / 'bad', digest(data), len(data))
        self.assertFalse((self.out / 'bad').exists())

    def test_original_source_archive_preserves_unix_fixture_names(self):
        payload = tar_bytes({'./tests/a:b\\c.c': b'int test;', './Makefile': b'test'})
        self.assertEqual(len(windows.validate_cache(self.cache(payload=payload), self.lock)), 4)

    def test_ffmpeg_source_archive_requires_build_and_library_sources(self):
        revision = self.lock['source']['ffmpeg']['revision']
        prefix = 'FFmpeg-' + revision + '/'
        files = {prefix + name: b'source' for name in ('configure', 'Makefile', 'COPYING.GPLv3',
                 'libavcodec/avcodec.h', 'libavformat/avformat.h', 'libavutil/avutil.h')}
        path = self.root / 'ffmpeg.tar.gz'
        path.write_bytes(tar_bytes(files, 'w:gz'))
        windows.validate_ffmpeg(path, revision)
        del files[prefix + 'configure']
        path.write_bytes(tar_bytes(files, 'w:gz'))
        with self.assertRaisesRegex(windows.SourceError, 'required source'):
            windows.validate_ffmpeg(path, revision)

    def rav1e_fixture(self):
        data = tar_bytes({'fixture-1.0.0/Cargo.toml': b'[package]\nname="fixture"\nversion="1.0.0"\n',
                          'fixture-1.0.0/src/lib.rs': b'pub fn source() {}'}, 'w:gz')
        lock = ('version = 3\n\n[[package]]\nname = "fixture"\nversion = "1.0.0"\n'
                'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
                'checksum = "' + digest(data) + '"\n')
        root = self.root / 'repo'
        (root / 'packaging').mkdir(parents=True)
        manifest = {'revision': '31435de9d76fddd38f6dcc31d4014574cebb2092',
                    'crates': [{'name': 'fixture', 'version': '1.0.0', 'sha256': digest(data)}],
                    'provenance': {'original_lock_sha256': '0' * 64}}
        spec = {'original_lock_sha256': '0' * 64}
        for field, name, content in [('manifest', 'source-windows-rav1e.json', json.dumps(manifest)),
                                    ('build_lock', 'source-windows-rav1e-Cargo.lock', lock)]:
            (root / 'packaging' / name).write_bytes(content.encode())
            spec[field] = {'file': name, 'sha256': digest(content.encode())}
        self.lock['source']['rav1e'] = spec
        self.lock['source']['recipe_audit']['recipes'].append({
            'recipe': 'scripts.d/50-rav1e.sh', 'enabled': True, 'download': None,
            'source_refs': {'COMMIT': manifest['revision']}})
        return root, data

    def test_rav1e_sources_build_lock_and_offline_vendor_recipe(self):
        root, data = self.rav1e_fixture()
        with patch.object(windows, 'ROOT', root), patch.object(windows, '_open', return_value=Response(data)):
            files = windows.collect_rav1e(self.out, self.lock)
        self.assertIn('rav1e-crates/fixture-1.0.0.crate', files)
        source = self.root / 'rav1e-source'
        source.mkdir()
        rav1e.prepare(self.out, source)
        checksums = json.loads((source / 'vendor/fixture-1.0.0/.cargo-checksum.json').read_text())
        self.assertEqual(checksums['package'], digest(data))
        self.assertEqual(checksums['files']['src/lib.rs'], digest(b'pub fn source() {}'))
        self.assertIn('offline = true', (source / '.cargo/config.toml').read_text())
        self.assertEqual((source / 'Cargo.lock').read_bytes(), (self.out / 'source-windows-rav1e-Cargo.lock').read_bytes())

    def test_rav1e_missing_corrupt_or_mismatched_crate_is_fatal(self):
        root, data = self.rav1e_fixture()
        with patch.object(windows, 'ROOT', root), patch.object(windows, '_open', return_value=Response(b'corrupt')):
            with self.assertRaisesRegex(windows.SourceError, 'SHA-256 mismatch'):
                windows.collect_rav1e(self.out, self.lock)
        self.assertFalse((self.out / 'rav1e-crates/fixture-1.0.0.crate').exists())
        (root / 'packaging/source-windows-rav1e.json').write_text('{}')
        with patch.object(windows, 'ROOT', root):
            with self.assertRaisesRegex(windows.SourceError, 'inventory/lock checksum'):
                windows.collect_rav1e(self.root, self.lock)

    def test_rav1e_vendor_rejects_corrupt_or_escaping_archive(self):
        for entry in ('../escape', 'fixture-1.0.0/../../escape'):
            bundle = self.root / ('bundle' + str(len(entry)))
            bundle.mkdir()
            (bundle / 'rav1e-crates').mkdir()
            data = tar_bytes({entry: b'source'}, 'w:gz')
            (bundle / 'rav1e-crates/fixture-1.0.0.crate').write_bytes(data)
            (bundle / 'source-windows-rav1e.json').write_text(json.dumps({'crates': [
                {'name': 'fixture', 'version': '1.0.0', 'sha256': digest(data)}]}))
            source = bundle / 'source'
            source.mkdir()
            with self.assertRaisesRegex(RuntimeError, 'unsafe'):
                rav1e.prepare(bundle, source)
        self.assertFalse((self.root / 'escape').exists())

    def test_compiler_runtime_lock_is_bound_to_windows_runtime(self):
        helper_lock = self.root / 'toolchain-lock.json'
        source = self.lock['source']
        contents = json.dumps({'runtime': {'repository': source['repository'],
            'build_scripts_commit': self.lock['build_scripts_commit'], 'run_id': source['run_id'],
            'target': source['target'], 'variant': source['variant'], 'addin': ' '.join(source['addins'])}}).encode()
        helper_lock.write_bytes(contents)
        self.lock['source']['compiler_runtime_lock'] = {'file': helper_lock.name, 'sha256': digest(contents)}
        def capture(out):
            (out / 'source.c').write_bytes(b'source')
            return [{'id': 'compiler-runtime', 'files': ['source.c']}]
        helper = types.SimpleNamespace(LOCK=helper_lock, collect=capture)
        with patch.dict('sys.modules', {'compiler_runtime_sources': helper}):
            records = windows.collect_compiler_runtimes(self.out, self.lock)
            self.assertEqual(records[0]['files'], ['compiler-runtime/source.c'])
            self.lock['source']['run_id'] += 1
            with self.assertRaisesRegex(windows.SourceError, 'compiler runtime run_id'):
                windows.collect_compiler_runtimes(self.out, self.lock)
            self.lock['source']['run_id'] -= 1
            helper_lock.write_bytes(b'{"drift":true}')
            with self.assertRaisesRegex(windows.SourceError, 'lock checksum'):
                windows.collect_compiler_runtimes(self.out, self.lock)

    def test_collect_fixture_retains_sources_recipes_provenance_and_all_records(self):
        cache = self.cache().read_bytes()
        root, crate = self.rav1e_fixture()
        revision = self.lock['source']['ffmpeg']['revision']
        ffmpeg = tar_bytes({'FFmpeg-' + revision + '/' + name: b'source' for name in
            ('configure', 'Makefile', 'COPYING.GPLv3', 'libavcodec/avcodec.h',
             'libavformat/avformat.h', 'libavutil/avutil.h')}, 'w:gz')
        required = ('build.sh', 'download.sh', 'generate.sh', 'util/dl_functions.sh',
                    'util/run_stage.sh', 'util/vars.sh', '.github/workflows/build.yml',
                    'images/base/Dockerfile', 'images/base/git-mini-clone.sh',
                    'images/base-win64/Dockerfile', 'images/base-win64/ct-ng-config',
                    'images/base-win64/toolchain.cmake', 'images/base-win64/cross.meson',
                    'variants/win64-gpl-shared.sh', 'variants/defaults-gpl-shared.sh',
                    'variants/defaults-gpl.sh', 'variants/windows-install-shared.sh', 'addins/8.1.sh')
        names = list(required) + [row['recipe'] for row in self.lock['source']['recipe_audit']['recipes']]
        scripts = tar_bytes({'FFmpeg-Builds-' + self.lock['build_scripts_commit'] + '/' + name: b'build recipe' for name in names}, 'w:gz')
        rust = self.lock['source']['rust_std']
        rust_data = tar_bytes({rust['archive_root'] + '/' + name: b'runtime source' for name in rust['required_files']})
        rust['sha256'] = digest(rust_data)
        self.lock['source']['ffmpeg']['sha256'] = digest(ffmpeg)
        self.lock['source']['build_scripts']['sha256'] = digest(scripts)
        self.lock['source']['cache'].update({'digest': 'sha256:' + digest(cache), 'size_in_bytes': len(cache)})
        self.lock['source']['completeness'] = {'status': 'complete', 'missing': []}
        lock = self.root / 'lock.json'
        lock.write_text(json.dumps(self.lock))
        payloads = {'ffmpeg.tar.gz': ffmpeg, 'btbn-build-scripts.tar.gz': scripts,
                    'dependency-source-cache.zip': cache, 'fixture-1.0.0.crate': crate,
                    'rust-runtime-source.tar.xz': rust_data}
        def fetch(url, dest, checksum, max_bytes, exact_size=None):
            data = payloads[dest.name]
            self.assertEqual(digest(data), checksum)
            self.assertLessEqual(len(data), max_bytes)
            if exact_size is not None: self.assertEqual(len(data), exact_size)
            dest.write_bytes(data)
        with patch.object(windows, 'ROOT', root), patch.object(windows, 'LOCK', lock), \
                patch.object(windows, 'api_json', side_effect=self.metadata), \
                patch.object(windows, 'artifact_url', return_value='https://download.invalid/pinned'), \
                patch.object(windows, 'fetch', side_effect=fetch), \
                patch.object(windows, 'collect_compiler_runtimes', return_value=[{'id': 'compiler-fixture', 'files': ['btbn-build-scripts.tar.gz']}]):
            records = windows.collect(self.stage, self.out)
        self.assertEqual(len(records), 9)
        self.assertIn('windows-ffmpeg', {r['id'] for r in records})
        self.assertIn('windows-rust-standard-library', {r['id'] for r in records})
        self.assertIn('windows-rav1e-crates', {r['id'] for r in records})
        for record in records:
            self.assertTrue(record['files'])
            for name in record['files']:
                self.assertTrue((self.out / name).is_file(), name)
        self.assertNotIn(str(self.root), json.dumps(records))
        from sources import source_inventory, validate_inventory
        validate_inventory(source_inventory(self.out), records)

    def test_rust_standard_library_requires_vendored_source_payload(self):
        spec = self.lock['source']['rust_std']
        payload = tar_bytes({spec['archive_root'] + '/library/std/src/lib.rs': b'pub mod std; '})
        spec['sha256'] = digest(payload)
        with patch.object(windows, '_open', return_value=Response(payload)):
            with self.assertRaisesRegex(windows.SourceError, 'required source/build/vendor'):
                windows.collect_rust_std(self.out, self.lock)

    def test_reviewed_rav1e_inventory_has_valid_package_identities_and_checksums(self):
        manifest = json.loads((windows.ROOT / 'packaging/source-windows-rav1e.json').read_text())
        crates = manifest['crates']
        self.assertEqual(len(crates), 275)
        self.assertEqual(windows.cargo_packages((windows.ROOT / 'packaging/source-windows-rav1e-Cargo.lock').read_text()),
                         {c['name'] + '-' + c['version']: c['sha256'] for c in crates})
        for crate in crates:
            self.assertIsNotNone(re.fullmatch(r'[A-Za-z0-9_-]+', crate['name']))
            self.assertIsNotNone(re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.+-]+)?', crate['version']))


if __name__ == '__main__':
    unittest.main()
