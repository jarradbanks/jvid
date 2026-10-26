# Installation and release packaging

Tracking: [#476](https://github.com/jarradbanks/jvid/issues/476) and the
[#62 packaging umbrella](https://github.com/jarradbanks/jvid/issues/62).
The Package workflow builds candidates on pull requests and manual runs without
publishing a release. These instructions describe those candidates; they do not
claim that every clean-machine acceptance criterion has passed.

## Supported candidates and installation

- **Windows 10 22H2 / Windows 11, x86_64 with AVX2:** run `setup.exe`, choose a
  per-user installation folder, and optionally create a desktop shortcut. Start
  Menu shortcuts and an Apps & Features uninstaller are included. No administrator
  rights, Rust, LLVM, Python, separate Whisper executable or FFmpeg installation
  are required. The default folder is `%LOCALAPPDATA%\Programs\jvid`.
- **Windows portable:** extract the entire ZIP to a writable folder and launch
  `jvid-app.exe`. Do not run it inside the ZIP viewer. No installation or elevation
  is needed. Keep DLLs and marker files beside the executable.
- **macOS 14+, Apple Silicon:** open the DMG, drag `jvid.app` to Applications,
  eject the image and open the app. The portable ZIP instead extracts a folder
  containing `jvid.app`; keep it in a writable location. Ad hoc CI signatures do
  not constitute Developer ID signing or Apple notarization. Public distribution
  remains a draft until those are completed and Gatekeeper validation is recorded.
- **Linux x86_64 with AVX2, Ubuntu 22.04+ or compatible glibc 2.35+ desktop:**
  extract the tarball and run `./AppRun`. Required media, audio and window-system
  client libraries are bundled. The host desktop supplies its kernel, glibc,
  graphics/Vulkan driver and desktop services (including file picker portals).
  There is no root install step. A portable tarball is the initial Linux format;
  AppImage is not currently produced.

The GUI and its integrated CPU `whisper-rs` engine are in every base package.
Dynamically linked FFmpeg and its playback/export dependencies are in every base
package, including the Windows ZIP. Whisper models are the only optional downloads.
The CPU baseline is inherited from the repository's `GGML_NATIVE=OFF` configuration;
these x86_64 packages require AVX2, FMA and F16C. Intel macOS, ARM Windows and ARM
Linux are not claimed by this initial matrix.

## Optional components

On first launch, **Optional Components** explains local transcription and subtitles,
shows all three models and their download sizes, and permits skipping with **Done**
or Escape. Open **Help > Optional Components** later to add models without
reinstalling. Generate Subtitles and Transcribe also retain their existing download
flow. Normal import, editing, playback and export require no model.

| Model | Exact download bytes | File |
| --- | ---: | --- |
| Whisper Base | 147,951,465 | `ggml-base.bin` |
| Whisper Small | 487,601,967 | `ggml-small.bin` |
| Whisper Large v3 Turbo Q5 | 574,041,195 | `ggml-large-v3-turbo-q5_0.bin` |

The component dialog calls `Model::download` from `jvid-transcribe`: the same pinned
Hugging Face revision, SHA-256 validation, bounded network waits, progress, temporary
files and atomic rename used by transcription. Cancel (or close the dialog) cancels
its job; an incomplete/corrupt download never becomes a model. Failures display an
error and allow another download; a subsequent launch can retry too. Closing setup
never blocks the base editor. The engine is linked into the app; no runtime
separation or separate Whisper executable was introduced.

## Settings and model locations

All paths come from the shared `jvid-paths` crate. Existing installed locations are
retained, including Windows local model storage rather than roaming storage.

| Mode | Settings | Whisper models |
| --- | --- | --- |
| Installed Windows | `%APPDATA%\jvid\config\settings.toml` | `%LOCALAPPDATA%\jvid\data\models` |
| Installed macOS | `~/Library/Application Support/jvid/settings.toml` | `~/Library/Application Support/jvid/models` |
| Installed Linux / unmarked development build | `$XDG_CONFIG_HOME/jvid/settings.toml` (default `~/.config/jvid`) | `$XDG_DATA_HOME/jvid/models` (default `~/.local/share/jvid/models`) |
| All portable packages | `<package root>/jvid-data/config/settings.toml` | `<package root>/jvid-data/data/models` |

A regular `jvid.portable` file **beside the actual executable** selects portable
mode. It does not depend on the working directory or the distribution marker.
Windows package root is the executable's folder; Linux root is the parent of
`bin`; macOS root is the folder containing `jvid.app`, outside the signed bundle.
Two packages in different folders therefore have independent data. Put portable
macOS apps in separate containing folders to isolate their data.

Portable `jvid-data/config` also holds themes, effects, key bindings, the first-run
component marker and eframe's `window.ron` (layout/recent projects). `jvid-data/data`
holds models, logs, untitled autosaves, sample media and untitled recordings.
`jvid-data/cache` holds thumbnails, waveforms, transcripts and proxies. An explicit
absolute `cache_dir` setting still overrides the default cache location. Saved
projects, their autosaves and recordings remain at their user-selected locations.
Moving a portable package does not relocate media referenced by absolute paths in
a project; use the existing relink flow when moving that media.

Startup probes the portable data/config/cache folders before opening the editor.
If writes fail, a native error explains that the entire package must be moved to a
writable folder. It does **not** fall back to the installed profile. Package smoke
checks exercise the failure path without a GUI and require a nonzero exit code.
Removing `jvid.portable` intentionally selects the installed profile; it does not
migrate or delete portable data. To migrate an existing installation, close jvid
and copy the settings/model files from the table into the portable destinations.

## Upgrades, uninstall and the updater contract (#477)

Inno Setup uses the stable AppId `{E8D1AB61-17F0-4C1B-8822-82A410FE435A}` across
versions, remembers the chosen folder, and replaces installer-owned files.
Installed user data, models and projects are not registered as installer files and
are retained by upgrades and uninstall. A setup cannot target a folder marked as
portable, including when invoked silently. Silent portable rejection writes its
reason to the installer log and exits without waiting for a dialog. Installers
support `/DIR=<folder>`,
`/VERYSILENT`, `/SUPPRESSMSGBOXES`, `/NORESTART`, `/NOCLOSEAPPLICATIONS` and
`/NORESTARTAPPLICATIONS`. The #477 helper waits for the app to exit, invokes the
verified setup, and handles restart; `/RESTARTAPPLICATIONS` alone is not a restart
contract. The updater flow and release activation steps are in [updating.md](updating.md).

Portable updates are manual: close jvid, keep `jvid-data`, and replace the program
files with the new package's files. For macOS, replace only the `.app`, keeping its
containing folder's `jvid-data`. Keep a backup until the new app has launched.
Do not delete the old containing folder before copying data. macOS DMG installs
also use manual app replacement. Linux currently has no atomic AppImage updater.

The artifact version is exactly `workspace.package.version` without `v`; release
tags are `v<version>` and must match. Initial targets/markers are fixed below.
`jvid-distribution` is a plain UTF-8 text file beside `jvid-app[.exe]`, containing
exactly the marker plus a newline. It selects distribution type for #477; it is
not an authenticity or trust signal.

| Artifact | Target | Marker | Payload |
| --- | --- | --- | --- |
| `jvid-{version}-windows-x86_64-setup.exe` | `windows-x86_64` | `windows-installer` | Inno setup; installed root `jvid-app.exe`, FFmpeg DLLs, notices, required updater helper |
| `jvid-{version}-windows-x86_64-portable.zip` | `windows-x86_64` | `windows-portable` | `jvid-{version}-windows-x86_64/` with executable, DLLs, both markers |
| `jvid-{version}-macos-aarch64.dmg` | `macos-aarch64` | `macos-dmg` | `jvid.app`, Applications link and inventory |
| `jvid-{version}-macos-aarch64-portable.zip` | `macos-aarch64` | `macos-portable` | `jvid-{version}-macos-aarch64/jvid.app` and inventory; both markers in `Contents/MacOS` |
| `jvid-{version}-linux-x86_64-portable.tar.gz` | `linux-x86_64` | `linux-portable` | `jvid-{version}-linux-x86_64/{AppRun,bin/jvid-app,lib/}`; both markers in `bin` |

macOS libraries live in `jvid.app/Contents/Frameworks`, the executable in
`Contents/MacOS`, and non-code data in `Contents/Resources`. Internal symlinks keep
`Contents/MacOS/THIRD-PARTY-NOTICES.txt`, `jvid-distribution`, and (portable only)
`jvid.portable` readable at the agreed beside-executable paths. Distribution-marker
readers follow those links normally; portable data still lives outside the app.
The ZIP uses `ditto` to preserve links and bundle metadata. Linux notices are in
`bin`, beside the executable. Every
package includes `THIRD-PARTY-NOTICES.txt`, jvid's licenses, runtime license texts,
`SOURCE-INFO.txt`, installation instructions and a `package-files.json` inventory.
Portable data is never part of an archive's payload or inventory.

Windows packaging builds
`cargo build --release --locked -p jvid-updater --bin jvid-update-helper --target x86_64-pc-windows-msvc` and puts
`jvid-update-helper.exe` beside the editor. The same repository variable
`JVID_UPDATE_PUBLIC_KEY` is passed at build time to the app and helper, along with
`JVID_UPDATE_VERSION` set to the canonical version without `v`. The public key
is a hex-encoded raw 32-byte Ed25519 key; absent configuration must never
permit an install. #477 owns `jvid-update.json`, its detached `.sig`, the signed
schema and generator. The generation hook belongs after collecting final native
assets (and signing/notarization) and before checksums/release creation; the protected release job requires this exact hook:

```sh
# JVID_UPDATE_SIGNING_KEY: secret 64-hex raw Ed25519 seed.
# JVID_UPDATE_PUBLIC_KEY: matching public key used to compile app and helper.
cargo run --release --locked -p jvid-updater --bin jvid-update-metadata -- \
  "$version" packaging/release-notes.md release-assets
```

The pipeline asserts the complete five-artifact contract before signing, then
uploads `jvid-update.json` and its 64-byte raw `jvid-update.json.sig` alongside the
exact native assets and their checksums. No signed metadata is fabricated by
packaging alone. The editor's update guard includes `self.components.is_busy()`:
it stays true while cancelled downloads unwind as well as during active model
transfers. The draft-release job runs only for enabled tag pushes; PR and manual
runs, including manual runs against a tag, never create a release.

## Building candidates and release controls

The **Package** workflow runs all three native toolchains, uploads artifacts and
smoke evidence, and never creates a release on PR or manual runs. Locally:

```sh
cargo build --locked --release -p jvid-app --features gpl-ffmpeg
python3 scripts/packaging/build.py --target linux-x86_64 --binary target/release/jvid-app
python3 scripts/packaging/smoke.py dist/jvid-0.1.0-linux-x86_64-portable.tar.gz --evidence dist/smoke
python3 -m unittest discover -s scripts/packaging -p 'test_*.py'
```

Linux assembly requires `patchelf`, `ldd`, `ldconfig`, dpkg copyright files, and the
runtime client libraries listed in the workflow. It recursively copies dependency
libraries and rewrites their relative loader paths; development prefix references
must not be needed at runtime. macOS assembly requires `otool`,
`install_name_tool`, `codesign`, `ditto`, `hdiutil` and Homebrew `ffmpeg@7` (a supported 7.x runtime; the workflow rejects
versions outside the supported 4.4–8.x range).
Windows requires Inno Setup 6 and `--ffmpeg-dir`; `fetch_ffmpeg.py` downloads the
reviewed GPL shared build and verifies `packaging/ffmpeg-windows.json`'s SHA-256.
The upstream daily URL has finite retention: review and update the lock if it
expires; do not replace it with an unchecked floating download. Windows Rust and
whisper use the static MSVC CRT; the supported OS provides UCRT. Bundled DLL
resolution is tested with development locations removed from PATH.

The existing [licensing policy](PLAN.md#licensing) is unchanged: release packages
use dynamically linked **GPL FFmpeg with x264/x265**, and the explicit
`gpl-ffmpeg` feature. The separate LGPL developer build option is unchanged.
There is no silent codec fallback and no nonfree FFmpeg variant. Runtime-specific
license/provenance files are collected in addition to the generated Rust/font/
Whisper/sample notices. Where a Homebrew bottle omits a nonstandard upstream
license (currently Leptonica 1.87.0), the version-pinned, checksum-checked text in
`packaging/licenses` is included with its exact upstream source URL; an unknown
missing notice is recovered from the exact source archive URL and SHA-256 in the
bottle's retained `.brew` formula. Archive paths are read only to recover notice
texts (including REUSE license directories and COPYING symlink targets); source
is never executed. A missing formula, checksum mismatch or absent source notices
fails assembly. Their full corresponding source and build scripts must
be reviewed before public distribution. Native package-manager provenance and
upstream links alone are not treated as the completed corresponding-source offer.

Package generates corresponding-source bundles alongside its native packages:

- The exact Git checkout and locked Cargo sources, including embedded native crate
  code, assets, build scripts and a working vendor configuration. Cargo metadata is
  verified offline with an empty Cargo home.
- Linux's exact recorded Debian source packages, original archives, Debian patches
  and build recipes, verified against authenticated APT metadata and `.dsc` checksums.
- macOS's retained Homebrew recipes and receipts, source archives/Git revisions,
  resource inputs, patches and build support.
- Windows's pinned FFmpeg checkout, BtbN dependency source inputs, patches and build
  recipes, with immutable revision/checksum provenance.

The `complete release source contract` job requires all four source bundles at the
same commit/tree and version as its five native assets. It verifies every archive
part and checks each source bundle's native package hashes before assembling the
`release-candidate` artifact. Missing providers, wrong versions/commits, missing or
changed parts and altered native package bytes fail before release signing.

Each bundle has `jvid-VERSION-sources-BUNDLE.json` plus ordered
`.tar.gz.partNNN` files, each at most 1 GiB, so large upstream source caches fit
GitHub's per-asset limit. `jvid-VERSION-source-manifest.json` records the complete
source/native relationship. `SOURCE-README.txt` and the standalone standard-library
`reassemble_sources.py` explain reconstruction. For example, with the Rust bundle
manifest and all its parts in one folder:

```sh
python reassemble_sources.py jvid-0.1.0-sources-rust.json workspace-sources.tar.gz
```

The helper verifies every part and the combined archive hash and refuses to overwrite
an existing output. Extract the resulting tarball with an archive utility. Provider
archives remain intact with their reconstruction/build recipes. Toolchains and OS
SDKs described by those recipes remain external build prerequisites.

Tagged builds create a **draft** only when `JVID_RELEASE_ENABLED=true` is configured
and the protected `release` environment allows it. The job downloads the verified
candidate, rechecks its source index/native hashes, then signs updater metadata and
attaches native/source assets and final checksums. No external corresponding-source
URL/checksum variables or separate hosting are required. PR/manual runs produce
candidate artifacts and never create releases. Source completeness and platform
acceptance still require maintainer review before publishing. Final signing and
notarization must precede metadata/checksums; keep drafts unpublished until the
validation checklist below is recorded.

## Verification and remaining platform acceptance

Automated checks distinguish packaged runtime tests from clean interactive use:

- Artifact extraction and inventory hashes; archive path containment.
- Actual packaged binary with no Rust/LLVM/FFmpeg paths: settings/model destination
  writes, import/decode of the embedded tutorial clip, CPU H.264+HEVC/AAC export,
  and decoding the resulting exports. No optional model is required.
- Portable launch from a path containing spaces, repeated launch preserving
  settings and a model sentinel, and a blocked portable data directory failing
  rather than writing to the profile.
- Disposable Windows CI user: silent install, packaged runtime check, build/run a
  numerically newer setup, verify directory reuse, uninstall, and assert settings,
  model sentinel and an extra user project survive. Do not run this lifecycle
  check on a personal account: it uses real Windows Known Folders and uninstall
  registration. Invoke `smoke.py --windows-install <setup.exe>` only on that user.
- macOS: verify/mount the DMG, run its packaged binary, verify app signatures,
  extract/test the portable ZIP. Linux: test the tarball again inside a fresh
  Ubuntu 22.04 container with no development libraries or FFmpeg install.

These checks do **not** prove interactive playback/audio/device behavior,
Gatekeeper/SmartScreen behavior, standard-user GUI shortcuts, or every GPU driver.
Before #476/#62 are closed, record on clean supported OS/VM accounts:

- [ ] Windows standard-user install, shortcuts, launch, edit/play/export with no
  optional model; real prior-version upgrade and uninstall preserving data.
- [ ] Extracted Windows ZIP launch and portable settings/model paths without admin.
- [ ] macOS Developer ID signing, notarization, Gatekeeper launch from a downloaded
  DMG and ZIP; edit, audio playback and export.
- [ ] Linux tarball launch on a clean supported desktop; edit, audio and export.
- [ ] Real Whisper model download from setup to the reported path; transcription,
  cancellation, failed network retry, skip and later addition on all platforms.
- [ ] Reviewed complete corresponding source and final download/metadata hashes.

CI results and actual local validation are recorded in the PR. Unchecked items
remain acceptance limitations, not claims of completion.
