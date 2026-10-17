---
version: 1
slug: "crates-jvid-app"
primary_target: "crates/jvid-app"
related_targets: []
---

# Surface: the editor (jvid-app)

**Mode:** Operate. Code-led: built in Rust/egui, so there is no comp gate. The mockups in `docs/prototypes/clean-cut/` are references, not contracts.

**Job:** open a long recording, find the moment, mark in and out, trim, and export, without hunting for anything.

**Constraints:** the panel layout follows Premiere and the first prototypes (bin left, preview centre, inspector right, timeline bottom). The user approved that layout and rejected only its look.

**Memorable moment:** one straight header line across the window, with the focused panel's number in acid.

**Unresolved:** the final clip colour for stills, icon set choice, meter design.

## Direction contract

THESIS: The editor is a cutting bench of flat plates on a dark ground. It refuses the category default of grey panels with one blue accent, and the rounded-card web editors.

OWN-WORLD: Cool near-black plates on a darker ground, 4px apart. Wide Archivo Expanded capitals prefixed with Shift+number focus shortcuts. Martian Mono for every number. Acid yellow-green only for here, picked and the primary action. Ultramarine, violet, magenta and teal clips. Square corners with a 45° top-left cut on objects.

STORY: The editor trusts that it can drop in a two-hour recording, scrub to the moment, mark I/O and export, always seeing which panel has focus and where the playhead is.

FIRST VIEWPORT: A 40px app bar (wordmark, project, undo/redo, acid Export). Below it three plates: 01 MEDIA bin, 02/04 SOURCE/PROGRAM preview on neutral black with a 32px transport row and acid timecode, and 05 INSPECTOR. 03 TIMELINE across the bottom 40%, with 160px track headers, a 24px ruler, the in/out band and the acid playhead. Every header sits on one line.

FORM: Y2K tech-graphic (PS1-era game menus and livery, after The Designers Republic and wipEout). Ranked first on the grounded list; the user chose it as Impeccable's pick over the assigned third. seed:5ce7c33bfc7b3189

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
