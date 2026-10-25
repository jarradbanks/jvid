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
`jvid-update-helper` and `jvid-update-metadata`. The [Package workflow](../.github/workflows/package.yml)
builds the helper for Windows, passes the same public key/version to the editor and helper,
asserts all five native assets, and generates metadata in its protected `release-draft` job.
The generator reads its seed only from the environment, rejects a mismatched public key,
hashes final versioned files, signs the exact UTF-8 manifest bytes and verifies its output.
The detached signature is 64 raw bytes, not hex/base64.

### Release-owner activation checklist

This is owner work after the installer and updater PRs merge. Repository variables,
secrets and environments were all unconfigured when reviewed on 2026-10-25. Merging the
code alone does not activate production updates; no production key is supplied by the repo.

1. **Create and protect the GitHub environment named `release`.** In repository Settings →
   Environments, require a release reviewer and restrict deployment to reviewed release
   tags. Ensure the repository plan supports the chosen protections. Protect the release
   workflow and tag creation too: build jobs run before environment approval, and approval
   must only authorize code and native packages that were reviewed.
2. **Provision a production Ed25519 key outside the repository and keep a recovery copy.**
   Use your secure key-management process; neither the agent nor this checklist generates
   a production private key. The seed is exactly 32 raw bytes encoded as 64 hex characters;
   the public key is the corresponding raw 32-byte key encoded as 64 hex characters.
   PEM/OpenSSH strings are not accepted. Never use the public disposable fixture seed
   from the tests. Configure the following names exactly:

   | Name | GitHub location | Value / purpose |
   | --- | --- | --- |
   | `JVID_UPDATE_PUBLIC_KEY` | Repository **variable** | 64-hex public key compiled into every editor and helper; do not shadow it with an environment variable. |
   | `JVID_UPDATE_SIGNING_KEY` | `release` environment **secret** | Matching 64-hex private seed; available only to the metadata step after approval. |
   | `JVID_RELEASE_ENABLED` | Repository **variable** | Literal `true` enables tagged draft creation; leave unset until the remaining prerequisites are ready. |
   | `JVID_CORRESPONDING_SOURCE_URL` | `release` environment **variable** | HTTPS URL of the reviewed complete corresponding-source archive for this exact release. |
   | `JVID_CORRESPONDING_SOURCE_SHA256` | `release` environment **variable** | SHA-256 of those archive bytes; update together with the URL for every release. |

   Example owner commands, run only after the environment is protected:

   ```sh
   gh variable set JVID_UPDATE_PUBLIC_KEY --repo jarradbanks/jvid --body "$JVID_UPDATE_PUBLIC_KEY"
   gh secret set JVID_UPDATE_SIGNING_KEY --repo jarradbanks/jvid --env release < /secure/path/jvid-update-seed.hex
   gh variable set JVID_CORRESPONDING_SOURCE_URL --repo jarradbanks/jvid --env release --body "$SOURCE_URL"
   gh variable set JVID_CORRESPONDING_SOURCE_SHA256 --repo jarradbanks/jvid --env release --body "$SOURCE_SHA256"
   ```

   The source archive must include the exact jvid/native dependencies, FFmpeg and its
   bundled libraries, patches and build/install scripts; a GitHub source snapshot alone
   does not satisfy the packaging contract. `GITHUB_TOKEN` is supplied by Actions; no
   personal access token is required by this workflow. Missing or mismatched updater keys
   prevent release metadata generation. Ordinary builds/tests need no production seed;
   an editor built without a public key runs with update checking/installation disabled.
3. **Complete the platform signing and clean-machine gates.** Current CI produces
   candidates: Windows Authenticode signing and macOS Developer ID provisioning/notarization
   are not implemented by this updater change. macOS defaults to ad hoc signing;
   `JVID_MACOS_SIGN_IDENTITY` is a build-process environment variable used by assembly,
   not a wired GitHub secret or complete notarization solution. If owner signing is
   required, add its reviewed steps to the native build jobs before artifact upload,
   metadata generation and checksums. Signing/stapling/repacking assets after metadata
   generation invalidates their hashes. Run the remaining manual validation below before
   approving publication; a green package smoke test is not that validation.
4. **Prepare a bootstrap release version in a reviewed PR.** Update `workspace.package.version`
   in `Cargo.toml`, regenerate `Cargo.lock` and notices as needed, and write the user notes
   in `packaging/release-notes.md`. The tag must be exactly `v` plus that stable SemVer;
   prereleases and build metadata are excluded by the updater. Run the **Package** workflow
   manually on the merged commit and inspect all three platforms' evidence:

   ```sh
   gh workflow run package.yml --repo jarradbanks/jvid --ref main
   gh run list --repo jarradbanks/jvid --workflow package.yml --limit 5
   gh run watch "$RUN_ID" --repo jarradbanks/jvid --exit-status
   gh run download "$RUN_ID" --repo jarradbanks/jvid --dir candidate-packages
   ```

   The workflow sets `JVID_UPDATE_VERSION` from the workspace version and passes the
   repository public key to both builds. For a local Windows build, explicitly export
   both variables, build the editor and `jvid-update-helper` for `x86_64-pc-windows-msvc`,
   and pass `--helper` to `scripts/packaging/build.py`. Confirm the installer contains
   `jvid-app.exe`, `jvid-update-helper.exe`, and `jvid-distribution` containing
   `windows-installer`, without `jvid.portable`.
5. **Enable tagged drafts and create the reviewed tag.** Run these owner commands from
   the reviewed release commit; `0.1.0` is an example, and must equal the workspace version:

   ```sh
   gh variable set JVID_RELEASE_ENABLED --repo jarradbanks/jvid --body true
   release_version=0.1.0
   git fetch origin
   git switch main
   git pull --ff-only origin main
   git tag -a "v$release_version" -m "jvid $release_version"
   git push origin "v$release_version"
   gh run list --repo jarradbanks/jvid --workflow package.yml --limit 5
   gh run watch "$TAG_RUN_ID" --repo jarradbanks/jvid --exit-status
   ```

   Approve the waiting `release` job only after reviewing that tag's build evidence.
   `workflow_dispatch` never creates a draft, even if dispatched against a tag. The tag
   push builds all five packages, verifies corresponding source, asserts completeness,
   signs metadata, writes checksums, and creates a **draft**. It does not publish it.
6. **Inspect and publish the complete stable release.** The draft must contain exactly
   the five native packages in the table below, `jvid-update.json`, its `.sig`,
   `SHA256SUMS.txt` and `jvid-VERSION-corresponding-source.tar.gz`. Metadata must reference
   those exact final package bytes. Review the assets and all manual gates, then publish:

   ```sh
   gh release view "v$release_version" --repo jarradbanks/jvid --json isDraft,isPrerelease,assets
   gh release download "v$release_version" --repo jarradbanks/jvid --dir release-review
   (cd release-review && sha256sum --check SHA256SUMS.txt)
   gh release edit "v$release_version" --repo jarradbanks/jvid --draft=false --prerelease=false --latest
   ```

   Drafts are invisible to the app's public release check. Do not overwrite assets after
   publication. If an asset needs signing or another byte change, regenerate its signed
   metadata and checksums in a reviewed protected job before publishing.
7. **Bootstrap installed users manually, then validate a higher release.** Manually install
   the first Windows installer compiled with the production public key, matching helper,
   version and marker. Existing unsigned/source builds cannot acquire a trust root from
   an update response. Publish a second, strictly higher stable version using the same
   key and steps 4–6; open Help → Check for Updates in the bootstrap install, save/defer as
   required, and verify that Update and Restart launches the higher installed version and
   preserves projects/settings/components/models. An equal version reports up to date.
   Keep #477 open until this real installed-editor path and the other acceptance gates
   are recorded; the Inno fixture alone does not complete that acceptance.

| Native release asset (`VERSION` is the canonical workspace version) | Rust target | Distribution |
| --- | --- | --- |
| `jvid-VERSION-windows-x86_64-setup.exe` | `x86_64-pc-windows-msvc` | `windows-installer` |
| `jvid-VERSION-windows-x86_64-portable.zip` | `x86_64-pc-windows-msvc` | `windows-portable` |
| `jvid-VERSION-macos-aarch64.dmg` | `aarch64-apple-darwin` | `macos-dmg` |
| `jvid-VERSION-macos-aarch64-portable.zip` | `aarch64-apple-darwin` | `macos-portable` |
| `jvid-VERSION-linux-x86_64-portable.tar.gz` | `x86_64-unknown-linux-gnu` | `linux-portable` |

For a protected pipeline that receives already finalized assets, the exact metadata hook is:

```sh
python scripts/packaging/assert_assets.py release-assets
cargo run --release --locked -p jvid-updater --bin jvid-update-metadata -- \
  "$release_version" packaging/release-notes.md release-assets
python scripts/packaging/checksums.py release-assets
```

The private seed must be injected by the protected job, never put in arguments or logs.
The standalone generator permits a subset of known packages for testing; the release
workflow's completeness assertion is mandatory for the production five-package contract.

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

The original implementation passed GPL workspace tests (**995 passed, 0 failed,
9 ignored**), strict workspace/all-targets GPL clippy, formatting, translation guard,
and cargo-deny. Follow-up integration checks include the updater suite on Linux and native
Windows, strict updater/all-targets clippy on both, and the real native Inno test below.
Existing ignored tests and unavailable media-corpus fixtures are not treated as
fresh-machine validation.

Native installed-editor validation is still required before closing #477. Automated Linux
tests exercise signed metadata, exact target/format matching, version ordering, HTTP
check/download staging, cancellation/retry, integrity/authenticity rejection, install guards,
and simulated installer/restart errors. Generator tests exercise disposable local fixtures
and prove that absent signing/public configuration emits no metadata. These tests never
publish a release or use a production private key.

Native Windows tests also exercise a real child-process wait, executing a signed fixture
`.exe`, and restarting a fixture app after installer exit codes 0 and 5. A changed staged
executable is refused. The fixture writes version/restart markers in an isolated temporary
installation, including a path with spaces; it is not Inno and does not replace the editor.

The dedicated [Updater workflow](../.github/workflows/updater.yml) additionally runs a real
Inno old→new fixture upgrade. This test passed locally on native Windows with Inno 6.3.3.
It compiles the production `packaging/windows.iss` file-copy and portable-guard logic into
two real setup executables, verifies a disposable signed stage, waits for the installed
fixture process to exit, upgrades it, and verifies that the newer executable restarts.
Project/settings/model/portable-data sentinels survive, and both Inno and the helper reject
a portable tree. Silent rejection is bounded to 30 seconds and must return a rejection
exit code with a portable reason in the log; initialization/mutex failures cannot pass.
The installer skips its interactive directory-error dialog in silent mode. To keep it
safe on a personal machine, the fixture isolates its installer
identity, disables uninstall registration and shortcuts, and installs only under temp.
It tests a tiny app fixture, not the editor or real Known Folder data. The production
installer lifecycle/shortcuts/uninstall test remains restricted to a disposable account.
Run the opt-in updater fixture on native Windows with Inno Setup 6:

```powershell
$env:JVID_INNO_ISCC = 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe'
cargo test --locked -p jvid-updater --lib native_inno_upgrade -- --ignored --nocapture
```

Not yet exercised: a real installed editor detecting and upgrading from published signed
GitHub assets into the newer editor, two running instances,
UAC/ACL/disk-full during upgrade, process/power interruption during Inno's transaction,
reboot-required upgrade, preservation of real installed projects/settings/models/components,
and macOS/Linux/Windows portable replacement on clean machines. Their manual update routes
do not implement self-update. These are acceptance limitations, not completed criteria.

New update copy is translated in English and Spanish. Other locales currently show the
English update copy.
