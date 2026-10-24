# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

A native desktop app for Windows, macOS and Linux, written in Rust: `egui`/`eframe` with `egui_dock` on `wgpu`, FFmpeg for media, `cpal` for audio. The full stack and crate layout are in [docs/PLAN.md](docs/PLAN.md).

Platform is recorded as `web` because Impeccable's platform field names the design language the app renders, not the toolchain. jvid draws its own UI and looks the same on every OS; it does not use native iOS, Android, Windows or macOS widgets. Browser-only guidance (CSS, the DOM, responsive breakpoints, live browser mode) does not apply. The shared visual language lives in the `suite-ui` crate.

## Users

The project owner and people like them: experienced enough to expect pro-editor conventions (I/O marking, J/K/L, ripple trims), who want a quick edit rather than a heavy NLE workflow. Two main jobs:

- **Gaming highlights:** cutting highlight clips out of long gameplay recordings. These sources can be hours long and high frame rate.
- **Screen recordings and demos:** trimming product demos, tutorials and bug repros; cropping or zooming in on part of the screen; adding a title or callout; cutting dead air.

## Product Purpose

jvid is a lightweight, functional video editor for quick clip editing: import, cut, layer, animate and export. Success means going from a raw recording to an exported clip in minutes, with the app never being the slow part.

## Positioning

**Instant and fast.** jvid opens in under a second, scrubs long high-resolution recordings without stutter, and stays small. That claim rests on its mechanism: native Rust, GPU compositing, decoded frames kept on the GPU, and a deliberately narrow feature set.

Supporting truths, which back the lead claim rather than compete with it: free and open source (`MIT OR Apache-2.0`), no account, standard pro-editor shortcuts, and hackable (plain-text projects, a CLI renderer, WGSL shader effects).

## Operating Context

- Usually opened right after recording, to pull a clip out of a longer file.
- Sources: game and screen recorder output, often variable frame rate, high frame rate and long; phone footage too.
- Keyboard-first with a standard pro-editor default keymap; every shortcut remappable.
- Desktop only, with a mouse or trackpad and a keyboard.

## Capabilities and Constraints

- Feature set, out-of-scope list, architecture and licensing: [docs/PLAN.md](docs/PLAN.md). Build order and issues: [docs/ROADMAP.md](docs/ROADMAP.md).
- Performance targets are product requirements, not nice-to-haves: cold start under 1 s, idle memory under 150 MB, 1080p H.264 scrubbing at 30 fps or better on a mid-range laptop.
- Same behaviour and look on Windows, macOS and Linux.
- Terminology: clip, track, layer, bin, inspector, source viewer, timeline, playhead, in/out, marker, keyframe, easing, proxy, preset.
- Undecided: the suite's name; which other tools follow jvid.

## Brand Commitments

- The name is **jvid**, always lowercase.
- jvid is the first tool in a suite. The suite shares one design language through `suite-ui`; each tool keeps its own name.
- UI direction is **dark, pro and minimal** (stated by the owner).
- In/out marking and default shortcuts follow the standard pro-editor conventions.

## Evidence on Hand

- [docs/PLAN.md](docs/PLAN.md) and [docs/ROADMAP.md](docs/ROADMAP.md), with the issues they link to.
- The README preview (`docs/media/`) is the real app, rendered by `scripts/preview/build`. There are no users, testimonials or benchmarks yet. Performance numbers are targets, not measurements. Do not present any of these as real.

## Product Principles

1. **Speed is the feature.** When a feature threatens startup time, scrub latency or memory, the feature gives way.
2. **Quick edits, not heavy workflows.** Prefer one obvious way to do something. Multicam, node compositing, 3D and collaboration stay out.
3. **Muscle memory over novelty.** Familiar pro-editor defaults, and everything remappable.
4. **Open and inspectable.** Plain-text projects, a CLI renderer, user shaders; no account and no lock-in.
5. **One look across the suite.** Visual decisions are made in `suite-ui` so the next tool inherits them.
