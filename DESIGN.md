# jvid design profile

Read the shared suite system through `~/.agents/design.md` (Home `DESIGN.md`)
and use the shared `design` skill. This file owns the editor-specific profile;
shared visual rules are maintained in Home, and widgets/tokens in j-ui.

## Current implementation

jvid currently pins j-ui v0.2.0. Keep its compact Inter typography, tabular
figures, 24pt controls and editor-specific layout while refining it. Lucide
comes from j-ui’s icon font and `widgets::paint_icon`; all visible strings use
the existing localisation catalog. A different app’s font/density is not a
reason to restyle this editor.

## Overview

**Creative North Star: "The Quiet Frame"**

jvid is a quiet frame around the footage. It has the standard pro-editor layout and conventions, with a calm, precise finish: neutral dark surfaces, thin borders, subtle rounded corners, clean inputs, one typeface and one blue accent. The chrome steps back so the picture is the brightest, most colourful thing on screen, and the polish shows in the details: exact alignment, consistent spacing, real hover and focus states, and numbers that don't jitter.

Density is compact (12px text, 24px controls) to fit as much timeline on screen as possible. Panels sit where pro editors expect them, so an editor knows where everything is before they notice anything about the style.

The usual scene is an evening at a desk after a gaming session, or a screen recording made beside the app, often in a dim room. So the UI is dark, the viewer is neutral black, and colour in the chrome is spent only on meaning.

Two looks have been tried and rejected: warm greys with an amber accent felt generic, and a Y2K tech-graphic direction (cut corners, wide capitals, acid accent) was too stylised.

**Key Characteristics:**
- Neutral, faintly cool dark surfaces separated by thin borders and 4px gaps
- Subtle rounding: 4px on small things, 6px on controls, 8px on panels
- One header height for every panel, so header lines run straight across the window
- Inter for everything, with tabular figures so numbers hold still
- One blue accent for focus, selection, the playhead and the primary action
- Muted clip colours that stay out of the accent's way

## Clip colours
- **Muted Violet** (#5E589F): video clips.
- **Muted Plum** (#8E4771): text and title clips.
- **Muted Ochre** (#88602D): still images and solid colour layers.
- **Muted Green** (#30715D): audio clips.

Every clip colour carries Ink clip names at 4.8:1 or better, and none sits close enough to the accent blue to be mistaken for selection. A new clip colour has to clear 4.5:1 against Ink.

## Theme files
Every value above is the Dark theme. Light and High Contrast are the other built-ins, and Edit > Preferences > General picks between them and any theme files.

- **Themes folder:** `themes` in the platform config folder (`~/.config/jvid/themes` on Linux, `%APPDATA%\jvid\config\themes` on Windows). It's created on first run holding Warm Dark and Soft Light, which double as commented templates, and Preferences has an Open Themes Folder button. A file deleted from it stays deleted.
- **Theme files:** one `.toml` per theme, named in Title Case after its file (`nord-night.toml` is Nord Night). Each line sets one token as `"#RRGGBB"`, and a token left out takes its Dark value. A misspelt key or a bad hex is an error, shown on that theme's card in Preferences. Saving the chosen file reloads it without a restart.
- **Token keys:** `blue`, `light_blue`, `red`, `amber`, `green`, `ground`, `panel`, `raised`, `field`, `border`, `border_strong`, `ink`, `ink_2`, `ink_3`, `ink_4`. In code each is `color::<key>()`; nothing else paints chrome.
- **`ink_2` and `ink_4`:** the second and fourth steps of the ink scale. `ink_2` is Ink 2 (secondary text, labels, icons at rest). `ink_4` is Ink 4, for disabled text and icons only, so unlike `ink` and `ink_3`, which must reach 4.5:1 on `panel`, it may fall below that; keep it visibly fainter than `ink_3`. Clip colours and Viewer are fixed and aren't theme tokens.

## Layout

Panels sit in the standard editor layout: media bin on the left, preview in the centre, inspector on the right, and the timeline across the bottom, all under a 40px app bar. Docking can rearrange them, and every rule below holds in any arrangement. Units are egui points; every edge lands on a whole physical pixel at every UI scale.


| Size | Value | Used for |
| --- | --- | --- |
| App bar | 40px | The window's top bar |
| Header | 32px | Every panel header and tab bar, inspector section heads, the preview's transport row |
| Ruler | 24px | The timeline ruler and the corner cell beside it |
| Marker lane | 16px | The Markers lane directly above the timeline ruler |
| Control | 24px | Buttons, inputs, dropdowns, tab pills, list rows, menu rows, inspector property rows |
| Tab | up to 160px | Tab width; tabs share the header equally up to this, at least 64px |
| Small control | 20px | Track-header toggles |
| Icon | 16px | Every glyph, centred in its control |
| Gutter | 4px | Between panels, and between panels and the window edge |
| Inset | 12px | Left and right padding inside a panel and its header |
| Track header | 160px | Width of the timeline's track-header column |
| Track | 40px | Default lane height; resizable from 24 to 120 in 4px steps |
| Inspector label | 72px | Width of the inspector's label column |
| Panel minimum | 240px | Narrowest a docked panel may get |

### Named Rules
**The One Header Line Rule.** Every dock group starts with a 32px tab strip and a 1px Border divider. Panels side by side start at the same y, so their strips, dividers and title baselines line up. Even a single panel keeps its dock tab visible and draggable. Editing controls never share the dock drag target: Timeline's tools sit in a 32px left rail below the strip, beside the track headers, and Render Queue's actions sit in a separate 32px toolbar inside its panel. The rail groups selection, razor and text tools above snapping and linking, with zoom controls below; short panels put the remaining actions in an overflow menu. Media, Preview and Inspector keep their view selectors inside the panel as well; an empty Inspector and Transcript need no second title row. Search and filters stay in the content. Standalone panels outside the editor dock retain their title or view header.

**The Rows Face Each Other Rule.** Rows that sit side by side line up. The timeline ruler and its corner cell are one 24px row. Each track header is exactly as tall as its lane and starts at the same y. Inspector section heads use the 32px header height, so a fully collapsed inspector reads as a stack of headers.





### Checks
Acceptance criteria for `j-ui` and the app shell, run as egui_kittest snapshot tests:
- Render the default layout at 1280×720 and 1920×1080, at 100% and 150% scale. In each row of panels, every header divider is at the same y and every header label's baseline is at the same y.
- Switch a panel between one view and several tabs. Its header height and label position don't move.
- Sample along each divider and border: no line is wider than 1 physical pixel.
- Measure the left edge of the first text or icon in every panel: it is 12px from the panel edge.

## Editor components

### Panels, headers and tabs
- **Panel:** Panel fill, 1px Border, 8px radius. A panel with keyboard focus swaps its border to Blue, because focus decides what shortcuts do.
- **Header:** a 32px row with a Border divider at its bottom.
- **Tabs:** a panel's tabs fill its header. The group runs from 4px in on the left to the header actions, or to 4px from the right edge, and the tabs share that width equally, 4px apart, up to 160px each. Side panels (Media, Inspector) drop the cap and share the full width. A wide panel such as the preview keeps its tabs at 160px, left-aligned. Each tab is a 24px pill centred vertically in the header (4px above and below), with its Body Strong label centred in the pill. Active: Ink text on Raised. Inactive: Ink 3, turning Ink 2 on hover. Tabs that would shrink below 64px move into a chevron menu at the end of the group.
- **Dock tabs:** the same 32px strip, 24px controls, 4px inset and spacing, Body Strong labels, Raised active fill and muted inactive text. Labels truncate to keep each tab at most 160px and never smaller than 64px, including the optional panel's separate 24px close target. Overflow uses a horizontal scrollbar and wheel scrolling. Light Blue drop targets preview grouping or a split. The editor keeps tabs docked in its main window; core panels cannot close. View-selector tabs inside panels still use the chevron overflow menu above.
- **Actions:** 24px icon buttons at the right, 4px apart.

### Inspector rows
A 72px label column (Body, Ink 2), then inputs 4px apart, then a keyframe toggle at the right edge: an 8px diamond in a 20px hit area. Rows are 24px tall and 4px apart. Section heads use the 32px header row: Body Strong in Ink, a chevron, and reset and keyframe-all buttons on the right.

### Clips
- **Shape:** the lane height minus 2px at top and bottom, 4px radius, with 2px between adjacent clips, so every edit point shows.
- **Content:** a 16px name strip in Body Strong and Ink, starting 6px in. Video clips show thumbnails below the strip, audio clips show a waveform in a lighter tint of Muted Green, and title clips show only the name.
- **States:** hover lightens the fill slightly. Selected adds a 2px Light Blue inner outline. A trim edge under the pointer shows a 4px Ink bracket, which turns Light Blue while dragging. Disabled clips and clips on hidden tracks drop to 40% opacity. Offline clips show a Red fill at 30% with "Offline" after the name.
- **Keyframes:** Ink diamonds, Light Blue when selected. Hold keyframes are squares and eased keyframes are circles.

### Playhead, ruler, in/out and markers
- **Playhead:** a 1px Light Blue line through the ruler and every lane, with a 10×12 Light Blue head in the ruler (rounded top, pointed bottom). The current time shows in Timecode type in the preview's transport row and in Body Strong in the timeline's corner cell.
- **Ruler:** Panel fill, Border Strong ticks, Small labels in Ink 3.
- **In/out:** the ruler between in and out is filled with Blue at 25%, and the lanes between them are lifted by Ink at 3%. The in and out points are 1px Light Blue brackets.
- **Markers:** small flags in their own 16px lane (Panel fill) directly above the ruler, which is left for scrubbing. A ranged marker fills its length in the lane and marks its end with a 1px line. Drag a flag or a range's body to move it, a range's end to resize it.

### Track headers
A 160px column on Panel. The track ID (V1, A1) in Body Strong and Ink, the name in Body and Ink 2, then 20px ghost toggles (lock, hide or mute, solo). A toggle that's on shows Ink on Raised; solo uses an Amber fill, because it silences everything else.

### Motion and state
- Hover and press colours fade over 100ms. Menus, popovers, dialogs and toasts open over 150ms (ease-out, opacity plus a 4px slide) and close over 100ms. Docking and resizing don't animate. The playhead and timeline never ease, because they report time.
- With Reduce Motion on (Edit > Preferences > General), hover fades, overlays and the New Track slot change instantly. Following the OS setting is a follow-up.
- Every interactive component defines default, hover, focus-visible, pressed and disabled states, plus loading and error wherever they can occur.

### Empty states and footers

Media offers its existing Import action; an Inspector awaiting selection explains
how to select a clip. Loading, no-speech recognition, failure and offline media
retain their own states. Keep the preview surround neutral black in every theme.
Model download controls and ordinary menu rows retain their row alignment;
shared action footers apply to closing/confirming a surface.

### Notifications

The existing `App::show_toasts` displays errors at bottom right, 16pt from the
edges, on Raised with a red alert icon, dismiss action and six-second lifetime.
Use current shared modal/footer helpers rather than introducing a floating window.

### Media tiles and list rows
- **Grid:** tiles at least 120px wide (`TILE_MIN`), 8px apart, sharing the panel width equally. Each tile is a 16:9 art area (6px radius) over a 24px name row. Hover outlines the art in Border Strong; selection outlines it 2px Light Blue. Badges (duration, HDR) sit 2px inside the art's bottom corners in Small on Ground at 80%.
- **List:** 24px rows: an 8px type dot, a 16:9 thumbnail 20px tall, the name, then right-aligned Ink 3 details. Hover fills Raised; selection fills Blue at 20%.
- **Bin tiles:** a bin is a tile like a media tile: Field art, Border outline, 28px Folder glyph, its name and media count. New Bin is the same tile with a Plus glyph. Tiles and media share one grid, so sizes and keyboard order never differ. Double-click opens; right-click offers Rename Bin and Delete Bin; media dropped on a tile files it, with a 2px Light Blue outline while it hovers.
- **Inline rename:** the name row becomes a `TextInput` in place. Enter or a click elsewhere commits, Esc cancels; an invalid name reddens the field and explains in its tooltip. Request focus once, when the field opens: asking every frame drops keystrokes typed quickly. Prefer this to a dialog for naming things the user can see.
- **Breadcrumb:** inside a container (a bin), a 24px row under the view switch: a ghost button back to the parent, a Chevron Right in Ink 3, then the current name in Body Strong. Dropping media on the parent crumb moves it out.

### Where things live in jvid
- **Preview row:** time, transport, then the view controls: guides, the status caption, Proxies, preview resolution (Full, ½, ¼), View Selection (zooms to the selected clip's bounds) and zoom (Fit, percentages). Narrow previews move the view controls into the row's More menu.
- **Media panel:** search, sort and grid/list on the first row; the All Media / Bins switch and Filters on the second; a breadcrumb inside a bin; Import and Add Solid pinned at the bottom.

The passive preview source caption appears only while proxies are on or playback
is struggling, beside the Proxies checkbox; details remain in its tooltip.

## References and verification

Read `PRODUCT.md`, `docs/PLAN.md` and `docs/prototypes/v4/png/index.png` plus its
sibling screens. Keep the Layout > Checks contract above and the commands in
`AGENTS.md`. Use `polish` and actual rendered egui checks across the stated
window/DPI sizes. The shared design skill’s Rust reference maps common j-ui
APIs and component tests; inspect the version actually pinned by this app.
