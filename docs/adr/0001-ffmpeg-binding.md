# ADR 0001: FFmpeg binding, versions and licensing

Status: accepted (issue #4)

## Context

jvid decodes and encodes through FFmpeg, dynamically linked ([PLAN.md · Licensing](../PLAN.md#licensing)). We need a Rust binding that builds on Windows, macOS and Linux, and a supported FFmpeg version range.

## Options

| | `ffmpeg-next` 9.x | `rsmpeg` 0.18 |
| --- | --- | --- |
| FFmpeg versions | 3.4 to 9.0, detected at build time | one major per release (0.18 = FFmpeg 8.0 only) |
| Safety | safe high-level API over `ffmpeg-sys-next` | thinner, more `unsafe` at call sites |
| Maintenance | maintenance mode: compiles against every release branch, few new APIs | active, but bumps break on every FFmpeg major |
| Distro FFmpeg | works with apt's 4.4 (Ubuntu 22.04) and 6.1 (24.04) | needs FFmpeg 8 everywhere, i.e. no distro packages |
| Licence | WTFPL (permissive) | MIT |

## Decision

- **`ffmpeg-next` 9**, as already listed in PLAN.md's stack. Its wide version range lets contributors use their distro's FFmpeg, and the safe API keeps `unsafe` out of jvid. We only enable the `codec` and `format` features; add others (`software-scaling`, `filter`) when a feature needs them. If it falls behind a future FFmpeg, `rsmpeg` is the fallback.
- **Supported FFmpeg: 4.4 to 8.x** (libavcodec ≥ 58.134). 4.4 is the oldest distro build contributors are likely to have (Ubuntu 22.04); `jvid_media::ffmpeg::init` logs a warning below it. CI tests 6.1 (Ubuntu 24.04 apt), Homebrew's current release (macOS) and 8.1 (Windows). No PPA or prebuilt build is needed on Ubuntu.
- **Windows** uses BtbN's LGPL shared builds (`ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip`) via `FFMPEG_DIR`, cached in CI. vcpkg also works but compiles FFmpeg from source, which is too slow for CI.
- **Licensing:** the default build expects an LGPL FFmpeg. The `gpl-ffmpeg` Cargo feature (on `jvid-media`, forwarded by `jvid-app`) marks a build that bundles a GPL FFmpeg with x264/x265, used for releases. At startup jvid logs the linked FFmpeg's version, licence and key codecs, and warns if a default build has loaded a GPL FFmpeg. Feature choice doesn't change what gets linked, because linking is dynamic; it records intent, and later issues will use it to enable GPL-only encoders.
- `ffmpeg-next`/`ffmpeg-sys-next` are WTFPL, allowed by name in `deny.toml`.

## Consequences

- Building needs FFmpeg dev libraries plus libclang (for bindgen) on every machine; see CONTRIBUTING.md.
- Code that uses APIs newer than 4.4 must gate them with `ffmpeg-sys-next`'s `ffmpeg_x_y` cfgs.
