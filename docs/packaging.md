# installing jvid

grab the right file from the [latest release](https://github.com/jarradbanks/jvid/releases/latest). no account, no admin, nothing else to install - FFmpeg and the speech engine come bundled.

| platform | file | how |
| --- | --- | --- |
| windows 10 22H2+ / 11, x86_64 | `…-windows-x86_64-setup.exe` | run it. installs per-user to `%LOCALAPPDATA%\Programs\jvid` |
| windows, portable | `…-windows-x86_64-portable.zip` | extract the whole thing somewhere writable, run `jvid-app.exe` |
| macos 15.8.1+, apple silicon | `…-macos-aarch64.dmg` | drag `jvid.app` to applications |
| macos, portable | `…-macos-aarch64-portable.zip` | extract somewhere writable, open `jvid.app` |
| linux x86_64, glibc 2.35+ (ubuntu 22.04+) | `…-linux-x86_64-portable.tar.gz` | extract, run `./AppRun` |

x86_64 builds need a CPU with AVX2. no intel mac, ARM windows or ARM linux builds yet. the mac builds aren't notarized yet, so gatekeeper will complain the first time.

## speech models

transcription and subtitles need a whisper model. they're optional - editing, playback and export work without one. the windows installer offers them (all unticked); everywhere else jvid asks on first launch, and **Help → Optional Components** adds them later.

| model | size |
| --- | ---: |
| whisper base | 148 MB |
| whisper small | 488 MB |
| whisper large v3 turbo q5 | 574 MB |

you only need one.

## where your stuff lives

| | settings | models |
| --- | --- | --- |
| windows | `%APPDATA%\jvid\config` | `%LOCALAPPDATA%\jvid\data\models` |
| macos | `~/Library/Application Support/jvid` | same folder, `models/` |
| linux / source builds | `~/.config/jvid` | `~/.local/share/jvid/models` |
| portable | `jvid-data/config` beside the app | `jvid-data/data/models` |

portable mode is switched on by a `jvid.portable` file beside the executable. all its data stays in `jvid-data` - move the folder and the data goes with it. if that folder isn't writable, jvid says so instead of quietly writing to your profile. on macos, give each portable copy its own folder.

## updating and uninstalling

the windows installer updates in place from **Help → Check for Updates**. everything else is manual: close jvid, swap the program files for the new ones, keep `jvid-data`. see [updating.md](updating.md).

uninstalling on windows (apps & features) leaves your projects, settings and models alone.

## building packages yourself

```sh
cargo build --locked --release -p jvid-app --features gpl-ffmpeg
python3 scripts/packaging/build.py --target linux-x86_64 --binary target/release/jvid-app
python3 scripts/packaging/smoke.py dist/jvid-<version>-linux-x86_64-portable.tar.gz --evidence dist/smoke
python3 -m unittest discover -s scripts/packaging -p 'test_*.py'
```

linux needs `patchelf`, `ldd` and `ldconfig`. macos needs xcode's tools and homebrew `ffmpeg@7`. windows needs inno setup 6.6 or newer and `--ffmpeg-dir` (`fetch_ffmpeg.py` downloads the pinned build). the [package workflow](../.github/workflows/package.yml) is the source of truth.

release packages link GPL FFmpeg with x264/x265 - see [PLAN.md · Licensing](PLAN.md#licensing). every package carries its third-party notices, and the full corresponding source for each version sits in the matching `sources-v<version>` release.

the Windows SDK and dependency source cache are retained in a pinned, separate `runtime-inputs-ffmpeg-2026-10-31` prerelease. it is never the latest app release. `packaging/ffmpeg-windows.json` records the upstream monthly SDK, identical retained SDK bytes, and individually hashed source-cache parts; reassembly also checks the original whole-artifact digest. the reviewed `ffmpeg-windows-provenance.json` captures GitHub metadata before artifact expiry and is itself hash-pinned. collection applies the same run, job, repository, recipe, source and completeness checks to those retained facts and inputs.

changing this runtime requires reviewing its binary and complete corresponding source together, including Rust and compiler runtime evidence. publish a new input tag with `--prerelease --latest=false` only after that review, then require green Package candidates before merging. never replace inputs under an existing hash or published tag. app releases continue through the [develop release flow](updating.md#releasing).
