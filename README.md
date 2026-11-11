# jvid

a small, fast video editor for quick edits

## what it can do

- **keyboard-first editing.** in/out points, J/K/L, insert and overwrite, ripple trims, razor, markers. every shortcut is remappable
- **quick to start.** drag media onto the window. projects are plain JSON, autosaved, and relink when you move files around.
- **proxies.** low-res copies of 4K and HEVC sources get made in the background so scrubbing stays smooth. export uses the originals.
- **titles and solids.** text presets with font, outline, shadow and box, all in the inspector.
- **keyframes.** transform, opacity, blend mode, crop, color and volume, with an easing editor and zoom-to-region on the program monitor.
- **transitions and audio.** crossfades and dips on cuts, clip fades, a volume line on each clip.
- **effects.** WGSL shaders with keyframeable parameters - [write your own](docs/effects.md).
- **export.** presets for highlights, screen recordings, youtube, shorts/reels and prores, a background render queue, and `jvid render` when you'd rather not open the app at all.

the [user guide](docs/guide.md) covers the rest, and see the [shortcut cheat sheet](docs/shortcuts.md)

## getting it

grab it from the [releases page](https://github.com/jarradbanks/jvid/releases/latest):

- **windows** - `setup.exe` (per-user, no admin) or a portable zip
- **macos** (apple silicon) - a dmg or a portable zip
- **linux** (x86_64, glibc 2.35+) - a portable tarball, run `./AppRun`

FFmpeg comes bundled. the windows installer updates itself from **Help → Check for Updates**; everything else links you to the new download. more in [the install guide](docs/packaging.md) and [the update guide](docs/updating.md).

<details>
<summary>build it yourself</summary>

you need rust from [rustup](https://rustup.rs) (`rust-toolchain.toml` pins the channel), plus FFmpeg 4.4-8.x dev libraries, `pkg-config` and libclang.

- **debian/ubuntu:** `sudo apt install clang libclang-dev pkg-config libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev libavfilter-dev`
- **macos:** `brew install ffmpeg pkg-config`
- **windows:** `winget install LLVM.LLVM`, unzip an LGPL shared build from [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds/releases) to `C:\ffmpeg`, set `FFMPEG_DIR=C:\ffmpeg` and `LIBCLANG_PATH=C:\Program Files\LLVM\bin`, and add `C:\ffmpeg\bin` to `PATH`

linking a GPL FFmpeg (like homebrew's)? build with `--features gpl-ffmpeg`.

```sh
cargo run --release -p jvid-app                                    # the editor
cargo run --release -p jvid-cli -- render project.jvid -o out.mp4  # headless render
```

</details>

## license

MIT or Apache-2.0, your pick ([MIT](LICENSE-MIT), [Apache](LICENSE-APACHE)). release builds bundle FFmpeg, which brings its own license - see [PLAN.md · Licensing](docs/PLAN.md#licensing).
