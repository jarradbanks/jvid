---
name: jvid
description: A small, fast video editor for cutting gaming highlights and screen recordings.
---

<!-- SEED: established with the user before implementation; re-run /impeccable document once there's code to capture the actual tokens and components. -->

# Design System: jvid

## Overview

**Creative North Star: "The Clean Cut"**

jvid takes its look from late-90s game graphics: PS1-era menus and racing-game livery, the tech-graphic style of The Designers Republic and wipEout. That look is held to the finish of Linear and arranged the way Premiere is. Near-black plates float on a darker ground. Small wide capitals name each panel, prefixed with the number that focuses it. Bold monospace numerals carry time. One acid yellow-green marks where you are and what you've picked. Nothing is rounded: anything you act on has its top-left corner cut at 45°. A cut is the editor's basic act, and on a clip the cut corner sits exactly on its in-point.

The world supplies four things: type, palette, density and the cut corner. It never supplies layout, navigation or controls. Those follow Premiere and ordinary desktop conventions, so an editor knows where everything is before they notice the style. Density is compact (12px text, 24px controls) to fit as much timeline on screen as possible.

The usual scene is an evening at a desk after a gaming session, or a screen recording made beside the app, often in a dim room. The footage has to be the brightest, most colourful thing on screen. So the chrome is dark, the viewer is neutral black, and colour in the chrome is spent only on meaning.

The look of the first prototypes in `docs/prototypes/` (warm greys, one amber accent, Inter) was rejected. Their panel layout stands.

**Key Characteristics:**
- Near-black plates on a darker ground, 4px apart
- One header height for every panel, so header lines run straight across the window
- Wide capital labels prefixed with each panel's focus shortcut; bold monospace numerals for anything that changes
- Acid accent for position, focus, selection and the one primary action, nothing else
- Square corners, with a 45° cut on the top-left corner of objects
- Clip colours are the only large areas of colour

## Colors

Restrained and cool: blue-tinted near-blacks, one acid accent that carries state, and four clip colours that own the timeline. These are starting values from the direction round. Confirm them in the first `suite-ui` build, then re-run `/impeccable document` to make them normative.

### Primary
- **Acid** (#CEFE34; pressed #B3DE1E): the playhead and the current timecode, the focused panel's number and tab underline, keyboard focus rings, selection outlines, the in/out range, and the single primary button. Text on Acid is Ground (17:1).

### Clips
- **Ultramarine** (#3453D3): video clips.
- **Violet** (#763EBD): text and title clips.
- **Magenta** (#AB3276): still images and solid colour layers.
- **Teal** (#037465): audio clips.

Every clip colour carries Ink clip names at 4.7:1 or better. A new clip colour has to clear 4.5:1 against Ink before it ships.

### State
- **Signal Red** (#F4514F): errors, offline media, meter clipping at 0 dBFS, destructive confirmations.
- **Amber** (#FCA443): warnings (a proxy is missing, lossless export isn't available), solo engaged, meter level above −6 dBFS.

### Neutral
- **Ground** (#050608): the window background, seen in the gutters between plates.
- **Plate** (#14171C): every panel's surface, its header included.
- **Raised** (#1E2227): hover fills, secondary buttons, menus, popovers, tooltips.
- **Well** (#0C0F12): one step below Plate, so it reads as recessed. Used for input fields, dropdowns and empty timeline lanes.
- **Line** (#272C32): the only hairline. Header dividers, section rules, track separators, overlay borders.
- **Line Strong** (#3D434A): hover outlines on fields and secondary buttons, ruler ticks.
- **Ink** (#E8EBF1): primary text and values, the active tab, clip names, on-state icons.
- **Ink 2** (#9EA5AE): secondary text, inactive tabs, inspector labels, icons at rest.
- **Ink 3** (#7C828B): unfocused panel numbers, field prefixes, ruler labels, shortcut hints, disabled text. At 4.6:1 on Plate it is the dimmest colour allowed for readable text.
- **Viewer** (#060606): the neutral, untinted black behind the picture in the preview.

### Named Rules
**The Acid Means Here Rule.** Acid marks where you are (playhead, current timecode, focused panel, keyboard focus), what you've picked (selection, in/out) and the one thing to do next (the primary button). Nothing decorative is ever acid. With no dialog open, acid covers well under 2% of the window.

**The Neutral Viewer Rule.** Nothing tinted touches the picture. The viewer surround is neutral black. Overlays on the frame (safe areas, grid, transform handles) are 1px Ink or Acid lines.

**The Clips Own Colour Rule.** Clip colours appear only on clips, their keyframe lanes and the type badges in the media bin. They never appear in chrome, buttons or text.

## Typography

**Body Font:** Archivo (Omnibus-Type, OFL) at normal width
**Label Font:** Archivo Expanded (width 125), SemiBold, in capitals
**Numeral Font:** Martian Mono (Evil Martians, OFL)

**Character:** A wide technical capital over a sturdy, plain grotesk, with a square monospace for every number that moves. The wide capitals carry the tech-graphic voice, and the normal-width body keeps 12px text readable. Embed static instances of each weight and width in `suite-ui` rather than depending on variable-font axis support.

### Hierarchy
Sizes are egui points; the scale steps by about 1.17.
- **Display** (Archivo Expanded Bold 700, 22px, line 28): the welcome screen and its lowercase "jvid" wordmark only.
- **Headline** (Archivo SemiBold 600, 16px, line 22): dialog titles.
- **Title** (Archivo SemiBold 600, 14px, line 20): dialog section titles, empty-state headings. The app bar wordmark uses this size in Archivo Expanded Bold.
- **Body** (Archivo Regular 400, Medium 500 for emphasis, 12px, line 16): every control, list row, menu item, clip name, inspector label and message.
- **Label** (Archivo Expanded SemiBold 600, 10px, line 12, +0.06em tracking, capitals): panel names and tabs, inspector section heads, column heads.
- **Timecode** (Martian Mono Bold 700, 16px): the current time under the preview.
- **Value** (Martian Mono Medium 500, 12px): numeric fields, timecodes in lists and in the timeline's corner cell, track IDs (V1, A2), durations, panel numbers.
- **Small numerals** (Martian Mono Regular 400, 10px): ruler labels, shortcut hints, meter scale.

### Named Rules
**The Numbers Hold Still Rule.** Any number that can change while you watch it (timecodes, dragged values, durations, meter readouts) is set in Martian Mono, so digits never shift width under the cursor.

**The Index Is the Shortcut Rule.** The number in front of a panel's name ("01 MEDIA") is the Shift+number shortcut that focuses it in the default keymap, which follows Premiere: 1 Media, 2 Source, 3 Timeline, 4 Program, 5 Inspector, 7 Effects. A view without a shortcut has no number, and when a user remaps a shortcut the label changes with it. Never number things for decoration.

## Layout

Panels sit where Premiere puts them, as in the first prototypes: media bin on the left, preview in the centre, inspector on the right, and the timeline across the bottom, all under a 40px app bar. Docking can rearrange them, and every rule below holds in any arrangement. Units are egui points; every edge lands on a whole physical pixel at every UI scale.

**Spacing scale (4px base):** 4 (gutters, icon-to-label gaps), 8 (between controls, list padding), 12 (the inset inside a plate), 16 (between inspector sections), 24, 32. 2px exists only for underline and focus-ring offsets.

| Size | Value | Used for |
| --- | --- | --- |
| App bar | 40px | The window's top bar |
| Header | 32px | Every panel header and tab bar, inspector section heads, the preview's transport row |
| Ruler | 24px | The timeline ruler and the corner cell beside it |
| Control | 24px | Buttons, fields, dropdowns, list rows, menu rows, inspector property rows |
| Small control | 20px | Track-header toggles |
| Icon | 16px | Every glyph, centred in its control |
| Gutter | 4px | Between plates, and between plates and the window edge |
| Inset | 12px | Left and right padding inside a plate and its header |
| Track header | 160px | Width of the timeline's track-header column |
| Track | 40px | Default lane height; resizable from 24 to 120 in 4px steps |
| Inspector label | 72px | Width of the inspector's label column |
| Plate minimum | 240px | Narrowest a docked plate may get |

### Named Rules
**The One Header Line Rule.** Every panel header is exactly 32px tall, sits at the top of its plate and ends in the same 1px Line divider. Plates side by side start at the same y, so their headers and dividers form one straight line across the window. A panel with a single view shows its title where a tab would be, so a titled panel and a tabbed panel match in height, type and label position. Tabs, toolbars and actions share that one row; a search field or filter goes in the panel's content, never in a second header row. This prevents the misalignment in `01-editor.png`, where the bin's tabs and divider sat on a different line from the preview's.

**The Rows Face Each Other Rule.** Rows that sit side by side line up. The timeline ruler and its corner cell are one 24px row. Each track header is exactly as tall as its lane and starts at the same y. Inspector section heads use the 32px header height, so a fully collapsed inspector reads as a stack of headers.

**The Twelve Inset Rule.** Text and icons inside a plate start 12px from its left edge, header labels included. Header action buttons sit 8px from the right edge, so their 16px glyphs end at 12px.

**The Single Hairline Rule.** Lines are 1 physical pixel of Line, never doubled. Where a header divider meets a plate edge or another rule, there is one line, not two. Plates have no border at all; their tone separates them from the ground.

### Checks
Acceptance criteria for `suite-ui` and the app shell, run as egui_kittest snapshot tests:
- Render the default layout at 1280×720 and 1920×1080, at 100% and 150% scale. In each row of plates, every header divider is at the same y and every header label's baseline is at the same y.
- Switch a panel between one view and several tabs. Its header height and label position don't move.
- Sample along each divider: no line is wider than 1 physical pixel.
- Measure the left edge of the first text or icon in every plate: it is 12px from the plate edge.

## Elevation & Depth

Flat and tonal. Depth comes from three tones: Ground behind, Plate for panels, Raised for whatever sits on top (hover fills, menus, popovers). Wells sit one step below Plate to read as recessed. Nothing inside the window casts a shadow except floating overlays (menus, popovers, dialogs, toasts). Those use one shadow (0 8px 24px, black at 55%) and a 1px Line border.

### Named Rules
**The No Glow Rule.** Nothing glows: no outer glows, gradients, blur or glass. Acid is only ever a flat fill or a 1px line.

## Shapes

Corners are square. The only exception is the cut: a 45° chamfer on the top-left corner of objects you act on or that hold work.
- **8px cut:** plates and dialogs.
- **4px cut:** clips, buttons, menus, popovers, toasts, bin thumbnails.
- **Pointed tag:** the playhead head and marker flags, with both lower corners cut to a point.
- **Square, no cut:** everything you type into (fields, dropdowns), tabs, checkboxes, scrollbars, tooltips, and anything under 20px tall.

45° is the only angle in the system. Hatching on disabled and offline clips runs at 45°, and keyframe diamonds are squares turned 45°.

### Named Rules
**The Cut Marks a Start Rule.** One corner, always the top-left, always 45°. Never cut two corners of one object (pointed tags excepted), never round a corner, never use another angle.

## Components

These specs are for the first `suite-ui` build. The next `/impeccable document` run replaces them with the components as built.

### Panel headers and tabs
- **Structure:** a 32px row on Plate with a Line divider at its bottom. The first label starts at 12px; tabs are 16px apart.
- **Tab:** the panel number in Value numerals, then the name in Label type ("01 MEDIA"). Active: Ink text and a 2px underline sitting on the divider, the width of the label. Inactive: Ink 2, turning Ink on hover.
- **Focus:** when the panel has keyboard focus, the active tab's number and underline turn Acid. Unfocused panels show the number in Ink 3 and the underline in Ink.
- **Actions:** 24px icon buttons at the right, 4px apart.

### Buttons
- **Shape:** 24px tall (32px in dialog footers), 4px cut, 12px side padding, Body Medium.
- **Primary:** Acid fill, Ground text; hover is a slightly lighter Acid, pressed is Acid pressed (#B3DE1E). One per surface: Export in the app bar, or the confirm button in a dialog.
- **Secondary:** Raised fill, Ink text. Hover adds a 1px Line Strong outline that follows the cut. Pressed fills Well.
- **Icon and ghost:** no fill, Ink 2 glyph. Hover fills Raised and turns the glyph Ink. A toggle that's on shows an Ink glyph on Raised.
- **Focus-visible:** a 1px Acid outline 2px outside the shape (Ink on a primary button).
- **Disabled:** Raised fill, Ink 3 text, no hover. **Loading:** the label stays, the button keeps its width, and a 12px spinner replaces the icon.
- **Destructive:** secondary styling with Signal Red text, only in confirmation dialogs.

### Fields
- **Style:** a square Well, 24px tall, no border at rest. An inline prefix (X, Y, W, H, ∠) in Ink 3 Body, then the value in Value numerals and Ink, like the inline-labelled grids in the reference inspectors.
- **States:** hover adds a 1px Line Strong border; editing uses a 1px Acid border; an error uses a 1px Signal Red border with the message in Signal Red Body under the row; disabled shows the value in Ink 3 with no hover.
- **Numeric fields:** drag sideways to scrub, double-click or press Enter to type.
- **Dropdowns:** the same Well with a chevron at the right.

### Inspector rows
A 72px label column (Body, Ink 2), then a grid of fields 4px apart, then a keyframe toggle at the right edge: an 8px diamond in a 20px hit area. Rows are 24px tall and 4px apart. Section heads use the panel header style: Label type, a chevron, and reset and keyframe-all buttons on the right.

### Clips
- **Shape:** the lane height minus 2px at top and bottom, with a 4px cut at the top-left, which is the clip's in-point. Adjacent clips are 1px apart, so every edit point shows.
- **Content:** a 16px name strip in Body Medium Ink, starting 8px in. Video clips show thumbnails below the strip; audio clips show a waveform in a lighter tint of Teal; title clips show only the name.
- **States:** hover lightens the fill slightly. Selected adds a 2px Acid inner outline that follows the cut. A trim edge under the pointer shows a 4px Ink bracket, which turns Acid while dragging. Disabled clips and clips on hidden tracks get 45° Ground hatching at 50% and an Ink 2 name. Offline clips show Signal Red 45° hatching on Well with "Offline" after the name.
- **Keyframes:** 8px diamonds in Ink, Acid when selected. Hold keyframes are squares; eased keyframes are diamonds with their side points cut flat.

### Playhead, ruler, in/out and markers
- **Playhead:** a 1px Acid line through the ruler and every lane, with a 10×12 Acid pointed tag in the ruler. The current time shows in Acid twice: Timecode numerals in the preview's transport row and Value numerals in the timeline's corner cell.
- **Ruler:** Plate, with Line Strong ticks and Small numerals in Ink 3.
- **In/out:** the ruler between in and out is filled with Acid at 20%, and the lanes between them are lifted by Ink at 4%. The in and out points are 1px Acid brackets with 4px feet in the ruler.
- **Markers:** Ink pointed flags in the ruler. A ranged marker adds a 2px Ink bar along its length.

### Track headers
A 160px column on Plate. The track ID (V1, A1) in Value numerals and Ink, the name in Body and Ink 2, then 20px toggles (lock, hide or mute, solo) in Ink 3. A toggle that's on shows an Ink glyph on Raised; solo shows an Amber fill, because it silences everything else.

### Menus, popovers and tooltips
- **Menus and popovers:** Raised, with a 1px Line border, the overlay shadow and a 4px cut. Rows are 24px with Body Ink text and shortcuts right-aligned in Small numerals and Ink 3. Hover fills a row with Line. Separators are 1px Line with 4px above and below.
- **Tooltips:** Raised and square, Body text, with the shortcut in Small numerals. They appear after 500ms.

### Motion and state
- Hover and press are instant; a tool feels faster with no transition on them. Menus, popovers, dialogs and toasts open over 150ms (ease-out, opacity plus a 4px slide) and close over 100ms. Docking and resizing don't animate. The playhead and timeline never ease, because they report time.
- With the OS reduced-motion setting on, overlays appear and disappear instantly.
- Every interactive component defines default, hover, focus-visible, pressed and disabled states, plus loading and error wherever they can occur.

## Do's and Don'ts

### Do:
- **Do** give every panel the same 32px header, and check that the header line runs straight across the window (The One Header Line Rule).
- **Do** start text, icons and header labels 12px in from the plate edge.
- **Do** set every changing number in Martian Mono.
- **Do** keep Acid for the playhead, the current time, focus, selection, in/out and the one primary button.
- **Do** keep the viewer neutral black, with nothing tinted touching the picture.
- **Do** build every control's full set of states: default, hover, focus-visible, pressed and disabled, plus loading and error where they apply.
- **Do** check that Ink clip names reach 4.5:1 on any new clip colour.

### Don't:
- **Don't** round a corner. Cut the top-left corner at 45° (8px on plates, 4px on objects) or leave it square.
- **Don't** add a second header row for tabs, toolbars or search.
- **Don't** use glows, gradients, blur, glass, or shadows on anything that isn't floating.
- **Don't** put clip colours in the chrome or use Acid as decoration.
- **Don't** return to the first prototypes' look: warm greys, an amber accent and Inter.
- **Don't** number panels for decoration; the number is the shortcut.
- **Don't** set controls or values in Archivo Expanded. Wide capitals appear only at Label size, plus the wordmark.
