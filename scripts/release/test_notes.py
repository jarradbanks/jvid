"""Replay the two published releases without changing anything on GitHub."""
import hashlib
import json
from pathlib import Path
import re
import tempfile
import unittest
from unittest.mock import patch

import automation as a
from notes import draft_highlights, render
from publish import SUFFIXES

ROOT = Path(__file__).resolve().parents[2]


class NotesTests(unittest.TestCase):
    def test_historical_examples_keep_real_downloads_and_essential_instructions(self):
        for fixture in json.loads((ROOT / 'scripts/release/fixtures/previous-releases.json').read_text()):
            with self.subTest(tag=fixture['tag']):
                draft = render(ROOT, fixture['tag'][1:], fixture['highlights'],
                               action=fixture['action'], sources_url=fixture['url'])
                self.assertEqual(draft, (ROOT / 'packaging/release-notes-examples' / (fixture['tag'] + '.md')).read_text())
                links = re.findall(r'\]\((https://[^)]+)\)', draft)
                downloads = {url for url in links if '/releases/download/' in url}
                self.assertEqual(downloads, set(fixture['downloads']))
                self.assertIn(fixture['url'], links)  # The old source bundles are still there.
                self.assertNotIn('@VERSION@', draft)
                self.assertLess(len(draft.split()), 130)
                self.assertNotRegex(draft.lower(), r'snapshot|fixture|protected release|checksum|during setup')
                self.assertIn('Ubuntu 22.04+', draft)
                self.assertIn('Vulkan driver required', draft)
                if fixture['tag'] == 'v0.2.1':
                    self.assertIn('0.2.0 or earlier', draft)
                    self.assertIn('existing installation folder', draft)
                    self.assertIn('settings, projects and models are kept', draft)
                else:
                    self.assertIn('SRT', draft)
                    self.assertIn('transcripts', draft)
                    self.assertIn('tabs', draft)

    def test_future_release_links_match_version_and_separate_sources(self):
        draft = render(ROOT, '1.0.0', ['First stable release.'])
        self.assertIn('/releases/tag/sources-v1.0.0', draft)
        self.assertEqual(set(re.findall(r'\]\((https://[^)]+/releases/download/[^)]+)\)', draft)),
                         {'https://github.com/jarradbanks/jvid/releases/download/v1.0.0/jvid-1.0.0-' + suffix
                          for _, _, suffix in SUFFIXES})
        self.assertNotRegex(draft, r'@[A-Z]+@')

    def test_preparation_freezes_rendered_notes_with_exact_repository_and_version_links(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'packaging').mkdir()
            (root / 'crates/app').mkdir(parents=True)
            (root / 'packaging/release-notes-template.md').write_bytes(
                (ROOT / 'packaging/release-notes-template.md').read_bytes())
            (root / 'Cargo.toml').write_text('[workspace]\nmembers = ["crates/*"]\n'
                                            '[workspace.package]\nversion = "0.2.1"\n')
            (root / 'crates/app/Cargo.toml').write_text('[package]\nname = "app"\nversion.workspace = true\n')
            (root / 'Cargo.lock').write_text('version = 4\n\n[[package]]\nname = "app"\nversion = "0.2.1"\n')
            source, main = 'a' * 40, 'b' * 40
            def run(*args, **kwargs):
                if args[:2] == ('git', 'rev-parse'):
                    return main if args[2] == 'origin/main' else source
                if args[:2] == ('git', 'diff'):
                    return 'feature.rs'
                if args[:2] == ('git', 'log'):
                    return 'feat(app): export subtitles (#485)\nci: update runners'
                return ''
            def api(path, method='GET', body=None):
                if method == 'POST':
                    return {'html_url': 'https://example.test/pr/1'}
                if '/actions/workflows/' in path:
                    return {'workflow_runs': []}
                return [] if '?' in path else None
            with patch.object(a, 'run', side_effect=run), patch.object(a, 'github', side_effect=api), patch.object(a, 'version_at', return_value='0.2.1'), patch.object(a, 'read_plan', return_value={}), patch.object(a, 'output'):
                a.prepare_candidate(root, 'major', 'owner/project')
            notes = (root / 'packaging/release-notes.md').read_bytes()
            plan = json.loads((root / a.PLAN).read_text())
            self.assertEqual(plan['notes_sha256'], hashlib.sha256(notes).hexdigest())
            self.assertEqual(notes.decode(), render(ROOT, '1.0.0', ['Export subtitles'], 'owner/project'))
            self.assertIn(b'https://github.com/owner/project/releases/tag/sources-v1.0.0', notes)

    def test_generated_draft_omits_internal_subjects_and_normalizes_changes(self):
        self.assertEqual(draft_highlights([
            'test(app): update transcript snapshots', 'Keep path fixtures stable',
            'ci: update runners', 'feat(app): enable draggable dock tabs (#482)',
            'Add standalone subtitle SRT export (#485)',
        ]), ['Enable draggable dock tabs', 'Add standalone subtitle SRT export'])
