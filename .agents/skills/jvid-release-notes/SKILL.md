---
name: jvid-release-notes
description: Draft or simplify jvid release descriptions into short user-facing changes and direct platform downloads; validate wording against previous releases.
---

# jvid release notes

Write for someone deciding whether to download jvid. Use the repository's `packaging/release-notes-template.md` for the download footer.

Read the release's existing notes, the relevant change range, and any upgrade instructions. Keep a short list of concrete changes users can see. Combine related changes, remove commit prefixes and PR numbers, and omit test fixtures, snapshots, CI, refactors without changed behavior, and publication mechanics. A small hotfix can have one bullet. Do not invent benefits or call an untested candidate stable.

Preserve any action needed to upgrade, supported platform restriction, or known limitation that changes the user's decision. Keep it brief and specific to that release. Put lengthy installation and licensing explanations in the linked packaging guide.

Use direct links to the exact five app packages: Windows installer/portable, Apple Silicon macOS DMG/portable, and x86_64 Linux portable. Mention optional speech-model downloads only when relevant to the release. The new Windows installer offers them during setup; older releases must not claim that feature.

Link corresponding source separately. Future releases use `sources-vVERSION`; historical examples must link to sources that actually exist. Updater metadata, signatures and source reconstruction files are not user download choices.

For candidates, edit `packaging/release-notes.md`. If a frozen `.github/release-plan.json` is present, update its `notes_sha256` to the exact new UTF-8 file bytes and require candidate checks on that new head. Writing notes does not authorize tags, publication, deletion, or workflow dispatch.

## Validation examples

Use `scripts/release/fixtures/previous-releases.json` as the captured original evidence for v0.2.0 and v0.2.1. The drafts in `packaging/release-notes-examples/` show the current preferred style. Refine these with the user; they are examples, not mandatory wording.

Check that v0.2.0 retains transcript copy/text export, SRT export, movable tabs/layout reset and shared transcription results, while dropping internal test changes. Check that v0.2.1 retains both the updater fix and the one-time manual Windows installation instruction. Confirm all five download links match the captured assets and the source link exists. Run `python3 -m unittest discover -s scripts/release -p 'test_notes.py'` after changes to the renderer, template or examples.
