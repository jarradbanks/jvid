"""The installer must download the same pinned model bytes that the app expects."""
import unittest

from installer_models import ROOT, definitions


class InstallerModelTests(unittest.TestCase):
    def test_inno_identities_match_app(self):
        source = (ROOT / 'crates/jvid-transcribe/src/models.rs').read_text()
        self.assertEqual((ROOT / 'packaging/whisper-models.iss').read_text(), definitions(source),
                         'Run python scripts/packaging/installer_models.py after updating app models')

    def test_missing_model_or_unpinned_revision_is_rejected(self):
        source = (ROOT / 'crates/jvid-transcribe/src/models.rs').read_text()
        for changed in (source.replace('Model::Small =>', 'Model::Other =>'),
                        source.replace('const REVISION:', 'const REMOVED:')):
            with self.assertRaises(ValueError):
                definitions(changed)

    def test_invalid_model_paths_duplicate_filenames_and_sizes_are_rejected(self):
        source = (ROOT / 'crates/jvid-transcribe/src/models.rs').read_text()
        for changed in (source.replace('ggml-base.bin', '..'),
                        source.replace('ggml-base.bin', 'ggml-small.bin'),
                        source.replace('147_951_465', '0'),
                        source.replace('147_951_465', str(2**63))):
            with self.subTest(source=changed), self.assertRaises(ValueError):
                definitions(changed)
