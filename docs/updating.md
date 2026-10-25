# In-app updates

Help → Check for Updates opens the update window. Its automatic-check preference is saved
as `automatic_update_checks` in `settings.toml` (default: true). Automatic checks happen
once at startup and never download, install, or restart without a click. Manual checks work
when automatic checks are off. Checks require network access to the public GitHub Releases
API and downloads; offline/rate-limit/TLS errors are shown with retry available.

The updater sorts published stable tags by semantic version, excluding drafts, prerelease
flags, prerelease version strings, equal versions and downgrades. It selects an exact
target/distribution pair from authenticated metadata. Newer releases without a compatible
asset cannot cause a wrong-platform download or an incorrect “up to date” result.

| Beside-executable `jvid-distribution` marker | Update behavior |
| --- | --- |
| `windows-installer` (`x86_64-pc-windows-msvc`) | Download, verify, save/defer, quit, run the per-user Inno installer against the existing directory, restart. Requires bundled `jvid-update-helper.exe`. |
| `windows-portable` | Show signed notes and release downloads; quit and extract manually, retaining `jvid-data`. |
| `macos-dmg` / `macos-portable` (`aarch64-apple-darwin`) | Show signed notes and release downloads; quit and replace the app manually, retaining the sibling `jvid-data` for portable copies. |
| `linux-portable` (`x86_64-unknown-linux-gnu`) | Show signed notes and release downloads; quit and replace program files manually, retaining `jvid-data`. No AppImage is initially shipped. |
| Missing / unrecognized marker, source build, external package | No self-install. Use the package manager or rebuild from source. An unmarked build can show the native platform's signed release notes. |

Opening a release page is a manual route, never a completed self-update. Other OS/architecture
pairs have no compatible artifact until packaging and this updater explicitly support them.
The target includes the packaging contract's runtime requirements; this updater does not
make an unsupported OS or CPU supported. Inno enforces its Windows installation constraints.

The Windows install button is disabled while recording, exporting (including cancelled jobs
still unwinding), downloading/loading/transcribing a model, or in a tutorial. Unsaved edits
offer **Save, Update and Restart**; cancelling the save dialog or a failed save leaves the
editor open and the verified download ready. **Later** retains the staged download for this
session. Closing the editor without installing removes that download. The install guard is
checked again after saving. The helper is launched only after eframe shuts down, waits for
the editor process to exit, and never force-closes a second instance or reboots Windows.

The updater replaces no files itself. It asks the #476 per-user installer to upgrade only
program files. Projects, settings, recordings, selected optional components and Whisper
models stay at their existing paths. The updater never resets settings or downloads models.
Portable copies are never recursively replaced by the updater. The installer must retain
these properties; see the installation layout in `docs/packaging.md` from #476.

## Authentication and pipeline integration

`jvid-updater` has no UI/media/FFmpeg dependency. It contains the app's check/download logic,
`jvid-update-helper` and `jvid-update-metadata`. Add this hook to #476's release pipeline;
this issue intentionally does not create or change the release build/publish workflow.

1. Provision an Ed25519 key outside the repository. Store its 32-byte seed, encoded as 64
   hex characters, in the protected release secret `JVID_UPDATE_SIGNING_KEY`. Store the
   corresponding raw 32-byte public key as 64 hex characters in repository variable
   `JVID_UPDATE_PUBLIC_KEY`. Never use the test fixture key for releases.
2. Set `JVID_UPDATE_PUBLIC_KEY` and `JVID_UPDATE_VERSION=VERSION` for **both** desktop and
   helper builds. VERSION is the canonical stable SemVer without the tag's `v` prefix.
   A build without a valid pinned public key fails closed; it never accepts an unsigned
   replacement. `JVID_UPDATE_VERSION` defaults to the Cargo package version for development.
3. Build the standalone helper for the same target and bundle it beside `jvid-app.exe` in
   the installed Windows package:

   ```sh
   cargo build --release --locked -p jvid-updater --bin jvid-update-helper --target x86_64-pc-windows-msvc
   ```

4. After all packaging/signing/notarization is finished, collect the final artifacts in
   `dist/` and the human release notes in `release-notes.txt`. In a protected signing job:

   ```sh
   cargo run --release --locked -p jvid-updater --bin jvid-update-metadata -- VERSION release-notes.txt dist
   ```

   The generator reads both key variables from the environment, rejects a mismatched key,
   hashes only the exact recognized versioned filenames, validates the manifest, signs
   its exact UTF-8 bytes and verifies its own output. It requires at least one artifact;
   the pipeline must additionally require every artifact promised for that release.
5. Upload `dist/jvid-update.json` and `dist/jvid-update.json.sig` alongside the **exact**
   hashed artifacts to the same stable GitHub Release. The signature is 64 raw bytes,
   not hex/base64. Generate metadata last; changing a signed/notarized executable later
   invalidates the hash. Publish no private key or seed in metadata, arguments or logs.

Schema 1 has `schema`, `repository` (`jarradbanks/jvid`), canonical `version`, plain-text
`notes`, and `assets`. Each asset has `target`, `distribution`, exact versioned `name`,
`size` in bytes and a hex `sha256`. The recognized filenames and target mapping are in
`jvid_updater::metadata::PACKAGES`. Unknown fields/schema, duplicate target/format pairs,
zero/oversized files, invalid hashes/names, mismatched tags and invalid signatures fail.
The pinned key authenticates the whole manifest (including notes, version and selection);
SHA-256 and size bind the installer to it. A checksum supplied with an unsigned download
is not treated as authentication. TLS is still required for every production request and
redirect. Verification happens after download and again in the helper immediately before
execution. A key rotation needs a separately reviewed trust migration: replacing a key in
release metadata cannot change the app's trust root.

## Failure and recovery

Downloads use a unique private temporary directory and bounded reads. Cancellation,
truncation, corruption, network errors and disk/write failures never touch the installation.
Failed/retried downloads do not reuse partial bytes. Network reads time out after 30 seconds;
a blocked read can take that long to observe Cancel. Permissions/helper/layout failures
before shutdown keep the editor open. A helper launch failure leaves the installation
unchanged and is recorded for the next launch.

Inno owns the Windows upgrade transaction and rollback. Cancellation, nonzero exit, or an
execution error attempts to restart the existing install. The helper never deletes it.
`/NORESTART` and `/RESTARTEXITCODE=3010` report a required Windows reboot separately. A
successful installer exit followed by a failed spawn tells the user to launch jvid manually;
it does not claim the new app ran. Automatic restart itself does not prove an upgrade: check
the installed version shown on the next launch.

`update-result.txt` beside settings and `update-result.installer.log` record the outcome and
Inno log. The update window shows the previous result. If installation was interrupted or
the helper cannot restart, launch the existing installed executable; if necessary rerun the
verified installer retained in its `jvid-update-*` system temp directory. Before execution
the helper can revalidate the signed stage by being invoked with that directory. Never
substitute an unverified downloaded executable as recovery. The OS may later clean temp
files; a fresh in-app download is then required. Reinstalling program files must preserve
the user data directories. No automatic retry runs on startup.

## Validation status

Checks recorded for this implementation: GPL workspace tests **995 passed, 0 failed,
9 ignored**; strict workspace/all-targets GPL clippy, formatting, translation guard,
and cargo-deny passed. Native Windows updater tests **10 passed**, including the metadata
generator integration test; Windows updater/all-targets clippy passed. Existing ignored
tests and unavailable media-corpus fixtures are not treated as fresh-machine validation.

The PR remains a draft until native installed-build validation is recorded. Automated Linux
tests exercise signed metadata, exact target/format matching, version ordering, HTTP
check/download staging, cancellation/retry, integrity/authenticity rejection, install guards,
and simulated installer/restart errors. The generator is exercised with disposable local
fixtures. These tests never publish a release or run an actual setup executable.

Native Windows tests also exercise a real child-process wait, executing a signed fixture
`.exe`, and restarting a fixture app after installer exit codes 0 and 5. A changed staged
executable is refused. The fixture writes version/restart markers in an isolated temporary
installation, including a path with spaces; it is not Inno and does not replace the editor.

Not yet exercised: a native Windows old→new Inno upgrade and relaunch, two running instances,
UAC/ACL/disk-full during upgrade, process/power interruption during Inno's transaction,
reboot-required upgrade, preservation of real installed projects/settings/models/components,
and macOS/Linux/Windows portable replacement on clean machines. Their manual update routes
do not implement self-update. These are acceptance limitations, not completed criteria.

New update copy is translated in English and Spanish. Other locales currently show the
English update copy.
