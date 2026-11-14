---
name: jvid
description: A small, fast video editor for cutting gaming highlights and screen recordings.
---

<!-- SEED: established with the user before implementation; re-run /impeccable document once there's code to capture the actual tokens and components. -->

# Design System: jvid

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

## Colors

Restrained: neutral darks with a faint cool tint, one blue accent in two tones, and four muted clip colours. These are starting values. Confirm them in the first `j-ui` build, then re-run `/impeccable document` to make them normative.

### Primary
- **Blue** (#2368C9; hover #2C70D3, pressed #215DB3): fills. Used for the primary button, the focused panel's border, the in/out band (at 25%) and selected rows in lists (at 20%). White text on it is 4.6:1.
- **Light Blue** (#7AACF6): lines and text on dark. Used for the playhead, focus rings, selection outlines on clips, selected keyframes, in/out brackets and links (8:1 on Panel).

### Clips
- **Muted Violet** (#5E589F): video clips.
- **Muted Plum** (#8E4771): text and title clips.
- **Muted Ochre** (#88602D): still images and solid colour layers.
- **Muted Green** (#30715D): audio clips.

Every clip colour carries Ink clip names at 4.8:1 or better, and none sits close enough to the accent blue to be mistaken for selection. A new clip colour has to clear 4.5:1 against Ink.

### State
- **Red** (#EC5B57): errors, offline media, meter clipping at 0 dBFS, destructive confirmations.
- **Amber** (#F2AF48): warnings (a proxy is missing, lossless export isn't available), solo engaged, meter level above −6 dBFS.
- **Green** (#4EBE7D): finished renders, meter level below −6 dBFS.

### Neutral
- **Ground** (#0A0B0D): the window background, seen in the gutters between panels and behind the app bar.
- **Panel** (#121416): every panel's surface, its header included.
- **Raised** (#1C1E21): the active tab, hover fills, secondary buttons, menus, popovers, tooltips.
- **Field** (#0D0E11): input and dropdown fills, and empty timeline lanes.
- **Border** (#232529): panel borders, header dividers, input borders, section rules, track separators.
- **Border Strong** (#36383C): borders on hover.
- **Ink** (#EDEEF1): primary text and values, the active tab, clip names.
- **Ink 2** (#ADB1B8): secondary text, inspector labels, icons at rest.
- **Ink 3** (#82868E): inactive tabs, field prefixes, ruler labels, shortcut hints, placeholders. At 5:1 on Panel it is the dimmest colour allowed for readable text.
- **Ink 4** (#5A5E63): disabled text and icons only.
- **Viewer** (#040404): the neutral, untinted black behind the picture in the preview.

### Themes
Every value above is the Dark theme. Light and High Contrast are the other built-ins, and Edit > Preferences > General picks between them and any theme files.

- **Themes folder:** `themes` in the platform config folder (`~/.config/jvid/themes` on Linux, `%APPDATA%\jvid\config\themes` on Windows). It's created on first run holding Warm Dark and Soft Light, which double as commented templates, and Preferences has an Open Themes Folder button. A file deleted from it stays deleted.
- **Theme files:** one `.toml` per theme, named in Title Case after its file (`nord-night.toml` is Nord Night). Each line sets one token as `"#RRGGBB"`, and a token left out takes its Dark value. A misspelt key or a bad hex is an error, shown on that theme's card in Preferences. Saving the chosen file reloads it without a restart.
- **Token keys:** `blue`, `light_blue`, `red`, `amber`, `green`, `ground`, `panel`, `raised`, `field`, `border`, `border_strong`, `ink`, `ink_2`, `ink_3`, `ink_4`. In code each is `color::<key>()`; nothing else paints chrome.
- **`ink_2` and `ink_4`:** the second and fourth steps of the ink scale. `ink_2` is Ink 2 (secondary text, labels, icons at rest). `ink_4` is Ink 4, for disabled text and icons only, so unlike `ink` and `ink_3`, which must reach 4.5:1 on `panel`, it may fall below that; keep it visibly fainter than `ink_3`. Clip colours and Viewer are fixed and aren't theme tokens.

### Named Rules
**The One Blue Rule.** Blue marks focus, selection, the playhead, the in/out range and the single primary action. Nothing decorative is ever blue, and no other accent exists.

**The Neutral Viewer Rule.** Nothing tinted touches the picture. The viewer surround is neutral black. Overlays on the frame (safe areas, grid, transform handles) are 1px Ink or Light Blue lines.

**The Clips Own Colour Rule.** Clip colours appear only on clips, their keyframe lanes and the type badges in the media bin. They never appear in chrome, buttons or text.

## Typography

**Font:** Inter (OFL), embedded in `j-ui` with tabular figures made the default (frozen in with fonttools' feature freezer), because egui can't switch OpenType features on.

**Character:** One neutral, highly legible UI face at small sizes. Hierarchy comes from weight and colour, not from extra families or capitals. Every digit is tabular, so timecodes and dragged values never change width.

### Hierarchy
Sizes are egui points; the scale steps by about 1.17.
- **Display** (SemiBold 600, 22px, line 28, −0.01em): the welcome screen only.
- **Headline** (SemiBold 600, 16px, line 22): dialog titles.
- **Title** (SemiBold 600, 14px, line 20): dialog section titles, empty-state headings, the app bar wordmark.
- **Body** (Regular 400, 12px, line 16): every control, list row, menu item, message and inspector label.
- **Body Strong** (Medium 500, 12px, line 16): tabs, panel titles, inspector section heads, clip names, button labels.
- **Small** (Regular 400, 10px, line 12): ruler labels, duration badges, shortcut hints, meter scale.
- **Timecode** (Medium 500, 16px): the current time under the preview.

### Named Rules
**The Title Case Rule.** Labels, buttons, menu items, tabs, section heads, property names and dialog titles are in Title Case ("Add Effect", "Go to Start"), with short articles, conjunctions and prepositions lowercase. Sentences such as descriptions, error messages and help text stay in sentence case. Nothing in the UI is set in all capitals or letter-spaced; user content such as title clips keeps whatever case the user typed.

**The Numbers Hold Still Rule.** Every number uses tabular figures, so timecodes, dragged values and meter readouts never shift width under the cursor.

## Layout

Panels sit in the standard editor layout: media bin on the left, preview in the centre, inspector on the right, and the timeline across the bottom, all under a 40px app bar. Docking can rearrange them, and every rule below holds in any arrangement. Units are egui points; every edge lands on a whole physical pixel at every UI scale.

**Spacing scale (4px base):** 4 (gutters, icon-to-label gaps), 8 (between controls, list padding), 12 (the inset inside a panel), 16 (between inspector sections), 24, 32.

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

**The Twelve Inset Rule.** Text and icons inside a panel start 12px from its left edge, header labels included. Tabs are the exception: the tab group starts 4px in, and each label is centred in its pill. Header action buttons sit 8px from the right edge, so their 16px glyphs end at 12px.

**The Single Hairline Rule.** Borders and dividers are 1 physical pixel, never doubled. Where a header divider meets a panel border or another rule, there is one line, not two.

### Checks
Acceptance criteria for `j-ui` and the app shell, run as egui_kittest snapshot tests:
- Render the default layout at 1280×720 and 1920×1080, at 100% and 150% scale. In each row of panels, every header divider is at the same y and every header label's baseline is at the same y.
- Switch a panel between one view and several tabs. Its header height and label position don't move.
- Sample along each divider and border: no line is wider than 1 physical pixel.
- Measure the left edge of the first text or icon in every panel: it is 12px from the panel edge.

## Elevation & Depth

Flat, tonal and bordered. Depth comes from three tones (Ground behind, Panel for panels, Raised for things on top) and 1px Border lines. Fields sit one step below Panel to read as recessed. Nothing inside the window casts a shadow except floating overlays (menus, popovers, dialogs, toasts). Those use one soft shadow (0 8px 24px, black at 40%) on top of their Border.

### Named Rules
**The Flat Inside Rule.** No glows, gradients, blur or glass, and no shadows on anything that isn't floating.

## Shapes

Subtle, consistent rounding from one scale:
- **4px:** clips, checkboxes, badges, tooltips, the playhead head.
- **6px:** buttons, inputs, dropdowns, tab pills, segmented controls, bin thumbnails, hover rows inside menus.
- **8px:** panels, menus, popovers, toasts.
- **12px:** dialogs and the welcome screen's cards.

Keyframes are 8px diamonds (squares turned 45° with 1px rounding). Scrollbars are 4px wide with fully rounded ends.

### Named Rules
**The Radius Scale Rule.** Corners use only 4, 6, 8 or 12px, matched to the object's size, and a nested element's radius is never larger than its container's. No sharp corners on objects, and no pill shapes larger than a scrollbar.

## Components

These specs are for the first `j-ui` build. The next `/impeccable document` run replaces them with the components as built.

### Panels, headers and tabs
- **Panel:** Panel fill, 1px Border, 8px radius. A panel with keyboard focus swaps its border to Blue, because focus decides what shortcuts do.
- **Header:** a 32px row with a Border divider at its bottom.
- **Tabs:** a panel's tabs fill its header. The group runs from 4px in on the left to the header actions, or to 4px from the right edge, and the tabs share that width equally, 4px apart, up to 160px each. Side panels (Media, Inspector) drop the cap and share the full width. A wide panel such as the preview keeps its tabs at 160px, left-aligned. Each tab is a 24px pill centred vertically in the header (4px above and below), with its Body Strong label centred in the pill. Active: Ink text on Raised. Inactive: Ink 3, turning Ink 2 on hover. Tabs that would shrink below 64px move into a chevron menu at the end of the group.
- **Dock tabs:** the same 32px strip, 24px controls, 4px inset and spacing, Body Strong labels, Raised active fill and muted inactive text. Labels truncate to keep each tab at most 160px and never smaller than 64px, including the optional panel's separate 24px close target. Overflow uses a horizontal scrollbar and wheel scrolling. Light Blue drop targets preview grouping or a split. The editor keeps tabs docked in its main window; core panels cannot close. View-selector tabs inside panels still use the chevron overflow menu above.
- **Actions:** 24px icon buttons at the right, 4px apart.

### Buttons
- **Shape:** 24px tall (28px in dialog footers), 6px radius, 12px side padding, Body Strong.
- **Primary:** Blue fill, white text; hover and pressed use Blue's hover and pressed values. One per surface: Export in the app bar, or the confirm button in a dialog.
- **Secondary:** Raised fill with a 1px Border, Ink text. Hover swaps in Border Strong.
- **Ghost and icon:** no fill, Ink 2 text or glyph. Hover fills Raised and turns it Ink. A toggle that's on shows Ink on Raised.
- **Focus-visible:** a 2px Light Blue ring 1px outside the shape, at 60%.
- **Disabled:** Raised fill, Ink 4 text, no hover. **Loading:** the label stays, the button keeps its width, and a 12px spinner replaces the icon.
- **Destructive:** secondary styling with Red text, only in confirmation dialogs.

### Empty states
- **Shared treatment:** Quiet Centre (Option A): an unframed, muted outline icon above a Title heading and one sentence of Body help. Centre the whole group horizontally and vertically within the visible panel body, excluding headers, transport controls and footers.
- **Actions:** show one existing, useful action when the panel can perform it. An empty Media bin offers Import Media; an Inspector awaiting selection explains how to select a clip. Keep action availability consistent with the command's actual prerequisites.
- **Small panels:** wrap the copy and let the content scroll when it cannot fit. Use the finite visible body bounds, including when a parent permits unbounded scroll content. Preserve the panel's existing drop and editing targets.
- **States:** loading, recognition with no speech, errors and offline media retain their own messages. Preview empty states keep the neutral black viewer and readable ink in every theme.

### Dialog action footers
- Dialogs and popups with action footers use the shared content-sized footer. Place secondary and cancellation actions first in reading and keyboard order, with the primary action last at the bottom right.
- Wrap action rows and long translated labels when necessary, keeping keyboard focus stable as the layout changes. Footer layout never claims spare vertical space between the body and its actions.
- Model download controls and ordinary menu rows keep their own row alignment; the footer rule applies to the surface's closing or confirming actions.

### Inputs
- **Style:** 24px tall, 6px radius, Field fill, 1px Border, 8px side padding. An optional inline prefix (X, Y, W, H) in Ink 3, then the value in Ink.
- **States:** hover swaps in Border Strong. Focus uses a Blue border plus the Light Blue focus ring. An error uses a Red border, with the message in Red Body under the row. Disabled drops the border and shows Ink 4 text.
- **Numeric inputs:** drag sideways to scrub; double-click or press Enter to type.
- **Dropdowns:** the same input with a chevron at the right.
- **Search:** the same input with a search icon as its prefix and an Ink 3 placeholder.

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

### Menus, popovers and tooltips
- **Menus and popovers:** Raised, 1px Border, 8px radius, the overlay shadow, 4px internal padding. Rows are 24px with 6px-radius hover fills in Border colour, Body Ink text, and shortcuts right-aligned in Small Ink 3. Separators are 1px Border with 4px above and below.
- **Tooltips:** Raised, 4px radius, Body text, with the shortcut in Small Ink 3. They appear after 500ms.

### Motion and state
- Hover and press colours fade over 100ms. Menus, popovers, dialogs and toasts open over 150ms (ease-out, opacity plus a 4px slide) and close over 100ms. Docking and resizing don't animate. The playhead and timeline never ease, because they report time.
- With Reduce Motion on (Edit > Preferences > General), hover fades, overlays and the New Track slot change instantly. Following the OS setting is a follow-up.
- Every interactive component defines default, hover, focus-visible, pressed and disabled states, plus loading and error wherever they can occur.

## Reference: Components as Built

This section maps every rule above to the code that implements it, so a change starts from an existing helper instead of a new one. Tokens and widgets live in [j-ui](https://github.com/jarradbanks/j-ui); reach for them before painting anything by hand. If a pattern you need is missing, add it to `j-ui` and list it here.

### Tokens
| Token | Code | Values |
| --- | --- | --- |
| Colours | `tokens::color::<key>()` (themed), `color::CLIP_*`, `color::VIEWER` | See Colors. Never a literal hex in app code. |
| Type scale | `theme::display()`, `headline()`, `title()`, `body()`, `body_strong()`, `small()`, `timecode()`, `content()` (user text) | 22 / 16 / 14 / 12 / 12 Medium / 10 / 16 Medium |
| Spacing | `tokens::space::{S4, S8, S12, S16, S24, S32, INSET, GUTTER}` | 4, 8, 12, 16, 24, 32; inset 12, gutter 4 |
| Radius | `tokens::radius::{SMALL, CONTROL, PANEL, DIALOG}` | 4, 6, 8, 12 |
| Sizes | `tokens::size::{APP_BAR, HEADER, RULER, CONTROL, SMALL_CONTROL, ICON, TRACK, ...}` | 40, 32, 24, 24, 20, 16, 40 |
| Icons | `theme::icon::*` (Lucide subset), `theme::icon_font()`, `widgets::paint_icon` | 16px in controls; 28px for empty-state and tile art only |

### Controls
| Pattern | Use | Notes |
| --- | --- | --- |
| Buttons | `widgets::Button::{primary, secondary, ghost, destructive, icon}` | 24px tall, 28px in dialog footers (the footer sets it). `.small()` is the 20px inspector tool slot. `.on(bool)` for ghost toggles, `.state(State::Disabled)` to disable, `.loading(true)` keeps the width. One Primary per surface. |
| Icon button | `Button::icon(glyph, name)` | 24px square ghost. `name` is the accessible name; always add a tooltip with the same name (and the shortcut when there is one). |
| Text input | `widgets::TextInput::new`, `.hint()`, `.prefix()`, `.error(Some(msg))`, `TextInput::search` | 24px, Field fill. `.error(Some(""))` reddens the border without a message line, for inline fields with no room below (put the message in the tooltip). |
| Numeric | `widgets::DragField`, `widgets::Slider` | Drag to scrub, double-click or Enter to type. |
| Dropdown | `widgets::Dropdown::new(&mut i, &options)`, `.width()`, `.placeholder()` | A fixed width keeps rows from reflowing as the selection changes. |
| Segmented control | `widgets::Segmented::new(&mut i, &labels)` or `::icons` | Use for two to four mutually exclusive views, such as the Media panel's All Media / Bins. Not for actions. |
| Checkbox | `widgets::Checkbox::new(&mut on, label)` | Label right of the box. |
| Section head | `widgets::SectionHeader` | The 32px collapsible inspector head. |
| Timecode | `widgets::Timecode`, `widgets::timecode()` | Tabular, never shifts width. |
| Fitting text | `widgets::fit_text` (+ `hover_full`) | Truncates with an ellipsis to a width; show the full text on hover. |
| Menu row | `widgets::MenuRow`, egui `ui.button` inside `Popup::menu` / `context_menu` | 24px rows, 4px menu padding, 8px radius (set globally in `theme.rs`). |

### Surfaces
| Pattern | Use | Notes |
| --- | --- | --- |
| Panel | `panel::Panel::{titled, tabbed, body, toolbar}` | One 32px header; `.actions()` for header icon buttons. |
| Empty state | `empty_state::EmptyState::new(icon, title, help)`, `.action()`, `.primary_action()` | Centred in the visible body; 28px icon, Title heading, one Body sentence. |
| Dialog frame | `dialog::modal(ctx, id, title, width, \|ui\| ...)` | The one modal frame: centred over the scrim, 12px radius, 16px padding, Headline title, width capped to the window less 24px a side. Esc and a backdrop click set `should_close`. |
| Dialog actions | `dialog::action_footer(ui, [secondary.., primary])` | Right-aligned; cancel first, primary last (rightmost). Call after the body, with `S16` above it. |
| Confirm dialog | `dialog::Dialog { title, body, buttons, cancel, long }.show(ctx)` | Built on `modal` + `action_footer`. `buttons[0]` is drawn rightmost and focused, so Enter picks it. Destructive confirms use `ButtonKind::Destructive` for `buttons[0]` and say in the body what is kept. |
| Popover / menu | `egui::Popup::menu(&response)`, `response.context_menu` | Anchored to the control that opened it. Raised, 1px Border, 8px radius, overlay shadow. |
| Tooltip | `.on_hover_text()`, `.on_disabled_hover_text()` | Raised, 4px radius. A disabled control explains why in its tooltip. |
| Toast | `App::show_toasts` | Errors only: bottom-right, 16px from the edges, stacked 8px apart, Raised, 12px padding, Red alert glyph, a dismiss icon button, 6s. |

### Media tiles and list rows
- **Grid:** tiles at least 120px wide (`TILE_MIN`), 8px apart, sharing the panel width equally. Each tile is a 16:9 art area (6px radius) over a 24px name row. Hover outlines the art in Border Strong; selection outlines it 2px Light Blue. Badges (duration, HDR) sit 2px inside the art's bottom corners in Small on Ground at 80%.
- **List:** 24px rows: an 8px type dot, a 16:9 thumbnail 20px tall, the name, then right-aligned Ink 3 details. Hover fills Raised; selection fills Blue at 20%.
- **Bin tiles:** a bin is a tile like a media tile: Field art, Border outline, 28px Folder glyph, its name and media count. New Bin is the same tile with a Plus glyph. Tiles and media share one grid, so sizes and keyboard order never differ. Double-click opens; right-click offers Rename Bin and Delete Bin; media dropped on a tile files it, with a 2px Light Blue outline while it hovers.
- **Inline rename:** the name row becomes a `TextInput` in place. Enter or a click elsewhere commits, Esc cancels; an invalid name reddens the field and explains in its tooltip. Request focus once, when the field opens: asking every frame drops keystrokes typed quickly. Prefer this to a dialog for naming things the user can see.
- **Breadcrumb:** inside a container (a bin), a 24px row under the view switch: a ghost button back to the parent, a Chevron Right in Ink 3, then the current name in Body Strong. Dropping media on the parent crumb moves it out.

### Named Rules
**The One Line Row Rule.** Everything in a row sits vertically centred on one line: build rows with `Layout::left_to_right(Align::Center)` at `size::CONTROL` height, and centre painted text on the row's `center().y` (as `widgets::fit_text` users do). Icons, labels, checkboxes, dropdowns and buttons in one row never sit on different baselines.

**The Anchored Or Centred Rule.** A floating surface is either anchored to what opened it (menus, popovers, dropdowns, tooltips, inline fields) or a centred modal over the scrim built with `dialog::modal`. Never a free-floating window somewhere else on screen, and never a control floating over the picture that belongs in a toolbar.

**The Right Actions Rule.** Dialog and popover actions are right-aligned through `dialog::action_footer`, cancellation first and the primary or destructive action last.

**The Passive Status Rule.** Read-only state is a label (Body, Ink 3, Amber when it needs attention) with details in its tooltip, never a button that opens nothing useful. A label shows only when it says something the user can act on or needs to know; the preview's media source caption shows only while proxies are on or playback is struggling, beside the Proxies checkbox it describes.

**The Case Rule.** Short labels, buttons, menu items, tabs, tooltips' first line and dialog titles are Title Case; descriptions, help, errors and confirmation bodies are sentence case (see The Title Case Rule).

### Where things live in jvid
- **Preview row:** time, transport, then the view controls: guides, the status caption, Proxies, preview resolution (Full, ½, ¼), View Selection (zooms to the selected clip's bounds) and zoom (Fit, percentages). Narrow previews move the view controls into the row's More menu.
- **Media panel:** search, sort and grid/list on the first row; the All Media / Bins switch and Filters on the second; a breadcrumb inside a bin; Import and Add Solid pinned at the bottom.

## Do's and Don'ts

### Do:
- **Do** give every panel the same 32px header, and check that the header line runs straight across the window (The One Header Line Rule).
- **Do** start text, icons and header labels 12px in from the panel edge.
- **Do** take every corner from the 4, 6, 8, 12px scale.
- **Do** keep blue for focus, selection, the playhead, in/out and the one primary button.
- **Do** keep the viewer neutral black, with nothing tinted touching the picture.
- **Do** build every control's full set of states: default, hover, focus-visible, pressed and disabled, plus loading and error where they apply.
- **Do** check that Ink clip names reach 4.5:1 on any new clip colour.

### Don't:
- **Don't** add a second header row for tabs, toolbars or search.
- **Don't** set labels in capitals or letter-space them.
- **Don't** add a second typeface or a second accent colour.
- **Don't** use glows, gradients, blur, glass, or shadows on anything that isn't floating.
- **Don't** put clip colours in the chrome or use blue as decoration.
- **Don't** return to either rejected look: warm greys with an amber accent, or cut corners, wide capitals and an acid accent.
