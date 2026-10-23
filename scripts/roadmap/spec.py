"""jvid issue breakdown. Issues are listed in implementation order; every dependency appears before its dependents."""

MILESTONES = {
    "M0": ("M0 · Foundations", "Workspace, CI, FFmpeg setup, project model, undo/redo, file format, app shell, theme, keymap."),
    "M1": ("M1 · Media & Preview", "Probe, decode, import, media bin, thumbnails, waveforms, GPU preview, audio output, source viewer, welcome screen."),
    "M2": ("M2 · Timeline", "Timeline panel, tracks, clip placement, trim/split/ripple, snapping, in/out, compositor, mixer, playback."),
    "M3": ("M3 · Animate", "Keyframes with easing, inspector, transform/crop/opacity/blend, preview gizmo, keyframe lanes, audio automation."),
    "M4": ("M4 · Export & v0.1.0", "Export pipeline, presets, export dialog with In to Out, hardware encode, loudness, queue, CLI render, release packaging."),
    "M5": ("M5 · Polish (v0.2.0)", "Text, transitions, color and LUTs, speed, proxies, hardware decode, autosave, keymap editor, themes."),
    "M6": ("M6 · Extend (v0.3.0)", "WGSL shader effects and SRT subtitles."),
}

RELEASES = [
    ("v0.1.0", ["M0", "M1", "M2", "M3", "M4"], "The first usable quick editor: import, cut, layer, animate, export."),
    ("v0.2.0", ["M5"], "Titles, transitions, color, speed, and the performance work that makes 4K comfortable."),
    ("v0.3.0", ["M6"], "Extensibility: user shader effects and subtitles."),
]

AREAS = {
    "core": ("1d76db", "jvid-core: model, time, commands, evaluation"),
    "media": ("0e8a16", "jvid-media: probe, decode, encode, caches"),
    "render": ("5319e7", "jvid-render: wgpu compositing and effects"),
    "audio": ("2e7d32", "jvid-audio: mixing, output, loudness"),
    "ui": ("c2185b", "jvid-app: panels, dialogs, interaction"),
    "timeline": ("e65100", "Timeline editing behaviour"),
    "export": ("b71c1c", "Export pipeline and presets"),
    "cli": ("455a64", "jvid-cli headless render"),
    "suite-ui": ("6a1b9a", "Shared suite-ui crate"),
    "infra": ("616161", "Build, CI, packaging, tooling"),
    "docs": ("0075ca", "Documentation"),
}

SIZES = {
    "S": ("c5def5", "Up to a day"),
    "M": ("bfd4f2", "Two to four days"),
    "L": ("d4c5f9", "One to two weeks"),
}

EXTRA_LABELS = {
    "good first issue": ("7057ff", "Good for newcomers"),
    "later": ("ededed", "Planned after v0.3.0, or as a plugin"),
}

PLAN = "https://github.com/jarradbanks/jvid/blob/main/docs/PLAN.md"
PROTO = "https://github.com/jarradbanks/jvid/blob/main/docs/prototypes"
DESIGN = "https://github.com/jarradbanks/jvid/blob/main/DESIGN.md"

ISSUES = []


def I(key, title, ms, areas, size, deps=(), summary="", scope=(), ac=(), refs=(), labels=()):
    ISSUES.append(dict(key=key, title=title, ms=ms, areas=list(areas), size=size, deps=list(deps),
                       summary=summary, scope=list(scope), ac=list(ac), refs=list(refs), labels=list(labels)))


# ───────────────────────── M0 · Foundations ─────────────────────────

I("workspace", "Set up the cargo workspace, licenses and README", "M0", ["infra"], "S",
  summary="Create the workspace layout from the plan so every later issue has a crate to land in.",
  scope=[
      "Crates: `jvid-core`, `jvid-media`, `jvid-render`, `jvid-audio`, `jvid-app` (bin), `jvid-cli` (bin), `suite-ui`",
      "Shared `[workspace.dependencies]`, Rust 2024 edition, `rust-toolchain.toml` pinned to stable",
      "`LICENSE-MIT`, `LICENSE-APACHE`, `license = \"MIT OR Apache-2.0\"` in every crate",
      "README with goals, build instructions and links to PLAN.md and ROADMAP.md; CONTRIBUTING.md",
      "`rustfmt.toml`, `.editorconfig`, workspace clippy lints",
  ],
  ac=[
      "`cargo build --workspace` and `cargo test --workspace` pass on a clean checkout",
      "`jvid-core` has no dependency on any other jvid crate, UI or I/O library",
  ],
  refs=[f"[PLAN.md · Cargo workspace]({PLAN}#cargo-workspace)"])

I("ci", "CI: fmt, clippy and tests on Windows, macOS and Linux", "M0", ["infra"], "S", ["workspace"],
  summary="GitHub Actions matrix so cross-platform breakage is caught on every PR.",
  scope=[
      "Matrix: `windows-latest`, `macos-latest`, `ubuntu-latest`",
      "`cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace`",
      "`Swatinem/rust-cache` for build caching",
      "`cargo-deny` for advisories and licenses, configured to flag GPL crates in our own dependency tree",
  ],
  ac=["A PR with a clippy warning or failing test is blocked on all three OSes"])

I("logging", "Logging, tracing and crash logs", "M0", ["infra"], "S", ["workspace"],
  summary="Structured logs and a panic hook so bug reports come with useful context.",
  scope=[
      "`tracing` + `tracing-subscriber`, log level from `JVID_LOG` env var",
      "Rolling log file in the platform data directory",
      "Panic hook that writes a crash log with backtrace, version and OS; exposes a hook autosave can use later",
  ],
  ac=["A forced panic produces a crash log file with a backtrace"],
  labels=["good first issue"])

I("roadmap-status", "Roadmap shows live status", "M0", ["infra"], "S",
  summary="ROADMAP.md shows where each issue stands, so the plan doubles as a progress board.",
  scope=[
      "`scripts/roadmap/roadmap.py` reads issue state with one `gh issue list --state all` call (plus one `gh pr list` call for open PRs)",
      "New Status column in every issue table: done (closed), need testing (label), in progress (an open PR references the issue), blank otherwise",
      "If `gh` is missing or fails, print a warning and render the Status column blank instead of failing",
  ],
  ac=[
      "Regenerating ROADMAP.md marks closed, `need testing` and open-PR issues correctly",
      "The status mapping has a small test that runs without network access",
  ])

I("ffmpeg-setup", "FFmpeg linking and developer setup on all platforms", "M0", ["infra", "media"], "M", ["ci"],
  summary="Pick the FFmpeg binding and make it build everywhere. This is the riskiest setup task, so it goes first.",
  scope=[
      "Spike `ffmpeg-next` vs `rsmpeg` (FFmpeg 7.x support, safety, maintenance) and record the decision in `docs/adr/`",
      "Dev setup docs: Linux (`apt` dev packages), macOS (Homebrew), Windows (vcpkg or prebuilt shared builds)",
      "CI installs FFmpeg on all three OSes and caches it",
      "Cargo feature `gpl-ffmpeg` vs the default LGPL build, per the licensing plan",
      "Runtime check of FFmpeg version and available codecs, logged at startup",
  ],
  ac=["CI opens and decodes a test file on Windows, macOS and Linux"],
  refs=[f"[PLAN.md · Licensing]({PLAN}#licensing)"])

I("test-media", "Generated test media corpus", "M0", ["infra", "media"], "S", ["ffmpeg-setup"],
  summary="A reproducible set of tricky media files, generated with FFmpeg's `lavfi` sources rather than committed.",
  scope=[
      "Script `scripts/gen-test-media` producing:",
      "1080p30 H.264 CFR with the frame index encoded as a pixel-block pattern (machine-checkable seeks)",
      "Variable-frame-rate clip (phone-style), 23.976 and 59.94 fps clips, 4K HEVC, ProRes",
      "Phone clip with rotation metadata, 9:16 clip, a clip with no audio",
      "Audio-only WAV/MP3/AAC with a 1 kHz tone at a known level; PNG/JPG/WebP stills",
      "A 2-hour 1440p60 recording, encoded cheaply (low bitrate, fast preset) so it stays small and quick to generate",
      "A 144 fps clip",
      "An MKV with three audio streams (game, mic, voice chat), each with a distinct tone and a stream title",
      "A 1440p screen-recording-style clip with fine text and UI lines, to catch scaling and encode blur",
      "CI caches the generated corpus",
  ],
  ac=[
      "`cargo test` can locate the corpus, and a helper decodes the frame index from a frame",
      "The long recording is generated in under 2 minutes on CI",
  ])

I("time", "Rational time and timecode", "M0", ["core"], "S", ["workspace"],
  summary="Integer time in flicks (705,600,000 per second) so frame and sample positions never drift.",
  scope=[
      "`Time(i64)` in flicks, `Rate { num, den }` for frame and sample rates",
      "Conversions: frame ↔ time, sample ↔ time, with explicit rounding",
      "Timecode format/parse `HH:MM:SS:FF` (non-drop-frame; drop-frame noted as follow-up)",
      "`proptest` round-trip tests at 23.976, 24, 25, 29.97, 30, 50, 59.94, 60 fps and 44.1/48 kHz",
  ],
  ac=["Round-trips are exact for every listed rate over a 24-hour range"],
  labels=["good first issue"])

I("model", "Project data model", "M0", ["core"], "M", ["time"],
  summary="The in-memory model for projects, media, tracks and clips, with no I/O or UI dependencies.",
  scope=[
      "`Project` settings: resolution, frame rate, sample rate, background color",
      "`MediaAsset`: id, path, probe info (filled in by jvid-media)",
      "`Sequence` → `Track` (video/audio; name, hidden, locked, muted, solo, volume, pan) → `Clip` (asset, source in/out, timeline start, link group, parameters)",
      "Markers and sequence in/out points",
      "Stable IDs; invariant: clips on one track never overlap",
      "Queries: clips at a time, clips in a range, next/previous edit point",
  ],
  ac=["Unit tests cover the invariants and queries", "The crate compiles without any media, render or UI dependency"])

I("undo", "Command system and undo/redo", "M0", ["core"], "M", ["model"],
  summary="Every model change goes through a command, so undo/redo is complete by construction.",
  scope=[
      "Command API: apply/revert, with transactions grouping several commands into one undo step",
      "Coalescing for continuous gestures (a drag is one undo step)",
      "History limit and a dirty flag for unsaved changes",
      "Model fields are private outside the edit API, so nothing can mutate state without a command",
  ],
  ac=["A `proptest` applies random command sequences, undoes them all and gets back an identical project"])

I("serialize", "Project file format (.jvid JSON)", "M0", ["core"], "S", ["model"],
  summary="Human-readable, versioned project files.",
  scope=[
      "`serde` JSON with a top-level `schema_version` and a migration hook",
      "Media paths stored relative to the project file, with an absolute fallback",
      "Atomic save (write to a temp file, then rename)",
      "Fixture projects in `tests/fixtures/`",
  ],
  ac=["Save → load round-trips are lossless", "Moving a project folder with its media keeps all links valid"],
  refs=[f"[PLAN.md · Foundations]({PLAN}#foundations-designed-in-from-day-one)"])

I("config", "Settings and config files (TOML)", "M0", ["core"], "S", ["workspace"],
  summary="User settings stored in the platform config directory.",
  scope=[
      "`directories` crate for config, data and cache paths",
      "`settings.toml`: default preview quality, autosave interval, cache location and size cap, recent projects",
      "Load with defaults, validate values, warn on unknown keys",
  ],
  ac=["A missing or partial settings file falls back to defaults without errors"],
  labels=["good first issue"])

I("suite-ui-theme", "suite-ui: theme tokens, fonts and base widgets", "M0", ["suite-ui"], "M", ["workspace"],
  summary="The shared look and widgets every suite tool will reuse.",
  scope=[
      "Color, type, spacing, radius and size tokens from `DESIGN.md` (The Quiet Frame)",
      "Embedded Inter (OFL) with tabular figures frozen in as the default, and an icon set with a permissive license",
      "egui `Style`/`Visuals` setup",
      "Panel frame, 32px header and tab widget, used by every dock panel: equal-width tabs that fill the header (up to 160px each) with centred labels, and the blue focused-panel border",
      "Widgets: button (primary, secondary, ghost, destructive), input with inline prefix, numeric drag field, search field, dropdown, segmented control, checkbox, collapsible section header, timecode label",
      "A gallery example: `cargo run -p suite-ui --example gallery`",
  ],
  ac=["The gallery shows every widget in default, hover, focus-visible, pressed and disabled states, plus loading and error where they apply",
      "Ink text on every clip colour reaches 4.5:1",
      "Every corner comes from the 4/6/8/12px radius scale, and no border or divider is wider than 1 physical pixel"],
  refs=[f"[DESIGN.md]({DESIGN})"])

I("app-shell", "App shell: window, dock layout and top bar", "M0", ["ui"], "M", ["suite-ui-theme", "serialize", "undo"],
  summary="The empty editor frame with all panels in place.",
  scope=[
      "`eframe` with the wgpu backend",
      "`egui_dock` layout with Media, Preview, Inspector and Timeline panels, arranged as in the main editor prototype and styled per `DESIGN.md`",
      "Every panel header is the shared 32px header widget from `suite-ui`, on one line across the window",
      "Top bar: wordmark, project name with unsaved marker, undo/redo, Export button",
      "New / Open / Save / Save As wired to the project file format; dock layout persisted",
      "Window title and app icon",
  ],
  ac=["Cold start to first frame under 1 s on a mid-range laptop", "Undo/redo buttons reflect history state",
      "egui_kittest snapshots at 1280×720 and 1920×1080, 100% and 150% scale: header dividers and label baselines in each row of panels share one y",
      "Switching a panel between one view and several tabs doesn't move its header or label",
      "Text and icons start 12px from every panel's left edge"],
  refs=[f"[DESIGN.md · Layout]({DESIGN}#layout)", f"[01-editor.png (layout only; styling per DESIGN.md)]({PROTO}/01-editor.png)",
        f"[Editor mockup in the DESIGN.md look]({PROTO}/v2/01-editor.png)"])

I("actions", "Action registry and keymap with Premiere defaults", "M0", ["ui"], "M", ["app-shell", "config"],
  summary="Every command is a named action with a remappable shortcut.",
  scope=[
      "Registry: id, display name, default shortcut, focus context (timeline, preview, text field)",
      "`keymap.toml` overrides with conflict detection",
      "Default keymap following Premiere: Space, J/K/L, I/O, Alt+I/O/X, Shift+I/O, X, /, ;, ', comma, period, Q/W, C, V, Ctrl+K, ←/→, Home/End",
      "Shift+number focuses panels with Premiere's numbering: 1 Media, 2 Source, 3 Timeline, 4 Program, 5 Inspector, 7 Effects",
      "Cmd instead of Ctrl on macOS",
  ],
  ac=["Remapping an action in `keymap.toml` takes effect on restart", "Shortcuts do not fire while typing in a text field"],
  refs=[f"[PLAN.md · In / Out points]({PLAN}#in--out-points)"])

# ───────────────────────── M1 · Media & Preview ─────────────────────────

I("probe", "Media probing", "M1", ["media"], "M", ["ffmpeg-setup", "model", "test-media"],
  summary="Read everything we need to know about a file before using it.",
  scope=[
      "Duration, container, streams, codecs",
      "Video: resolution, sample/display aspect, average and real frame rate, VFR detection, rotation metadata, color primaries/transfer/range",
      "Audio: sample rate, channels and layout",
      "Clear error types for unsupported or corrupt files",
  ],
  ac=["Every file in the test corpus probes correctly, including VFR and rotation"])

I("video-decode", "Video decoder with frame-accurate seeking", "M1", ["media"], "L", ["probe"],
  summary="Return the exact frame for any timestamp, including variable-frame-rate footage.",
  scope=[
      "Frame lookup by timestamp: the frame shown at time t is the last frame with pts ≤ t",
      "Accurate seek: jump to the preceding keyframe, decode forward",
      "Apply rotation metadata",
      "Output a GPU-friendly format (NV12 / YUV420 planes) for upload",
      "Decoder instances pooled per asset; decoding off the UI thread",
  ],
  ac=[
      "Random-seek test over the corpus returns the correct frame index every time",
      "VFR clip frames match FFmpeg's own timestamp mapping",
  ])

I("audio-decode", "Audio decoder and resampling", "M1", ["media", "audio"], "M", ["probe"],
  summary="Decode any audio stream to f32 at the project sample rate.",
  scope=[
      "Decode to f32 at 48 kHz (project rate), resampled with `rubato` or swresample",
      "Downmix any channel layout to stereo",
      "Sample-accurate seek",
  ],
  ac=["The 1 kHz test tone decodes at the right frequency and level after resampling from 44.1 kHz"])

I("stills", "Still image import", "M1", ["media"], "S", ["model"],
  summary="PNG, JPG and WebP as clips.",
  scope=["Decode with the `image` crate", "Respect EXIF orientation", "Default still duration (5 s) from settings"],
  ac=["A rotated phone JPEG displays upright"],
  labels=["good first issue"])

I("jobs", "Background job system", "M1", ["core"], "M", ["workspace"],
  summary="One place to run slow work (thumbnails, waveforms, proxies, exports) without blocking the UI.",
  scope=[
      "Thread pool with priorities: interactive > thumbnails/waveforms > proxies",
      "Cancellation tokens and progress reporting to the UI",
      "De-duplication of identical jobs",
  ],
  ac=["Cancelling a job stops it within 100 ms", "The UI stays at 60 fps while 8 jobs run"])

I("import", "Import media with a file dialog and drag and drop", "M1", ["ui", "media"], "M", ["probe", "stills", "jobs", "app-shell"],
  summary="Get media into a project.",
  scope=[
      "Native file dialog (`rfd`), multi-select",
      "Drag and drop from the OS onto the window, bin or timeline",
      "Probe in the background; show errors as toasts",
      "Import is an undoable command; skip duplicates",
  ],
  ac=["Dropping 20 files keeps the UI responsive and adds them all to the bin"])

I("bin", "Media bin panel", "M1", ["ui"], "M", ["import"],
  summary="Browse, search and manage project media.",
  scope=[
      "Tabs: Media, Text, Effects (Text and Effects are placeholders until M5/M6)",
      "Grid and list views; thumbnail tiles with duration badges; audio tiles with a waveform icon",
      "Search filter and sort",
      "Rename, and remove from project with a warning when the asset is in use",
      "Import button",
  ],
  ac=["Matches the bin in the main editor prototype"],
  refs=[f"[01-editor.png (layout only; styling per DESIGN.md)]({PROTO}/01-editor.png)"])

I("thumbs", "Thumbnail generation and cache", "M1", ["media"], "M", ["video-decode", "jobs"],
  summary="Poster frames and filmstrips for the bin and timeline.",
  scope=[
      "Poster thumbnail and filmstrip thumbnails at fixed intervals",
      "Disk cache keyed by path, size and mtime; LRU size cap from settings",
  ],
  ac=["Re-opening a project shows thumbnails instantly from cache"])

I("waveforms", "Waveform peak generation and cache", "M1", ["audio"], "M", ["audio-decode", "jobs"],
  summary="Fast waveforms at any zoom level.",
  scope=["Multi-resolution min/max peak pyramid", "Disk cache alongside thumbnails", "Query API returning peaks for any time range and pixel width"],
  ac=["Drawing a 1-hour waveform at any zoom takes under 1 ms per frame"])

I("preview-gpu", "GPU frame upload and preview viewport", "M1", ["render"], "M", ["video-decode", "app-shell"],
  summary="Show decoded frames in the Preview panel with correct color.",
  scope=[
      "Upload YUV planes to wgpu textures and convert in a shader with the correct BT.601/BT.709 matrix and range",
      "Display inside egui via a paint callback",
      "Fit/zoom with letterboxing; preview scale setting (full, ½, ¼)",
  ],
  ac=["Color bars from the corpus match reference values within tolerance"])

I("audio-out", "Audio output and master clock", "M1", ["audio"], "M", ["audio-decode"],
  summary="Play audio and drive playback timing from the audio device.",
  scope=[
      "`cpal` output stream fed from a lock-free ring buffer",
      "Output device selection; survive device unplug and replug",
      "Master clock derived from samples played; wall-clock fallback when no device is present",
  ],
  ac=["Unplugging headphones mid-playback switches device without crashing"])

I("source-viewer", "Source viewer: playback, J/K/L and source in/out", "M1", ["ui"], "L", ["preview-gpu", "audio-out", "actions", "bin"],
  summary="Watch a bin clip and mark the part you want before it goes on the timeline.",
  scope=[
      "Double-clicking a bin clip opens it in the Preview panel's Source tab",
      "Play/pause, J/K/L shuttle, ←/→ frame step, Home/End",
      "I/O marks stored per asset; current and duration timecodes",
      "Transport row as in the prototype",
  ],
  ac=["Audio and video stay within one frame of sync over a 5-minute clip", "Source in/out survives save and reload"],
  refs=[f"[01-editor.png (layout only; styling per DESIGN.md)]({PROTO}/01-editor.png)", f"[PLAN.md · In / Out points]({PLAN}#in--out-points)"])

I("skim", "Hover-skim in the media bin", "M1", ["ui"], "S", ["thumbs", "bin"],
  summary="Moving the cursor across a thumbnail scrubs through the clip.",
  scope=["Scrub filmstrip thumbnails under the cursor", "Optional live decode when hovering for longer"],
  ac=["Skimming feels instant on cached clips"],
  labels=["good first issue"])

I("welcome", "Welcome screen and new-project flow", "M1", ["ui"], "M", ["app-shell", "config", "import"],
  summary="The start screen from the prototype.",
  scope=[
      "New Project and Open Project buttons",
      "Preset cards, with Match source first and selected by default: Match source, 16:9 · 1080p, 16:9 · 1440p, 16:9 · 4K, 9:16 · Vertical, 1:1 · Square",
      "Match source takes resolution and frame rate from the first clip added; the fixed presets offer 30 and 60 fps",
      "Recent projects with poster thumbnails and relative dates",
      "Drop zone: dropping media creates a Match source project, then imports everything",
  ],
  ac=[
      "Dropping a 1440p60 screen recording creates a 2560 × 1440, 60 fps project",
      "Dropping a vertical phone clip creates a 9:16 project at the clip's frame rate",
  ],
  refs=[f"[04-welcome.png (layout only; styling per DESIGN.md)]({PROTO}/04-welcome.png)"])

I("relink", "Missing media detection and relink", "M1", ["ui", "media"], "S", ["import", "serialize"],
  summary="Handle projects whose media has moved.",
  scope=[
      "Mark missing assets as offline on load and show a placeholder frame",
      "Relink dialog; after one relink, search the same folder for the other missing files",
  ],
  ac=["Relinking one file in a moved folder relinks the rest automatically"])

# ───────────────────────── M2 · Timeline ─────────────────────────

I("eval", "Timeline evaluation: time to layer stack", "M2", ["core"], "M", ["model"],
  summary="A pure function from sequence time to what should be seen and heard. Playback, export and the CLI all use it.",
  scope=[
      "For a time t: active video layers top to bottom, each with its source time and parameters",
      "For a time range: the audio segments to mix",
      "Respects hidden, muted and solo tracks",
  ],
  ac=["Covered by unit tests, including clips at exact cut points"])

I("compositor", "GPU compositor: layer stack with opacity", "M2", ["render"], "L", ["eval", "preview-gpu"],
  summary="Composite the layer stack at project resolution.",
  scope=[
      "Fit sources of any resolution or aspect into the project frame",
      "Opacity with normal blending over the project background color",
      "Render at preview scale for the viewer, and offscreen with readback for export",
      "Golden-image tests on a software adapter (lavapipe/WARP) so CI does not need a GPU",
  ],
  ac=["Golden tests pass on all three CI platforms"])

I("mixer", "Audio mixer: tracks, volume, pan, mute and solo", "M2", ["audio"], "M", ["eval", "audio-decode"],
  summary="Mix any number of audio tracks in real time.",
  scope=[
      "Per-clip gain, per-track volume and constant-power pan",
      "Mute and solo; master bus",
      "Peak meter data for the UI",
      "Click-free parameter changes (short ramps)",
  ],
  ac=["Mixing 16 stereo tracks uses under 5% of one core"])

I("playback", "Timeline playback engine", "M2", ["render", "audio"], "L", ["compositor", "mixer", "audio-out"],
  summary="Smooth, in-sync playback of the whole timeline.",
  scope=[
      "Decode-ahead for every clip about to become active",
      "Frame cache with a memory budget",
      "Sync video to the audio clock; drop frames rather than drift",
      "Play, pause, J/K/L, loop, preview scale",
  ],
  ac=[
      "A two-layer 1080p30 timeline plays at full rate on a mid-range laptop",
      "A/V stays within ±1 frame over 5 minutes",
  ])

I("timeline-widget", "Timeline panel: ruler, tracks, clips, zoom and playhead", "M2", ["ui", "timeline"], "L", ["app-shell", "model", "thumbs", "waveforms"],
  summary="The custom-painted timeline from the prototype.",
  scope=[
      "Ruler with timecode ticks that adapt to zoom; click or drag to seek",
      "Track rows; clips colored by type with names, filmstrips and waveforms",
      "Zoom (slider, Ctrl+wheel, `=`/`-`), horizontal scroll, draggable playhead",
      "Only visible clips are drawn",
  ],
  ac=["Stays at 60 fps with 500 clips on the timeline"],
  refs=[f"[01-editor.png (layout only; styling per DESIGN.md)]({PROTO}/01-editor.png)"])

I("tracks", "Track management and headers", "M2", ["timeline"], "M", ["timeline-widget", "undo"],
  summary="Add, arrange and control tracks.",
  scope=[
      "Add, remove, reorder and rename tracks",
      "Header controls: hide (eye), lock, mute (M), solo (S)",
      "Resizable track height",
      "Target-track selection for insert and overwrite",
  ],
  ac=["Locked tracks reject every edit", "All changes are undoable"])

I("place-clips", "Place clips: drag from bin, insert and overwrite", "M2", ["timeline"], "M", ["tracks", "source-viewer"],
  summary="Put clips on the timeline and move them around.",
  scope=[
      "Drag from the bin onto a track with a ghost preview",
      "`,` insert (ripple) and `.` overwrite from the source viewer's in/out at the playhead",
      "Files with audio create linked video and audio clips",
      "Drag clips within and between tracks; overlaps are overwritten",
  ],
  ac=["Insert and overwrite match Premiere's behaviour for the same source in/out and playhead"])

I("multi-audio", "Multi-track audio sources", "M2", ["timeline", "media"], "M", ["place-clips", "audio-decode"],
  summary="Recordings with several audio streams, such as OBS saving game audio, mic and voice chat separately.",
  scope=[
      "Each audio stream becomes its own audio clip on its own track, linked to the video clip",
      "Clips and tracks are named from stream metadata (e.g. \"Game\", \"Mic\"), falling back to \"Audio 1\", \"Audio 2\"",
      "Per-stream mute/solo and volume work through the normal track controls",
      "Option, per import or per clip, to merge all streams into a single clip",
  ],
  ac=[
      "The three-stream test MKV lands as one video clip and three linked audio clips on three tracks",
      "Trimming or moving the video clip moves all its linked audio clips",
  ])

I("select-clipboard", "Selection, clipboard and delete", "M2", ["timeline"], "M", ["place-clips"],
  summary="Select clips and move them through the clipboard.",
  scope=[
      "Click, Shift-click, Ctrl-click and marquee selection",
      "Copy, cut, paste at playhead on the target track, duplicate",
      "Delete (leave gap) and Shift+Delete (ripple delete)",
      "Linked selection toggle; link and unlink",
  ],
  ac=["Pasting linked clips keeps them linked"])

I("trim-split", "Trim, split, razor and ripple", "M2", ["timeline"], "L", ["select-clipboard"],
  summary="The core cutting tools.",
  scope=[
      "Drag clip edges to trim, limited by available media",
      "Ripple trim with a modifier",
      "Split at playhead (Ctrl+K) on selected or all tracks; razor tool (C) and select tool (V)",
      "Ripple delete gaps",
      "`Q`/`W`: ripple trim the previous/next edit to the playhead",
      "The viewer shows the trim frame while dragging",
  ],
  ac=["Every operation is a single undo step", "Q/W on a linked clip trims video and audio together"])

I("snapping", "Snapping", "M2", ["timeline"], "S", ["trim-split"],
  summary="Edits snap to things that matter.",
  scope=["Snap to playhead, clip edges, markers and in/out", "Toggle with `S`; visual snap indicator; pixel threshold"],
  ac=["Dragging a clip near the playhead snaps exactly onto it"],
  labels=["good first issue"])

I("edit-ui", "Edit menu, timeline toolbar and clip context menu", "M2", ["ui", "timeline"], "M", ["snapping"],
  summary="Every timeline action is reachable with the mouse, not only by shortcut.",
  scope=[
      "Edit menu in the app bar: undo, redo, cut, copy, paste, duplicate, delete, ripple delete, split at playhead, link/unlink, select all, each showing its live shortcut from the keymap",
      "Timeline toolbar: select (V) and razor (C) tools, snapping toggle (S), linked selection toggle, zoom in/out/fit, split at playhead; icons with tooltips naming the shortcut",
      "Right-click context menu on timeline clips: cut, copy, paste, duplicate, delete, ripple delete, split at playhead, link/unlink, enable/disable clip, reveal in media bin; acts on the selection (right-clicking an unselected clip selects it first)",
      "Right-click on empty lane space: paste, add track, close gap when on a gap",
      "Menu items are disabled when their action cannot apply",
  ],
  ac=["Every action in keymap ACTIONS that edits the timeline appears in at least one menu or the toolbar",
      "Context menu actions run the same commands as their shortcuts and undo identically"])

I("clip-enable", "Enable and disable clips", "M2", ["timeline", "render"], "S", ["edit-ui"],
  summary="Turn a clip off without deleting it.",
  scope=[
      "Clip enabled flag in jvid-core (serde default true), undoable toggle command",
      "Disabled clips are skipped by evaluation, so playback, compositor and export all ignore them",
      "Disabled clips draw dimmed on the timeline; toggle from the clip context menu and Shift+E",
  ],
  ac=["A disabled clip is absent from playback and export and reappears after undo"])

I("program-monitor", "Program monitor plays the timeline", "M2", ["ui", "render"], "M", ["playback", "timeline-widget"],
  summary="The Program tab shows and plays the composited timeline at the playhead.",
  scope=[
      "Wire the playback engine (playback.rs, currently dead code) into the app: the Program tab draws the composited frame at the timeline playhead, paused or playing",
      "Space / J/K/L / loop drive timeline playback when the timeline or Program tab has focus; the playhead follows the engine while playing",
      "Seeking (clicking the ruler, dragging the playhead, edits under the playhead) redraws the composited frame",
      "Replace the newest-asset placeholder in the Program tab; keep the trim-point frame while a clip edge is dragged",
      "Transform handles (#47) line up with the composited frame",
  ],
  ac=["A two-layer timeline with opacity/blend shows the composite in the Program tab and plays with audio",
      "Moving the playhead while paused updates the frame without playing"])

I("program-transport", "Program monitor transport bar", "M2", ["ui"], "S", ["program-monitor"],
  summary="The Program tab gets the same transport bar as the Source tab, driving timeline playback.",
  scope=[
      "Reuse the Source monitor's transport bar (timecode, go-to-start, step back, play/pause, step forward, go-to-end, duration, scrub strip) in the Program tab, wired to the timeline playback engine",
      "Timecode and duration show sequence time; the scrub strip moves the timeline playhead",
      "Keep the existing Fit / quality / Proxies / grid controls in the same row as the Source tab does",
  ],
  ac=["Program tab shows play/pause and step buttons that play and step the timeline",
      "Program tab timecode tracks the timeline playhead while playing and scrubbing"])

I("ui-polish-1", "UI polish from first user test", "M2", ["ui"], "S", ["program-monitor"],
  summary="Fixes from the owner's first hands-on test of the editor shell.",
  scope=[
      "Inspector panel scrolls vertically (ScrollArea) when its sections overflow",
      "Timeline toolbar actions (select, razor, snap, link, zoom...) are left-aligned in the timeline header",
      "Media / Effects / Text tabs in the bin share the full panel width",
      "Add a File menu left of Edit: New, Open, Open Recent, Save, Save As, Import Media, Export..., Quit, using existing actions and keymap shortcuts",
  ],
  ac=["Inspector with all sections expanded scrolls to its last field in a small window",
      "File > Export opens the export dialog; File > Save saves; shortcuts shown match the keymap",
      "Snapshot tests updated for the toolbar and bin tabs"])

I("text-to-timeline", "Text presets: feedback and drag to timeline", "M2", ["ui", "timeline"], "S", ["program-monitor"],
  summary="Text presets from the Text tab are visibly created and can be placed on the timeline like media.",
  scope=[
      "Clicking a Text preset gives clear feedback (switch to/select and scroll to the new asset, or place it at the playhead on the top free video track: pick one, note it under Decisions)",
      "Text assets drag from the bin onto the timeline exactly like media and solids (insert/overwrite, snapping), with a default duration",
      "Text presets themselves can be dragged straight from the Text tab onto the timeline",
  ],
  ac=["Dragging a Title preset onto V2 creates a text clip that renders in the Program monitor",
      "Clicking a preset shows a visible result without hunting through the Media tab"])

I("in-out", "In/Out points on the timeline", "M2", ["timeline"], "M", ["trim-split", "actions"],
  summary="Premiere-style range marking for playback, editing and export.",
  scope=[
      "`I`/`O` set; `Alt+I`/`Alt+O`/`Alt+X` clear; `Shift+I`/`Shift+O` jump",
      "`X` marks around the clip under the playhead; `/` marks around the selection",
      "Shaded band on the ruler and across tracks; draggable I/O flags",
      "Loop playback over the range",
      "`;` lift (leave a gap) and `'` extract (close the gap)",
  ],
  ac=["Every shortcut in the PLAN.md table works", "In/out is saved with the project"],
  refs=[f"[PLAN.md · In / Out points]({PLAN}#in--out-points)", f"[01-editor.png (layout only; styling per DESIGN.md)]({PROTO}/01-editor.png)"])

I("markers", "Markers", "M2", ["timeline"], "M", ["timeline-widget", "undo", "in-out"],
  summary="Mark moments on the timeline, including ranges you want to keep, while watching.",
  scope=[
      "`M` adds a marker at the playhead, during playback too, without stopping",
      "Ranged markers: a marker with a duration, created from the current in/out (or by `M` on press and release while playing)",
      "Name, color and note",
      "Jump to next/previous marker",
      "Markers are snap targets",
  ],
  ac=[
      "Pressing `M` repeatedly during playback drops markers without a pause or dropped frames",
      "Markers, including ranged markers, survive save and reload",
  ])

I("scrub", "Scrubbing and audio scrub", "M2", ["timeline", "render"], "M", ["playback"],
  summary="Low-latency scrubbing on the timeline.",
  scope=[
      "Dragging the playhead shows the nearest decodable frame first, then refines to the exact frame",
      "Audio scrub: short snippets while dragging (toggleable)",
  ],
  ac=["Scrubbing 1080p H.264 shows 30+ updates per second on a mid-range laptop"])

I("long-hfr", "Long and high-frame-rate recordings", "M2", ["media", "timeline"], "M", ["playback", "thumbs", "waveforms"],
  summary="Gameplay recordings can be hours long and recorded at 120 fps or more. Both need to feel as quick as a short clip.",
  scope=[
      "Progressive thumbnails and waveforms: the timeline is usable immediately and fills in as generation proceeds",
      "Timeline zoom that goes from the whole recording to single frames, with ruler ticks that stay readable at every level",
      "Fast seeks anywhere in a long file, without decoding from the start",
      "120, 144 and 240 fps sources conform to the project frame rate by timestamp, with no drift",
  ],
  ac=[
      "A 2-hour 1440p60 recording is editable within 2 seconds of import, and seeking to any point shows a frame in under 150 ms",
      "A 144 fps clip in a 60 fps project plays in sync and exports at 60 fps with no drift over its full length",
  ])

I("perf-harness", "Performance harness and budgets", "M2", ["infra"], "M", ["playback"],
  summary="Measure the performance targets so regressions are visible.",
  scope=[
      "Benchmarks: cold start, idle memory, seek latency, playback fps over corpus timelines",
      "`criterion` benches for core hot paths (evaluation, keyframes)",
      "A non-blocking CI job that reports numbers on each PR",
  ],
  ac=["Benchmarks for every target in PLAN.md · Performance targets"],
  refs=[f"[PLAN.md · Performance targets]({PLAN}#performance-targets)"])

# ───────────────────────── M3 · Animate ─────────────────────────

I("keyframes", "Keyframe model and interpolation", "M3", ["core"], "M", ["model", "undo"],
  summary="Any numeric property can be animated.",
  scope=[
      "Animatable values: scalar, 2D vector, color",
      "Keyframes stored relative to the clip, so they move with it",
      "Interpolation: linear, ease in, ease out, ease in/out, hold, cubic bezier",
      "Add, remove and move keyframes as commands",
  ],
  ac=["Bezier evaluation is monotonic in time", "Trimming a clip keeps the remaining keyframes in place"])

I("inspector", "Inspector panel", "M3", ["ui"], "L", ["app-shell", "keyframes", "select-clipboard"],
  summary="Edit the selected clip's properties.",
  scope=[
      "Sections: Transform, Opacity, Crop, Color (Color is a placeholder until M5)",
      "Numeric drag fields; diamond toggle to animate a property",
      "Add/remove keyframe at playhead; previous/next keyframe buttons; reset per property",
      "Mixed-value display when several clips are selected",
  ],
  ac=["Matches the inspector in the main editor prototype", "Every change is undoable"],
  refs=[f"[01-editor.png (layout only; styling per DESIGN.md)]({PROTO}/01-editor.png)"])

I("transform", "Transform, crop and opacity in the compositor", "M3", ["render"], "M", ["compositor", "keyframes"],
  summary="Apply keyframed clip properties when compositing.",
  scope=[
      "Position, scale (linked or separate X/Y), rotation, anchor point",
      "Crop left/top/right/bottom",
      "Opacity",
      "Evaluated per frame in project pixel space",
  ],
  ac=["Golden-image tests for each property, static and animated"])

I("blend", "Blend modes", "M3", ["render"], "S", ["transform", "inspector"],
  summary="Layer blend modes.",
  scope=["Normal, Multiply, Screen, Overlay, Add, Darken, Lighten, Difference", "Blend dropdown in the inspector"],
  ac=["A golden-image test for each mode"],
  labels=["good first issue"])

I("gizmo", "On-preview transform handles", "M3", ["ui", "render"], "M", ["transform", "inspector"],
  summary="Move, scale and rotate directly in the preview.",
  scope=[
      "Bounding box with corner and edge handles, a rotation handle and an anchor crosshair",
      "Drag to move; Shift constrains",
      "Creates keyframes automatically when the property is animated",
  ],
  ac=["Matches the keyframe editing prototype", "A whole drag is one undo step"],
  refs=[f"[02-keyframes.png (layout only; styling per DESIGN.md)]({PROTO}/02-keyframes.png)"])

I("keyframe-lanes", "Keyframe lanes on the timeline", "M3", ["timeline"], "L", ["keyframes", "timeline-widget", "inspector"],
  summary="See and edit keyframes under each clip.",
  scope=[
      "Expand a clip to show one lane per animated property",
      "Drag keyframes (with snapping), marquee select, delete",
      "Easing from a context menu",
      "Copy and paste keyframes relative to the playhead",
  ],
  ac=["Matches the keyframe lanes in the keyframe editing prototype"],
  refs=[f"[02-keyframes.png (layout only; styling per DESIGN.md)]({PROTO}/02-keyframes.png)"])

I("easing-ui", "Easing picker and bezier curve editor", "M3", ["ui"], "M", ["keyframes", "inspector"],
  summary="Pick and shape easing for selected keyframes.",
  scope=[
      "Segmented control: Linear, Ease In, Ease Out, Ease In/Out, Hold, Bezier",
      "Small curve widget with draggable bezier handles",
      "Applies to all selected keyframes",
  ],
  ac=["Editing the curve updates playback immediately"],
  refs=[f"[02-keyframes.png (layout only; styling per DESIGN.md)]({PROTO}/02-keyframes.png)"])

I("audio-keys", "Volume and pan keyframes with rubber band", "M3", ["audio", "timeline"], "M", ["keyframes", "mixer", "timeline-widget"],
  summary="Automate audio directly on the timeline.",
  scope=[
      "Clip gain keyframes drawn as a rubber-band line on audio clips; Ctrl+click adds a point, drag moves it",
      "Pan keyframes in the inspector",
      "Sample-accurate gain ramps in the mixer",
  ],
  ac=["A volume ramp renders without zipper noise"],
  refs=[f"[02-keyframes.png (layout only; styling per DESIGN.md)]({PROTO}/02-keyframes.png)"])

I("fades", "Clip fade handles", "M3", ["timeline"], "S", ["audio-keys", "transform"],
  summary="Drag a handle on a clip's corner to fade it in or out.",
  scope=["Audio fades use gain; video fades use opacity", "Linear and equal-power curves"],
  ac=["Fades are undoable and survive trims"])

# ───────────────────────── M4 · Export & v0.1.0 ─────────────────────────

I("encode", "Export pipeline: render, encode and mux", "M4", ["export", "media"], "L", ["compositor", "mixer", "transform"],
  summary="Render the timeline offline and write a finished file.",
  scope=[
      "Render each frame at project resolution with double-buffered GPU readback",
      "Audio mixdown",
      "FFmpeg encode and mux to MP4, MOV and MKV",
      "Any time range; progress reporting; cancel; clean up partial files",
      "Re-probe the output to validate it",
  ],
  ac=["Output duration matches the range within one frame", "A/V sync within one frame from first to last frame"])

I("presets", "Export presets", "M4", ["export"], "M", ["encode"],
  summary="One click to the right settings.",
  scope=[
      "Built-in, in this order: Highlight 1080p60, Highlight 1440p60, Screen recording (sharp text), YouTube 1080p, YouTube 4K, Shorts/Reels 9:16, ProRes master, GIF, audio only (WAV, MP3, AAC)",
      "Screen recording (sharp text) keeps source resolution and uses encoder settings that keep small text crisp",
      "User presets as TOML in the config directory",
      "Warn when a preset's aspect ratio doesn't match the project",
  ],
  ac=[
      "Each built-in preset produces a file that plays in VLC and uploads to YouTube",
      "Text in the test screen recording stays legible at 100% zoom after a Screen recording export",
  ])

I("export-dialog", "Export dialog with range selection", "M4", ["ui", "export"], "M", ["presets", "in-out", "select-clipboard"],
  summary="The export dialog from the prototype, including In to Out.",
  scope=[
      "Preset list",
      "Range: Entire Timeline, In to Out, Selected Clips. In to Out is the default when a range is set",
      "In, Out and Duration timecodes with in/out frame thumbnails",
      "Video and audio settings; output path with folder picker; estimated size",
      "Cancel, Add to Queue, Export",
  ],
  ac=["Matches the export prototype", "Exporting In to Out produces exactly the marked range"],
  refs=[f"[03-export.png (layout only; styling per DESIGN.md)]({PROTO}/03-export.png)", f"[PLAN.md · In / Out points]({PLAN}#in--out-points)"])

I("hw-encode", "Hardware encoding with software fallback", "M4", ["export", "media"], "M", ["encode"],
  summary="Use the GPU's encoder when it's available.",
  scope=[
      "Detect NVENC, QSV, VideoToolbox, AMF and VAAPI by test-opening each encoder at startup",
      "Auto picks the best available; software fallback (x264 in GPL builds, OpenH264 in permissive builds)",
      "Map the quality slider to each encoder's rate control",
  ],
  ac=["Export works on a machine with no hardware encoder", "Hardware export is at least 3× faster than software on supported hardware"])

I("vaapi-encode", "VAAPI hardware encoding on Linux", None, ["export", "media"], "S", ["hw-encode"],
  summary="Hardware H.264/HEVC export on Linux via VAAPI.",
  scope=[
      "VAAPI encoders need GPU hardware frames, which ffmpeg-next has no safe API for; needs a small audited unsafe block (ADR 0001 exception) or an upstream safe API",
      "Same probe/fallback path as the other hardware encoders",
  ],
  ac=["On a Linux machine with VAAPI, export uses h264_vaapi and falls back to software when it fails"])

I("audio-export", "Audio-only export", "M4", ["export", "audio"], "S", ["encode"],
  summary="Export just the mix.",
  scope=["WAV (16/24-bit PCM), MP3, AAC", "Honours the selected range"],
  ac=["Exported WAV is sample-identical to the mixer output"],
  labels=["good first issue"])

I("loudness", "Loudness normalization (EBU R128)", "M4", ["audio", "export"], "M", ["encode"],
  summary="Hit a loudness target on export.",
  scope=[
      "Measure integrated loudness in a first pass (`ebur128` crate)",
      "Apply gain and a true-peak limiter at −1 dBTP",
      "Targets: −14 LUFS (default), −16, −23, custom",
  ],
  ac=["Exports land within ±0.5 LU of the target"])

I("queue", "Background render queue", "M4", ["export", "ui"], "M", ["export-dialog", "jobs"],
  summary="Keep editing while exports run.",
  scope=[
      "Queue with progress, cancel and open-folder on completion",
      "Each job renders from a snapshot of the project, so later edits don't affect it",
      "System notification when a job finishes",
  ],
  ac=["Editing during an export doesn't change the exported file"])

I("frame-png", "Export current frame as PNG", "M4", ["export"], "S", ["compositor"],
  summary="Save the frame under the playhead.",
  scope=["Action with a default shortcut", "Full project resolution"],
  ac=["The PNG matches the composited frame exactly"],
  labels=["good first issue"])

I("gif", "GIF export", "M4", ["export"], "S", ["encode"],
  summary="Good-looking GIFs.",
  scope=["FFmpeg palettegen/paletteuse", "Frame rate, width and loop options"],
  ac=["A 5-second 480p GIF has no visible banding on the test clip"])

I("lossless-export", "Lossless fast export", "M4", ["export", "media"], "L", ["encode", "export-dialog"],
  summary="When all you did was cut, copy the original video instead of re-encoding it: no quality loss and close to disk speed.",
  scope=[
      "Available when the range contains only cuts: no transforms, effects, titles, speed changes or audio changes, and all clips share one source format",
      "Stream copy into the same container, with cut points snapped to the nearest keyframe",
      "Smart render option: re-encode only the GOPs at each cut edge so cuts land on the exact frame",
      "The export dialog shows whether lossless is available, and if not, which clips or changes prevent it",
  ],
  ac=[
      "Exporting a 1-minute cut from the 2-hour test recording takes under 3 seconds",
      "With smart render, the output starts and ends on the exact marked frames",
  ])

I("target-size", "Export to a target file size", "M4", ["export"], "M", ["encode", "presets"],
  summary="Hit an upload limit without trial and error.",
  scope=[
      "Two-pass encode with the video bitrate worked out from duration, audio bitrate and container overhead",
      "Quick picks for common upload limits: 10, 25, 50, 100 and 500 MB, plus a custom size",
      "Warn when the target is too small for the chosen resolution, and suggest a lower one",
  ],
  ac=["Outputs land under the target and within about 3% of it, across the test corpus"])

I("cli", "jvid render CLI", "M4", ["cli"], "M", ["encode", "presets", "serialize"],
  summary="Render projects without the UI.",
  scope=[
      "`jvid render project.jvid -o out.mp4 [--preset NAME] [--in TC --out TC | --range in-out]`",
      "Progress on stderr, `--json` progress for automation, meaningful exit codes",
      "Headless wgpu with a software adapter fallback",
  ],
  ac=["The CLI and the app produce identical output for the same project and range"])

I("text", "Text and title layers", "M5", ["render", "ui"], "L", ["transform", "inspector", "bin"],
  summary="Titles, captions and lower thirds.",
  scope=[
      "Shaping and rasterization with `cosmic-text`",
      "Font family, size, weight, color, alignment, outline, shadow, background box",
      "All properties animatable",
      "Edit in the inspector, or double-click in the preview",
      "Text tab in the bin with presets: Title, Lower third, Caption",
  ],
  ac=["Non-Latin scripts (Arabic, CJK) shape correctly"])

I("solids", "Solid color layers", "M5", ["render"], "S", ["transform"],
  summary="Plain color layers for backgrounds and fades.",
  scope=["Color picker in the inspector", "Animatable color"],
  ac=["Solid layers export at the exact color"],
  labels=["good first issue"])

I("transitions", "Transitions: crossfade and dip to black/white", "M5", ["timeline", "render"], "M", ["keyframes", "transform", "audio-keys"],
  summary="Simple transitions at cuts.",
  scope=[
      "Apply at a cut by dragging from the Effects tab or with a default shortcut",
      "Duration handle; uses media handles on both sides",
      "Audio crossfade (equal power)",
  ],
  ac=["A transition with no available handles shows a clear message instead of failing silently"])

I("color", "Color adjustments", "M5", ["render"], "M", ["transform", "inspector"],
  summary="Basic correction.",
  scope=["Exposure, contrast, saturation, temperature and tint as a shader pass", "Keyframeable", "Done in linear light"],
  ac=["Neutral settings leave the image bit-identical"])

I("lut", ".cube LUT support", "M5", ["render"], "S", ["color"],
  summary="Apply creative or technical LUTs.",
  scope=["Load 1D and 3D `.cube` files", "3D texture lookup with trilinear filtering", "Intensity slider"],
  ac=["Output matches a reference LUT implementation within tolerance"])

I("speed", "Clip speed, reverse and freeze frame", "M5", ["core", "timeline"], "M", ["eval", "trim-split", "mixer"],
  summary="Change how fast a clip plays.",
  scope=[
      "Constant speed percentage, with optional ripple",
      "Reverse",
      "Freeze frame at playhead",
      "Audio: resampled at non-100% speed (pitch-preserving stretch is a follow-up)",
  ],
  ac=["A clip at 200% has exactly half its duration"])

I("proxies", "Proxy generation and switching", "M5", ["media"], "M", ["jobs", "video-decode"],
  summary="Smooth editing of 4K and HEVC on modest hardware.",
  scope=[
      "Generate low-res, intra-frame proxies in the background",
      "Toggle proxy playback; export always uses the originals",
      "Suggest proxies automatically for 4K and HEVC sources",
  ],
  ac=["4K HEVC scrubs as smoothly as 1080p H.264 with proxies on"])

I("hw-decode", "Hardware video decoding", "M5", ["media", "render"], "L", ["video-decode", "preview-gpu"],
  summary="Decode on the GPU where possible.",
  scope=[
      "FFmpeg hwaccel: D3D11VA, VAAPI, VideoToolbox, NVDEC",
      "Software fallback; a setting to turn it off",
      "Zero-copy into wgpu where feasible, otherwise a fast download path",
  ],
  ac=["Hardware decode is used for H.264/HEVC on all three platforms when available"])

I("autosave", "Autosave and crash recovery", "M5", ["core"], "M", ["serialize", "logging"],
  summary="Never lose work.",
  scope=[
      "Periodic autosave to a sidecar file, keeping the last N versions",
      "Save on panic via the crash hook",
      "On startup, offer to restore a newer autosave",
  ],
  ac=["Killing the process mid-edit loses at most one autosave interval of work"])

I("keymap-editor", "Keyboard shortcut editor", "M5", ["ui"], "M", ["actions"],
  summary="Remap shortcuts without editing TOML.",
  scope=["Searchable action list", "Record a shortcut; show conflicts; reset", "Import and export keymaps"],
  ac=["Changes apply immediately without restarting"])

I("themes", "Themes", "M5", ["suite-ui"], "S", ["suite-ui-theme"],
  summary="Custom color themes.",
  scope=["Theme files in TOML using the `DESIGN.md` tokens", "Built-in dark, light and high-contrast", "Live reload"],
  ac=["A theme file change shows up without restarting"])

I("overlays", "Preview overlays: safe areas and grid", "M5", ["ui"], "S", ["preview-gpu"],
  summary="Framing guides in the preview.",
  scope=["Title-safe and action-safe", "Rule-of-thirds grid and center cross", "Aspect guides (e.g. 9:16 inside 16:9)"],
  ac=["Overlays never appear in exports"],
  labels=["good first issue"])

I("project-settings", "Project settings dialog", "M5", ["ui"], "S", ["welcome"],
  summary="Change project settings after creation.",
  scope=["Resolution, frame rate, aspect, background color", "Warn about how existing clips will be refit"],
  ac=["Changing settings is undoable"])

I("user-docs", "User guide and shortcut cheat sheet", "M5", ["docs"], "M", ["export-dialog"],
  summary="Docs for people using jvid.",
  scope=["Getting started, editing basics, keyframes, export", "Keyboard shortcut cheat sheet generated from the action registry", "Building from source"],
  ac=["The cheat sheet always matches the default keymap"])

I("batch-highlights", "Export marked highlights as separate files", "M5", ["export", "timeline"], "M", ["markers", "queue"],
  summary="Mark several highlights while watching a recording, then export each one as its own file in one go.",
  scope=[
      "Choose which ranged markers to export, or all of them",
      "Filename template, e.g. `{project}-{marker}-{index}`",
      "One render-queue job per highlight, using the selected preset",
  ],
  ac=["Ten ranged markers on the 2-hour test recording export as ten correctly trimmed files"])

I("zoom-region", "Zoom-to-region for screen recordings", "M5", ["ui", "render"], "M", ["gizmo", "keyframes", "easing-ui"],
  summary="Draw a rectangle on the preview to zoom in on part of the screen, hold, then zoom back out.",
  scope=[
      "Draw the target region on the preview; jvid creates eased scale and position keyframes for zoom in, hold and zoom out",
      "Duration and easing presets (e.g. quick, smooth), editable afterwards like any keyframes",
      "Region keeps the project aspect ratio so nothing letterboxes",
  ],
  ac=["A zoom is one undo step and its keyframes show in the clip's keyframe lanes"])

# ───────────────────────── M6 · Extend (v0.3.0) ─────────────────────────

I("wgsl", "WGSL shader effects", "M6", ["render", "ui"], "L", ["color", "inspector", "keyframes"],
  summary="Users write effects as a single `.wgsl` file.",
  scope=[
      "Effects folder in the config directory; effects listed in the bin's Effects tab",
      "Header parsing (`@jvid effect`, `@param name type range = default`)",
      "Validate with `naga` and show readable errors",
      "Inspector UI generated from params; params are keyframeable; effects stack in order",
      "Hot reload on file change",
      "Bundled examples: vignette, chromatic aberration, film grain; an authoring guide",
  ],
  ac=["Dropping a new `.wgsl` file in the folder makes it usable without restarting"],
  refs=[f"[PLAN.md · Customization]({PLAN}#customization-wgsl-shader-effects)"])

I("srt", "SRT subtitles: import, burn-in and sidecar", "M6", ["export", "render"], "M", ["text", "encode"],
  summary="Subtitles in and out.",
  scope=[
      "Import `.srt` onto a subtitle track as editable text clips",
      "Subtitle style settings",
      "On export: burn in, or write an `.srt` sidecar timed to the export range",
  ],
  ac=["An exported sidecar lines up with an In to Out export"])

# ───────────────────────── Later ─────────────────────────

for key, title, summary in [
    ("chroma-key", "Chroma key", "Green and blue screen keying with spill suppression."),
    ("masks", "Masks", "Rectangle, ellipse and bezier masks with feather, animatable."),
    ("scopes", "Video scopes", "Waveform, vectorscope and histogram."),
    ("speed-ramps", "Speed ramps", "Keyframed speed with smooth ramps."),
    ("nested", "Nested sequences", "Use a sequence as a clip in another sequence."),
    ("voiceover", "Voiceover recording", "Record from a microphone straight onto an audio track."),
    ("audio-fx", "Audio EQ and compressor", "Basic per-clip and per-track audio effects."),
    ("hdr", "HDR support", "PQ/HLG sources, tone mapping and HDR export."),
    ("graph-editor", "Curve graph editor", "A full graph editor for keyframe curves."),
    ("silence-detect", "Silence detection to cut dead air", "Find silent stretches in screen recordings and offer to cut or speed them up."),
]:
    I(key, title, None, [], None, summary=summary, labels=["later"])

I("kf-discover", "Inspector gutter and keyframe lane discoverability", "M3", ["ui", "timeline"], "S", ["keyframe-lanes"],
  summary="Fixes from the owner's second hands-on test: cramped inspector buttons and hidden keyframe lane toggle.",
  scope=[
      "Inspector: leave a gutter between the vertical scrollbar and the row buttons (prev/add/next key, reset) so the keyframe diamond is easy to hit; buttons get a comfortable hit size",
      "Keyframe lane toggle: today it sits at the clip's right end of the name strip, offscreen for long clips. Keep it visible: pin it to the visible left edge of the clip name strip (clamped to the viewport)",
      "Selecting a clip that has keyframes, or adding a keyframe in the inspector, expands its lanes automatically (toggle still collapses)",
      "Clips with keyframes show small diamond ticks on the clip body at key times even when collapsed",
  ],
  ac=["The keyframe diamond in the Inspector does not touch the scrollbar and is clickable at its edge",
      "Adding a keyframe in the Inspector to a long clip shows its lane in the timeline without scrolling",
      "kittests cover the auto-expand and the toggle visible when the clip extends past the right edge"])

I("text-solid-fixes", "Text and solid clips: source view, live edit, fit-to-text bounds", "M3", ["ui", "render"], "M", ["keyframe-lanes"],
  summary="Fixes from the owner's third hands-on test of text and solid clips.",
  scope=[
      "Double-clicking a solid or text asset in the bin opens it in the Source tab without 'Can't show this frame: invalid argument' (render the generated frame instead of decoding a file)",
      "Editing text properties in the Inspector redraws the Program frame immediately while paused (today it only updates after play or a seek): invalidate the composited/text raster cache on SetClipProps",
      "Text clip transform box and hit area fit the rendered text bounds, not the full frame",
      "Double-click on text in the Program preview enters in-place text edit (existing double_clicked_text in handles.rs/app.rs: make it actually trigger, e.g. the full-frame transform handles currently swallow it)",
  ],
  ac=["Double-click a Title in the bin: the Source tab shows it, no error",
      "Typing in the Inspector text field updates the Program monitor on each keystroke while paused",
      "Selecting a text clip draws handles around the text only; double-click on the text starts editing",
      "kittests cover live update and fit-to-text bounds"])

I("title-case", "Title Case pass on all UI labels", "M3", ["ui", "suite-ui"], "S", ["text-solid-fixes", "kf-discover"],
  summary="Owner preference: every UI label, button, menu item, tab, section header and property name uses Title Case.",
  scope=[
      "Sweep every user-visible string in jvid-app and suite-ui to Title Case: buttons ('Add Effect'), menus, tabs, section headers, property labels, tooltips' titles, dialog titles",
      "Effect parameter names from WGSL headers are displayed in Title Case (amount -> Amount) without changing the stored parameter ids",
      "Update DESIGN.md's copy rules to say Title Case, and update kittest snapshots",
      "Leave sentence-style text (descriptions, error messages, help paragraphs) as sentences",
      "Shorten verbose button labels: 'Import media' -> 'Import', 'New solid'/'Add solid' -> 'Add Solid'",
  ],
  ac=["No lowercase-first labels remain in the editor, inspector, menus or dialogs",
      "Bundled effects show 'Amount', 'Strength', 'Softness', 'Grain'; existing .jvid projects still load"])

I("effects-pane", "Effects pane: browse and apply effects", "M6", ["ui"], "M", ["title-case"],
  summary="The Effects tab is a bare name list; make it a browser that says what each effect does and how to use it.",
  scope=[
      "Effect entries as cards/rows like the Media tab: name, one-line description (from the WGSL header), status (bundled / user / compile error with message)",
      "Drag an effect onto a timeline clip, or click 'Apply' / double-click to add it to the selected clip(s); empty-state hint explains this",
      "'Open Effects Folder' and 'New Effect' (copies a template .wgsl) move to a bottom bar like the Media tab's Import / Add Solid",
      "Per-effect context menu: Reveal File, Duplicate, Delete (user effects only, with confirm), Disable (hidden from Add Effect)",
      "Search box like the Media tab",
  ],
  ac=["Dragging Vignette onto a clip adds it in the Inspector",
      "A broken user shader shows its error in the pane",
      "Bottom bar matches the Media tab layout"])

I("render-queue-menu", "Render queue in the Export button menu", "M5", ["ui", "export"], "S", ["title-case"],
  summary="Replace the floating Render Queue window with a menu under the top-right Export button.",
  scope=[
      "Remove the floating, collapsible Render Queue window",
      "While jobs exist, the top-right Export button shows progress (e.g. 'Exporting 35%') and gets a dropdown arrow; clicking the arrow opens a menu listing queued/running/finished jobs: name, progress bar, status, and per-row Cancel (right-aligned, danger/red style) or Reveal for finished jobs",
      "Menu footer: 'Cancel All' (danger style, confirm when more than one running) and 'Clear Finished'; 'New Export...' opens the export dialog",
      "Finished/failed jobs keep the existing completion notification",
  ],
  ac=["No floating render queue window remains",
      "Two queued exports: the Export menu lists both; Cancel on one leaves the other; Cancel All stops both",
      "Cancel buttons are right-aligned and use the danger colour"])

I("timeline-arrow-nav", "Timeline arrow-key navigation with modifiers", "M5", ["ui", "timeline"], "S", ["title-case"],
  summary="Arrow keys move the playhead with modifier steps, configurable in the keymap.",
  scope=[
      "Keep Left/Right = one frame; Shift+Left/Right = one second (or 10 frames, pick and note)",
      "Ctrl+Left/Right (Cmd on macOS) = previous/next edit point on any track (clip start or end); Ctrl+Shift+Left/Right = start/end of the clip under the playhead on the selected or topmost track",
      "Up/Down = previous/next edit point (Premiere default) if not already bound; Home/End = sequence start/end",
      "All bindings are keymap actions, editable in the keymap editor and listed in docs/shortcuts.md; no conflicts with existing bindings",
  ],
  ac=["Each binding moves the playhead to the expected frame (kittest per binding)",
      "Bindings appear in the keymap editor and can be rebound"])

I("gizmo-clip", "Transform handles drawn over the transport bar", "M3", ["ui"], "S", ["text-solid-fixes"],
  summary="When a clip is scaled past the frame, its transform box and handles draw over the monitor's transport bar.",
  scope=[
      "Clip the transform gizmo painter and its hit areas to the video viewport rect, so nothing draws over or steals clicks from the transport bar, tabs or other panels",
      "Off-frame handles stay reachable: when the box extends past the viewport, draw an edge indicator or allow zooming out (Fit already exists); note the choice",
  ],
  ac=["Scale a clip to 150%: no handle or outline draws over the transport bar, and the play button stays clickable",
      "kittest: play button click works with an oversized transformed clip selected"])

I("in-app-dialogs", "In-app confirm dialogs", "M4", ["ui", "suite-ui"], "M", ["title-case"],
  summary="Replace native rfd message boxes with suite-ui modals that match DESIGN.md.",
  scope=[
      "A suite-ui modal (dimmed backdrop, title, body, right-aligned buttons, danger style for destructive actions, Enter/Esc keyboard handling, focus trap)",
      "Replace every rfd::MessageDialog use: unsaved changes on close/new/open (Save / Don't Save / Cancel), errors, overwrite prompts, delete confirmations, render Cancel All",
      "Native file pickers (rfd::FileDialog) stay native",
  ],
  ac=["No rfd::MessageDialog remains (grep)",
      "Closing with unsaved changes shows the in-app modal; each button does the right thing (kittest)",
      "Modal is keyboard-operable: Enter = default, Esc = Cancel"])

I("i18n", "Localisation (i18n) support", "M5", ["ui", "suite-ui"], "M", ["in-app-dialogs", "title-case"],
  summary="Move every user-visible string into translation files so jvid can ship in multiple languages.",
  scope=[
      "Pick a small, well-maintained Rust i18n approach (e.g. fluent / rust-i18n); new dependency: ask the orchestrator before adding",
      "Extract all user-visible strings in jvid-app and suite-ui into an English source file; no hard-coded labels remain",
      "Language picker in Settings (default: system locale, fallback English); switching applies without restart if cheap, otherwise on restart",
      "Fonts cover the shipped languages' glyphs (note CJK font size cost before adding)",
      "Ship English plus one second language as proof (e.g. machine-reviewed Spanish), with a docs/translating.md guide",
  ],
  ac=["Switching to the second language translates menus, inspector, dialogs and the export dialog",
      "A test fails if a new UI string bypasses the translation layer (e.g. key-coverage test)"])

I("canvas-text-edit", "On-canvas text editing in the Program monitor", "M3", ["ui", "render"], "M", ["gizmo-clip", "title-case"],
  summary="Double-clicking a text clip in the Program monitor edits its text in place, on the canvas.",
  scope=[
      "Double-click a text clip in the Program view enters edit mode: a caret appears in the rendered text at the click position, respecting the clip's transform (position, scale, rotation)",
      "Typing, Backspace/Delete, arrows, Home/End, Shift-selection, Ctrl+A, copy/paste and Enter (newline) work; the frame redraws live with the real text renderer",
      "Esc or clicking outside commits; the whole edit session is one undo step (SetClipProps); transform handles hide while editing",
      "Playback shortcuts (Space, J/K/L, arrows) are suppressed while editing",
  ],
  ac=["Double-click 'Title' on a rotated text clip, type, see it update live, Esc commits, Ctrl+Z restores the old text in one step (kittest)",
      "Space types a space while editing instead of starting playback"])

I("font-picker", "Font picker with inline search", "M3", ["ui"], "S", ["title-case"],
  summary="The text clip Font field is a free text input; make it a searchable font select.",
  scope=[
      "Replace the Font text input with a combo: shows the current family (rendered in that font if cheap), opens a list of installed + bundled font families from the font database the text renderer already uses",
      "Typing filters the list inline (case-insensitive substring); arrow keys + Enter select; Esc cancels",
      "Hovering/arrowing previews the font on the canvas; Enter/click commits one undo step",
      "A missing font (project from another machine) shows as 'Arial (missing)' with fallback noted",
  ],
  ac=["Typing 'ari' narrows to Arial-matching families; Enter applies it (kittest)",
      "Only fonts the renderer can actually draw are listed"])

I("release", "Packaging and v0.1.0 release", "M4", ["infra"], "L", ["cli", "export-dialog", "hw-encode", "loudness", "lossless-export", "target-size", "title-case", "gizmo-clip", "canvas-text-edit", "font-picker", "in-app-dialogs", "effects-pane", "render-queue-menu", "timeline-arrow-nav"],
  summary="Installable builds for all three platforms.",
  scope=[
      "Windows zip/MSI with FFmpeg DLLs",
      "macOS `.app`/`.dmg` with bundled dylibs, signing and notarization",
      "Linux AppImage and tarball",
      "Bundle the GPL FFmpeg build with its license texts and a source offer",
      "Release workflow on tag; smoke-test every artifact",
  ],
  ac=["Fresh machines on all three OSes can install, edit and export without installing anything else"],
  refs=[f"[PLAN.md · Licensing]({PLAN}#licensing)"])

# ───────────────────────── M5 · Polish (v0.2.0) ─────────────────────────

# Owner-requested backlog; no release or milestone assignment yet.
I("edit-preferences", "Move application settings to Edit > Preferences", None, ["ui"], "S",
  summary="Make application preferences discoverable under Edit > Preferences instead of the title-bar project menu.",
  scope=[
      "Move the application Settings entry from the title-bar project-name menu into the existing Edit menu, labelled Preferences…",
      "Open the existing application preferences dialog, including the language picker; preserve live switching and saved preferences",
      "Keep Project Settings as a separate project action; translate the new menu label in all shipped languages",
  ],
  ac=["Edit > Preferences opens the application preferences dialog in English and Spanish",
      "The old application Settings entry is absent from the project-name menu, while Project Settings remains available",
      "Changing language from Preferences still applies live and persists"],
  labels=["later"])

I("drop-create-track", "Create a track when dropping assets outside existing tracks", None, ["ui", "timeline"], "M",
  summary="Dropping text, solids or other assets into empty timeline space should create a track above the existing one.",
  scope=[
      "Dragging an asset from the bin into timeline space outside an existing track row creates an appropriate new track above the existing track and places the clip at the dragged time",
      "Cover text, solid and other supported asset types using the existing media/track compatibility rules",
      "A drop directly onto a valid existing track keeps the existing placement behaviour and does not create a track",
      "Keep track creation and clip placement one undoable operation; cancelling or rejecting a drop leaves no empty track",
  ],
  ac=["With a video clip on an existing track, dropping a title or solid into empty space above it creates a track above it and places the new clip at the drop time",
      "Dropping the same asset directly onto a compatible existing track adds no track",
      "Undo removes the added clip and its new track together; redo restores both"],
  labels=["later"])

I("in-out-snap-edges", "Snap timeline In/Out flags to clip starts and ends", None, ["ui", "timeline"], "S",
  summary="The timeline In/Out flags currently do not snap to clip boundaries when dragged.",
  scope=[
      "Apply the timeline's existing snapping behaviour to dragged In and Out flags, with clip starts and ends as snap targets",
      "Respect the existing snapping toggle and pixel threshold, and show the existing snap indicator",
      "Preserve frame alignment, valid In/Out ordering and undo behaviour; keyboard I/O marking at the playhead keeps its existing meaning",
  ],
  ac=["With snapping enabled, dragging either In or Out within the snap threshold of a clip start or end lands exactly on that boundary",
      "With snapping disabled, or outside the threshold, dragging does not jump to a clip boundary",
      "Regression coverage exercises both flags, both boundary types and snapping enabled/disabled"],
  labels=["later"])

MOCK3 = f"[Inspector, Effects and Text mockup]({PROTO}/v3/inspector-effects-text.html)"

I("inspector-rows", "Inspector rows: one diamond, reset on change, sliders", None, ["ui"], "M",
  summary="Inspector rows are noisy and mostly empty: every row ends in the same reset and keyframe icons far from its value. Tidy them so values, changes and keyframes read at a glance.",
  scope=[
      "One keyframe diamond per row: a click starts animating; once animated it shows and toggles the key at the playhead. Stop Animating moves to its right-click menu beside easing",
      "Reset shows only when the value differs from its default, in a reserved slot so nothing shifts. Section heads do the same",
      "Previous and next keyframe arrows show only on animated rows, also in reserved slots",
      "Ranged values get a slider after the field; colour grades, Rotation and Pan fill from the centre. The field still drags and types",
      "Crop on one row (L, T, R, B); LUT Mix only once a LUT is set",
      "Speed row uses suite-ui's drag field and checkboxes instead of egui's DragValue",
  ],
  ac=["A clip with default values shows no reset buttons; changing Scale shows reset on that row and on Transform's head, and nothing else moves",
      "An unanimated row shows one diamond; clicking it adds a key at the playhead and shows the arrows",
      "Dragging a slider changes the value as one undo step",
      "Rows match the mockup"],
  refs=[MOCK3, f"[DESIGN.md]({DESIGN})"],
  labels=["later"])

I("inspector-effects", "Inspector effect stack: blocks, bypass, reorder, Add Effect menu", None, ["ui", "core", "render"], "M", ["inspector-rows"],
  summary="Effects in the Inspector are bare labels with Up, Down and Remove text buttons that appear and vanish, and Add Effect is a dropdown. Make each effect a block you can collapse, bypass and reorder.",
  scope=[
      "Each effect is a block: a header with collapse, bypass (eye), name, reset and a More menu (Move Up, Move Down, Reset, Remove), then its parameter rows",
      "Drag a block's header to reorder; menu items disable instead of disappearing",
      "Bypass: `enabled: bool` on `Effect`, default true. The compositor skips bypassed effects in preview, export and CLI render. Toggling is undoable",
      "Add Effect is a button opening a menu with a search field and each effect's name and description (thumbnails come with the Effects tab thumbnails issue)",
  ],
  ac=["Bypassing an effect removes it from the Program view and from an export, keeps its settings and keyframes, and undo restores it",
      "Bypass survives save and reopen",
      "Reordering by drag and by menu both change the render order and are undoable",
      "No buttons appear, vanish or move when the stack changes"],
  refs=[MOCK3],
  labels=["later"])

I("inspector-tabs", "Inspector tabs: Video, Audio and Effects", None, ["ui"], "S", ["inspector-effects"],
  summary="Split the Inspector into tabs so each one is short and effects are one click away (option B in the mockup).",
  scope=[
      "Tabs in the Inspector's header row in place of its title, styled like the bin's Media, Effects and Text tabs; no second header row",
      "Video (Transform, Opacity, Color, Speed), Audio (Audio) and Effects (the effect stack, with its count in the tab label); text clips add a Text tab first",
      "Tabs follow the selection: an audio-only selection shows Audio alone; the chosen tab is kept while it exists",
      "Sections inside a tab start open",
  ],
  ac=["A video clip with linked audio shows Video, Audio and Effects; a text clip shows Text first",
      "The Effects tab label shows how many effects the selection has",
      "The Inspector header stays on one line with the other panels' headers (DESIGN.md Layout checks)"],
  refs=[MOCK3, f"[DESIGN.md · Layout]({DESIGN}#layout)"],
  labels=["later"])

I("thumb-frame", "Thumbnail frame from the Program view", None, ["render", "ui"], "M",
  summary="Effect and text thumbnails need a picture to draw on: use the Program frame at the playhead, with a bundled fallback.",
  scope=[
      "A 160×90 copy of the composited Program frame at the playhead, kept on the GPU and shown through an egui texture",
      "Refreshed when the playhead settles or the Effects or Text tab opens; never during playback or scrubbing",
      "With nothing on the timeline, a bundled fallback frame (`docs/prototypes/v3/sample.jpg`, the owner's own footage), its source recorded in the assets README",
      "Run one effect, or draw one text preset, over that frame into a thumbnail texture",
  ],
  ac=["Playback and scrubbing do no thumbnail work while the Effects tab is open",
      "An empty project shows the fallback frame",
      "A test renders a bundled effect's thumbnail and checks it differs from the source frame"],
  refs=[MOCK3],
  labels=["later"])

I("effect-thumbs", "Effects tab thumbnails and a row layout that never shifts", None, ["ui"], "M", ["thumb-frame", "inspector-effects"],
  summary="In the Effects tab, hovering draws Apply over the description, and nothing shows what an effect does. Add thumbnails and fixed columns.",
  scope=[
      "Each row: a thumbnail (the effect run on the thumbnail frame), name, two-line description, then Apply (+) and More (⋯) in fixed columns. + is disabled with nothing selected; hover only changes the fill",
      "Bundled and My Effects groups replace the per-row tag; rows call out only Error (thumbnail dimmed with a warning) and Disabled",
      "Backticked parameter names in descriptions show as their labels in Ink 2",
      "Grid view behind the Media tab's list and grid toggle",
      "Effect names show in Title Case whatever the shader header says",
      "The Inspector's Add Effect menu reuses the thumbnails",
  ],
  ac=["Hovering a row changes nothing but its fill",
      "Each bundled effect's thumbnail visibly shows the effect on the current frame",
      "A broken user shader shows a dimmed thumbnail and its error"],
  refs=[MOCK3],
  labels=["later"])

I("text-tiles", "Text tab preset tiles", None, ["ui"], "S", ["thumb-frame"],
  summary="The Text tab is three plain buttons with no hint of what each makes. Show each preset on the current frame.",
  scope=[
      "Two-column tiles: each preset drawn on the thumbnail frame with the text renderer at its true size and position, then its name and a one-line description",
      "Click adds to the bin and drag goes onto the timeline, as now",
      "\"Lower third\" becomes \"Lower Third\" in the English catalog",
  ],
  ac=["Each tile shows its preset's text where it lands on the frame",
      "Clicking a tile adds the preset to the bin; dragging one onto the timeline places it"],
  refs=[MOCK3],
  labels=["later"])
