# jvid UI prototype brief

> **Superseded look.** The four screens below set the panel layout, which still stands. Their look (warm greys, amber accent) was replaced by the one in [`DESIGN.md`](../../DESIGN.md); mockups in that look are in [`v2/`](v2/).

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

## v2 mockups

The look in [`DESIGN.md`](../../DESIGN.md) ("The Quiet Frame"). `v2/01-editor.png` was generated with the Clean Cut mockup attached for layout and content, from this prompt:

```text
Use case: ui-mockup. A realistic, shippable desktop application screenshot of "jvid", a lightweight video editor. Flat, viewed straight on, no device frame, no perspective, no shadow around the window.
Reference: the attached image is this same screen. Keep its layout, panel arrangement, header alignment and content (the desert racing footage, thumbnails, clip names, timecodes and inspector values), but restyle it completely in the style below. Remove its cut corners, wide all-caps labels, two-digit panel numbers and acid yellow-green colour.
Style: minimal and refined, in the spirit of Linear's desktop app and modern Premiere Pro. Quiet, precise and calm, so the footage is the brightest thing on screen. Neutral dark greys with a faint cool tint, one blue accent, subtle rounded corners, thin 1px borders.
Ground and panels: window background #0A0B0D. Each panel is a #121416 surface with a 1px #232529 border and 8px rounded corners, with 4px gaps between panels and around the window edge.
Header line: every panel has the same 32px header row with a 1px #232529 divider under it. The three upper panels' headers and dividers sit on one perfectly straight horizontal line.
Tabs: sentence-case 12px medium sans labels. Left panel "Media", "Effects", "Text" (Media active). Centre "Source", "Program" (Program active). Right "Inspector". Bottom "Timeline". The active tab is white text on a subtle rounded pill #1C1E21; inactive tabs are grey #82868E. The timeline panel has keyboard focus, so its 1px border is blue #2368C9.
Top bar (40px, background colour): lowercase "jvid" wordmark in a semibold sans at left; project name "ranked-highlights" in the centre; thin undo and redo icons; at right a blue #2368C9 "Export" button with white text and 6px rounded corners.
Left panel, media bin: a search input with 6px rounded corners, a 1px border and placeholder "Search"; a 2-column grid of thumbnails with 6px rounded corners, durations in small dark rounded badges ("2:04:31", "0:47", "12:09", "1:33", "0:28", "4:21").
Centre panel, preview: neutral pure-black viewer showing the desert racing frame with its HUD. Under it a 32px transport row: current timecode "00:01:12:08" in white medium tabular numerals, duration "00:02:04:20" in grey, thin line transport icons (go to in, step back, play, step forward, go to out, loop) centred, a "1/2" dropdown at right.
Right panel, inspector: file name "clutch_round_3.mkv"; sentence-case semibold section heads "Transform", "Opacity", "Volume" with small chevrons; property rows with grey labels "Position", "Scale", "Rotation", "Anchor", "Opacity", "Level". Clean inputs: 24px tall, 6px rounded corners, 1px #232529 border, dark #0D0E11 fill, a grey inline prefix and a white value ("X 960", "Y 540", "100%", "0°", "X 0", "Y 0", "100%", "-3.0 dB"). A small diamond keyframe toggle at the right of each row, one of them blue.
Bottom, timeline (about 40% of the height): a track header column with "V3", "V2", "V1", "A1", "A2", "A3" in medium white text, small grey line icons for lock and visibility, and small "M" and "S" toggles; above it a corner cell showing "00:01:12:08"; a ruler with tiny grey timecodes. Clips have 4px rounded corners and 2px gaps between neighbours: muted violet #5E589F video clips with thumbnail strips ("arena_01.mkv", "clutch_round_3.mkv", "push.mp4", "finale.mkv"), a muted plum #8E4771 title clip "TRIPLE KILL", a muted ochre #88602D still clip "logo.png", muted green #30715D audio clips with waveforms ("game audio", "mic", "music"). The "clutch_round_3.mkv" clip is selected with a 2px light blue #7AACF6 outline. An in/out range is a translucent blue band in the ruler with thin light blue brackets at each end. The playhead is a thin 1px light blue #7AACF6 line with a small rounded head in the ruler.
Colours: text #EDEEF1, secondary text #ADB1B8, tertiary #82868E. Blue appears only on the Export button, the focused timeline border, the playhead, the selection outline, the in/out range and one keyframe.
Typography: one clean, neutral UI sans (Inter-like) for everything, sentence case, 12px UI text, tabular numerals for timecodes. No all-caps labels, no monospace.
Avoid: cut or chamfered corners, wide or all-caps labels, panel numbers, acid or neon colours, glows, gradients, glassmorphism, heavy shadows, real game titles or brand logos, watermarks.
Text: render every quoted label verbatim and legibly.
```

Since this was generated, tabs changed to fill their header in equal widths with centred labels (see `DESIGN.md`, Panels, headers and tabs); where the mockup and `DESIGN.md` disagree, `DESIGN.md` wins.

A Y2K tech-graphic direction ("Clean Cut": cut corners, wide capitals, acid accent) was mocked up first and rejected; it is in git history.
