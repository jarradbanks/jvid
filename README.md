# jvid

a small, fast video editor for quick edits. pull a highlight out of a two-hour recording, trim a screen capture, slap a title on it, export, done. written in rust, UI drawn on the GPU, runs on windows, macos and linux. free, open source, no account.

## why this exists

most editors are built for a feature film. i mostly want to cut thirty seconds out of something and get on with my day, and waiting for a timeline app to finish its splash screen to do that gets old fast.

so jvid opens quick, stays out of the way, and puts everything on the keyboard. it isn't trying to replace your NLE - it's trying to be the thing you reach for before you'd bother opening it.

## what it does

- **keyboard-first editing.** in/out points, J/K/L, insert and overwrite, ripple trims, razor, markers. every shortcut is remappable, because yours are probably different from mine.
- **quick to start.** drag media onto the window. projects are plain JSON, autosaved, and relink when you move files around.
- **proxies.** low-res copies of 4K and HEVC sources get made in the background so scrubbing stays smooth. export uses the originals.
- **titles and solids.** text presets with font, outline, shadow and box, all in the inspector.
- **keyframes.** transform, opacity, blend mode, crop, color and volume, with an easing editor and zoom-to-region on the program monitor.
- **transitions and audio.** crossfades and dips on cuts, clip fades, a volume line on each clip.
- **effects.** WGSL shaders with keyframeable parameters - [write your own](docs/effects.md).
- **export.** presets for highlights, screen recordings, youtube, shorts/reels and prores, a background render queue, and `jvid render` when you'd rather not open the app at all.

the [user guide](docs/guide.md) covers the rest, and the [shortcut cheat sheet](docs/shortcuts.md) is the page you'll actually keep open.

## getting it

grab it from the [releases page](https://github.com/jarradbanks/jvid/releases/latest):

- **windows** - `setup.exe` (per-user, no admin) or a portable zip
- **macos** (apple silicon) - a dmg or a portable zip
- **linux** (x86_64, glibc 2.35+) - a portable tarball, run `./AppRun`

FFmpeg comes bundled. the windows installer updates itself from **Help → Check for Updates**; everything else links you to the new download. more in [the install guide](docs/packaging.md) and [the update guide](docs/updating.md).

or build it yourself. you need rust from [rustup](https://rustup.rs) (`rust-toolchain.toml` pins the channel), plus FFmpeg 4.4-8.x dev libraries, `pkg-config` and libclang.

- **debian/ubuntu:** `sudo apt install clang libclang-dev pkg-config libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev libavfilter-dev`
- **macos:** `brew install ffmpeg pkg-config`
- **windows:** `winget install LLVM.LLVM`, unzip an LGPL shared build from [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds/releases) to `C:\ffmpeg`, set `FFMPEG_DIR=C:\ffmpeg` and `LIBCLANG_PATH=C:\Program Files\LLVM\bin`, and add `C:\ffmpeg\bin` to `PATH`

linking a GPL FFmpeg (like homebrew's)? build with `--features gpl-ffmpeg`.

```sh
cargo run --release -p jvid-app                                    # the editor
cargo run --release -p jvid-cli -- render project.jvid -o out.mp4  # headless render
```

## encoders, briefly

H.264 export needs working hardware encoding or an FFmpeg built with OpenH264. x264/x265 need a GPL FFmpeg *and* jvid built with `--features gpl-ffmpeg` - installing a GPL FFmpeg alone won't switch them on. jvid will never quietly hand you MPEG-4 Part 2 and call it H.264; if nothing fits, it says so and you pick another preset (ProRes and GIF always work). the export dialog lists candidates in preference order and the log records which one actually opened.

<details>
<summary>headless rendering and the CLI's json output</summary>

audio presets (WAV included), `--lossless` and `--smart` never touch the GPU. composited video does need a rendering device - if the adapter fails, `jvid render` exits with code 4 and leaves any existing output alone. on a headless linux box, mesa's lavapipe works fine as a software vulkan adapter.

`--json` emits `phase`, `pass`, `retry`, phase `progress` (`null` when unknown) and `phase_eta_seconds`, which covers the current phase only. completion is reported after the output is validated and in place.

</details>

## the workspace

| crate | job |
| --- | --- |
| [`jvid-core`](crates/jvid-core) | project model, timeline, keyframes, commands and undo, serialization. no I/O, no UI, on purpose |
| [`jvid-media`](crates/jvid-media) | FFmpeg probe, decode and encode, thumbnails, waveforms, proxies |
| [`jvid-render`](crates/jvid-render) | wgpu compositor, transforms, blend modes, color, WGSL effects |
| [`jvid-audio`](crates/jvid-audio) | mixer, cpal output, resampling, loudness |
| [`jvid-app`](crates/jvid-app) | the eframe desktop app |
| [`jvid-cli`](crates/jvid-cli) | headless render |
| [`j-ui`](https://github.com/jarradbanks/j-ui) | shared themes, widgets and localisation for the suite (lives in its own repo) |

the bigger picture - feature set, architecture, licensing - is in [the plan](docs/PLAN.md), and what's coming next is on [the roadmap](docs/ROADMAP.md).

## bugs and ideas

[open an issue](https://github.com/jarradbanks/jvid/issues). PRs welcome too - run `cargo fmt`, `cargo clippy -- -D warnings` and `cargo test --workspace` first.

## license

MIT or Apache-2.0, your pick ([MIT](LICENSE-MIT), [Apache](LICENSE-APACHE)). release builds bundle FFmpeg, which brings its own license - see [PLAN.md · Licensing](docs/PLAN.md#licensing).

unless you say otherwise, anything you contribute is dual licensed the same way, as defined in the Apache-2.0 license, no extra terms.
