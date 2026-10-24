# Translating jvid

jvid ships in English and Spanish, and registers 17 more languages whose translations land one
at a time: French, German, Italian, Brazilian Portuguese, Dutch, Polish, Czech, Turkish,
Russian, Ukrainian, Swedish, Danish, Norwegian Bokmål, Catalan, Japanese, Simplified Chinese
and Korean. Every built-in string the editor shows comes from a catalog, so translating a
language means filling its catalog files, not editing code. The terms an editor sees most are
in the [glossary](i18n/glossary.md); each language keeps its translation of it in
`docs/i18n/<code>.md`.

## Where the strings live

| Catalog | Read by |
| --- | --- |
| `crates/jvid-app/i18n/<code>.toml` | the editor, through `tr!`/`trn!` in `crates/jvid-app/src/i18n.rs` |
| `crates/suite-ui/i18n/<code>.toml` | the shared widgets (search fields, tab overflow, theme errors) |
| `crates/suite-ui/i18n/gallery/<code>.toml` | the component gallery example only |

`en.toml` is the source: every key exists there, and its text is what any other language
falls back to. The files are embedded into the binary with `include_str!`, parsed once on first
use and never read from disk at run time.

## Picking the language

The `language` key in `settings.toml` holds the choice, which Settings (the project menu in
the title bar, then Settings…) writes:

| `language` | Shows |
| --- | --- |
| missing, empty or `system` | the operating system's preferred language, if jvid ships it, else English |
| a code from `LANGUAGES` (`en`, `es`, `pt-BR`, `zh-Hans`, ..) | that language, whatever the system says |
| a regional code such as `es-MX` | its language (`es`); `pt` and `pt-BR` are Brazilian Portuguese, `zh`, `zh-CN` and `zh-Hans-*` Simplified Chinese, `no` Bokmål |
| anything else (`eo`, `pt-PT`, `zh-TW`) | English; the value is kept, so a later build that ships it picks it up |

A pick in Settings applies on the next frame, no restart, and saves at once. Saving changes only
the `language` value in `settings.toml`; comments and every other setting stay as written. A
settings file that isn't valid TOML is left alone and the dialog says so.

The system language comes from [`sys-locale`](https://crates.io/crates/sys-locale): the user's
preferred UI languages on Windows and macOS, and `LANGUAGE`, `LC_ALL`, `LC_MESSAGES` then `LANG`
on Linux. The first one jvid ships wins. A registered language whose catalogs aren't translated
yet is picked like any other and shows in English. To try Spanish without changing settings:

```sh
LANGUAGE=es cargo run -p jvid-app                      # with language = "system"
LANGUAGE=es cargo run -p suite-ui --example gallery   # the gallery always follows the system
```

## Keys

Keys are `section.name` in snake_case. Sections group by place (`menu`, `inspector`,
`export`, `settings`) and become TOML tables:

```toml
[export]
add_to_queue = "Add to Queue"
range = "Range"
```

- Share a key only between places that mean the same thing. Otherwise give each place its own
  key, even when the English matches.
- Translate naturally; length is the layout's problem. Text too long for its spot ends in an
  ellipsis and shows in full on hover, as in other editors.
- A comment above a section or key says where it shows and anything a translator can't see
  from the text: what a placeholder holds, words that must stay as they are.
- English labels use Title Case ("Add to Queue"); descriptions and messages are sentences.
  Other languages follow their own conventions: Spanish labels are sentence case
  ("Añadir a la cola").
- Never rename a key in only one file. The parity test fails when the files disagree.

## Placeholders

`{name}` in a text is filled from a named argument:

```toml
[bin]
remove_title = "Remove “{name}”?"
```

```rust
tr!("bin.remove_title", name = asset.display_name())
```

A translation may move placeholders around or repeat them, but it must use the same set as
English. If code forgets an argument, the text shows `{name}` rather than failing, and the guard
test (below) catches it before that ships. An argument the text doesn't use is ignored, and the
guard flags that too.

Numbers, durations and timecodes are formatted by code and passed in as arguments, so they read
the same in every language.

## Plurals

A text that depends on a count is a small table of forms, chosen by the language's
[CLDR](https://cldr.unicode.org/index/cldr-spec/plural-rules) rule:

```toml
[timeline]
copied.one = "{count} jvid clip"
copied.other = "{count} jvid clips"
```

```rust
trn!("timeline.copied", clips.len())
```

`trn!` passes the count as `{count}`; further named arguments follow it. The forms are:

| Form | English | Spanish |
| --- | --- | --- |
| `one` | 1 | 1 |
| `many` | (unused) | exact millions: "1000000 **de** clips" |
| `other` | everything else, including 0 | everything else, including 0 |

Each language uses the forms of its rule in the
[CLDR chart](https://www.unicode.org/cldr/charts/latest/supplemental/language_plural_rules.html)
(only whole numbers occur, so the fraction conditions don't apply):

| Languages | Forms, for whole numbers |
| --- | --- |
| en, de, nl, tr, sv, da, nb | `one` 1; `other` |
| es, it, ca | `one` 1; `many` exact millions; `other` |
| fr, pt-BR | `one` 0 and 1; `many` exact millions; `other` |
| pl | `one` 1; `few` ending 2–4 but not 12–14; `many` the rest |
| ru, uk | `one` ending 1 but not 11; `few` ending 2–4 but not 12–14; `many` the rest |
| cs | `one` 1; `few` 2–4; `other` |
| ja, zh-Hans, ko | `other` only |

`other` is required, even where the rule never picks it (Polish, Russian, Ukrainian), and a
missing `one`, `few` or `many` falls back to it. In Spanish, give a group a
`many` form whenever `{count}` comes straight before a noun. A form may leave `{count}` out
(`bin.remove_body.one` says "1 clip" in words). The rules live in `plural_category` in
`crates/suite-ui/src/i18n.rs`, the one place to change for a language with other forms, tested
by `wave_one_plural_rules_match_cldr`.

## Fallback

A key missing from a language shows the English text. A catalog that holds no key but
`language.name` is a stub, a language registered before its translation: the parity tests skip
it, so the stubs can merge before any translation does. Once a catalog has any other key it must
have every key. A key missing from English shows the key
itself (`export.add_to_queue`), which is readable and easy to search for. A catalog file that
doesn't parse falls back to English as a whole. None of these panic; the tests below fail
instead.

## What stays as written

These are data, not interface text, and must never be translated:

- anything the user typed or imported: project, clip, marker and track names, text clips,
  subtitles, file names and paths;
- an effect's name, description and parameter names, which its `.wgsl` file declares. The
  three bundled examples are the exception while their file is exactly as shipped: their text
  comes from the `[bundled]` keys. Once the user edits the file, even by a character, or
  saves their own effect under the same file name, it shows as the file says;
- identifiers saved in files: keymap action ids, serialized enum values, effect ids, keyframe
  property names, preset file names, egui ids, the autosave file name. A keyframe lane shows
  a built-in property's `[lane]` caption and a bundled effect's parameter caption, but its
  keyframes stay stored under the name;
- shortcut notation and keycaps (`Ctrl+Shift+Z`, `Space`), file format and codec names
  (`H.264`, `WAV`), and the jvid name.

Code shows the last group through `verbatim("H.264")`, which marks it as reviewed. Logs,
panics and jvid-cli output stay English so bug reports read the same everywhere. Errors from the
operating system, FFmpeg and the TOML parser are shown inside a translated sentence but keep
their own wording.

Switching language redraws the interface, not what was already made from it. Names and text
jvid puts into the project (a text preset's name and words, a solid's name, the Subtitles track)
are written in the language in use at the time and from then on are project data like anything
the user typed; undo and redo bring back that same text. Messages already on screen (error
toasts, the error lines in Settings and Keyboard Shortcuts, the reason on a failed export in
the queue) keep the language they were made in until they're replaced or go away.

## Translating a registered language

The languages above already have stub catalogs: `language.name` in `crates/suite-ui/i18n/<code>.toml`
and nothing else. Copy each `en.toml`'s keys into the three `<code>.toml` files and translate
them, keeping `language.name` as it is. For Japanese, Chinese or Korean, then re-run the CJK subset
script (below) and commit its new face. No code changes.

## Adding a language

1. Add its code to `LANGUAGES` in `crates/suite-ui/src/i18n.rs`. The Settings picker lists
   languages in this order, after System.
2. Copy each `en.toml` to `<code>.toml` in all three catalog folders and translate the values.
   Set `language.name` in `crates/suite-ui/i18n/<code>.toml` to the language's own name
   ("Deutsch"); the picker shows that.
3. Add the new file's `include_str!` to each `Catalog::new([..])`: `crates/jvid-app/src/i18n.rs`,
   `crates/suite-ui/src/i18n.rs` and `crates/suite-ui/examples/gallery.rs`.
4. Add the language's plural rule to `plural_category` if it isn't English's one/other.
5. Run the tests below, then look at the editor in the new language at 100% and 150% scaling
   (the editor asks for a minimum window of 960 × 600 points).

A language whose script isn't Latin also needs a font; see the font policy below first.

## Tests

| Command | Checks |
| --- | --- |
| `cargo test -p jvid-app --test i18n_guard` | no UI text bypasses the catalogs; every `tr!`/`trn!` key exists and gets its placeholders |
| `cargo test -p jvid-app --lib i18n` | jvid-app catalogs match English; every character has a glyph |
| `cargo test -p jvid-app --test i18n` | the editor, a selected clip's Inspector, the bundled effects (bin, search, Add Effect, the Inspector) and keyframe lanes, the export dialog and Settings in Spanish and back without a restart; the export dialog fits 640 points wide, narrower than the 960-point minimum the editor asks for (its height isn't tested). A pseudo-locale (every English text about 40% longer) checks that the editor, Preferences, the Export dialog and the Render Queue cut text too long for its spot with an ellipsis rather than clip it or draw it over other text. The tests that open the whole editor run on Linux only, the one place the test can move its folders |
| `cargo test -p jvid-app --lib -- shipped_examples lane_captions` | bundled effects translate only while their file is as shipped; English lane captions read as before |
| `cargo test -p jvid-app --lib -- settings_file preferences` | the language setting saves and reads back, including older files |
| `cargo test -p suite-ui --test i18n` | live switching, fallback, every language listed by its own name, placeholders, plural rules, glyphs (with Polish, Russian and other samples), the Japanese face loading only while Japanese is shown, the CJK size budget, the gallery in Spanish |
| `cargo test -p suite-ui --lib i18n` | locale matching, each language's CLDR plural rule at 0, 1, 2, 5, 11, 12, 21, 22, 25, 101 and 111 |
| `cargo test -p suite-ui --test gallery gallery_catalogs_match` | gallery catalogs match English |

The tests that switch language sit in their own test binaries because the language is global;
the library's unit tests always run in English, so the English snapshots don't change.

### The guard

`crates/jvid-app/tests/i18n_guard.rs` parses every shipped module of jvid-app, suite-ui and the
gallery with `syn` and fails on a string literal that would reach the screen untranslated:

- any literal with letters passed to something that draws text (`label`, `button`,
  `on_hover_text`, `RichText::new`, `Window::new`, `Dialog::new`, `painter.text`,
  `WidgetInfo::labeled`, ..), directly or through `format!`, `concat!`, `if`/`match` arms or
  helper calls, or held first by a `let`, a `for` over it, a const or static, or returned by
  a function or method of the same file;
- the body of a `macro_rules!` macro, checked as the code it expands to; a macro that draws
  one of its arguments makes that argument drawn text wherever the macro is used;
- prose (a literal with letters that isn't identifier-shaped like `settings.toml` or
  `JVID_LOG`) anywhere else: constants, arrays, struct fields, `Display` impls;
- `{x:?}` in drawn text, which would show a Rust name.

The fix is nearly always `tr!`, or `verbatim(..)` for text that stays as written. The test's
exemptions are by context, each with its reason: logging and panic macros, egui id salts, and a
short list of functions that only name files, write file formats or report to the log, the CLI
or translators. Add to them only for text that can't reach the screen, never to let a
particular string through.

## Fonts and binary size

jvid draws every glyph from fonts it bundles, so a language can only use characters those fonts
have. There is no fallback to system fonts.

- **Latin and Cyrillic**: the three Inter weights are subset to Basic Latin, Latin-1 Supplement,
  Latin Extended-A, basic Cyrillic with Ukrainian Ґ, and a few punctuation marks (see
  `crates/suite-ui/assets/README.md`), about 43 KB each. That covers every wave-1 Latin and
  Cyrillic language, including Polish „quotes”. Romanian's ș ț (Latin Extended-B) and Serbian or
  Kazakh extras are outside it: widen the README's `pyftsubset` ranges for those.
- **CJK**: one Noto Sans CJK Regular face per language (JP, SC, KR), used for every weight, within
  a budget the owner set: **at most 600 KB per CJK language**. A full face is over 15 MB, so each
  is subset to just the characters its language's catalogs use, by
  `crates/suite-ui/assets/subset-cjk.py`; re-run it after every change to a `ja`, `zh-Hans` or `ko`
  catalog and commit the face it writes. It fails past the budget, and so does
  `cjk_faces_stay_within_budget`. A language's face is Inter's fallback only while that language
  is shown, because Japanese, Chinese and Korean draw some of the same Han characters differently;
  a 3 KB face of just 日本語, 简体中文 and 한국어 is always loaded for the language picker. The
  script downloads the source faces on the developer's machine; jvid itself never downloads fonts:
  it works offline and makes no network requests it doesn't need. User text (clip names,
  subtitles) in characters a catalog doesn't use still shows as boxes.
- **Right-to-left scripts** (Arabic, Hebrew) need shaping and bidirectional layout, which egui
  doesn't do yet.

`every_character_has_a_glyph` (in both test suites above) fails when a catalog uses a character
the fonts can't draw, which egui would otherwise show as a box. Two symbols outside the subset
predate translation and still show that way in every language: `≈` before estimated export sizes
and `⏵` after Open Recent. The jvid-app test lists `≈` as known and fails once it draws, so the
exception is removed when the subset grows.
