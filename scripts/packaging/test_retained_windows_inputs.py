#!/usr/bin/env python3
"""Retained inputs keep the original provenance and whole-cache checksum gates."""
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import retained_windows_inputs as retained
import source_windows as windows


def digest(data):
    return hashlib.sha256(data).hexdigest()


class RetainedInputsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'packaging').mkdir()
        self.lock = json.loads(windows.LOCK.read_text())

    def write_provenance(self, change=None):
        source = self.lock['source']
        prefix = 'repos/' + source['repository'] + '/actions'
        run = {**windows.expected_run(source, self.lock),
               'repository': {'full_name': source['repository'], 'id': source['repository_id']}}
        cache = {**source['cache'], 'expired': False, 'created_at': '2026-10-31T12:00:00Z',
                 'expires_at': '2026-11-14T12:00:00Z',
                 'workflow_run': {'id': source['run_id'], 'head_sha': self.lock['build_scripts_commit'],
                                  'repository_id': source['repository_id']}}
        records = {f"{prefix}/runs/{source['run_id']}": run,
                   f"{prefix}/artifacts/{source['cache']['id']}": cache}
        for field in ('build_job', 'variant_job'):
            job = {**source[field], 'run_id': source['run_id'], 'status': 'completed', 'conclusion': 'success'}
            records[f"{prefix}/jobs/{job['id']}"] = job
        if change:
            change(records)
        contents = json.dumps({'schema': 1, 'captured_at': '2026-11-14T05:00:00Z', 'records': records}).encode()
        (self.root / 'packaging/provenance.json').write_bytes(contents)
        source['provenance'] = {'file': 'provenance.json', 'sha256': digest(contents)}

    def test_retained_provenance_passes_every_existing_identity_check(self):
        self.write_provenance()
        with patch.object(windows, 'ROOT', self.root), patch.object(windows, 'api_json') as api:
            proof = windows.validate_provenance(self.lock)
        api.assert_not_called()
        self.assertEqual(proof['run']['id'], self.lock['source']['run_id'])

    def test_wrong_run_failed_job_and_expired_capture_are_rejected(self):
        original = copy.deepcopy(self.lock)
        for key, value in [('run_id', 0), ('conclusion', 'failure'), ('expired', True)]:
            self.lock = copy.deepcopy(original)
            def change(records):
                for record in records.values():
                    if key in record:
                        record[key] = value
            self.write_provenance(change)
            with patch.object(windows, 'ROOT', self.root), self.assertRaises(windows.SourceError):
                windows.validate_provenance(self.lock)

    def test_corrupt_or_incomplete_provenance_is_rejected(self):
        self.write_provenance()
        path = self.root / 'packaging/provenance.json'
        path.write_bytes(path.read_bytes() + b' ')
        with patch.object(windows, 'ROOT', self.root), self.assertRaisesRegex(windows.SourceError, 'SHA-256'):
            windows.validate_provenance(self.lock)
        self.write_provenance(lambda records: records.pop(next(iter(records))))
        with patch.object(windows, 'ROOT', self.root), self.assertRaises(windows.SourceError):
            windows.validate_provenance(self.lock)

    def test_capture_must_precede_artifact_expiry(self):
        self.write_provenance()
        path = self.root / 'packaging/provenance.json'
        value = json.loads(path.read_text())
        value['captured_at'] = '2026-11-15T00:00:00Z'
        contents = json.dumps(value).encode()
        path.write_bytes(contents)
        self.lock['source']['provenance']['sha256'] = digest(contents)
        with patch.object(windows, 'ROOT', self.root), self.assertRaisesRegex(windows.SourceError, 'capture time'):
            windows.validate_provenance(self.lock)

    def test_source_parts_and_combined_original_digest_are_checked(self):
        payloads = [b'first source part', b'second source part']
        parts = [{'url': 'https://example.com/' + str(index), 'sha256': digest(data), 'size': len(data)}
                 for index, data in enumerate(payloads)]
        cache = {'retained_parts': parts, 'size_in_bytes': sum(map(len, payloads)),
                 'digest': 'sha256:' + digest(b''.join(payloads))}
        def fetch(url, destination, expected, limit, *, exact_size):
            data = payloads[int(url.rsplit('/', 1)[1])]
            self.assertEqual((digest(data), len(data), limit), (expected, exact_size, exact_size))
            destination.write_bytes(data)
        destination = self.root / 'cache.zip'
        with patch.object(windows, 'fetch', side_effect=fetch):
            retained.fetch_cache(cache, destination)
            self.assertEqual(destination.read_bytes(), b''.join(payloads))
            cache['digest'] = 'sha256:' + '0' * 64
            with self.assertRaisesRegex(windows.SourceError, 'SHA-256'):
                retained.fetch_cache(cache, self.root / 'corrupt.zip')
        self.assertFalse((self.root / 'corrupt.zip').exists())
        self.assertFalse((self.root / 'corrupt.zip.part').exists())

    def test_missing_and_oversized_parts_fail_before_download(self):
        for parts in ([], [{'size': 2 * 1024**3}], [{'size': 1}]):
            cache = {'retained_parts': parts, 'size_in_bytes': 2}
            with patch.object(windows, 'fetch') as fetch, self.assertRaises(windows.SourceError):
                retained.fetch_cache(cache, self.root / 'cache.zip')
            fetch.assert_not_called()

    def test_part_failure_removes_partial_cache(self):
        cache = {'retained_parts': [{'url': 'https://example.com/part', 'sha256': '0' * 64, 'size': 1}],
                 'size_in_bytes': 1}
        destination = self.root / 'cache.zip'
        with patch.object(windows, 'fetch', side_effect=windows.SourceError('download failed')):
            with self.assertRaises(windows.SourceError):
                retained.fetch_cache(cache, destination)
        self.assertFalse(destination.exists())
        self.assertFalse((self.root / 'cache.zip.part').exists())


if __name__ == '__main__':
    unittest.main()
