# jvid

A lightweight, fast, cross-platform video editor written in Rust. It's for quick clip editing: trim, stitch, title, add music and export, without the weight of a full NLE.

jvid is the first tool in a planned suite of small, high-performance, open source apps.

![jvid main editor prototype](docs/prototypes/01-editor.png)

> **Status:** early development. The cargo workspace exists but no features are implemented yet; the screenshot is a generated prototype.

- [Plan](docs/PLAN.md): feature set, architecture and licensing
- [Roadmap](docs/ROADMAP.md): implementation order, milestones and issues
- [UI prototypes](docs/prototypes/): main editor, keyframe editing, export dialog, welcome screen

## Goals

- **Instant and fast.** Opens in under a second, scrubs long high-resolution recordings without stutter, and stays small.
- **Built for quick edits.** Pulling highlights out of long gameplay recordings, and trimming screen recordings for demos and tutorials.
- **Premiere-style editing.** In/out points, J/K/L, ripple trims and a Premiere default keymap, with every shortcut remappable.
- **Frame-accurate.** Rational time throughout, so variable-frame-rate recordings never drift out of sync.
- **Hackable.** Plain-text JSON projects, a headless CLI renderer and WGSL shader effects.
- **Windows, Linux and macOS**, free and open source.

## Building

jvid needs a Rust toolchain from [rustup](https://rustup.rs). `rust-toolchain.toml` pins the stable channel, so rustup installs the right toolchain, with `rustfmt` and `clippy`, the first time you run `cargo` in the repo.

```sh
cargo build --workspace
cargo test --workspace
cargo run -p jvid-app    # the desktop app
cargo run -p jvid-cli    # headless render
```

The crates are skeletons for now. FFmpeg setup is added with [#4](https://github.com/jarradbanks/jvid/issues/4).

## Workspace

| Crate | Role |
| --- | --- |
| [`jvid-core`](crates/jvid-core) | Project model, timeline, keyframes, commands and undo, serialization. No I/O, no UI. |
| [`jvid-media`](crates/jvid-media) | FFmpeg probe, decode and encode, thumbnails, waveforms, proxies |
| [`jvid-render`](crates/jvid-render) | wgpu compositor, transforms, blend modes, color, WGSL effects |
| [`jvid-audio`](crates/jvid-audio) | Mixer, cpal output, resampling, loudness |
| [`jvid-app`](crates/jvid-app) | eframe desktop app |
| [`jvid-cli`](crates/jvid-cli) | Headless render |
| [`suite-ui`](crates/suite-ui) | Shared theme, widgets and keybinding system for the whole suite |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Release builds bundle FFmpeg, which has its own license; see [PLAN.md · Licensing](docs/PLAN.md#licensing).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in jvid by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
