"""Release transitions use disposable local remotes and a strict mock GitHub API."""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import urllib.parse

import automation as a
from github_api import pages, tag_commit

REPOSITORY = 'owner/project'


def git(root, *args):
    return subprocess.check_output(['git', '-c', 'gc.auto=0', '-c', 'maintenance.auto=false',
                                    *args], cwd=root, text=True, stderr=subprocess.DEVNULL).strip()


class FakeGitHub:
    def __init__(self, root, remote):
        self.root, self.remote = root, remote
        self.prs, self.runs, self.tags, self.releases, self.calls = [], [], {}, {}, []
        self.fail = None
        self.fail_after = None

    def __call__(self, path, method='GET', body=None):
        self.calls.append((path, method, body))
        if self.fail and self.fail in path and method == 'POST':
            self.fail = None
            raise RuntimeError('simulated API outage')
        result = self.route(path, method, body)
        if self.fail_after and self.fail_after in path and method == 'POST':
            self.fail_after = None
            raise RuntimeError('response lost after mutation')
        return result

    def route(self, path, method, body):
        prefix = f'repos/{REPOSITORY}/'
        if not path.startswith(prefix):
            raise AssertionError(path)
        path = path[len(prefix):]
        if path.startswith('pulls?'):
            return self.prs
        if path == 'pulls' and method == 'POST':
            pr = {'state': 'open', 'html_url': 'https://example.test/pr/1',
                  'head': {'ref': body['head'], 'repo': {'full_name': REPOSITORY}}}
            self.prs.append(pr)
            return pr
        if path.startswith('git/ref/heads/'):
            branch = urllib.parse.unquote(path.removeprefix('git/ref/heads/'))
            try:
                sha = git(self.remote, 'rev-parse', '--verify', 'refs/heads/' + branch)
            except subprocess.CalledProcessError:
                return None
            return {'object': {'type': 'commit', 'sha': sha}}
        if path.startswith('git/ref/tags/'):
            sha = self.tags.get(path.removeprefix('git/ref/tags/'))
            return {'object': {'type': 'commit', 'sha': sha}} if sha else None
        if path == 'git/tags' and method == 'POST':
            return {'sha': body['object']}
        if path == 'git/refs' and method == 'POST':
            self.tags[body['ref'].removeprefix('refs/tags/')] = body['sha']
            return {}
        if path.startswith('releases/tags/'):
            return self.releases.get(path.removeprefix('releases/tags/'))
        if path.startswith('releases?'):
            return list(self.releases.values())
        if path.startswith('actions/workflows/'):
            workflow = path.split('/')[2]
            if '/runs?' in path:
                return {'workflow_runs': [r for r in self.runs if r['workflow'] == workflow]}
            if path.endswith('/dispatches') and method == 'POST':
                ref = body['ref']
                if ref.startswith('v'):
                    sha, title = self.tags[ref], 'Release ' + ref
                else:
                    sha, title = git(self.remote, 'rev-parse', 'refs/heads/' + ref), 'Package ' + ref
                self.runs.append({'workflow': workflow, 'head_sha': sha, 'display_title': title,
                                  'status': 'queued', 'conclusion': None})
                return None
        raise AssertionError((path, method, body))


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / 'repo'
        self.remote = Path(self.temp.name) / 'remote.git'
        self.root.mkdir()
        git(self.root, 'init', '-b', 'main')
        git(self.root, 'config', 'user.name', 'Release test')
        git(self.root, 'config', 'user.email', 'fixture@example.test')
        git(self.root, 'config', 'commit.gpgsign', 'false')
        (self.root / '.github').mkdir()
        (self.root / 'packaging').mkdir()
        (self.root / 'crates/app').mkdir(parents=True)
        (self.root / 'Cargo.toml').write_text('[workspace]\nmembers = ["crates/*"]\n'
                                            '[workspace.package]\nversion = "0.1.0"\n')
        (self.root / 'crates/app/Cargo.toml').write_text('[package]\nname = "app"\nversion.workspace = true\n')
        (self.root / 'Cargo.lock').write_text('version = 4\n\n[[package]]\nname = "app"\nversion = "0.1.0"\n'
            '\n[[package]]\nname = "app"\nversion = "0.1.0"\nsource = "registry+https://example.test/index"\n'
            'checksum = "' + 'f' * 64 + '"\n')
        (self.root / 'packaging/release-notes-template.md').write_text('Download packages and separate source archives.\n')
        (self.root / 'packaging/release-notes.md').write_text('Previous notes\n')
        git(self.root, 'add', '.')
        git(self.root, 'commit', '-m', 'Initial state')
        self.before = git(self.root, 'rev-parse', 'HEAD')
        git(self.root, 'init', '--bare', str(self.remote))
        git(self.root, 'remote', 'add', 'origin', str(self.remote))
        git(self.root, 'push', 'origin', 'main')
        git(self.root, 'switch', '-c', 'develop')
        self.feature()
        self.api = FakeGitHub(self.root, self.remote)
        self.api_patch = patch.object(a, 'github', self.api)
        self.api_patch.start()
        self.addCleanup(self.api_patch.stop)
        self.silence = contextlib.redirect_stdout(io.StringIO())
        self.silence.__enter__()
        self.addCleanup(self.silence.__exit__, None, None, None)

    def feature(self):
        (self.root / 'feature.txt').write_text('new feature\n')
        git(self.root, 'add', '.')
        git(self.root, 'commit', '-m', 'Add a feature')
        git(self.root, 'push', 'origin', 'develop')

    def prepare(self):
        a.prepare_candidate(self.root, 'minor', REPOSITORY)
        return git(self.root, 'rev-parse', 'HEAD')

    def merge_release(self):
        candidate = self.prepare()
        git(self.root, 'switch', 'main')
        git(self.root, 'merge', '--no-ff', candidate, '-m', 'Merge reviewed release')
        commit = git(self.root, 'rev-parse', 'HEAD')
        git(self.root, 'push', 'origin', 'main')
        return commit

    def writes(self):
        return [c for c in self.api.calls if c[1] != 'GET']

    def test_lock_preserves_external_record_even_with_colliding_name_version(self):
        original = (self.root / 'Cargo.lock').read_text().split('[[package]]')[2]
        a.version_files(self.root, '0.2.0')
        self.assertEqual(original, (self.root / 'Cargo.lock').read_text().split('[[package]]')[2])
        self.assertEqual(a.version(self.root), '0.2.0')
        self.assertIn('version = "0.2.0"', (self.root / 'Cargo.lock').read_text().split('[[package]]')[1])

    def test_actual_workspace_lock_preserves_every_nonworkspace_record(self):
        checkout = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as temporary:
            snapshot = Path(temporary)
            for name in ('Cargo.toml', 'Cargo.lock'):
                (snapshot / name).write_bytes((checkout / name).read_bytes())
            for manifest in checkout.glob('crates/*/Cargo.toml'):
                destination = snapshot / manifest.relative_to(checkout)
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(manifest.read_bytes())
            before = (snapshot / 'Cargo.lock').read_text().split('[[package]]')
            a.version_files(snapshot, a.bump(a.version(snapshot), 'patch'))
            after = (snapshot / 'Cargo.lock').read_text().split('[[package]]')
            self.assertEqual(len(before), len(after))
            names = {a.tomllib.loads(m.read_text())['package']['name']
                     for m in snapshot.glob('crates/*/Cargo.toml')
                     if a.tomllib.loads(m.read_text())['package'].get('version') == {'workspace': True}}
            changed = set()
            for original, updated in zip(before[1:], after[1:]):
                package = a.tomllib.loads('[[package]]' + original)['package'][0]
                if original != updated:
                    self.assertNotIn('source', package)
                    changed.add(package['name'])
            self.assertEqual(changed, names)

    def test_lock_preserves_crlf_and_third_party_comments(self):
        lock = self.root / 'Cargo.lock'
        original = lock.read_text().replace('source = ', '# Third-party record stays byte exact\nsource = ').replace('\n', '\r\n').encode()
        lock.write_bytes(original)
        a.version_files(self.root, '0.2.0')
        self.assertEqual(original.split(b'[[package]]')[2], lock.read_bytes().split(b'[[package]]')[2])
        self.assertIn(b'version = "0.2.0"\r\n', lock.read_bytes())

    def test_lock_error_does_not_write_manifest_or_lock(self):
        lock = self.root / 'Cargo.lock'
        lock.write_text(lock.read_text().replace('name = "app"', 'name = "other"', 1))
        original = (self.root / 'Cargo.toml').read_bytes(), lock.read_bytes()
        with self.assertRaisesRegex(ValueError, 'Missing workspace'):
            a.version_files(self.root, '0.2.0')
        self.assertEqual(original, ((self.root / 'Cargo.toml').read_bytes(), lock.read_bytes()))

    def test_version_bump_rejects_nonstable_and_nonincrease(self):
        for value in ('0.1.0', '0.0.9', '01.2.0', '0.2.0-rc1', '1.0.0+build'):
            with self.assertRaises(ValueError):
                a.version_files(self.root, value)
        self.assertEqual(a.bump('1.2.3', 'patch'), '1.2.4')
        self.assertEqual(a.bump('1.2.3', 'major'), '2.0.0')

    def test_prepare_candidate_dispatches_both_and_preserves_develop(self):
        source = git(self.root, 'rev-parse', 'HEAD')
        candidate = self.prepare()
        plan = a.read_plan(self.root)
        self.assertEqual(plan['source_commit'], source)
        self.assertEqual(git(self.remote, 'rev-parse', 'develop'), source)
        self.assertNotEqual(candidate, source)
        self.assertEqual([r['workflow'] for r in self.api.runs], ['ci.yml', 'package.yml'])
        self.assertTrue(all(c[2] == {'ref': 'release/v0.2.0'} for c in self.writes() if '/dispatches' in c[0]))

    def test_retry_recovers_branch_after_pr_creation_failure(self):
        self.api.fail = '/pulls'
        with self.assertRaisesRegex(RuntimeError, 'outage'):
            self.prepare()
        candidate = git(self.root, 'rev-parse', 'HEAD')
        git(self.root, 'switch', 'develop')
        self.assertEqual(self.prepare(), candidate)
        self.assertEqual(len(self.api.prs), 1)
        self.assertEqual(len(self.api.runs), 2)

    def test_retry_recovers_lost_pr_and_dispatch_responses(self):
        self.api.fail_after = '/pulls'
        with self.assertRaisesRegex(RuntimeError, 'response lost'):
            self.prepare()
        candidate = git(self.root, 'rev-parse', 'HEAD')
        git(self.root, 'switch', 'develop')
        self.api.fail_after = 'ci.yml/dispatches'
        with self.assertRaisesRegex(RuntimeError, 'response lost'):
            self.prepare()
        git(self.root, 'switch', 'develop')
        self.assertEqual(self.prepare(), candidate)
        self.assertEqual(len(self.api.prs), 1)
        self.assertEqual(len(self.api.runs), 2)

    def test_retry_keeps_frozen_candidate_after_develop_advances(self):
        candidate = self.prepare()
        git(self.root, 'switch', 'develop')
        (self.root / 'later.txt').write_text('later work')
        git(self.root, 'add', '.')
        git(self.root, 'commit', '-m', 'Later work')
        git(self.root, 'push', 'origin', 'develop')
        self.assertEqual(self.prepare(), candidate)
        self.assertFalse((self.root / 'later.txt').exists())

    def test_changed_frozen_candidate_is_rejected(self):
        self.prepare()
        (self.root / 'feature.txt').write_text('changed frozen feature')
        git(self.root, 'add', '.')
        git(self.root, 'commit', '-m', 'Alter candidate')
        git(self.root, 'push', 'origin', 'HEAD:release/v0.2.0')
        git(self.root, 'switch', 'develop')
        with self.assertRaisesRegex(ValueError, 'unplanned code'):
            self.prepare()

    def test_changed_notes_or_lock_is_rejected(self):
        self.prepare()
        (self.root / 'packaging/release-notes.md').write_text('changed')
        with self.assertRaisesRegex(ValueError, 'notes differ'):
            a.read_plan(self.root)

    def test_conflicting_existing_release_candidate_or_tag_rejected(self):
        self.api.tags['v0.2.0'] = self.before
        with self.assertRaisesRegex(RuntimeError, 'already has a tag'):
            self.prepare()
        self.api.tags.clear()
        self.api.prs = [{'state': 'open', 'head': {'ref': 'release/v0.3.0'}}]
        with self.assertRaisesRegex(RuntimeError, 'existing release PR'):
            self.prepare()

    def test_closed_candidate_requires_explicit_branch_deletion_before_replacement(self):
        candidate = self.prepare()
        self.api.prs[0]['state'] = 'closed'
        git(self.root, 'switch', 'develop')
        with self.assertRaisesRegex(RuntimeError, 'explicitly abandoned'):
            self.prepare()
        git(self.remote, 'update-ref', '-d', 'refs/heads/release/v0.2.0')
        git(self.root, 'branch', '-D', 'release/v0.2.0')
        (self.root / 'fixed.txt').write_text('Correct candidate bug')
        git(self.root, 'add', '.')
        git(self.root, 'commit', '-m', 'Fix candidate bug')
        git(self.root, 'push', 'origin', 'develop')
        replacement = self.prepare()
        self.assertNotEqual(candidate, replacement)
        self.assertEqual(len(self.api.prs), 2)
        self.assertEqual(sum(p['state'] == 'open' for p in self.api.prs), 1)
        self.assertTrue((self.root / 'fixed.txt').is_file())

    def test_changed_lock_or_invalid_plan_cannot_dispatch(self):
        self.prepare()
        lock = self.root / 'Cargo.lock'
        lock.write_text(lock.read_text().replace('f' * 64, 'e' * 64))
        git(self.root, 'add', '.')
        git(self.root, 'commit', '-m', 'Alter third-party lock record')
        with self.assertRaisesRegex(ValueError, 'Version/lock edits differ'):
            a.read_plan(self.root)

    def test_wrong_source_or_main_advancing_is_rejected(self):
        (self.root / 'unexpected.txt').write_text('Wrong local source')
        git(self.root, 'add', '.')
        git(self.root, 'commit', '-m', 'Not fetched develop')
        with self.assertRaisesRegex(ValueError, 'fetched develop'):
            self.prepare()

    def test_delayed_release_dispatch_tags_reviewed_commit_after_main_advances(self):
        commit = self.merge_release()
        (self.root / 'ordinary.txt').write_text('later main change')
        git(self.root, 'add', '.')
        git(self.root, 'commit', '-m', 'Later main change')
        git(self.root, 'push', 'origin', 'main')
        git(self.root, 'switch', '--detach', commit)
        self.api.calls.clear()
        a.dispatch(self.root, REPOSITORY, commit, self.before)
        self.assertEqual(self.api.tags, {'v0.2.0': commit})
        self.assertEqual(self.api.runs[-1]['head_sha'], commit)

    def test_main_tag_and_explicit_dispatch_are_idempotent(self):
        commit = self.merge_release()
        self.api.calls.clear()
        a.dispatch(self.root, REPOSITORY, commit, self.before)
        writes = self.writes()
        self.assertEqual(self.api.tags, {'v0.2.0': commit})
        self.assertIn((f'repos/{REPOSITORY}/actions/workflows/package.yml/dispatches', 'POST',
                       {'ref': 'v0.2.0', 'inputs': {'release': 'true'}}), writes)
        self.api.calls.clear()
        a.dispatch(self.root, REPOSITORY, commit, self.before)
        self.assertEqual(self.writes(), [])

    def test_tag_creation_response_failure_recovers_without_retagging(self):
        commit = self.merge_release()
        self.api.fail_after = 'git/refs'
        with self.assertRaises(RuntimeError):
            a.dispatch(self.root, REPOSITORY, commit, self.before)
        self.api.calls.clear()
        a.dispatch(self.root, REPOSITORY, commit, self.before)
        self.assertFalse(any('git/refs' in c[0] or 'git/tags' in c[0] for c in self.writes()))
        self.assertEqual(self.api.tags['v0.2.0'], commit)

    def test_published_release_and_incomplete_draft_are_never_repackaged(self):
        commit = self.merge_release()
        self.api.tags['v0.2.0'] = commit
        for draft in (False, True):
            self.api.releases['v0.2.0'] = {'draft': draft, 'tag_name': 'v0.2.0'}
            self.api.calls.clear()
            a.dispatch(self.root, REPOSITORY, commit, self.before)
            self.assertEqual(self.writes(), [])

    def test_conflicting_tag_is_rejected_even_for_published_release(self):
        commit = self.merge_release()
        self.api.tags['v0.2.0'] = self.before
        self.api.releases['v0.2.0'] = {'draft': False, 'tag_name': 'v0.2.0'}
        self.api.calls.clear()
        with self.assertRaisesRegex(ValueError, 'different commit'):
            a.dispatch(self.root, REPOSITORY, commit, self.before)
        self.assertEqual(self.writes(), [])

    def test_regular_later_commit_and_bootstrap_never_release(self):
        a.dispatch(self.root, REPOSITORY, git(self.root, 'rev-parse', 'HEAD'), self.before)
        self.assertEqual(self.writes(), [])
        commit = self.merge_release()
        (self.root / 'later.txt').write_text('ordinary main work')
        git(self.root, 'add', '.')
        git(self.root, 'commit', '-m', 'Ordinary later commit')
        later = git(self.root, 'rev-parse', 'HEAD')
        self.api.calls.clear()
        a.dispatch(self.root, REPOSITORY, later, commit)
        self.assertEqual(self.writes(), [])
        with self.assertRaisesRegex(ValueError, 'first-parent'):
            a.dispatch(self.root, REPOSITORY, later, self.before)
        self.assertEqual(self.writes(), [])

    def test_missing_or_stale_plan_cannot_release(self):
        commit = self.merge_release()
        (self.root / a.PLAN).unlink()
        with self.assertRaisesRegex(ValueError, 'valid frozen release plan'):
            a.dispatch(self.root, REPOSITORY, commit, self.before)

    def test_pagination_and_annotated_tag_resolution(self):
        calls = []
        def api(path):
            calls.append(path)
            return list(range(100)) if '&page=1' in path else [100]
        self.assertEqual(list(pages('items?state=open', api=api)), list(range(101)))
        self.assertEqual(len(calls), 2)
        responses = [
            {'object': {'type': 'tag', 'sha': 'a' * 40}},
            {'object': {'type': 'commit', 'sha': self.before}},
        ]
        self.assertEqual(tag_commit(REPOSITORY, 'v0.2.0', api=lambda _: responses.pop(0)), self.before)


if __name__ == '__main__':
    unittest.main()
