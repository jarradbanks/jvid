---
version: 1
slug: "crates-jvid-app"
primary_target: "crates/jvid-app"
related_targets: []
---

# Surface: the editor (jvid-app)

**Mode:** Operate. Code-led: built in Rust/egui, so there is no comp gate.

**Job:** open a long recording, find the moment, mark in and out, trim, and export, without hunting for anything.

**Constraints:** the panel layout is the standard editor one (bin left, preview centre, inspector right, timeline bottom). The user approved that layout. They rejected two looks: warm grey with amber, and the Y2K "Clean Cut".

**Memorable moment:** footage that is clearly the brightest thing on screen, inside chrome that lines up exactly.

**Unresolved:** icon set choice, meter design, marker colours.

## Direction contract

THESIS: The editor is a quiet frame around the footage. It has the standard editor layout with a calm, precise finish and refuses both a busy NLE and a stylised theme.

OWN-WORLD: Neutral, faintly cool dark panels with 1px borders, 8px corners and 4px gaps. Inter throughout, in sentence case, with tabular figures. One blue in two tones, for focus, selection, the playhead, in/out and Export. Muted violet, plum, ochre and green clips. Clean 24px inputs with 6px corners.

STORY: The editor trusts that it can drop in a two-hour recording, scrub to the moment, mark I/O and export, always seeing which panel has focus and where the playhead is.

FIRST VIEWPORT: A 40px app bar (wordmark, project, undo/redo, blue Export). Below it three panels: the Media bin, the Source/Program preview on neutral black with a 32px transport row, and the Inspector. The Timeline runs across the bottom 40%, with 160px track headers, a 24px ruler, the in/out band and the light blue playhead. Every header sits on one line.

FORM: User-pinned: a minimal pro tool, a calm, precise finish on the standard editor layout, with subtle radius, clean inputs and clean text. This replaces the rolled Y2K direction (seed:5ce7c33bfc7b3189), which the user rejected.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
