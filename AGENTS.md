# Agent Rules

## Architecture & Scope
- `jvid-core` stays free of UI and I/O.
- Architecture and crate responsibilities live in `docs/PLAN.md`. Do not add a dependency outside `docs/PLAN.md`'s stack without asking.
- Roadmap and issue tracking live in `docs/ROADMAP.md` (generated from `scripts/roadmap/spec.py` + `scripts/roadmap/created.json`; ready queue via `python3 .orchestrator/ready.py`). Workers do not edit `docs/ROADMAP.md` or `scripts/roadmap/` unless the task explicitly targets them.

## UI (`jvid-app` & `j-ui`)
- Read `PRODUCT.md` and `DESIGN.md` for anything user-facing, and verify UI changes with the `polish` skill.
- Visual reference is `docs/prototypes/v4/png/index.png` (and sibling v4 screens); where an older mockup and `DESIGN.md` disagree, `DESIGN.md` wins.
- `DESIGN.md` Layout > Checks must pass as `egui_kittest` snapshot/geometry tests across `1280×720` and `1920×1080` at `1.0` and `1.5` scale.
- Title Case for short labels (buttons, headings, tabs, menu items, panel titles, property names, dialog titles); sentence case for sentences (descriptions, empty-state text, hints, errors). All user-facing strings go through the `j-ui` i18n catalog.

## Verification
- Required checks before finishing:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
- Forced GPU snapshot tests on Linux (lavapipe):
  - `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json WGPU_BACKEND=vulkan JVID_GPU_TESTS=required cargo test -p jvid-app -- --test-threads=1`
