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
  refs=[f"[DESIGN.md · Layout]({DESIGN}#layout)"])

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
  refs=[f"[DESIGN.md · Layout]({DESIGN}#layout)"])

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
  refs=[f"[DESIGN.md · Layout]({DESIGN}#layout)", f"[PLAN.md · In / Out points]({PLAN}#in--out-points)"])

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
  refs=[f"[DESIGN.md · Layout]({DESIGN}#layout)"])

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
  refs=[f"[DESIGN.md · Layout]({DESIGN}#layout)"])

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
  refs=[f"[PLAN.md · In / Out points]({PLAN}#in--out-points)", f"[DESIGN.md · Layout]({DESIGN}#layout)"])

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
  refs=[f"[DESIGN.md · Layout]({DESIGN}#layout)"])

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
  refs=[f"[DESIGN.md · Layout]({DESIGN}#layout)"])

I("keyframe-lanes", "Keyframe lanes on the timeline", "M3", ["timeline"], "L", ["keyframes", "timeline-widget", "inspector"],
  summary="See and edit keyframes under each clip.",
  scope=[
      "Expand a clip to show one lane per animated property",
      "Drag keyframes (with snapping), marquee select, delete",
      "Easing from a context menu",
      "Copy and paste keyframes relative to the playhead",
  ],
  ac=["Matches the keyframe lanes in the keyframe editing prototype"],
  refs=[f"[DESIGN.md · Layout]({DESIGN}#layout)"])

I("easing-ui", "Easing picker and bezier curve editor", "M3", ["ui"], "M", ["keyframes", "inspector"],
  summary="Pick and shape easing for selected keyframes.",
  scope=[
      "Segmented control: Linear, Ease In, Ease Out, Ease In/Out, Hold, Bezier",
      "Small curve widget with draggable bezier handles",
      "Applies to all selected keyframes",
  ],
  ac=["Editing the curve updates playback immediately"],
  refs=[f"[DESIGN.md · Layout]({DESIGN}#layout)"])

I("audio-keys", "Volume and pan keyframes with rubber band", "M3", ["audio", "timeline"], "M", ["keyframes", "mixer", "timeline-widget"],
  summary="Automate audio directly on the timeline.",
  scope=[
      "Clip gain keyframes drawn as a rubber-band line on audio clips; Ctrl+click adds a point, drag moves it",
      "Pan keyframes in the inspector",
      "Sample-accurate gain ramps in the mixer",
  ],
  ac=["A volume ramp renders without zipper noise"],
  refs=[f"[DESIGN.md · Layout]({DESIGN}#layout)"])

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
  refs=[f"[DESIGN.md · Layout]({DESIGN}#layout)", f"[PLAN.md · In / Out points]({PLAN}#in--out-points)"])

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

# ───────────────────────── Later: scoped 2026-10-24 ─────────────────────────

I("chroma-key", "Chroma key", None, ["render"], "S", labels=["later"],
  summary="Green and blue screen keying with spill suppression.",
  scope=[
      "A bundled Chroma Key effect: key colour (green by default, blue preset or any hue), tolerance, edge softness and spill suppression",
      "Keyed pixels become transparent so lower tracks show through, in preview, export and CLI render like every bundled effect",
      "Parameters are animatable like other effect parameters",
  ],
  ac=["A green-screen frame keyed over a red lower track renders red where the green was and keeps the foreground",
      "Spill suppression measurably reduces the green cast on foreground edge pixels (render test)"])

I("masks", "Masks", None, ["core", "render", "ui"], "L", ["inspector-tabs"], labels=["later"],
  summary="Rectangle, ellipse and bezier masks with feather, animatable.",
  scope=[
      "Clips carry a list of masks: rectangle, ellipse or bezier path, each with feather, opacity, invert and add/subtract mode",
      "Mask shape and feather are keyframeable through the existing keyframe model",
      "Draw and edit masks with handles on the Program view; list and edit them in the Inspector",
      "Masks apply in preview, export and CLI render; older project files load unchanged",
  ],
  ac=["A feathered ellipse mask leaves the clip transparent outside the ellipse with a soft edge (render test)",
      "A keyframed rectangle mask animates between two keys",
      "Masks survive save and reopen, and every mask edit is one undo step"])

I("scopes", "Video scopes", None, ["render", "ui"], "M", labels=["later"],
  summary="Waveform, vectorscope and histogram.",
  scope=[
      "A Scopes panel showing luma waveform, vectorscope and RGB histogram of the Program frame",
      "Computed from a downscaled copy of the composited frame on the GPU or a small readback; updates when paused and at a reduced rate during playback",
      "Scope work never makes playback drop frames",
  ],
  ac=["A solid red frame puts the vectorscope trace at red and the histogram spike at red's code values (test)",
      "A black-to-white ramp draws a diagonal luma waveform (test)",
      "Playback with the Scopes panel open keeps the existing playback tests' frame budget"])

I("speed-ramps", "Speed ramps", None, ["core", "media", "ui"], "L", labels=["later"],
  summary="Keyframed speed with smooth ramps.",
  scope=[
      "Clip speed becomes keyframeable with the existing easing; the source time at any timeline time integrates the speed curve",
      "The clip's timeline length follows the integrated curve; Ripple keeps its current meaning",
      "Preview, export and audio follow the ramp (audio resampled to the speed; pitch preservation is a follow-up)",
      "The Inspector Speed row gets the keyframe diamond like other rows",
  ],
  ac=["A 100% to 200% linear ramp maps timeline time to the analytically expected source time within one frame (test)",
      "An export of a ramped clip has the expected duration and frame sequence",
      "Adding, moving and removing speed keys are each one undo step"])

I("nested", "Nested sequences", None, ["core", "render", "ui", "timeline"], "L", labels=["later"],
  summary="Use a sequence as a clip in another sequence.",
  scope=[
      "A project holds several sequences; the bin lists them and the Timeline switches between them",
      "Nest: replace selected clips with one clip that plays a new sequence holding them; sequences can also be dragged from the bin onto the timeline",
      "A sequence clip composites and mixes its sequence recursively in preview, export and CLI render; cycles are refused",
      "Older single-sequence project files load unchanged",
  ],
  ac=["A nested sequence renders the same frame as the clips it replaced (render test)",
      "Nesting a sequence inside itself, directly or indirectly, is refused",
      "Nest is one undo step, and nested projects survive save and reopen"])

I("voiceover", "Voiceover recording", None, ["audio", "ui"], "M", labels=["later"],
  summary="Record from a microphone straight onto an audio track.",
  scope=[
      "Arm an audio track and record from the selected input device (default device, choosable in Preferences) through cpal",
      "Audio is written as WAV into the project's media folder and placed on the armed track at the record start, as one undo step",
      "A level meter shows while recording; Stop or Space ends the take",
      "Missing device or permission errors show as a toast and leave the project unchanged",
  ],
  ac=["With a fake input feed, a recording writes a WAV of the fed samples and places a clip of that length at the record start (test)",
      "Undo removes the recorded clip; the WAV stays in the media folder",
      "Recording from a real microphone works on Windows (needs user testing)"])

I("audio-fx", "Audio EQ and compressor", None, ["audio", "core", "ui"], "M", ["inspector-tabs"], labels=["later"],
  summary="Basic per-clip and per-track audio effects.",
  scope=[
      "A three-band EQ (low shelf, peak, high shelf) and a compressor (threshold, ratio, attack, release, makeup) per clip and per track",
      "Applied in the playback mixer and in export, each bypassable; clip settings live in the Inspector's Audio tab, track settings on the track header",
      "Parameters are undoable and saved with the project",
  ],
  ac=["A +6 dB peak at 1 kHz raises a 1 kHz sine by 6 dB within 0.5 dB and leaves 100 Hz within 0.5 dB (DSP test)",
      "The compressor reduces a signal 12 dB over threshold at 4:1 to 3 dB over, after attack (DSP test)",
      "An export of a clip with EQ matches the playback mixer's output"])

I("hdr", "HDR support", None, ["media", "render"], "L", labels=["later"],
  summary="PQ/HLG sources, tone mapping and HDR export.",
  scope=[
      "Probe detects PQ and HLG (BT.2020) sources and the bin marks them",
      "HDR sources decode at 10 bits and tone-map to SDR BT.709 for preview and SDR export with one documented operator",
      "Project setting for HDR output: HEVC 10-bit PQ export with BT.2020 tags and mastering metadata",
  ],
  ac=["A generated PQ test clip probes as PQ BT.2020 10-bit",
      "Tone-mapped preview of the PQ clip stays within SDR range and keeps mid-grey near mid-grey (test)",
      "An HDR export probes as HEVC 10-bit PQ BT.2020; viewing on an HDR display needs user testing"])

I("graph-editor", "Curve graph editor", None, ["ui", "timeline"], "M", labels=["later"],
  summary="A full graph editor for keyframe curves.",
  scope=[
      "A graph view for the selected clip's animated properties: value over time with every key and its bezier handles",
      "Select, drag and box-select keys; drag handles to shape the easing; frame-fit and zoom",
      "Open it from the keyframe lanes and close back to them; edits are undoable",
  ],
  ac=["Dragging a key in the graph changes its time and value as one undo step",
      "Dragging a bezier handle changes that key's easing to the matching custom curve",
      "kittests cover key drag, handle drag and undo"])

I("silence-detect", "Silence detection to cut dead air", None, ["media", "timeline", "ui"], "M", labels=["later"],
  summary="Find silent stretches in screen recordings and offer to cut or speed them up.",
  scope=[
      "Detect Silence on selected clips: find stretches below a threshold (dB) longer than a minimum length, with padding, from the clip's audio",
      "A dialog previews the regions on the timeline and offers Cut (ripple delete) or Speed Up (split and set the region's speed)",
      "Threshold, minimum length and padding are adjustable with a live region preview",
      "The whole operation is one undo step",
  ],
  ac=["A tone, two seconds of silence, tone clip yields one region within one frame of the silence (test)",
      "Cut leaves two adjacent clips with the silence removed; undo restores the original clip",
      "Speed Up shortens the timeline by the expected amount"])

# ───────────────────────── Owner polish batch, 2026-10-24 ─────────────────────────

I("prefs-redesign", "Preferences redesign: sections and dropdowns that fit", None, ["ui", "suite-ui"], "M", labels=["later"],
  summary="The Preferences dialog is a small box whose Input Device list spills past it and over Done. Redesign it with the impeccable skill, starting from an HTML mockup like the v3 Inspector one.",
  scope=[
      "Build option A of `docs/prototypes/v4/preferences.html` (owner's pick): a sidebar of sections (General: Language, Theme; Audio: Input Device) beside the selected section's fields",
      "No helper text under fields (owner: drop lines like Applies Right Away or Records Voiceovers); labels alone. Error messages stay",
      "Dropdown popups never extend past the dialog or the window: in suite-ui's dropdown, cap the popup to the available space, truncate long items with an ellipsis and show the full name on hover. Every dropdown in the app gets this",
      "The dialog is sized for its content, scrolls if the window is too small, and nothing covers Done",
  ],
  ac=["With input devices whose names are longer than the field, the open list stays inside the window, truncates and shows each full name on hover",
      "The dialog matches the mockup at the minimum window size (kittest snapshot)",
      "Language and Input Device still apply live and persist"],
  refs=[f"[DESIGN.md]({DESIGN})", f"[Preferences mockup, option A]({PROTO}/v4/preferences.html)"])

I("theme-picker", "Theme picker in Preferences", None, ["ui", "suite-ui"], "S", ["prefs-redesign"], labels=["later"],
  summary="Themes work (built-in Dark, Light and High Contrast, TOML files, live reload), but only by editing the settings file. Add them to Preferences with a few good defaults.",
  scope=[
      "Theme swatch cards in Preferences > General (option A of `docs/prototypes/v4/preferences.html`), one per built-in and per TOML theme in the themes folder, each labelled with its name only; Dark is the default",
      "Choosing one applies live with no restart and persists to the `theme` setting",
      "Ship two more bundled defaults (for example a warmer dark and a softer light) as TOML files that also serve as templates for custom themes",
      "An Open Themes Folder button with no hint text beside it; a broken theme file shows its error on its card and falls back to Dark",
      "Check every panel in Light and High Contrast and fix any colour that ignores the palette",
  ],
  ac=["Switching to Light recolours the whole editor at once and survives a restart",
      "A valid TOML theme dropped into the themes folder appears in the list",
      "kittest snapshots of the editor in Light and High Contrast"],
  refs=[f"[Preferences mockup, option A]({PROTO}/v4/preferences.html)"])

I("text-inspector-polish", "Text tab layout cleanup", None, ["ui"], "M", labels=["later"],
  summary="The Text tab is a ragged list: a narrow one-line text box, Size and Weight sharing a row, an unlabelled Left/Center/Right row, and colour swatches that render as odd half-checker squares. Bring it up to the Inspector row standard.",
  scope=[
      "A full-width, multi-line text box that grows with its content",
      "Sections like the Video tab: Text (Font, Size, Weight, Align), Fill (Color), Outline (Width, Color), Shadow (Offset X/Y, Color), Box (Padding, Color), using the shared Inspector rows (one diamond, reset slot, sliders for ranged values)",
      "One label column width and one field rhythm across the tab; Align becomes a labelled icon segmented control",
      "Colour swatches share one component: a rounded swatch with a border, with a checkerboard only behind transparent colours",
      "Build option A of `docs/prototypes/v4/text-tab.html` (owner's pick: collapsible sections like the Video tab), including its States and Build Notes. No helper text under fields",
      "Use the impeccable skill; follow DESIGN.md",
  ],
  ac=["Every row lines up on the same label column and field start (kittest snapshot)",
      "Opaque swatches show solid colour; a 50% black box colour shows the checkerboard through it",
      "Editing every Text field still updates the Program view live and undoes as one step"],
  refs=[f"[Text tab mockup, option A]({PROTO}/v4/text-tab.html)", f"[DESIGN.md]({DESIGN})"])

I("transport-align", "Transport bar: vertical centring check", None, ["ui"], "S", labels=["later"],
  summary="The Program transport bar may sit off-centre vertically: the timecodes, transport buttons, grid toggle, Preview checkbox and Full/Fit dropdowns may not share one centre line.",
  scope=[
      "Measure each item's vertical centre in the bar; fix any that is off by a pixel or more, including the timecodes' text baseline against the icons",
      "Add a layout check (in the style of DESIGN.md Layout > Checks) asserting every transport item's centre is within 1 px of the bar's centre",
      "If everything is already centred, the check alone closes this",
  ],
  ac=["The layout check passes and fails if an item is moved 2 px",
      "The bar looks centred at 100% and 150% UI scale"])

I("export-polish", "Export dialog and Render Queue visual upgrade", None, ["ui", "export"], "L", labels=["later"],
  summary="The Export button's queue menu is a bare list (file name, Done, Reveal, then New Export, Cancel All and Clear Finished as text buttons), and the Export dialog needs the same impeccable polish. Owner picked queue option C and dialog option A of the v4 export mockup.",
  scope=[
      "Queue: option C of `docs/prototypes/v4/export.html`. The Export button menu becomes a 300px summary (overall progress, the last few results, New Export) with Show Render Queue, which opens a dockable Render Queue tab beside Timeline",
      "Render Queue panel rows: file name, preset and duration or size, a progress bar with percent and time left while running, a status chip with icon (Waiting, Rendering, Done, Failed, Cancelled), and Reveal and Cancel as icon buttons in fixed columns; Cancel All and Clear Finished in its header; an empty state",
      "Export dialog: option A of the same page (Format, Range and Output groups in one column, the Inspector row standard, segmented controls for Range, Subtitles and Lossless, warnings under their row, Estimated Size beside Export, the body scrolls and the footer stays put)",
      "Follow the page's States and Decisions sections (Job stores preset name, duration and file size; time left from elapsed time and progress, shown from 5%; a neutral progress bar)",
  ],
  ac=["A running export shows overall progress in the menu and live progress and time left in the Render Queue panel; a failed one shows its error on hover",
      "The Render Queue tab docks, closes and reopens from Show Render Queue, and its layout persists",
      "Rows don't shift as jobs change state (kittest snapshot of each state)",
      "The Export dialog matches option A at 960x600 (kittest snapshot)"],
  refs=[f"[Export mockup: queue C, dialog A]({PROTO}/v4/export.html)", f"[DESIGN.md]({DESIGN})"])

I("drop-new-track-above", "Drop above the top track to create a track there", None, ["ui", "timeline"], "S", labels=["later"],
  summary="Making a new track for a title or solid means dragging it into the empty space below every track, though it usually belongs on top. Open a slot above the top track while dragging there.",
  scope=[
      "While dragging a video, title or solid asset within a few pixels of the top track's upper edge, or in the gap under the ruler, open an animated New Track slot above the top track",
      "Dropping into the slot creates a video track above all others and places the clip at the drop time, as one undo step; leaving the zone closes the slot",
      "Audio-only assets and drops onto existing tracks keep their current behaviour; the empty-space-below drop from #229 stays",
  ],
  ac=["Dragging a title to the top edge opens the slot; dropping it creates a track above V1 with the title at the drop time",
      "Undo removes the clip and the new track together",
      "Moving the drag away from the top closes the slot and adds no track"])

I("graphic-eq", "Graphic EQ with presets", None, ["audio", "core", "ui"], "M", labels=["later"],
  summary="The EQ is seven rows of sliders (Low, Low Freq, Mid, Mid Freq, Mid Q, High, High Freq). Editors and players use a horizontal graphic EQ; build one, with presets.",
  scope=[
      "A 10-band graphic EQ (31, 62, 125, 250, 500, 1K, 2K, 4K, 8K and 16K Hz, ±15 dB each, plus Post-Gain) replacing the three-band parametric EQ for clips and tracks",
      "Option A of `docs/prototypes/v4/graphic-eq.html` (owner's pick, Slider Bank): one vertical slider per band with its dB value above and its frequency below, the response curve drawn behind over a ±15 dB grid; double-click a band to reset it. Follow the page's States, Preset values and build notes",
      "A Presets dropdown: Flat, Voice, Podcast, Bass Boost, Bass Cut, Treble Boost, Low Cut and Loudness. The Compressor gets the same dropdown with Gentle, Voice, Broadcast and Limiter",
      "Old projects with the three-band EQ load with an equivalent 10-band curve",
      "The track Audio Effects submenu opens the same widget in a popover",
  ],
  ac=["A 1 kHz tone through the EQ with only the 1K band at +6 dB comes out within 0.5 dB of +6 dB (test)",
      "Each preset sets the bands it names, as one undo step",
      "A project saved with the old EQ reopens with the same sound (test against the three-band response)"],
  refs=[f"[Graphic EQ mockup, option A]({PROTO}/v4/graphic-eq.html)"])

I("locked-drag", "Locked tracks: no drag ghost for clips that can't move", None, ["ui", "timeline"], "S", labels=["later"],
  summary="Dragging a clip on a locked track draws the full move ghost, then nothing moves.",
  scope=[
      "A drag or trim that starts on a clip on a locked track shows no ghost; the cursor shows not-allowed and a tooltip says the track is locked",
      "A selection or linked pair that includes a clip on a locked track can't be moved, with the same feedback (linked clips keep sync)",
      "Trim handles are hidden on locked clips",
  ],
  ac=["Dragging a clip on a locked V1 shows no ghost and leaves the project and undo stack unchanged",
      "With V1 locked and A1 unlocked, dragging the linked A1 clip is refused with the same feedback"])

I("record-transport", "Record button in the transport bar", None, ["ui", "audio"], "S", ["transport-align"], labels=["later"],
  summary="Voiceover only starts by arming a track in its header and pressing Shift+R, which nothing on screen explains. Add a Record button next to the play controls, as other editors have.",
  scope=[
      "A Record button beside the Program transport controls, with a tooltip showing Shift+R",
      "Clicking it starts recording at the playhead into the armed track; with none armed, it adds a new audio track, arms it and records there",
      "While recording, the button becomes a red Stop with the elapsed time and a small input meter beside it; clicking it, Space or Shift+R ends the take",
      "An unsaved project asks to save first, as Shift+R does",
  ],
  ac=["With no armed track, one click records into a new audio track and Stop places the clip",
      "The button and Shift+R drive the same recording state"])

I("polish-pass", "Impeccable polish pass on the remaining panels", None, ["ui", "suite-ui", "docs"], "L", ["prefs-redesign", "text-inspector-polish", "export-polish", "transport-align"], labels=["later"],
  summary="Bring every panel not covered by the issues above to the same finish, using the impeccable skill's critique and audit.",
  scope=[
      "Critique the title bar, media bin, Source and Program headers, timeline toolbar, track headers, keyframe lanes, context menus, toasts and the welcome screen against DESIGN.md, and fix the material findings",
      "Known items: keyframe lane names are lowercase (anchor, crop bottom, scale) and must be Title Case; a lane's name overlaps the keyframe line at its left edge; the Arm for Recording tooltip covers + Track",
      "Follow-ups folded in from #268-#273 (owner, 2026-10-24): Export dialog Loudness and Target Size, Project Settings and Inspector Font move onto suite-ui Dropdown with .bounds(); the EQ preset dropdown shows Custom greyed and keeps it out of the menu",
      "Inspector: the two-slot tool column (Reset, diamond) from the Text tab applies to every Inspector tab, so slider widths no longer change when Reset appears; Reset on an animated property is one undo step; the Speed row's Reverse checkbox is not clipped at 288 px; colour swatches get a border on Light",
      "Graphic EQ: hovering a band and pressing Up/Down nudges it 0.5 dB (Shift 0.1 dB)",
      "Render Queue panel header measures its buttons and drops labels to icons instead of clipping them",
      "Timeline: the New Track slot respects reduced motion; locked clips also hide fade and keyframe handles; a refused drag near the edge doesn't auto-scroll",
      "suite-ui Button centres icon glyphs by their ink, app-wide; the gallery gains ColorField",
      "DESIGN.md documents the themes folder and the ink_2/ink_4 tokens",
      "List any finding too big for this issue as a follow-up in the PR",
  ],
  ac=["Keyframe lane names show in Title Case and never overlap their lane's line",
      "Every Inspector tab's sliders keep their width whether or not Reset shows (kittest snapshot)",
      "No dropdown in Export, Project Settings or the Inspector Font field overflows its popup (kittest)",
      "The PR lists each finding with a before and after screenshot"],
  refs=[f"[DESIGN.md]({DESIGN})"])

I("ci-cross-os-green", "Main CI green on Windows and macOS", None, ["infra"], "S", labels=["later"],
  summary="Main's full matrix fails off Linux: three kittest snapshots differ on Windows WARP (bin_effects_hover, editor_light, preferences), and macOS playback::tests::loop_wraps_without_dropping_frames is timing-flaky. PRs only run Ubuntu, so it goes unseen.",
  scope=[
      "Snapshots: Linux (lavapipe) stays the pixel reference; other OSes still render every snapshot test but compare with a tolerance that absorbs driver rasterisation, or skip only the pixel compare, decided in one shared place",
      "Make loop_wraps_without_dropping_frames deterministic (no wall-clock dependence) rather than retrying it",
  ],
  ac=["A workflow_dispatch run on the branch passes ubuntu, windows and macOS",
      "A real layout regression still fails on Linux"])

I("ci-cross-os-green-2", "Main CI: Windows record test file lock, macOS slow-decoder flake", None, ["infra"], "S", labels=["later"],
  summary="After #275, main run 40062193057 still fails off Linux: on Windows, record_button_records_into_a_new_armed_track_and_shift_r_stops_it (#276) panics at its final remove_dir_all with os error 32 because the recorded file is still open; on macOS, playback::tests::slow_decoder_drops_frames_instead_of_drifting is wall-clock flaky like the loop test #275 fixed.",
  scope=[
      "Find what still holds the recorded take open on Windows: if the app (probe, waveform or decoder) leaks the handle, fix that; if it is only test teardown order, drop the harness before removing the directory",
      "Make slow_decoder_drops_frames_instead_of_drifting deterministic with the stepped fake clock #275 introduced, keeping what it proves",
  ],
  ac=["A workflow_dispatch run on the branch passes ubuntu, windows and macOS",
      "The slow-decoder test still fails if frames are shown late instead of dropped"])

# Translation wave 1 (owner, 2026-10-24): Extended Latin + Cyrillic, Nordic + Catalan, CJK; each language
# gets an independent review pass before merge.
I("i18n-wave1-foundation", "Translation wave 1 foundation: 17 languages, plural rules, fonts", None, ["suite-ui", "ui", "docs"], "M", labels=["later"],
  summary="Register the wave-1 languages so each translation lands as its own catalog files without touching shared code: fr, de, it, pt-BR, nl, pl, cs, tr, ru, uk, sv, da, nb, ca, ja, zh-Hans, ko.",
  scope=[
      "LANGUAGES, the Settings picker and every Catalog::new([..]) list the 17 codes; each gets stub <code>.toml catalogs (only language.name, in the language's own name) so missing keys fall back to English until its translation merges",
      "plural_category gains the CLDR cardinal rule for each code (pl, cs, ru, uk: one/few/many/other; fr, it, pt-BR, ca: one/many/other; de, nl, tr, sv, da, nb: one/other; ja, zh, ko: other only), with a test per rule at 0, 1, 2, 5, 11, 12, 21, 22, 25, 101, 111",
      "The Inter subset widens to Latin Extended-A and Cyrillic (crates/suite-ui/assets/README.md pyftsubset command updated)",
      "CJK: one Noto Sans CJK face per language (JP, SC, KR; Regular only, used for every weight; OFL), subset by a checked-in script to the characters that language's catalogs use, installed as Inter's fallback only while that language is active (Han glyph shapes differ per language). Budget: at most 600 KB per CJK language",
      "docs/translating.md: the font policy records the owner's CJK size decision and the subset script; docs/i18n/glossary.md lists about 40 core editing terms (Timeline, Clip, Track, Keyframe, Render Queue, Inspector, Program, Source, Proxy, Trim, Ripple, ...) each with a one-line meaning, for every language's glossary to translate",
  ],
  ac=["Each of the 17 codes appears in the picker under its own name and falls back to English text without errors",
      "Plural tests cover every new rule; every_character_has_a_glyph passes for Polish and Russian sample strings",
      "Switching to Japanese and back loads and unloads the JP face, and the binary grows by less than 200 KB before any CJK catalog is translated"],
  refs=["[docs/translating.md](https://github.com/jarradbanks/jvid/blob/main/docs/translating.md)", "[CLDR plural rules](https://www.unicode.org/cldr/charts/latest/supplemental/language_plural_rules.html)"])

for code, name in [("fr", "French"), ("de", "German"), ("it", "Italian"), ("pt-BR", "Portuguese (Brazil)"), ("nl", "Dutch"),
                   ("pl", "Polish"), ("cs", "Czech"), ("tr", "Turkish"), ("ru", "Russian"), ("uk", "Ukrainian"),
                   ("sv", "Swedish"), ("da", "Danish"), ("nb", "Norwegian Bokmål"), ("ca", "Catalan"),
                   ("ja", "Japanese"), ("zh-Hans", "Chinese (Simplified)"), ("ko", "Korean")]:
    cjk = code in ("ja", "zh-Hans", "ko")
    I(f"i18n-{code.lower()}", f"Translate jvid into {name}", None, ["ui", "suite-ui"], "M", ["i18n-wave1-foundation"], labels=["later"],
      summary=f"Fill the {name} ({code}) catalogs registered by the wave-1 foundation, accurately and consistently with what {name}-speaking editors see in DaVinci Resolve and Premiere Pro.",
      scope=[
          f"docs/i18n/{code}.md: the glossary's terms in {name}, each matching the localized Resolve/Premiere UI where one exists, with a note where they differ",
          f"Translate every key of the jvid-app, suite-ui and gallery {code}.toml catalogs using en.toml's comments (width limits, placeholder meaning, words that stay as written) and the glossary; labels follow {name}'s own capitalization convention, not English Title Case",
          "Every plural group has the forms the language's rule uses; placeholders match English's set",
      ] + (["Re-run the CJK subset script for this language and commit the resulting face (within the 600 KB budget)"] if cjk else []),
      ac=["i18n_guard, the plural tests and every_character_has_a_glyph pass with the language active",
          "kittest snapshots of the editor and Preferences in the language at 1280x720 show no clipped or overlapping labels",
          "An independent review pass (back-translation plus glossary check) is recorded in the PR and its findings fixed"])

I("ci-playback-stepped", "Playback tests: no wall-clock waits left", None, ["infra"], "S", labels=["later"],
  summary="#275 and #277 made two playback tests stepped; others still poll with Instant::now() and fail under CI load, e.g. failed_decodes_retry_and_relink_redecodes ('never shown with 1 layers', PR #297 run 40068128664).",
  scope=["Move every remaining wall-clock wait in crates/jvid-app/src/playback/tests.rs onto the stepped Fake clock pattern from #275/#277, or an explicit completion signal; no retries or longer timeouts"],
  ac=["grep finds no Instant::now()-based polling left in playback tests, or each remaining use is justified in a comment",
      "Each converted test still fails under a mutation of the behaviour it covers"])
I("ci-i18n-fast", "Translation-only PRs skip the 11-minute CI", None, ["infra"], "S", labels=["later"],
  summary="Owner, 2026-10-24: a PR that only changes translation catalogs should not wait on the full build and GPU test suite. A seconds-long check covers what a catalog change can break.",
  scope=["ci.yml's pull_request trigger ignores PRs whose files are all under crates/*/i18n/** or docs/i18n/**; push to main still runs everything",
         "A new workflow runs on those paths only: a stdlib Python script (scripts/i18n_check.py) that parses every catalog, checks its keys and {placeholders} against en.toml and checks plural tables carry the categories the language needs; no Rust build"],
  ac=["A PR touching only one language's catalogs finishes CI in under a minute",
      "The script fails on a missing key, a renamed placeholder, a TOML syntax error and a missing plural category, each shown by a test or a documented manual run"])
I("ui-long-text", "UI handles long translated text", None, ["ui", "suite-ui"], "M", ["i18n-wave1-foundation"], labels=["later"],
  summary="Owner, 2026-10-24: text that is too long is a UI problem, not a translation one. Translators should translate naturally and the layout should cope, as in other editors.",
  scope=["Every label, button, tab, menu item and column header in a fixed-width spot truncates with an ellipsis and shows the full text on hover; wrap instead where the layout has room",
         "One pseudo-locale test (every en string padded about 40%) proves the editor, Preferences, Export dialog and Render Queue have no overlap or clipping without an ellipsis",
         "Delete the per-language editor/preferences/export_dialog snapshot tests and PNGs in crates/jvid-app/tests; the pseudo-locale test replaces them"],
  ac=["The pseudo-locale test passes and fails if one truncation is removed",
      "No per-language snapshot PNGs remain; cargo test --workspace passes"])
I("readme-preview", "README: real UI preview video and current pitch", None, ["docs"], "M", labels=["later"],
  summary="Owner, 2026-10-24: the README still leads with the AI-generated prototype and 'first tool in a planned suite'. Replace both with a short, professional, minimal motion preview made from the real UI, and copy that matches what jvid does today.",
  scope=["A reproducible preview reel rendered from the real app (egui_kittest + wgpu, as the snapshot tests do): a scripted 15-25 s seamless loop of a real edit (clips on the timeline, playback in the viewer, a trim, a title, export), with eased motion and a drawn cursor; no text callouts or effects beyond the UI itself",
         "One command regenerates it (an example or ignored test plus a script that runs ffmpeg); footage is downloaded at build time from a CC-BY/CC0 source, never committed, and credited in the README",
         "docs/media/preview.webp (animated, autoplays inline on GitHub) and docs/media/preview.mp4, each small enough to keep the repo lean",
         "README rewrite: lead with the preview, drop the 'first tool in a planned suite' line, the prototype image and the stale status note; describe what ships now, keep Building, Workspace, Contributing and License"],
  ac=["The README shows the animated preview at the top on github.com and no prototype image",
      "Re-running the documented command regenerates both files from a clean checkout",
      "Every claim in the README matches a merged feature"])
I("i18n-wave1-leftovers", "Translation leftovers found in wave 1", None, ["ui", "render"], "S", ["i18n-wave1-foundation"], labels=["later"],
  summary="Gaps the wave-1 translators found that sit in code, not catalogs (PRs #301, #302).",
  scope=["Title-preset thumbnails and title rendering draw CJK as boxes on a machine with no system CJK font: jvid-render's FontSystem::new only loads system fonts; register the bundled Noto CJK faces too",
         "The Export dialog's custom size field appends a hard-coded ' MB'; take the unit from the catalog",
         "Messages built as '<label>: <reason>' in code force the reason's capital after the colon (fr 'Indisponible : ...'); make the whole message one catalog string with a placeholder",
         "The ', ' list join near crates/jvid-app/src/timeline.rs:1770 is ASCII; use a catalog separator so ja/zh-Hans/ko get theirs"],
  ac=["A title preset in ja renders its glyphs, not boxes, with no system CJK font installed (test with the bundled faces only)",
      "grep finds no user-visible unit, colon join or list separator built in code for these spots"])
I("preview-audio-only", "Preview stops decoding video from audio-only files", None, ["ui"], "S", labels=["later"],
  summary="Owner, 2026-10-24, Windows: 246 'preview decode of D:\\fishing\\media\\Voiceover 2.wav failed: Stream not found' warnings in one two-minute session. The audio played fine; the preview asks the decode pool for a video frame from a WAV on every new time (crates/jvid-app/src/preview.rs around line 745) and logs each failure.",
  scope=["The preview never submits a video decode for a source with no video stream (use the probed media info); it shows what it already shows for audio-only media",
         "A decode failure for a source is logged once until the source or its file changes, not once per requested time"],
  ac=["A test with an audio-only source plays or scrubs across many times and submits no video decode",
      "A failing video source logs its warning once per source, shown by a test"])
I("voiceover-live-clip", "Voiceover take grows on the timeline as it records", None, ["timeline", "audio"], "M", ["voiceover"], labels=["later"],
  summary="Owner, 2026-10-24: while a voiceover records, the timeline should show it happening, as other editors do. Today the take only lands as a clip when recording stops; until then the track header shows an input level.",
  scope=["While a take records, the armed track shows a live clip from the record start to the playhead, in the recording colour, with its waveform drawn from the incoming samples as they arrive",
         "The live clip is drawing only: it is not in the project or undo history until the take stops, when the existing one-step commit replaces it with the real clip, whose cached waveform matches what was shown",
         "Samples reach the timeline through a small lock-free or bounded buffer of peaks from jvid-audio's Recorder; the audio thread never blocks on the UI"],
  ac=["A test records a fake input for N seconds and the live clip's width and peak count follow the playhead every frame",
      "After stop, the committed clip sits exactly where the live clip was, and undo removes it in one step",
      "Recording on a real microphone shows the waveform growing smoothly (Needs user testing)"])
I("inspector-tabs-wide", "Inspector tabs fill the header width", None, ["ui"], "S", labels=["later"],
  summary="Owner, 2026-10-24: the Inspector's tabs stop at 160px each and leave the right of the header empty; they should share the full width evenly, as the Media panel's tabs do (Panel::wide, crates/jvid-app/src/media_bin.rs).",
  scope=["The Inspector panel's tabs use Panel::wide, whatever panes the selection shows (Video/Audio/Effects, Clip/Text/Effects, ...)",
         "DESIGN.md's Tab row says side panels (Media, Inspector) share the full header width; other panels keep the 160px cap"],
  ac=["A kittest test shows the Inspector's last tab ending 4px from the header's right edge for two and three panes",
      "Snapshots that show the Inspector are updated"])

I("highlight-points", "Highlights from point markers", None, ["export"], "S", ["batch-highlights"], labels=["later"],
  summary="Tap M when something happens, then export a clip around it: point markers count as highlights.",
  scope=[
      "The Highlights range also takes point markers: each becomes a highlight from Before to After around its time, clamped to the sequence",
      "Before and After fields in the Highlights section, defaulting to 15 s and 5 s, remembered like the other export settings",
      "Ranged markers keep their own range; overlapping highlights still export as separate files",
      "Highlights is disabled only when there are no markers at all, and the reason reads \"Add markers first\"",
  ],
  ac=["Two point markers export as two files of Before + After length (test)",
      "A point marker 3 s into the sequence exports from 0 (clamped)",
      "New strings are in every catalog"])

I("hold-m-range", "Hold M to mark a range while playing", None, ["timeline"], "S", ["markers"], labels=["later"],
  summary="The press-and-release ranged marker from the Markers issue (#40), which shipped without it.",
  scope=["While playing, holding M marks a range from press to release; a tap (under about 0.3 s) still drops a point marker, and playback never stops"],
  ac=["Press at 1 s and release at 4 s during playback makes one 3 s ranged marker (test with simulated key events)",
      "A tap still makes a point marker, and one undo removes either"])

# ───────────────────────── Later · tutorials, subtitles, grading (owner request 2026-10-24) ─────────────────────────

I("tutorial-engine", "Interactive tutorials: lesson engine and Tutorials menu", None, ["ui"], "L", labels=["later"],
  summary="Hands-on lessons that run inside the real editor on sample footage, one checked step at a time.",
  scope=[
      "Lessons are Rust data: steps with a target UI element, a short instruction, a completion check on project or UI state, and an optional Show Me that does the step through the normal undoable edits",
      "Panels and controls register named anchors each frame; the overlay dims the window around the current anchor and a step card sits beside it with Back, Show Me, Skip Step and Exit; input is not blocked",
      "A step whose anchor is hidden (another tab active, panel too narrow) switches to it first",
      "A lesson opens its own sandbox project built from the sample media; the user's open project and undo history are untouched, and nothing is saved outside the app's data folder unless they choose Keep Project",
      "Help > Tutorials lists lessons with their length and a done mark; progress is saved in config",
      "Lesson text lives in the i18n catalogs",
  ],
  ac=["A test lesson runs end to end in egui_kittest by pressing Show Me on every step, and each step's check passes after it",
      "A test fails when any lesson step's anchor is never drawn, so a renamed or removed control breaks the build instead of the lesson",
      "Exiting a lesson halfway leaves the user's project and undo history exactly as before"])

I("tutorial-samples", "Sample media for tutorials", None, ["media", "docs"], "S", labels=["later"],
  summary="A few short, freely licensed clips installed with the app so every lesson works offline.",
  scope=[
      "4 to 6 clips, 720p H.264/AAC, 10 s or less each, 15 MB or less in total, installed with the app: a shot with a clear subject to mask, a green-screen shot, a slightly underexposed warm shot for colour, a screen recording with dead air, a music bed and a voice line",
      "Freely licensed footage only (owner decision 2026-10-24): every clip's licence is checked and recorded, CC0 or CC-BY credited in About (for example Blender open-movie footage); no game footage",
      "Lessons find the clips through the install's data path",
  ],
  ac=["Every bundled clip probes and decodes in the test suite, and every clip a lesson names exists",
      "About credits every CC-BY clip"])

I("lesson-basics", "Tour: your first edit", None, ["ui"], "M", ["tutorial-engine", "tutorial-samples", "welcome"], labels=["later"],
  summary="The first-launch tour, done hands-on with the sample footage.",
  scope=[
      "The Welcome screen offers Take the Tour on first launch, once; Help > Tutorials runs it again",
      "Steps: import the sample, drag it onto the timeline, play and scrub (Space, J/K/L), set In and Out, split and delete a section, add a title, export",
  ],
  ac=["The Show Me run passes in egui_kittest", "Declining the offer never shows it again"])

I("lesson-masks", "Lesson: masks", None, ["ui"], "M", ["tutorial-engine", "tutorial-samples", "masks"], labels=["later"],
  summary="Learn masks by spotlighting a subject: darken everything but them, then make the mask follow them.",
  scope=[
      "Steps: select the clip, duplicate it to the track above, darken the lower copy with Color Adjustments, add an ellipse mask to the top copy, drag it over a ghost outline drawn on the preview, feather it, try Invert and Subtract to see the difference",
      "Then animate it: add a key, move the playhead, move the mask to follow the subject, play it back",
      "Checks read the project: mask count and shape, feather past a threshold, overlap with the target outline, at least two path keys",
  ],
  ac=["The Show Me run passes in egui_kittest", "Dragging the mask over the outline completes the step at 60% overlap or more (unit test)"])

I("lesson-keyframes", "Lesson: keyframes and easing", None, ["ui"], "M", ["tutorial-engine", "easing-ui", "graph-editor"], labels=["later"],
  summary="Animate a title in and learn keys, easing and the graph editor.",
  scope=["Steps: key a title's position and opacity, move between keys, change the easing, shape the curve in the graph editor, play it back"],
  ac=["The Show Me run passes in egui_kittest"])

I("lesson-colour", "Lesson: colour and scopes", None, ["ui"], "M", ["tutorial-engine", "tutorial-samples", "color", "lut", "scopes"], labels=["later"],
  summary="Fix a dark, too-warm shot by reading the scopes, then apply a look.",
  scope=[
      "Waveform: raise Exposure until the brightest parts near 100% without flattening against it; see clipping when you go too far",
      "Vectorscope: pull Temperature and Tint until something that should be white or grey sits in the centre",
      "Histogram: see the R, G and B lines line up once the cast is gone",
      "Apply a LUT and lower its intensity; compare before and after",
  ],
  ac=["The Show Me run passes in egui_kittest", "Each scope step's check reads the scope analysis, not the slider value"])

I("lesson-green-screen", "Lesson: green screen", None, ["ui"], "S", ["tutorial-engine", "tutorial-samples", "chroma-key", "masks"], labels=["later"],
  summary="Key the green-screen sample over a background.",
  scope=["Steps: put a background under the green-screen clip, add Chroma Key, tune tolerance, softness and spill, mask off the edges the key misses"],
  ac=["The Show Me run passes in egui_kittest"])

I("lesson-screen-recording", "Lesson: screen recordings", None, ["ui"], "S", ["tutorial-engine", "tutorial-samples", "zoom-region", "silence-detect"], labels=["later"],
  summary="Tidy a screen recording: cut dead air, zoom into the important part, add a callout.",
  scope=["Steps: Detect Silence and cut, zoom to a region, add a title as a callout"],
  ac=["The Show Me run passes in egui_kittest"])

I("lesson-audio", "Lesson: audio", None, ["ui"], "M", ["tutorial-engine", "tutorial-samples", "mixer", "audio-keys", "voiceover", "audio-fx"], labels=["later"],
  summary="Balance a voice over music, duck the music and record a voiceover.",
  scope=["Steps: set levels in the mixer, duck the music under the voice with volume keys, EQ the voice, record a voiceover (skippable when there is no microphone)"],
  ac=["The Show Me run passes in egui_kittest, skipping the recording step"])

I("transcribe", "Speech-to-text engine", None, ["media"], "L", ["srt", "silence-detect"], labels=["later"],
  summary="Local, offline transcription with word timings, the base for auto subtitles.",
  scope=[
      "whisper.cpp through `whisper-rs` (MIT), approved by the owner on 2026-10-24 as an addition to PLAN.md's stack",
      "A clip's or range's audio is resampled to 16 kHz mono with FFmpeg; silence detection skips quiet stretches",
      "Runs as a background job with progress and Cancel, and returns words with timestamps; kept out of jvid-core",
      "Models download on first use into the data folder (base, small, large-v3-turbo quantised), each checked by SHA-256; none is bundled",
  ],
  ac=["A short speech fixture transcribes to the expected words with the smallest model (test runs with --ignored)",
      "Cancel stops within a second, and a corrupt model download is rejected"])

I("auto-subtitles", "Generate Subtitles", None, ["ui", "timeline"], "M", ["transcribe", "srt"], labels=["later"],
  summary="Turn speech into editable subtitles in one click.",
  scope=[
      "Generate Subtitles on the selected clips or the In/Out range: language (auto or chosen), Fast or Accurate, and Translate to English",
      "Words become caption lines: at most 2 lines of about 42 characters and about 7 s, breaking at punctuation and pauses",
      "Captions land on a new subtitle track as editable text clips, which SRT burn-in and sidecar export already handle",
  ],
  ac=["Line-splitting unit tests over a fake transcript",
      "A fake transcriber drives the whole flow in egui_kittest, and one undo removes the generated track"])

I("text-edit", "Edit by text", None, ["ui", "timeline"], "L", ["transcribe"], labels=["later"],
  summary="Cut video by editing its transcript: delete words and the matching video goes with them.",
  scope=[
      "A Transcript panel shows the words of the selected clips or the whole sequence, transcribed with the speech-to-text engine and cached per asset",
      "Clicking a word seeks to it; the word under the playhead is highlighted during playback; Find searches the transcript",
      "Selecting words and pressing Delete ripple-deletes those time ranges from the timeline across linked tracks, as one undo step",
      "The transcript follows edits made on the timeline",
  ],
  ac=["Deleting three words from a fake transcript removes exactly their ranges from video and audio (test)",
      "Undo restores the clip and the transcript"])

I("filler-removal", "Remove filler words and long pauses", None, ["ui", "timeline"], "M", ["text-edit", "silence-detect"], labels=["later"],
  summary="One pass that finds um, uh and dead air and cuts them out.",
  scope=[
      "Finds filler words (um, uh, er, and a per-language list) in the transcript and pauses longer than a set length",
      "Whisper tends to leave fillers out, so transcription for this pass prompts it to keep them",
      "A list previews every hit with its context; Remove All or pick, then ripple delete as one undo step",
  ],
  ac=["A fake transcript with four fillers and one long pause removes all five ranges (test)",
      "Unticked hits stay in the edit"])

I("voice-cleanup", "Voice cleanup: noise reduction and a voice preset", None, ["audio"], "M", ["audio-fx"], labels=["later"],
  summary="Clean up a mic recording in one click.",
  scope=[
      "Noise reduction effect with `nnnoiseless` (BSD-3, pure Rust), approved by the owner on 2026-10-24 as an addition to PLAN.md's stack",
      "Clean Up Voice: noise reduction, high-pass, EQ and compressor as one preset",
  ],
  ac=["The noise floor of a noisy fixture drops by 10 dB or more", "Applying the preset is one undo step"])

I("find-highlights", "Find Highlights in long recordings", None, ["media", "timeline"], "M", ["markers", "batch-highlights"], labels=["later"],
  summary="Scan a long recording for loud moments and scene changes and mark them.",
  scope=[
      "Scan a clip's audio for loud peaks and its video for scene changes; each result becomes a ranged marker",
      "Sensitivity and padding settings, with a preview before markers are added",
  ],
  ac=["A synthetic clip with known loud bursts gets markers at the right times, within one frame"])

I("color-wheels", "Colour wheels: lift, gamma, gain and offset", None, ["render", "ui"], "M", ["color", "scopes"], labels=["later"],
  summary="Primary colour wheels, copy and paste of a grade, and a skin-tone line on the vectorscope.",
  scope=[
      "Lift, Gamma, Gain and Offset wheels with master sliders, in linear light like Color Adjustments, keyframeable",
      "Copy Grade and Paste Grade across selected clips",
      "The vectorscope gets a skin-tone line",
  ],
  ac=["Default wheels leave pixels unchanged (render test); lift moves the shadows and leaves white alone",
      "Paste Grade onto several clips is one undo step"])

I("color-curves", "Curves: RGB, luma and hue curves", None, ["render", "ui"], "M", ["color"], labels=["later"],
  summary="Curve grading: master, R, G and B curves plus Hue vs Sat, Hue vs Hue and Luma vs Sat.",
  scope=["Curves edit with points on a small graph and bake to a lookup texture on the GPU; keyframeable like other effect parameters"],
  ac=["An identity curve is pixel-identical (render test)", "An S-curve darkens shadows and brightens highlights (render test)"])

I("color-secondary", "Secondary grading and grade compare", None, ["render", "ui"], "M", ["color", "masks"], labels=["later"],
  summary="Grade only one colour range, and compare before and after.",
  scope=[
      "Qualifier: pick a colour range by hue, saturation and luminance with softness, limit a grade to it, and combine it with masks",
      "Program monitor: a before/after split and a Bypass Grade toggle",
  ],
  ac=["Qualifying a red patch and desaturating leaves a blue patch unchanged (render test)"])

I("privacy-blur", "Blur and Pixelate effects", None, ["render"], "S", ["masks", "wgsl"], labels=["later"],
  summary="Hide emails, keys and faces in recordings.",
  scope=["Bundled Gaussian Blur and Pixelate effects, animatable; with a mask they hide one region"],
  ac=["Render tests for both; pixels outside the mask are unchanged"])

I("reframe", "Reframe for Shorts", None, ["core", "ui"], "M", ["transform", "keyframes", "gizmo", "overlays", "project-settings"], labels=["later"],
  summary="Turn a 16:9 edit into a vertical Short, panning the crop to follow the action.",
  scope=[
      "Reframe: Save As 9:16 (also 1:1 and 4:5) makes a copy of the project at that frame size; every video clip is scaled to fill the new frame, centred",
      "On the Program monitor a reframe box shows the new frame over the original wide shot; dragging it keys the clip's position, so the shot pans with the action",
      "Text and titles keep their size and move inside the new frame's safe area",
  ],
  ac=["A 16:9 clip in the 9:16 copy fills the frame with no bars (render test)",
      "The original project is unchanged",
      "Dragging the box at two times makes two position keys, and one undo removes each"])

I("marker-popup-style", "Marker popup: house widgets and layout", None, ["timeline", "ui"], "S", ["hold-m-range"], labels=["later"],
  summary="The marker popup gets the same cleanup the Inspector got (owner request).",
  scope=[
      "Name uses suite-ui TextInput and Note a matching multi-line field; the colour row is built like the Inspector's clip-colour swatches (selected ring, focusable, accessible colour names); Delete Marker is a destructive Button",
      "DESIGN.md spacing and a fixed popup width; Title Case labels; no hint or helper text",
  ],
  ac=["An egui_kittest snapshot of the popup",
      "Typing into Name and Note still works, and each edit gesture is one undo step"])

I("lesson-subtitles", "Lesson: subtitles and edit by text", None, ["ui"], "S", ["tutorial-engine", "tutorial-samples", "auto-subtitles", "text-edit"], labels=["later"],
  summary="Make subtitles from speech, fix them, then cut the video by editing its transcript.",
  scope=["Steps: Generate Subtitles on the voice sample, fix a misheard word, restyle the subtitles, then delete a phrase in the transcript and see the cut appear on the timeline",
         "With no speech model installed, the first step is the normal model download prompt; the lesson can be skipped offline"],
  ac=["The Show Me run passes in egui_kittest with a fake transcriber"])

I("lesson-markers", "Lesson: markers and highlights", None, ["ui"], "S", ["tutorial-engine", "tutorial-samples", "highlight-points", "hold-m-range", "marker-popup-style"], labels=["later"],
  summary="Mark moments while you watch, then export them as highlight clips.",
  scope=["Steps: tap M on moments while playing, hold M to mark a range, jump between markers with Shift+M and Ctrl+Shift+M, right-click a marker to name and colour it, export Highlights with Before and After padding"],
  ac=["The Show Me run passes in egui_kittest"])

I("fast-tests", "Faster test suite: slim the slow export tests", None, ["export"], "S", [], labels=["later"],
  summary="PR CI spends about 3 minutes in three export tests; make them prove the same thing on tiny inputs.",
  scope=["jvid-app's export::tests::a_ramped_clip_exports_its_curve, every_builtin_preset_exports and target_size_lands_within_three_percent each run over 60 s in CI (run 40108900410). Encode the smallest input that still proves each claim (tiny frame size, frames or seconds) instead of full-size media",
         "Measure `cargo test --workspace` before and after on the same machine and put both numbers in the PR",
         "Requested by the owner on 2026-10-25 to cut CI time"],
  ac=["No CI test runs over 30 s, and each of the three still fails when the behaviour it guards breaks",
      "The jvid-app lib tests take at most half as long as before"])

I("windows-playback-hang", "Windows: playback retry test hangs intermittently", None, ["timeline"], "S", [], labels=["later"],
  summary="playback::tests::failed_decodes_retry_and_relink_redecodes hung for 65 minutes on windows-latest and passed on a rerun.",
  scope=["CI run 40115784502 (PR #361): jvid-app lib test playback::tests::failed_decodes_retry_and_relink_redecodes printed 'running for over 60 seconds' at 17:26 UTC and was still running at 18:29 UTC, when it was cancelled. The rerun of the same commit passed. Find the wait that can block forever (a channel recv, join or condvar with no timeout) and fix the race, or show it is test-only and fix the test",
         "Add a timeout to the test so a hang fails in seconds rather than burning the job"],
  ac=["The test passes 50 times in a row on Windows (a loop in CI or locally), and a forced hang fails within 30 s"])

I("whisper-cancel", "Speech-to-text: Cancel stops whisper promptly", None, ["media"], "S", ["transcribe"], labels=["later"],
  summary="After Cancel, the jvid-whisper thread runs on for one encoder pass (3.8-8 s measured in #361).",
  scope=["Use whisper's abort callback so Cancel stops between encoder steps, not after the next pass",
         "Return the detected language from a transcription job"],
  ac=["The jvid-whisper thread exits within 1 s of Cancel (the ignored model test asserts it)"])

I("track-volume", "Track Volume control on the audio track header", None, ["timeline", "ui"], "S", ["mixer"], labels=["later"],
  summary="TrackSettings.volume exists but has no UI; the audio lesson (#341) had to use clip Volume.",
  scope=["A compact Volume control on each audio track header, undoable and saved, reusing the house slider"],
  ac=["Changing a track's Volume changes the mix of every clip on it, and undo restores it (test)"])

I("tour-polish", "Tutorials: back to Welcome on exit, per-lesson baselines", None, ["ui"], "S", ["tutorial-engine"], labels=["later"],
  summary="Follow-ups from #335 (PR #362).",
  scope=["Exiting a tour started from the Welcome screen returns to Welcome",
         "Checks such as \"an export is queued\" compare against state captured when the lesson starts, so earlier state can't pass a step"],
  ac=["Exit from a Welcome-started tour shows Welcome again (test)",
      "An export queued before the lesson doesn't complete the export step (test)"])

I("export-drop-panic", "Render: no BufferAsyncError panic when an export is dropped mid-frame", None, ["render"], "S", [], labels=["later"],
  summary="From #338 (PR #365): dropping the app mid-export panics a job thread in composite.rs copy_out with BufferAsyncError. Tests still pass, but it's a panic on a real code path (quit during export).",
  scope=["copy_out handles a failed or abandoned buffer map as an error, not a panic"],
  ac=["A test that drops the renderer mid-export shows no panic in its output and the job ends with an error or cancel"])

I("mp3-range-padding", "Export: MP3 audio stops within a frame of the range end", None, ["export"], "S", [], labels=["later"],
  summary="From #360 (PR #366): MP3 encoder padding can run up to about 40 ms past a short export range, while validate allows one frame.",
  scope=["Trim or account for MP3 encoder delay and padding so the output duration matches the range"],
  ac=["A short-range MP3 export ends within one video frame of the range (test)"])

I("seek-test-speed", "Tests: slim the random-seek corpus test", None, ["media", "infra"], "S", ["fast-tests"], labels=["later"],
  summary="From #360 (PR #366): decode::tests::random_seeks_match_ffmpeg_pts_across_the_corpus takes about 30 s under parallel load.",
  scope=["Cut the seek count or corpus subset the way #360 slimmed the other slow tests, keeping one seek per codec and container"],
  ac=["The test runs under 5 s on CI and still covers every corpus codec"])

I("third-party-notices", "Ship third-party licence notices", None, ["infra", "ui"], "S", ["transcribe"], labels=["later"],
  summary="From #342 (PR #361): whisper.cpp (MIT) and webpki-roots (CDLA-Permissive-2.0 section 3.1) need their licence text shipped once jvid-app links jvid-transcribe. The repo has no notices mechanism.",
  scope=["Generate a notices file from cargo metadata (and the bundled samples' LICENSES.toml), bundle it, and show it from About"],
  ac=["The notices file lists every linked crate's licence and the whisper.cpp and webpki-roots texts (test)",
      "About opens the notices"])

I("hide-region", "Hide Region: one click to blur part of a clip", None, ["ui"], "S", ["privacy-blur"], labels=["later"],
  summary="From #351 (PR #374): hiding a region today takes duplicate clip, add mask, add Blur.",
  scope=["A Hide Region action that duplicates the clip above itself, adds a rectangle mask and Blur, and selects the mask for placing"],
  ac=["Hide Region is one undo step and blurs only inside the mask (test)"])

I("separable-blur", "Blur: separable two-pass blur for large radii", None, ["render"], "S", ["privacy-blur"], labels=["later"],
  summary="From #351 (PR #374): the single 7x7 pass leaves visible steps at big radii, which can leak detail when blurring faces or text.",
  scope=["Two-pass (horizontal then vertical) Gaussian with enough taps for the maximum radius"],
  ac=["At maximum radius a 1 px detail is no longer distinguishable (render test with a threshold)"])

I("effect-masks", "Per-effect masks", None, ["render", "ui"], "M", ["privacy-blur"], labels=["later"],
  summary="From #351 (PR #374): limit an effect to a mask region without duplicating the clip.",
  scope=["An effect can reference one of the clip's masks; the effect only applies inside it"],
  ac=["An effect limited to a mask leaves pixels outside it unchanged (render test)", "Old projects load unchanged"])

I("relink-stale-frame", "Playback: no stale frame after relink", None, ["timeline"], "S", ["windows-playback-hang"], labels=["later"],
  summary="From #371 (PR #393): decode jobs are keyed without the media path, so an old-path decode still pending at relink can cache a stale frame; the keyframe stand-in path has the same race.",
  scope=["Key decode jobs (and the stand-in path) so a relink can't receive a result decoded from the old file"],
  ac=["A test that relinks while an old-path decode is pending shows only frames from the new file"])

I("highlights-fast-scan", "Find Highlights: fast scene scan on long recordings", None, ["media"], "S", ["find-highlights"], labels=["later"],
  summary="From #347 (PR #392): the scene scan decodes every frame at full size, so a two-hour recording takes many minutes.",
  scope=["Scan with keyframe-only or downscaled decode (or both), keeping the scene accuracy the tests check"],
  ac=["The long-2h-1440p60 corpus file scans at least 5x faster than PR #392's scan, with the same scene test passing"])

I("reframe-keys", "Reframe: honour crop, rotation and title keys", None, ["core"], "S", ["reframe"], labels=["later"],
  summary="From #352 (PR #396): Reframe ignores a clip's crop and rotation and a title's own position and text_size keys, so those clips can end up with bars or a title outside title-safe.",
  scope=["Project::reframed takes crop, rotation, and a title's position and text_size keys into account when it fills and keeps titles safe"],
  ac=["Tests: a cropped clip and a rotated clip fill the reframed frame with no bars; a title with position and text_size keys stays inside title-safe at every key"])

I("overlay-design-lines", "Transform overlay: DESIGN.md lines instead of amber", None, ["ui"], "S", ["reframe"], labels=["later"],
  summary="From #352 (PR #396): the Program monitor's transform overlay draws an amber outline, but DESIGN.md's Neutral Viewer Rule asks for 1px Ink or Light Blue lines.",
  scope=["Draw the transform overlay's outline and handles in the DESIGN.md viewer colours, keeping the reframe box distinct"],
  ac=["A kittest snapshot of a selected clip shows no amber in the overlay"])
