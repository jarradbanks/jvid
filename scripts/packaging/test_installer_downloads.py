"""Exercise the actual Inno model download/cache flow with tiny local fixtures.

The private test installer has its own AppId, no shortcuts/registration/uninstaller,
and redirects all data/cache paths into a sandbox. No real jvid installation or
model files are touched. Native Windows runs when Inno is available. For WSL,
set JVID_TEST_ISCC and JVID_TEST_WINDOWS_TEMP to Windows-accessible paths.
"""
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import threading
import time
import unittest
import urllib.parse

ROOT = Path(__file__).resolve().parents[2]


def compiler():
    explicit = os.environ.get('JVID_TEST_ISCC')
    if explicit and Path(explicit).is_file():
        return explicit
    if os.name == 'nt':
        standard = Path(r'C:\Program Files (x86)\Inno Setup 6\ISCC.exe')
        return shutil.which('ISCC.exe') or (str(standard) if standard.is_file() else None)
    return None


ISCC = compiler()


def native(path):
    if os.name == 'nt':
        return str(path)
    return subprocess.check_output(['wslpath', '-w', str(path)], text=True).strip()


def fixture_script(source, local, roaming, url):
    """Sandbox reviewed paths, and expose choices only in this private installer."""
    script = re.sub(r'(?m)^AppId=.*$', 'AppId=jvid-isolated-model-fixture', source)
    script = script.replace('[Setup]\n', '[Setup]\nUninstallable=no\nCreateUninstallRegKey=no\nUsePreviousAppDir=no\n')
    script = script.replace('CloseApplications=yes', 'CloseApplications=no')
    script = script.replace('SetupMutex=jvid-setup', 'SetupMutex=jvid-isolated-model-fixture')
    script = script.replace('SetupIconFile=..\\assets\\jvid.ico',
                            'SetupIconFile={#OutputDir}\\fixture-icon.ico')
    script = re.sub(r'(?ms)^\[Icons\]\n.*?(?=^\[Run\])', '', script)
    script = re.sub(r'(?ms)^\[Run\]\n.*?(?=^\[Code\])', '', script)
    script = script.replace('{localappdata}\\Programs\\jvid', local + '\\Programs\\jvid')
    script = script.replace('{localappdata}\\jvid\\', local + '\\jvid\\')
    script = script.replace('{userappdata}\\jvid\\', roaming + '\\jvid\\')
    for forbidden in ('{localappdata}', '{userappdata}', '{userprograms}', '{userdesktop}',
                      '[Registry]', '[UninstallRun]', '[Icons]', '[Run]'):
        if forbidden in script:
            raise ValueError('Unsandboxed installer fixture: ' + forbidden)
    # Emulate interactive choices headlessly, while retaining silent-install coverage.
    interactive = "(ExpandConstant('{param:TEST_INTERACTIVE|0}') = '1')"
    if script.count('if WizardSilent then Exit;') != 1:
        raise ValueError('Review the interactive model-download guard')
    script = script.replace('if WizardSilent then Exit;',
                            'if WizardSilent and (not ' + interactive + ') then Exit;')
    script = script.replace('(not WizardSilent)', '((not WizardSilent) or ' + interactive + ')')
    choices = '\n'.join("  if ExpandConstant('{param:TEST_MODEL_" + str(i) + "|0}') = '1' then\n"
        "    ModelsPage.Values[" + str(i) + "] := True;" for i in range(3))
    if script.count('  ModelDownloads := CreateDownloadPage(') != 1:
        raise ValueError('Review model choice injection')
    script = script.replace('  ModelDownloads := CreateDownloadPage(',
                            choices + '\n  ModelDownloads := CreateDownloadPage(')
    # Click the real Inno abort button after receiving bytes of the requested model.
    script = script.replace('  Total := ProgressMax;',
        "  if (Progress > 0) and (IntToStr(DownloadingModel) = "
        "ExpandConstant('{param:TEST_CANCEL|-1}')) then\n"
        '    SendMessage(ModelDownloads.AbortButton.Handle, $00F5, 0, 0);\n'
        '  Total := ProgressMax;')
    return script.replace('https://huggingface.co/ggerganov/whisper.cpp/resolve/', url)


class InstallerFixtureTests(unittest.TestCase):
    def test_fixture_isolation_fails_closed_for_new_profile_paths(self):
        source = (ROOT / 'packaging/windows.iss').read_text()
        script = fixture_script(source, r'C:\sandbox\local', r'C:\sandbox\roaming',
                                'http://127.0.0.1:1/')
        self.assertIn('#include "whisper-models.iss"', script)
        self.assertIn(r'C:\sandbox\local\jvid\data\models', script)
        self.assertIn(r'C:\sandbox\roaming\jvid\config', script)
        for path in ('{localappdata}\\other-app', '{userappdata}\\other-app', '{userdesktop}'):
            with self.subTest(path=path), self.assertRaisesRegex(ValueError, 'Unsandboxed'):
                fixture_script(source + '\n; ' + path, 'local', 'roaming', 'http://localhost/')


@unittest.skipUnless(ISCC, 'Inno compiler/Windows execution is unavailable')
class InstallerDownloadTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='jvid-model-tests-',
            dir=os.environ.get('JVID_TEST_WINDOWS_TEMP'))
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.files = ['ggml-base.bin', 'ggml-small.bin', 'ggml-large-v3-turbo-q5_0.bin']
        self.payloads = {name: ('test weights ' + name + '\n').encode() * 1024 for name in self.files}
        self.counts = dict.fromkeys(self.files, 0)
        self.corrupt = set()
        self.oversized = set()
        self.slow = set()
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                name = Path(urllib.parse.urlsplit(self.path).path).name
                if name not in owner.payloads:
                    self.send_error(404)
                    return
                owner.counts[name] += 1
                data = bytes(len(owner.payloads[name])) if name in owner.corrupt else owner.payloads[name]
                if name in owner.oversized:
                    data += b'overflow'
                self.send_response(200)
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                try:
                    if name in owner.slow:
                        for start in range(0, len(data), 4096):
                            self.wfile.write(data[start:start + 4096])
                            self.wfile.flush()
                            time.sleep(0.05)
                    else:
                        self.wfile.write(data)
                except ConnectionError:
                    pass  # Expected when the installer cancels/rejects a response.

            def log_message(self, *args):
                pass

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.models = self.root / 'local/jvid/data/models'
        self.cache = self.root / 'local/jvid/cache/model-downloads'
        self.stage = self.root / 'stage'
        self.stage.mkdir()
        (self.stage / 'jvid-app.exe').write_bytes(b'Disposable compilation fixture, never executed.')
        (self.stage / 'DISTRIBUTION-LICENSE.txt').write_text('Test fixture\n')
        self.build_installer()

    def build_installer(self):
        shutil.copy2(ROOT / 'assets/jvid.ico', self.root / 'fixture-icon.ico')
        definitions = ['#define WhisperRevision "' + '0' * 40 + '"']
        for index, name in enumerate(self.files):
            prefix = '#define Whisper' + str(index)
            definitions.extend([prefix + 'File "' + name + '"',
                prefix + 'Size ' + str(len(self.payloads[name])), prefix + 'MB 1',
                prefix + 'SHA256 "' + hashlib.sha256(self.payloads[name]).hexdigest() + '"'])
        (self.root / 'whisper-models.iss').write_text('\n'.join(definitions) + '\n')
        script = fixture_script((ROOT / 'packaging/windows.iss').read_text(),
            native(self.root / 'local'), native(self.root / 'roaming'),
            'http://127.0.0.1:' + str(self.server.server_port) + '/')
        script = script.replace('jvid-isolated-model-fixture', self.root.name)
        source = self.root / 'test.iss'
        source.write_text(script)
        compiled = subprocess.run([ISCC, '/Q', '/DAppVersion=0.0.0',
            '/DStageDir=' + native(self.stage), '/DOutputDir=' + native(self.root), native(source)],
            capture_output=True, text=True, timeout=60)
        self.assertEqual(compiled.returncode, 0, compiled.stdout + compiled.stderr)
        self.installer = self.root / 'jvid-0.0.0-windows-x86_64-setup.exe'

    def install(self, choices=(0, 1, 2), interactive=True, cancel=-1):
        # Both private test data and the dummy payload remain inside this sandbox.
        return subprocess.run([str(self.installer), '/VERYSILENT', '/SUPPRESSMSGBOXES',
            '/NORESTART', '/SP-', '/DIR=' + native(self.root / 'installed'),
            '/LOG=' + native(self.root / 'setup.log'),
            '/TEST_INTERACTIVE=' + str(int(interactive)), '/TEST_CANCEL=' + str(cancel),
            *('/TEST_MODEL_' + str(i) + '=1' for i in choices)],
            capture_output=True, text=True, timeout=45)

    def assert_success(self, result):
        self.assertEqual(result.returncode, 0, (self.root / 'setup.log').read_text(errors='replace'))

    def assert_no_install(self):
        self.assertFalse((self.root / 'installed/jvid-app.exe').exists())
        self.assertFalse(self.models.exists())

    def test_default_choices_and_silent_upgrades_never_download(self):
        self.assert_success(self.install(choices=()))
        self.assertEqual(list(self.counts.values()), [0, 0, 0])
        self.assertFalse(self.models.exists())
        self.assert_success(self.install(interactive=False))
        self.assertEqual(list(self.counts.values()), [0, 0, 0])
        self.assertFalse(self.models.exists())

    def test_only_selected_models_download_and_existing_models_and_settings_survive(self):
        self.models.mkdir(parents=True)
        (self.models / self.files[0]).write_bytes(self.payloads[self.files[0]])
        unrelated = self.models / 'custom-model.bin'
        unrelated.write_bytes(b'user model')
        config = self.root / 'roaming/jvid/config/settings.toml'
        config.parent.mkdir(parents=True)
        config.write_bytes(b'user settings')
        self.assert_success(self.install(choices=(0, 1)))
        self.assertEqual(list(self.counts.values()), [0, 1, 0])
        self.assertFalse((self.models / self.files[2]).exists())
        for name in self.files[:2]:
            self.assertEqual((self.models / name).read_bytes(), self.payloads[name])
        self.assertEqual(unrelated.read_bytes(), b'user model')
        self.assertEqual(config.read_bytes(), b'user settings')
        # An upgrade must preserve even an existing model with a different identity.
        (self.models / self.files[0]).write_bytes(b'previous user model')
        self.assert_success(self.install(interactive=False))
        self.assertEqual((self.models / self.files[0]).read_bytes(), b'previous user model')
        self.assertEqual(config.read_bytes(), b'user settings')
        self.assertEqual(list(self.counts.values()), [0, 1, 0])

    def assert_models(self):
        for name, contents in self.payloads.items():
            self.assertEqual((self.models / name).read_bytes(), contents)

    def cached(self, name):
        return self.cache / (hashlib.sha256(self.payloads[name]).hexdigest() + '-' + name)

    def remove_installed_models(self):
        for name in self.files:
            (self.models / name).unlink(missing_ok=True)

    def test_completed_downloads_survive_sessions_and_corrupt_cache_is_replaced(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, (self.root / 'setup.log').read_text(errors='replace'))
        self.assert_models()
        self.assertEqual(self.counts, dict.fromkeys(self.files, 1))
        for name, contents in self.payloads.items():
            self.assertEqual(self.cached(name).read_bytes(), contents)
        self.remove_installed_models()
        self.cached(self.files[1]).write_bytes(b'invalid cache')
        result = self.install()
        self.assertEqual(result.returncode, 0, (self.root / 'setup.log').read_text(errors='replace'))
        self.assert_models()
        self.assertEqual(list(self.counts.values()), [1, 2, 1])
        self.remove_installed_models()
        # With complete verified cache, even incorrect server files must not be fetched.
        self.corrupt.update(self.files)
        result = self.install()
        self.assertEqual(result.returncode, 0, (self.root / 'setup.log').read_text(errors='replace'))
        self.assert_models()
        self.assertEqual(list(self.counts.values()), [1, 2, 1])

    def test_failed_later_download_keeps_only_completed_verified_cache(self):
        self.corrupt.add(self.files[2])
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assert_no_install()
        self.assertIn('hash is incorrect', (self.root / 'setup.log').read_text(errors='replace'))
        for name in self.files[:2]:
            self.assertTrue(self.cached(name).is_file(),
                str(self.counts) + '\n' + (self.root / 'setup.log').read_text(errors='replace'))
            self.assertEqual(self.cached(name).read_bytes(), self.payloads[name])
        self.assertFalse(self.cached(self.files[2]).exists())
        self.corrupt.clear()
        result = self.install()
        self.assertEqual(result.returncode, 0, (self.root / 'setup.log').read_text(errors='replace'))
        self.assert_models()
        self.assertEqual(list(self.counts.values()), [1, 1, 2])

    def test_failed_checksum_preserves_preexisting_model(self):
        self.models.mkdir(parents=True)
        previous = self.models / self.files[0]
        previous.write_bytes(b'previous user model')
        self.corrupt.add(self.files[0])
        result = self.install(choices=(0,))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(previous.read_bytes(), b'previous user model')
        self.assertFalse((self.root / 'installed/jvid-app.exe').exists())
        self.assertFalse(self.cached(self.files[0]).exists())

    def test_cancel_keeps_completed_cache_and_never_installs_partial_models(self):
        self.slow.add(self.files[1])
        result = self.install(cancel=1)
        self.assertNotEqual(result.returncode, 0)
        self.assert_no_install()
        log = (self.root / 'setup.log').read_text(errors='replace')
        self.assertIn('Optional model download cancelled.', log)
        progress = re.findall(r'(\d+) of (\d+) bytes done', log)
        self.assertTrue(any(0 < int(done) < int(total) for done, total in progress), log)
        self.assertEqual(self.cached(self.files[0]).read_bytes(), self.payloads[self.files[0]])
        self.assertFalse(self.cached(self.files[1]).exists())
        self.assertFalse(self.cached(self.files[2]).exists())
        self.assertFalse(list(self.cache.glob('*.partial')))
        self.assert_success(self.install())
        self.assert_models()
        self.assertEqual(list(self.counts.values()), [1, 2, 1])

    def test_cancel_on_final_progress_update_still_prevents_install(self):
        result = self.install(choices=(0,), cancel=0)
        self.assertNotEqual(result.returncode, 0)
        self.assert_no_install()
        self.assertFalse(self.cached(self.files[0]).exists())
        self.assertIn('Optional model download cancelled.',
                      (self.root / 'setup.log').read_text(errors='replace'))

    def test_unexpected_size_is_rejected_before_install_or_cache(self):
        self.oversized.add(self.files[0])
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assert_no_install()
        self.assertFalse(self.cached(self.files[0]).exists())
        self.assertIn('unexpected size', (self.root / 'setup.log').read_text(errors='replace'))


if __name__ == '__main__':
    unittest.main()
