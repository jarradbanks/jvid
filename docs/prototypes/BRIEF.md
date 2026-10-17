# jvid UI prototype brief

> **Superseded look.** The four screens below set the panel layout, which still stands. Their look (warm greys, amber accent, Inter) was replaced by the Clean Cut direction in [`DESIGN.md`](../../DESIGN.md); mockups in that look are in [`clean-cut/`](clean-cut/).

These prompts went to Codex's built-in `image_gen` tool to generate the four screens. Every screen shares the design language below so they read as one app.

## Shared design language

- **Use case:** ui-mockup. A realistic, shippable desktop application screenshot, not concept art.
- **Canvas:** landscape 16:9 desktop window, crisp and flat, viewed straight on, no device frame, no perspective, no drop shadow around the window.
- **Style:** dark, professional, minimal. Think of a focused, de-cluttered pro editor: generous spacing, hairline 1px dividers, no gradients, no glassmorphism, no glow.
- **Colors:**
  - App background `#111214`, panels `#18191C`, raised controls `#222327`, dividers `#2A2B30`
  - Primary text `#E6E7EA`, secondary text `#8A8E97`
  - Single accent, warm amber `#F2A93B`: playhead, selection outlines, primary buttons, in/out range, active keyframes
  - Video clips muted slate blue `#3A5683`, audio clips muted green `#3C7457`, text/title clips muted violet `#65558F`
- **Typography:** clean geometric sans (Inter-like) at small UI sizes; timecodes in a tabular monospace (JetBrains Mono-like).
- **Icons:** thin monochrome line icons.
- **Window chrome:** slim 40px top bar with a small lowercase "jvid" wordmark at left, the project name in the center, undo/redo icons, and an amber "Export" button at right.
- **Constraints:** no real brand logos or trademarks, no watermark, legible text, all literal labels rendered verbatim.

## Screens

### 01 — Main editor (`01-editor.png`)

Four-panel layout:
- **Left: media bin** — tabs "Media", "Text", "Effects"; a search field; a 2-column grid of video/photo thumbnails with duration badges (e.g. "0:12", "0:47"); a couple of audio files with waveform icons; an "Import" button.
- **Center: preview** — tabs "Source" and "Timeline" (Timeline active); a 16:9 frame showing a sunset coastal road clip with a white title "SUMMER ROAD TRIP"; below it, timecode "00:00:12:08" left and "00:01:34:20" right, transport controls (go to in, step back, play, step forward, go to out, loop), and a "½" preview resolution dropdown.
- **Right: inspector** — header "Clip: coast_drive.mp4"; sections "Transform" (Position X 960, Position Y 540, Scale 100%, Rotation 0°, Anchor), "Opacity" 100% with a "Blend: Normal" dropdown, "Crop", and "Color"; each property row has a small diamond keyframe toggle at the right.
- **Bottom: timeline** (about 40% of the height) — ruler with timecodes; tracks V3 (violet title clip "SUMMER ROAD TRIP"), V2, V1 (several slate-blue video clips with thumbnail strips), A1, A2 (green audio clips with waveforms), A3 (music track spanning wide); track headers with eye/lock icons and M/S buttons; a translucent amber in/out band across the ruler and tracks with small "I" and "O" flags; an amber vertical playhead; a zoom slider at the bottom right.

### 02 — Keyframe editing (`02-keyframes.png`)

Generated with `01-editor.png` attached as the layout and style reference, so the two screens match. Same editor layout, focused on animation:
- **Preview:** the video clip scaled down to about 60% and offset, with an amber transform bounding box, corner and edge handles, a rotation handle, and a center anchor crosshair.
- **Inspector:** "Transform" section with Scale at 62% and Position values; diamond toggles filled amber on animated properties; a "Keyframe" subsection showing "Easing" with segmented options "Linear", "Ease In", "Ease Out", "Ease In/Out", "Hold", "Bezier" ("Ease In/Out" selected), and a small curve preview with bezier handles.
- **Timeline:** the selected V1 clip is expanded to show keyframe lanes labeled "Scale", "Position", "Opacity", each with small diamond keyframes (selected ones amber, others gray) connected by thin lines; on A2 an audio clip shows a volume rubber-band line with keyframe dots ramping down (a fade); the playhead sits on a keyframe.

### 03 — Export dialog (`03-export.png`)

A centered modal over the dimmed editor:
- **Left column: presets** — "YouTube 1080p", "YouTube 4K", "Shorts / Reels 9:16", "ProRes Master", "GIF", "Audio Only — WAV", "Audio Only — MP3"; "YouTube 1080p" selected.
- **Right column: settings**
  - "Range" segmented control: "Entire Timeline", "In to Out" (selected, amber), "Selected Clips"; beneath it "In 00:00:08:12", "Out 00:00:42:03", "Duration 00:00:33:15", with two small thumbnails of the in and out frames.
  - "Video": Format "MP4", Codec "H.264", Encoder "Hardware (NVENC)", Resolution "1920 × 1080", Frame rate "30 fps", and a Quality slider.
  - "Audio": Codec "AAC", Bitrate "320 kbps", a toggle "Normalize loudness" set on with "−14 LUFS".
  - "Output": file path field "~/Videos/summer-road-trip.mp4" with a folder button; "Estimated size: 84 MB".
- **Footer:** "Cancel", "Add to Queue", and an amber "Export" primary button.

### 04 — Welcome screen (`04-welcome.png`)

A calm, minimal start screen. The top bar shows only the "jvid" wordmark: no project name, no undo/redo, no Export button.
- **Center:** the "jvid" wordmark, the tagline "Quick edits, fast.", and two buttons: amber "New Project" and outlined "Open Project".
- **Below:** a row of new-project presets as small cards showing their aspect shape: "16:9 · 1080p", "16:9 · 4K", "9:16 · Vertical", "1:1 · Square".
- **A large dashed drop zone:** "Drop media here to start a quick edit".
- **Right side: "Recent"** — a list of 5 projects with small thumbnails, names (e.g. "summer-road-trip", "product-demo", "gym-reel"), and relative dates ("2 hours ago", "Yesterday", "Oct 12").
- **Footer:** "v0.1.0" at left and a shortcut hint "Ctrl+N new · Ctrl+O open" at right.

## Clean Cut mockups

The look in [`DESIGN.md`](../../DESIGN.md). `clean-cut/01-editor.png` was generated with `01-editor.png` attached for layout only, from this prompt:

```text
Use case: ui-mockup. A realistic, shippable desktop application screenshot of "jvid", a lightweight video editor. Flat, viewed straight on, no device frame, no perspective, no shadow around the window.
Layout reference: the attached image shows the panel arrangement to keep (media bin left, preview centre, inspector right, timeline across the bottom). Keep its arrangement only; replace all of its colours, typography and styling.
Style: Y2K tech-graphic (late-90s game menu graphics) cleaned up to the precise, minimal finish of a modern pro tool. Clean, compact, perfectly aligned, uncluttered.
Ground and plates: window background very dark blue-black #050608. Each panel is a separate flat plate #14171C floating on that ground, with 4px gaps between plates and around the window edge. NO rounded corners anywhere. Each plate has only its top-left corner cut off at 45 degrees (a small chamfer); every other corner is a sharp square.
Header line: every panel has the same 32px header row with a 1px divider #272C32 under it. The three upper panels' headers and dividers sit on one perfectly straight horizontal line across the window.
Header labels: tiny, wide, extended capitals with letter-spacing, each preceded by a two-digit monospace number. Left panel tabs "01 MEDIA", "07 EFFECTS", "08 TEXT" (MEDIA active). Centre tabs "02 SOURCE", "04 PROGRAM" (PROGRAM active). Right panel "05 INSPECTOR". Timeline "03 TIMELINE". Active tab: white text with a 2px underline on the divider; inactive tabs grey. The timeline has keyboard focus, so its "03" and its underline are acid yellow-green #CEFE34.
Top bar (40px, ground colour): lowercase "jvid" wordmark in a wide bold sans at left; project name "ranked-highlights" in the centre; small undo and redo icons; at right an acid #CEFE34 "Export" button with near-black text and its top-left corner cut at 45 degrees.
Left panel, media bin: a square dark search field; a 2-column grid of thumbnails from fictional sci-fi arena shooter gameplay and one desktop screen recording, each with its top-left corner cut and a duration in monospace ("2:04:31", "0:47", "12:09").
Centre panel, preview: neutral pure-black viewer showing a 16:9 frame of fictional sci-fi arena shooter gameplay with a minimal HUD. Under it a 32px transport row: current timecode "00:01:12:08" in bold monospace acid #CEFE34, duration "00:02:04:20" in grey monospace, thin transport icons (go to in, step back, play, step forward, go to out, loop) centred, a "1/2" resolution dropdown at right.
Right panel, inspector: clip name "clutch_round_3.mkv"; section heads in the same tiny wide capitals: "TRANSFORM", "OPACITY", "VOLUME"; property rows with a grey label column ("Position", "Scale", "Rotation", "Anchor", "Level") and square, borderless, darker input wells holding a grey inline prefix and a white monospace value ("X 960", "Y 540", "100%", "0°", "-3.0 dB"); a small diamond keyframe toggle at the right of each row, one of them acid.
Bottom, timeline (about 40% of the height): a track header column with "V3", "V2", "V1", "A1", "A2", "A3" in bold monospace and tiny lock, eye, mute and solo icons; above it a corner cell showing "00:01:12:08" in acid monospace; a ruler with tiny monospace timecodes. Flat clip blocks: ultramarine #3453D3 video clips with thumbnail strips, a violet #763EBD title clip "TRIPLE KILL", a magenta #AB3276 still clip "logo.png", teal #037465 audio clips with waveforms named "game audio", "mic", "music". Every clip has its top-left corner cut at 45 degrees and a 1px gap from its neighbour. One video clip is selected with a 2px acid outline. An in/out range is a translucent acid band across the ruler with thin acid brackets at each end. The playhead is a thin 1px acid #CEFE34 vertical line with a small pointed tag in the ruler.
Colours: text white #E8EBF1, secondary text grey #9EA5AE; acid #CEFE34 appears only on the Export button, the playhead, the current timecode, the focused "03", the selection outline, the in/out band and one keyframe.
Typography: tiny wide extended capitals only for panel labels and section heads; a plain clean grotesk for body text at small sizes; a bold square monospace for every timecode and number.
Avoid: rounded corners, glows, gradients, glassmorphism, shadows inside the window, amber or orange accents, real game titles or brand logos, watermarks.
Text: render every quoted label verbatim and legibly.
```

A follow-up edit replaced the gameplay in the viewer, bin thumbnails and V1 clips with an original desert racing scene, because the first render resembled an existing game.
