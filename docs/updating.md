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
asserts all five native assets, and generates metadata in its protected **approve, sign and publish** job.
The generator reads its seed only from the environment, rejects a mismatched public key,
hashes final versioned files, signs the exact UTF-8 manifest bytes and verifies its output.
The detached signature is 64 raw bytes, not hex/base64.

### Release prerequisites

The automation bootstrap changes workflows, tests and documentation only. It keeps
`workspace.package.version` at `0.1.0` and adds no `.github/release-plan.json`;
merging it cannot release. The normal flow is **develop → temporary
release/vVERSION → main → immutable vVERSION tag → protected approval → publish**.

Retain the existing `release` environment, its required reviewer `jarradbanks`,
and its `v*` tag-only deployment policy. The approval authorizes signing **and
public publication**, so record source review, candidate testing and any required
platform signing/notarization before approving. Build and source checks complete
before the environment asks for approval. The automation does not approve itself.

Keep the existing configuration names and locations:

| Name | GitHub location | Purpose |
| --- | --- | --- |
| `JVID_UPDATE_PUBLIC_KEY` | Repository variable | Raw 32-byte Ed25519 public key as 64 hex characters, compiled into the editor/helper and recorded in source provenance. |
| `JVID_UPDATE_SIGNING_KEY` | `release` environment secret | Matching 32-byte private seed as 64 hex characters; injected only into the metadata signing step after approval. |
| `JVID_RELEASE_ENABLED` | Repository variable | Literal `true` enables release tag dispatch and protected publication. |

An owner provisions and recovers production keys outside this repository. No
production seed, PAT or test fixture seed belongs in code, command arguments or
logs. Missing/mismatched keys stop signing. The publisher independently verifies
the detached signature using the public key; it never needs the private seed.

Repository Actions must allow `GITHUB_TOKEN` to **create pull requests** for
Prepare release. This setting does not authorize bot approval, bypass reviews or
remove required checks. The repository's default workflow token remains read-only;
Prepare alone requests contents/PR/actions write, main dispatch requests
contents/actions write, and the protected publisher requests contents write.
Keep `CI required` enforced on both main and develop. It now includes the cheap
Python release automation tests as well as the existing conditional checks.

Windows Authenticode signing and macOS Developer ID provisioning/notarization
remain separate platform acceptance work. macOS assembly currently uses ad hoc
signing by default; `JVID_MACOS_SIGN_IDENTITY` alone is not a complete notarization
pipeline. If required for a release, finalize signing/stapling in the native build
jobs before source/native verification, metadata and checksums. Never change
asset bytes after metadata generation. A green smoke test does not complete the
manual acceptance checklist in [packaging.md](packaging.md).

### Prepare and test a candidate

1. Merge reviewed feature/fix PRs into **develop**. Sync current main into develop
   through a reviewed PR when needed after a prior release. Normal feature PRs do
   not bump the version. Develop pushes run Linux CI; manual CI runs all three OSes.
2. From main's Actions page, run **Prepare release**, choosing patch/minor/major
   (minor is the default). For example, from `0.1.0`, minor creates
   `release/v0.2.0`. The helper snapshots develop, includes current main on the
   temporary branch, changes only workspace versions in `Cargo.toml`/`Cargo.lock`,
   generates commit-subject notes plus `packaging/release-notes-template.md`, and
   opens its release PR against main. It never pushes a version bump to develop.
3. Prepare explicitly dispatches **CI** and **Package** on that frozen branch.
   CI's manual path tests all three OSes; Package builds the five native assets,
   runs package smoke checks and verifies all four corresponding-source bundles.
   Bot-created pushes/PRs alone do not trigger recursive Actions runs, so both
   dispatches are required. These candidate runs have publication disabled.
4. Review the PR's notes, all CI/Package results, source provider records and clean
   supported-OS evidence. Download `release-candidate` from the exact Package run:

   ```sh
   gh run download "$CANDIDATE_RUN_ID" --repo jarradbanks/jvid \
     --name release-candidate --dir candidate-packages
   (cd candidate-packages && sha256sum --check SHA256SUMS.txt)
   ```

   Test the native installer/DMG/portable downloads for each platform. The app
   downloads are separate from source manifests and `.tar.gz.partNNN` files;
   source bundles are for license/source review and rebuilding, not installation.
   Reconstruction verifies every part and the combined archive:

   ```sh
   python reassemble_sources.py jvid-VERSION-sources-rust.json workspace-sources.tar.gz
   ```

The schema-1 plan records version, previous version, bump, develop source commit,
previous main commit, frozen merged base and the notes SHA-256. Validation checks
exact workspace/lock edits against that base and rejects unplanned code changes,
changed notes and third-party lock changes. Prepare retries reuse the branch and
PR, without adding later develop commits, and only redispatch missing/failed runs.
If notes need deliberate editing, update their `notes_sha256` in the plan in the
same reviewed PR change and rerun CI and Package for that new head. Prepare itself
never overwrites a frozen candidate. Do not rebase or add feature changes to it.

### Merge, approve and publish

Merge the reviewed release PR into main using a **merge commit**, preserving its
frozen base history. Keep main at the plan's recorded base until that merge; if
main advances, replace/review the candidate instead of bypassing the stale-plan
check. Develop can continue independently while the candidate is being tested.

Main's release workflow requires an exact first-parent version increase with a
valid plan. It creates `vVERSION` at that precise merged commit and explicitly
dispatches Package against the tag with `release=true`; the run is named
**Release vVERSION**. A tag created with `GITHUB_TOKEN` does not itself trigger a
tag-push workflow. Bootstrap merges, ordinary later commits, missing plans,
conflicting tags and already published versions cannot create a fresh release.

The tag build repeats all native smoke/source checks for the actual merged
commit; the merge has a different source identity from the premerge candidate.
The existing protected `release` environment then waits for its required reviewer.
After approval, the job revalidates the exact source/native contract, signs and
verifies updater metadata, creates/resumes a draft, and uploads final native
packages, separate source bundles, metadata/signature and `SHA256SUMS.txt`.
Before publishing, it verifies the exact remote names, completed upload states,
byte sizes and GitHub SHA-256 digests, and rechecks local sources/signatures.
It then publishes automatically as a stable latest release. No separate manual
“Publish release” step follows approval.

Manual Package dispatch defaults to `release=false`, including on tags. An
explicit `release=true` dispatch can publish only on a matching existing version
tag, with `JVID_RELEASE_ENABLED=true` and the same required environment approval.
Existing human tag-push behavior remains enabled under those same protections.
Branch/PR runs never publish. Duplicate queued tag runs skip existing drafts and
published releases before rebuilding. Published assets/releases are never edited,
replaced, deleted or retagged by this automation.

### Release retry and hotfix procedure

- If Prepare fails after pushing the branch or creating the PR, rerun it with the
  same bump on main. It recovers the original candidate/PR and dispatches missing
  CI/Package work. An active/successful dispatch is reused. GitHub's creation API
  can briefly lag its run list; workflow concurrency queues duplicate tag runs,
  and the publication gate skips the already-created release before rebuilding.
- If main tagging/dispatch fails, rerun the original **Release merged version**
  run. It uses the original immutable event commit even if main has advanced;
  ordinary newer commits are never tagged. Existing matching tags are reused;
  a tag at another commit is an error. Do not delete/move a version tag to retry.
  Once Package is queued, monitor/rerun that original run rather than dispatching
  a fresh build. An existing draft causes main dispatch to stop without rebuilding.
- If signing/upload/publication fails, rerun **failed jobs** of the original
  Package run, retaining its `release-candidate` inputs (retained for seven days).
  Matching uploaded assets are kept; incomplete GitHub `starter` uploads are
  removed and retried. Unexpected names or completed assets with conflicting
  sizes/digests stop recovery without overwriting them. Full workflow reruns skip
  an existing draft. If artifacts expired or uploaded bytes conflict, inspect the
  draft and original run as an owner; do not bypass verification or substitute
  newly rebuilt bytes into that frozen release. A published release is a no-op.
- To abandon an unmerged candidate, an owner closes its PR and deletes its
  `release/vVERSION` branch explicitly. With no tag/release for that version,
  Prepare may then create a replacement snapshot and a new PR; it never deletes
  or force-pushes an existing candidate itself. Fix notes/code on develop first,
  sync main if necessary, and repeat the full candidate review. Closed candidates
  with their branch still present are refused rather than silently rewritten.
- For a hotfix, branch the fix from current main and merge it into develop through
  review, then run Prepare with **patch**. It follows the same candidate tests,
  release PR and protected approval. If develop already contains unrelated work,
  this flow includes that work: isolate/review the desired develop snapshot first;
  there is no hidden main-only hotfix bypass. Always publish a strictly higher
  version for corrections to a published release.

Bootstrap installed users manually with the first installer carrying the production
public key, matching helper, version and distribution marker. Then publish a higher
stable release through this flow and validate Help → Check for Updates, save/defer,
Update and Restart, and data preservation on an actual installed editor. Equal
versions report up to date. Keep #477 open until this path and the remaining manual
platform gates are recorded; disposable fixtures do not complete that acceptance.

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
