# User guide

How to use jvid as it is today. It assumes you know the basics of a Premiere-style editor. Every shortcut is listed in the [shortcut cheat sheet](shortcuts.md); "Ctrl" means Cmd on macOS.

## Building from source

jvid isn't packaged yet, so you build it. You need:

- A Rust toolchain from [rustup](https://rustup.rs). `rust-toolchain.toml` pins the channel, so rustup installs the right one on first use.
- FFmpeg 4.4 to 8.x development libraries, `pkg-config` and libclang. The per-OS commands are in [CONTRIBUTING.md](../CONTRIBUTING.md#developer-setup).

Then:

```sh
cargo run --release -p jvid-app
```

A debug build works, but use `--release` for real footage: decoding and scrubbing are much slower without it.

## Getting started

The start screen offers **New project** (pick a frame size preset, or **Match source** to size the project from your footage), **Open project…**, your recent projects, and a drop zone for media.

- **Import:** drag files onto the window, or click **Import media** in the Media panel. Files are probed in the background and land in the bin together as one undoable step.
- **Save:** the project menu at the top left has New, Open, Save, Save as, Project settings and Keyboard shortcuts. Projects are plain JSON (`.jvid`).
- **Autosave:** jvid autosaves beside the project file (untitled projects go to the app's data folder). If an autosave is newer than the last save when you open a project, usually after a crash, jvid offers to restore it.
- **Missing media:** if a source file has moved, the clip shows as offline. Right-click it in the bin and choose **Relink…**; other missing files in the same folder are found too.
- **Panels:** Media, Source, Program, Timeline, Inspector and Effects. `Shift+1` to `Shift+7` focus them.

## Editing basics

**Playback.** `Space` plays and stops. `J`/`K`/`L` shuttle; press `J` or `L` again to go faster. `Left`/`Right` step one frame, `Home`/`End` jump to the ends. The Program monitor's preview resolution menu trades sharpness for speed while scrubbing.

**Proxies.** For 4K or HEVC sources, tick **Proxies** above the Program monitor. jvid makes low-res copies in the background and previews from them; export always uses the originals.

**Getting clips on the timeline.** Drag from the bin to the timeline to overwrite at the drop point. Or open a clip in Source, mark In (`I`) and Out (`O`), and press `,` to insert or `.` to overwrite at the playhead on the targeted tracks. Track headers hold lock, hide or mute, solo and target toggles; the track menu adds, renames, reorders and removes tracks.

**Selecting and moving.** Click or marquee to select. Dragged clips overwrite where they land. Copy, cut, paste (at the playhead), duplicate and delete use the usual shortcuts. Linked selection (`Ctrl+Shift+L`) keeps a clip's video and audio together; `Ctrl+L` links or unlinks them.

**Cutting.**

- Drag a clip's edge to trim it; hold Ctrl to ripple the rest of the track.
- `Ctrl+K` splits at the playhead. `C` switches to the razor tool, `V` back to selection.
- `Q`/`W` ripple trim the previous or next edit to the playhead.
- `Shift+Delete` ripple deletes. To close a gap, click it and press `Shift+Delete`, or right-click it and choose **Close gap**.
- With In and Out set on the timeline, `;` lifts the range (leaving a gap) and `'` extracts it (closing the gap). `X` marks the clip under the playhead.

**Markers.** `M` adds one at the playhead, `Alt+M` turns In/Out into a ranged marker, `Shift+M` and `Ctrl+Shift+M` jump between them. Right-click a marker flag to name it, color it or add a note.

**Solids and text.** **New solid** in the Media panel adds a color solid. The Text tab has title presets; add one to the bin, put it on a track above your footage, and edit its words, font, size, outline, shadow and box in the Inspector.

**Audio.** Drag the square in either top corner of a clip inward to fade it; double-click the handle to switch between linear and equal-power. The line across an audio clip's waveform is its volume: drag it, or Ctrl+click it to add a point. `Shift+S` toggles audio while scrubbing.

**Undo.** Everything you do on the timeline, in the bin and in the Inspector undoes with `Ctrl+Z` and redoes with `Ctrl+Shift+Z` or `Ctrl+Y`.

## Keyframes

Select a clip and use the Inspector. It holds Transform (position, scale, rotation, anchor), opacity, blend mode, crop, color (exposure, contrast, saturation, temperature, tint) and volume.

- Click the diamond beside a property to animate it. That sets a keyframe at the playhead; change the value at another time to add another.
- The arrows either side of the diamond jump to the previous and next keyframe. The middle diamond is filled when there's a key at the playhead; click it to add or remove one.
- Right-click a key's diamond to pick its easing.
- **Reset** returns a property to its default.

On the timeline, an animated clip has a toggle in its name strip that shows a lane per animated property. Keys there click and marquee select, drag (snapping to frames, the playhead, clip edges and markers), delete, copy and paste at the playhead. Right-click a key to open the easing editor.

## Export

Click **Export** in the header. The dialog opens on In to Out when you've marked a range, otherwise the whole timeline.

- **Presets:** Highlight 1080p60, Highlight 1440p60, Screen recording (sharp text), YouTube 1080p, YouTube 4K, Shorts/Reels 9:16 and ProRes master. jvid warns when a preset's aspect ratio doesn't match the project.
- Pick the video and audio codecs, loudness target and output file, and check the estimated size.
- Exports run in the **Render Queue** while you keep editing. Cancel, remove, or **Show in Folder** when done.

**Export frame** (`Ctrl+Shift+E`) saves the current Program frame as a PNG.

## Shortcuts

See the [cheat sheet](shortcuts.md). To change one, open **Keyboard shortcuts…** from the project menu. Your changes are saved to `keymap.toml` beside `settings.toml` in jvid's config folder, holding only the shortcuts that differ from the defaults; set one to `""` to unbind it.
