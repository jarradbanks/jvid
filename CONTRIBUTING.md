# Contributing to jvid

Thanks for helping. jvid is small on purpose, so the bar for new features is "does this make quick clip editing faster?" The [plan](docs/PLAN.md) lists what's in and out of scope.

## Finding work

Work is tracked as GitHub issues, grouped into milestones. [ROADMAP.md](docs/ROADMAP.md) gives the order they land in and what each one depends on. Pick an issue whose dependencies are closed, and comment on it before starting so work isn't duplicated.

## Developer setup

jvid links FFmpeg 4.4 to 8.x dynamically ([ADR 0001](docs/adr/0001-ffmpeg-binding.md)). Building needs the FFmpeg development libraries, `pkg-config` (Linux and macOS) and libclang (for bindgen).

- **Linux (Debian/Ubuntu):**

  ```sh
  sudo apt install clang libclang-dev pkg-config \
    libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev libavfilter-dev
  ```

  Ubuntu 22.04's FFmpeg 4.4 is the oldest supported version.
- **macOS:** `brew install ffmpeg pkg-config` (Xcode's command line tools provide libclang).
- **Windows:** install LLVM (`winget install LLVM.LLVM`), download an LGPL shared build such as `ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip` from [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds/releases), unzip it to `C:\ffmpeg`, then set `FFMPEG_DIR=C:\ffmpeg`, `LIBCLANG_PATH=C:\Program Files\LLVM\bin`, and add `C:\ffmpeg\bin` to `PATH` so the DLLs load.

The default build expects an LGPL FFmpeg. Build with `--features gpl-ffmpeg` if you link a GPL one (e.g. Homebrew's); otherwise jvid logs a warning at startup. The startup log line `ffmpeg loaded` shows the version, licence and codecs found.

## Making a change

1. Branch from `main`.
2. Keep each pull request to one issue, and put `Closes #N` in its description.
3. Before pushing, run:

   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```

4. Add tests for any logic you add. `jvid-core` should be covered by plain unit tests.

### GPU tests

`suite-ui`'s snapshot and layout tests render headless through wgpu on Mesa's lavapipe (software Vulkan, no GPU needed). Install it with `sudo apt install mesa-vulkan-drivers`; the tests then select it themselves, which also avoids the default WSL2 Vulkan driver, which segfaults. Without lavapipe they print one line and skip. Set `JVID_GPU_TESTS=required` to make them run on whatever driver is present instead (CI does this so it can't pass by skipping), or set `VK_ICD_FILENAMES` to pick a driver yourself. Snapshots are recorded with lavapipe; `UPDATE_SNAPSHOTS=1 cargo test -p suite-ui` rewrites them. `jvid-render` compositor goldens (`crates/jvid-render/testdata`) fail when missing; `JVID_BLESS=1 cargo test -p jvid-render` writes them.

## Ground rules

- **`jvid-core` has no I/O and no UI.** It must not depend on another jvid crate, a UI library or anything that touches files, devices or the GPU. That keeps it testable in isolation and lets both the app and the CLI drive it. Check with `cargo tree -p jvid-core`.
- **`suite-ui` is shared by the whole suite**, so it must not depend on any `jvid-*` crate.
- **Dependencies:** shared versions go in the root `Cargo.toml` under `[workspace.dependencies]`, and crates opt in with `name.workspace = true`. New dependencies outside the stack in [PLAN.md](docs/PLAN.md#stack) need a reason in the pull request.
- **Lints** are set once for the workspace in the root `Cargo.toml`; every crate opts in with `[lints] workspace = true`. `unsafe` code needs an explicit `#[allow(unsafe_code)]` and a `// SAFETY:` comment on every block.
- **Formatting** follows `rustfmt.toml` and `.editorconfig`.
- **UI** follows [DESIGN.md](DESIGN.md).

## License

jvid is licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in jvid by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
