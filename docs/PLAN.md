# jvid — plan

jvid is a lightweight, cross-platform video editor written in Rust, and the first tool in a suite of small, fast, open source apps.

**Who it's for:** quick clip editing, mainly gaming highlights cut from long recordings and screen recordings for demos and tutorials. It should be fast to open, fast to scrub, and functional without becoming a heavy NLE. Product context for design work is in [PRODUCT.md](../PRODUCT.md).

**Platforms:** Windows, Linux, macOS.

**License:** `MIT OR Apache-2.0` for all jvid source (see [Licensing](#licensing)).

---

## Feature set

### Foundations (designed in from day one)

- **Undo/redo** — every edit is a command on a history stack.
- **Project file** — human-readable JSON (`.jvid`), versioned schema, relative media paths.
- **Autosave and crash recovery.**
- **Headless CLI render** — `jvid render project.jvid -o out.mp4 [--in 00:00:08:12 --out 00:00:42:03]`.
- **Proxies and preview scaling** — full, ½, ¼ preview resolution; optional low-res proxy generation.
- **Timestamp-based frame lookup** — so variable-frame-rate phone footage never drifts out of sync.
- **Project settings** — resolution, frame rate, aspect. Presets: 16:9 1080p, 16:9 4K, 9:16 vertical, 1:1 square.
- **Rational time** — all times are integer ticks (flicks, 705,600,000/s), which divide every common frame rate and sample rate exactly. No float drift.

### Import and media

- Import by file dialog or drag and drop from the OS
- Video, audio, still images (PNG/JPG/WebP)
- Media bin with thumbnails, duration, search, and relinking of missing files
- Thumbnails and waveforms generated in the background

### Timeline and editing

- Layers: unlimited video tracks (top track on top) and unlimited audio tracks
- Trim, split at playhead, ripple delete, snapping (playhead, clip edges, markers, in/out)
- Ripple trim previous/next edit to playhead (`Q` / `W`) — the fastest way to cut down clips
- Linked audio/video clips, with unlink
- Track mute, solo, lock, hide
- Markers
- Clip speed (constant, reversed) and freeze frame
- Copy/paste of clips and keyframes

### In / Out points

Premiere-style marking, used for playback, editing, and export.

| Key | Action |
| --- | --- |
| `I` / `O` | Set in / out at playhead |
| `Alt+I` / `Alt+O` | Clear in / out |
| `Alt+X` | Clear both |
| `Shift+I` / `Shift+O` | Jump to in / out |
| `X` | Mark in/out around the clip under the playhead |
| `/` | Mark in/out around the selection |
| `;` | Lift — remove the in/out range, leave a gap |
| `'` | Extract — remove the in/out range and close the gap |

- The range shows as a shaded band on the ruler and across all tracks.
- Playback can loop the in/out range.
- **Source mode:** double-clicking a bin clip opens it in the preview's *Source* tab, where `I`/`O` mark the part you want; `,` inserts and `.` overwrites it at the playhead.
- **Export range:** the export dialog offers *Entire timeline*, *In to Out* (selected by default when a range is set), or *Selected clips*. The CLI takes `--in` / `--out`.

### Visual

- Transform: position, scale, rotation, anchor point
- Opacity and blend modes
- Crop
- Text/title layers (font, size, color, outline, shadow, background box)
- Solid color layers
- Transitions: crossfade, dip to black/white (presets built on keyframed opacity)
- Color: exposure, contrast, saturation, temperature, `.cube` LUTs
- Safe-area and grid overlays on the preview

### Keyframes and animation

- Any numeric property can be keyframed — transform, opacity, crop, color, volume, pan, effect parameters
- Easing per keyframe: linear, ease in, ease out, ease in/out, hold, bezier
- Keyframe lanes under a clip on the timeline, and diamond toggles in the inspector
- On-preview transform handles create keyframes when animation is armed
- Volume rubber-band line drawn directly on audio clips

### Audio

- Multiple audio tracks with per-track volume and pan
- Clip fade handles
- Waveforms on the timeline
- Master peak meter
- Audio scrubbing while skimming
- Loudness normalization on export (EBU R128, e.g. −14 LUFS)

### Playback and skimming

- Space, J/K/L shuttle, ←/→ frame step, Home/End
- Hover-skim in the media bin
- Frame cache and background decode
- Hardware decode: NVDEC, VAAPI, VideoToolbox, D3D11VA

### Export

- Presets: YouTube 1080p, YouTube 4K, Shorts/Reels 9:16, ProRes master, GIF, audio only (WAV, MP3, AAC)
- Range: entire timeline, in to out, selected clips
- Export current frame as PNG
- Hardware encode: NVENC, QSV, VideoToolbox, AMF, with a software fallback
- Background render queue
- SRT subtitles: burn in or write a sidecar file

### App

- Every keybinding remappable; the default keymap follows Premiere so muscle memory carries over
- Dockable panels and themes (dark by default)
- Config and keymaps in TOML

### Customization: WGSL shader effects

Users drop a `.wgsl` file into the effects folder. A small header declares its parameters:

```wgsl
// @jvid effect "Vignette"
// @param strength float 0.0..1.0 = 0.5
// @param softness float 0.0..1.0 = 0.3
```

jvid generates inspector sliders for each parameter, and they are keyframeable like any other property. That gives a plugin system with almost no API to maintain.

### Later, or as plugins

Chroma key, masks, scopes, speed ramps, nested sequences, voiceover recording, audio EQ/compressor, HDR, curve graph editor.

### Out of scope

Multicam, node compositing, 3D, motion tracking, collaboration, built-in AI.

---

## Architecture

### Stack

| Concern | Choice |
| --- | --- |
| UI | `egui` / `eframe` with `egui_dock` |
| GPU compositing | `wgpu` (shared with the UI) |
| Decode / encode | FFmpeg via `ffmpeg-next` (dynamically linked) |
| Audio output | `cpal` |
| Resampling | `rubato` |
| Text shaping | `cosmic-text` |
| Serialization | `serde` (JSON project files, TOML config) |

### Cargo workspace

```
crates/
  jvid-core/     project model, timeline, keyframes, commands + undo, serialization (no I/O, no UI)
  jvid-media/    FFmpeg probe/decode/encode, thumbnails, waveforms, proxies
  jvid-render/   wgpu compositor, transforms, blend modes, color, WGSL effects
  jvid-audio/    mixer, cpal output, resampling, loudness
  jvid-app/      eframe desktop app
  jvid-cli/      headless render
  suite-ui/      shared theme, widgets, keybinding system — reused by the rest of the suite
```

`jvid-core` stays free of I/O and UI so it can be tested in isolation and driven by both the app and the CLI.

### Performance targets

- Cold start under 1 s
- Idle memory under 150 MB
- Scrub 1080p H.264 at 30 fps or better on a mid-range laptop
- Decoded frames stay on the GPU from decode through compositing to display

---

## Licensing

- **jvid source:** `MIT OR Apache-2.0`, the Rust ecosystem convention. Anyone can reuse `jvid-core` or `suite-ui` in other projects, including the rest of the suite.
- **FFmpeg:** dynamically linked, never compiled into jvid's own crates.
- **Release builds** bundle an FFmpeg with x264/x265 for the best software encoding. That FFmpeg is GPL, so the release bundle as a whole is distributed under GPL terms, which permissive code allows.
- **Permissive-only builds:** a build option links an LGPL FFmpeg and relies on hardware encoders (plus OpenH264) instead of x264/x265.

---

## Milestones

| # | Milestone | Release |
| --- | --- | --- |
| M0 | Foundations | v0.1.0 |
| M1 | Media & Preview | v0.1.0 |
| M2 | Timeline | v0.1.0 |
| M3 | Animate | v0.1.0 |
| M4 | Export & v0.1.0 | v0.1.0 |
| M5 | Polish | v0.2.0 |
| M6 | Extend | v0.3.0 |

The implementation order, issue for each item, dependencies and critical path are in [ROADMAP.md](ROADMAP.md).

---

## UI prototypes

Generated with Codex image generation from the shared brief in [`prototypes/BRIEF.md`](prototypes/BRIEF.md):

1. `prototypes/01-editor.png` — main editor
2. `prototypes/02-keyframes.png` — keyframe editing
3. `prototypes/03-export.png` — export dialog
4. `prototypes/04-welcome.png` — welcome screen
