# user guide

how to use jvid. every shortcut is in the [cheat sheet](shortcuts.md), and "Ctrl" means Cmd on macos.

## getting started

the start screen has **New Project** (pick a frame size, or **Match Source** to size it from your footage), **Open Project…**, your recent projects, and a drop zone.

- **import:** drag files onto the window, or **Import Media** in the media panel.
- **save:** the project menu (top left) has new, open, save, settings and keyboard shortcuts. projects are plain JSON (`.jvid`).
- **autosave:** happens beside the project file. if jvid crashed, it offers to restore on next open.
- **missing media:** moved files show as offline. right-click in the bin, **Relink…**, and anything else missing from the same folder gets found too.
- **language:** follows your system if jvid has it, otherwise english. change it in **Settings…**.

## panels

media, source, program, timeline, inspector and effects. `Shift+1` to `Shift+7` focus them.

every panel has a draggable tab. drag one onto another panel's tabs to share, or onto an edge to split. optional panels (transcript, render queue, captions) open beside the timeline and close with the **×** on their tab. layout is saved between runs; **Edit → Reset Panel Layout** puts it back.

## editing

**playback.** `Space` plays and stops. `J`/`K`/`L` shuttle - tap again to go faster. `Left`/`Right` step a frame, `Home`/`End` jump to the ends. the program monitor's resolution menu trades sharpness for speed.

**proxies.** for 4K or HEVC, tick **Proxies** above the program monitor. export always uses the originals.

**getting clips in.** drag from the bin to overwrite where you drop. or open a clip in source, mark `I` and `O`, then `,` to insert or `.` to overwrite at the playhead.

**selecting and moving.** click or marquee. dragged clips overwrite where they land. copy, cut, paste, duplicate and delete do what you'd expect. `Ctrl+L` links or unlinks video and audio.

**cutting.**

- drag a clip edge to trim, hold Ctrl to ripple.
- `Ctrl+K` splits at the playhead. `C` is razor, `V` back to selection.
- `Q`/`W` ripple trim the previous or next edit to the playhead.
- `Shift+Delete` ripple deletes. click a gap and `Shift+Delete` to close it.
- with in/out set, `;` lifts (leaves a gap) and `'` extracts (closes it).

**markers.** `M` adds one, `Alt+M` turns in/out into a ranged marker, `Shift+M` / `Ctrl+Shift+M` jump between them. right-click a flag to name, color or note it. jplay recordings bring their markers along from the `.jplay.json` beside them.

**text.** `T` then click in program to start typing a title. **Media → Text** has title, lower third and caption presets. font, outline, shadow and box are in the inspector.

**audio.** drag the square in a clip's top corner inward to fade. the line across the waveform is volume - drag it, Ctrl+click to add a point. `Shift+S` toggles audio scrubbing.

**transitions.** `Ctrl+D` crossfades the selected clip's end (or the nearest cut). right-click for dips to black or white. needs spare media past the cut on both sides. drag the amber block to change its length.

**undo.** `Ctrl+Z`, and `Ctrl+Shift+Z` or `Ctrl+Y` to redo. it covers everything.

## keyframes

select a clip and use the inspector: transform, opacity, blend mode, crop, color and volume.

- click the diamond beside a property to animate it. change the value at another time to add a key.
- the arrows jump between keys. right-click a key for easing.
- on the timeline, an animated clip has a toggle that shows a lane per property. keys there drag, snap, copy and paste.

**framing.** right-click a clip and pick **Fit to Frame** (bars), **Fill Frame** (crop) or **Stretch to Fit** (squash). stretch is the classic 4:3-gameplay-to-widescreen look.

**zoom to a region.** hold `Alt` and drag a box on the program monitor. the clip zooms in, holds two seconds, and zooms back out - all ordinary keyframes you can tweak.

**presets.** the inspector's text, audio and effects tabs can save title styles, voice treatments and effect stacks to reuse across projects.

## effects

the bin's effects tab lists the WGSL effects in your effects folder. add them from the inspector, where every parameter is a keyframeable slider. they run top to bottom, drag to reorder, and the eye bypasses one. [write your own](effects.md).

## transcript and speech

**Timeline → Transcript** shows recognized speech for the selected clips (or the whole sequence). copy it, or export it as `.txt`. recognition runs locally and is cached, so generating subtitles and transcribing never do the same work twice.

**Edit → Review Speech Cuts** finds silences and filler words. tick the ones you want gone, audition before/after, then **Apply Cuts** - one undo step, every track stays in sync.

## subtitles

**File → Import Subtitles** for an SRT, or **Timeline → Generate Subtitles** to make them from speech. **Timeline → Captions** is a table of text and times you can edit, split, merge, find and replace, and restyle in bulk.

to save just the captions, **Export Subtitles (.srt)…** - whole timeline or in to out.

## export

click **Export** in the header. it uses in to out if you've marked one, otherwise the whole timeline.

- presets for highlights, screen recordings, youtube, shorts/reels and prores.
- subtitles can be burned in or saved as a sidecar `.srt`.
- exports run in the render queue while you keep editing.

`Ctrl+Shift+E` saves the current frame as a PNG.

**File → Export Versions…** makes landscape, vertical and size-limited copies in one go, each with its own framing you check before it queues.

## updating

**Help → Check for Updates**. it checks at startup (you can turn that off) but never downloads anything without a click. the windows installer updates itself; everything else gets a link to the download. details are in [updating.md](updating.md).

## shortcuts

remap any of them from **Keyboard Shortcuts…** in the project menu. changes go to `keymap.toml` in jvid's config folder - set one to `""` to unbind it.
