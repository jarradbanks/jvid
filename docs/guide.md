# User guide

How to use jvid as it is today. Every shortcut is listed in the [shortcut cheat sheet](shortcuts.md); "Ctrl" means Cmd on macOS.

## Building from source

Native installation and portable candidates are described in the [packaging guide](packaging.md). To build from source, you need:

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
- **Save:** the project menu at the top left has New, Open, Save, Save as, Settings, Project settings and Keyboard shortcuts. Projects are plain JSON (`.jvid`).
- **Language:** jvid follows your system language if it ships it (English and Spanish so far), otherwise English. Pick another in **Settings…** from the project menu; it switches at once and is remembered. See [translating.md](translating.md) to add one.
- **Autosave:** jvid autosaves beside the project file (untitled projects go to the app's data folder). If an autosave is newer than the last save when you open a project, usually after a crash, jvid offers to restore it.
- **Missing media:** if a source file has moved, the clip shows as offline. Right-click it in the bin and choose **Relink…**; other missing files in the same folder are found too.
- **Panels:** Media, Source, Program, Timeline, Inspector and Effects. `Shift+1` to `Shift+7` focus them.

## Arranging panels

Every editor panel has a draggable tab strip. Open **Timeline → Transcript** or **Export → Show Render Queue** to place that optional panel beside Timeline. Drag its tab onto Timeline's strip to share one full-width panel; click a tab to switch views. All three can share a group. Drag a tab along the strip to reorder it, or into another panel's body and choose a highlighted edge to create a separate split. Drop feedback shows the destination before you release. Panels stay inside the editor window.

Timeline's editing actions sit in a separate toolbar below its tab strip. Drag the tab to move the panel; use the toolbar to edit. At narrow widths the tab strip scrolls horizontally with the mouse wheel or its scrollbar, and long labels have an ellipsis and show their full name on hover.

Close Transcript or Render Queue with the **×** on its tab (or middle-click the tab). Showing an already-open panel activates its existing tab wherever you moved it. Reopening a closed optional panel creates a split immediately to Timeline's right. Closing Render Queue leaves exports running; closing Transcript keeps its cached recognition.

Groups, tab order, split sizes and active tabs are saved automatically across restarts. **Edit → Reset Panel Layout** restores the standard editor arrangement and places any open optional panels beside Timeline. Resetting the layout keeps the project, transcript search and render jobs intact.

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

jplay recordings import markers from the adjacent `<name>.jplay.json` file. Placing the media adds the markers within its source In/Out range to the timeline at their corresponding times. They then behave like other timeline markers. Separate audio streams get their own lanes, named from the sidecar when available.

**Solids and text.** **New solid** in the Media panel adds a color solid. The Text tab has title presets; add one to the bin, put it on a track above your footage, and edit its words, font, size, outline, shadow and box in the Inspector.

**Audio.** Drag the square in either top corner of a clip inward to fade it; double-click the handle to switch between linear and equal-power. The line across an audio clip's waveform is its volume: drag it, or Ctrl+click it to add a point. `Shift+S` toggles audio while scrubbing.

**Transitions.** Select a clip and press `Ctrl+D` for a one-second crossfade at its end (or right-click it for a dip to black or white); with nothing selected it goes on the cut nearest the playhead. It needs media past the cut on both sides: if there is none, a message says so; trim the clips to leave some. Drag the amber block on the cut to change its length, or right-click it to change or remove it. Audio crossfades at equal power.

**Undo.** Everything you do on the timeline, in the bin and in the Inspector undoes with `Ctrl+Z` and redoes with `Ctrl+Shift+Z` or `Ctrl+Y`.

## Keyframes

Select a clip and use the Inspector. It holds Transform (position, scale, rotation, anchor), opacity, blend mode, crop, color (exposure, contrast, saturation, temperature, tint) and volume.

- Click the diamond beside a property to animate it. That sets a keyframe at the playhead; change the value at another time to add another.
- The arrows either side of the diamond jump to the previous and next keyframe. The middle diamond is filled when there's a key at the playhead; click it to add or remove one.
- Right-click a key's diamond to pick its easing.
- **Reset** returns a property to its default.

On the timeline, an animated clip has a toggle in its name strip that shows a lane per animated property. Keys there click and marquee select, drag (snapping to frames, the playhead, clip edges and markers), delete, copy and paste at the playhead. Right-click a key to open the easing editor.

**Frame a clip.** Right-click the image in Program or a clip in the timeline and choose **Fit to Frame** (whole image with bars), **Fill Frame** (preserve proportions and crop overflow), or **Stretch to Fit** (fill the project with the whole image, changing proportions). For example, Stretch turns 4:3 gameplay into the stretched widescreen look. These commands affect the composition and export; the monitor's **Fit**/percentage dropdown only changes viewing zoom.

Program targets the top visible clip under the pointer and selects it. To frame a lower layer, right-click that clip in the timeline. A context command changes only the clicked clip, even within a multi-selection. Locked tracks and audio clips cannot be framed.

Each preset centers the whole source, clears crop and anchor offsets, and resets clip rotation. Source orientation and pixel aspect are respected. Grade, effects, masks and opacity stay as set. If any spatial property is animated, the preset adds a grouped spatial key at the playhead using the Inspector's existing rules; other keys remain. Each command is one undo step. The Inspector shows **Scale X/Y** independently, and dragging a transform handle preserves their current ratio.

**Zoom to a region.** Select a video clip, hold `Alt` and drag a rectangle on the Program monitor. From the playhead the clip zooms in on it, holds for 2 seconds and zooms back out. The rectangle keeps the project's shape. Pick **Quick** or **Smooth** in the bar that appears while `Alt` is held. The zoom is one undo step, made of scale and position keyframes you can edit like any others.

## Transcript text

Open **Transcript** from the Timeline menu to work with recognized speech. **Copy** copies the selected words; the platform Copy shortcut (`Ctrl+C`, or `Cmd+C` on macOS) does the same when the Transcript panel has focus. The Find field keeps its normal text-copy behavior.

**Copy All** and **Export Transcript (.txt)…** use the whole displayed transcript scope: selected clips when clips are selected, otherwise the current sequence, using the chosen audio sources. They follow timeline order, trims, removals and repeated clip instances. The scope is shown above the text; a partial-results message explains when some sources still need recognition. Export uses a native save dialog and writes readable UTF-8 text with punctuation and paragraph breaks. These actions use available recognition results and leave the project and playhead unchanged.

## Effects

The bin's Effects tab lists the WGSL effects in your effects folder; add one to the selected clips
from the Inspector's Effects section (**Add Effect** searches names and descriptions), where each
parameter is a keyframeable slider. Effects run top to bottom: drag a block's header, or use its
**…** menu, to reorder. The eye bypasses an effect, keeping its settings and keyframes, so it
drops out of the Program monitor and exports until you turn it back on. To write your own, see
[Writing effects](effects.md).

## Subtitles

Use **File > Import Subtitles** for an existing SRT, or **Timeline > Generate Subtitles** to turn speech into editable captions. Edit a caption's text in the Inspector and trim or move its timeline clip to adjust timing.

To save captions without exporting video, choose **Timeline > Export Subtitles (.srt)…**, **File > Export Subtitles (.srt)…**, or right-click a subtitle track header or caption clip and choose **Export Subtitles (.srt)…**. Choose **Entire Timeline** for the whole current sequence, or explicitly choose **In to Out**, then **Save SRT…** to open the native save dialog. One missing mark means the sequence start or end. Captions crossing the range edges are clipped, and times start at zero at the range's In point.

The SRT contains the current edited text (including Unicode and line breaks), ordered by time, from visible subtitle tracks in the current sequence and its placed nested sequences. Hidden tracks, tracks excluded by video solo, and disabled clips are omitted, just as in video sidecar export. Ordinary titles, unplaced sequences and source transcripts are not captions and are excluded. This export does not render, transcribe, download a speech model or need a GPU. An empty range explains that there are no subtitles to export; cancelling the save dialog writes nothing. Existing SRT destinations use the native replacement confirmation, and project/source files are protected.

## Export

Click **Export** in the header. The dialog opens on In to Out when you've marked a range, otherwise the whole timeline.

- **Presets:** Highlight 1080p60, Highlight 1440p60, Screen Recording (Sharp Text), YouTube 1080p, YouTube 4K, Shorts/Reels 9:16 and ProRes Master. jvid warns when a preset's aspect ratio doesn't match the project.
- Pick the video and audio codecs, loudness target and output file, and check the estimated size.
- With visible captions, the **Subtitles** row offers **Burn In** to draw them into the video, or **Sidecar .srt** to render the video and save a separate SRT beside it. Both use the chosen video range; for an SRT alone, use **Timeline > Export Subtitles (.srt)…**.
- Exports run in the **Render Queue** while you keep editing. The menu beside **Export** shows the running export and the last few results; **Show Render Queue** opens the full list as a panel beside the Timeline, with **Cancel**, **Reveal**, **Cancel All** and **Clear Finished**.

**Export frame** (`Ctrl+Shift+E`) saves the current Program frame as a PNG.

## Shortcuts

See the [cheat sheet](shortcuts.md). To change one, open **Keyboard shortcuts…** from the project menu. Your changes are saved to `keymap.toml` beside `settings.toml` in jvid's config folder, holding only the shortcuts that differ from the defaults; set one to `""` to unbind it.
