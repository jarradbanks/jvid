# Reusing speech recognition

Generate Subtitles and Transcript share recognized words in source-file time. Generating
subtitles for an interval makes its spoken words available to Transcript, including fillers;
transcribing first lets Generate Subtitles lay out captions without recognizing the same audio
again. Recognition is local and the existing model download remains optional until there is
unrecognized audio to process.

Transcript shows the current selected clips, or the sequence when nothing is selected. It
places cached words through the current clip trims, positions, speed and repeated uses every
time those edits change. Transcribe requests that scope rather than an entire source stream.
Cached partial words remain visible, and a message identifies missing audio. Only missing
source intervals are recognized; a successfully processed interval with no words is covered
silence, rather than a cache miss. Remove Fillers never treats an unrecognized gap as a pause.

The recognition dialogs reuse compatible results by default and report reuse in a toast. Use
**Recognize Again** to deliberately replace recognition for the requested scope. Changing
language, model, translation, or prompt policy uses another cache identity; choose the desired
settings in Transcribe to fill that scope with compatible results. The spoken-word policy of
Generate Subtitles and Transcript is identical, including the language-specific filler prompt.
Translated captions have their own identity and never supply words for Edit by Text.

Recognition files live in the existing `transcripts` folder under the configured application
cache directory, outside the project. Compatible results survive project/application reopen.
The identity includes the canonical source path, SHA-256 of the complete source bytes, audio
stream index, pinned model weight fingerprint, language/translation options, exact initial
prompt, and a recognizer/timing/cache version. Content is verified off the UI thread before a
recognition run and when first loading an existing cache. Filesystem identity changes discard
live placed words too; a changed or missing source cannot keep displaying its old words.
Legacy transcript caches lack settings and coverage, so they are deliberately ignored.

The persistent JSON includes the full compatibility key and non-overlapping covered intervals,
with words, confidence and detected language per interval. Writes use unique temporary files
and atomic replacement. Truncated, corrupt or incompatible entries are misses. Recognition
that fails or is cancelled adds no coverage; earlier successful intervals from the same run
remain reusable. A source changed while recognition runs cannot accept that job's output.

Caption text and style are editable project data. Editing, undoing, deleting or regenerating a
caption never rewrites the source recognition store. Regenerating captions therefore uses the
original recognized words unless Recognize Again is selected.

## Integration points

- `jvid_transcribe::store::Store` owns persistence, compatibility, interval subtraction and
  source identity. `Settings::version` must change when recognition/timing behavior changes.
- `Subtitles::store` and `TranscriptPanel::cache.store` must share the same `Shared` instance;
  `App::new` wires them using the existing configured transcript-cache directory.
- Before drawing Transcript, the app synchronizes `Cache::set_settings` with
  `Subtitles::transcript_settings()`. Recognition choices retain their existing dialog lifetime;
  after an application restart, selecting matching choices can reuse previously saved entries.
- Both `Done::Edit` and `Done::Transcribed` arrive after the shared store has been populated.
  The app must not reinsert `Done::Transcribed` under default settings. Its payload remains
  available for existing consumers.
- `Cache::placed` retains `(words, missing_sources, languages)`, with words as
  `(ClipId, Word)` in timeline order. Selection, Find, focus and `Output` remain available for
  Transcript Copy/Copy All/export integration. `missing_sources` now contains uncovered spans,
  rather than whole-stream placeholders. Initial persistent-cache loading runs asynchronously.

## Focused validation

`cargo test -p jvid-transcribe --lib` checks exact compatibility inputs, silent/partial coverage,
interval replacement, corrupt persistence and live source mutation. The tests use temporary
source files without models.

`cargo test -p jvid-app --lib subtitles::` counts fake recognizer calls in both directions,
after reopen, after changing language and forced recognition, and around trims, moves, speed
and repeated clips. Real WAV decoding feeds the fake engine; no model is downloaded.

`cargo test -p jvid-app --lib transcript::` checks placed-word selection and ripple cuts,
language-specific fillers, live source changes, and the distinction between an unrecognized
gap and recognized silence.
