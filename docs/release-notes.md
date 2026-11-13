# jvid release-note facts

Use Home’s shared `release-notes` skill. Edit `packaging/release-notes.md`;
`packaging/release-notes-template.md` and `scripts/release/notes.py` own the
rendered body/footer. The version and release range come from the actual
candidate/release inputs, not an old example or an auxiliary source release.

- Downloads are the exact five user packages: Windows installer/portable,
  Apple Silicon macOS DMG/portable, and x86_64 Linux portable. Use the template’s
  actual names/URLs for the selected version. There is no fixed product heading
  requirement in the rendered body.
- Link corresponding source separately. New source releases use
  `sources-vVERSION`; historical examples must link only to sources that exist.
  Updater metadata, signatures and source reconstruction files are not app choices.
- Mention speech-model downloads only when relevant. The new Windows installer
  can offer them; do not claim that behaviour for historical installers.
- Preserve the one-time manual Windows installation instruction when describing
  the historical updater fix, and other upgrade restrictions relevant to the release.
- If `.github/release-plan.json` is frozen, update `notes_sha256` to the exact
  new UTF-8 file bytes and require candidate checks on the resulting head.
  This is not permission to tag, publish, delete or dispatch a workflow.

Captured v0.2.0/v0.2.1 evidence is in
`scripts/release/fixtures/previous-releases.json`; preferred editorial examples
are under `packaging/release-notes-examples/`. The first example retains transcript
copy/text export, SRT export, movable tabs/layout reset and shared transcription
results. The second retains both the updater fix and manual-install action.
Examples guide style, not mandatory wording for every release.

After changing the renderer, template or examples, run
`python3 -m unittest discover -s scripts/release -p 'test_notes.py'`.
Read `docs/updating.md` and the existing release tools for execution procedures.
