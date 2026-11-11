# translating jvid

every string jvid shows comes from a catalog, so translating means filling in TOML, not touching code. english and spanish are done; 17 more are registered and waiting. the most common terms are in the [glossary](i18n/glossary.md), and each language keeps its own version in `docs/i18n/<code>.md`.

## where the strings live

| catalog | used by |
| --- | --- |
| `crates/jvid-app/i18n/<code>.toml` | the editor |
| `../j-ui/i18n/<code>.toml` | shared widgets, in the [j-ui repo](https://github.com/jarradbanks/j-ui) |
| `../j-ui/i18n/gallery/<code>.toml` | j-ui's gallery example only |

`en.toml` is the source. anything missing in another language falls back to english.

to try a language: `LANGUAGE=es cargo run -p jvid-app`, or pick it in **Settings…** - it switches live.

## keys

`section.name`, snake_case. sections become TOML tables:

```toml
[export]
add_to_queue = "Add to Queue"
```

- only share a key when it means the same thing in both places.
- translate naturally. text that's too long gets an ellipsis.
- english labels are Title Case; other languages follow their own rules.
- never rename a key in just one file - the parity test will catch you.

## placeholders and plurals

`{name}` gets filled from a named argument. move it around or repeat it, but keep the same set as english:

```toml
remove_title = "Remove “{name}”?"
```

counts use [CLDR plural forms](https://www.unicode.org/cldr/charts/latest/supplemental/language_plural_rules.html):

```toml
copied.one = "{count} jvid clip"
copied.other = "{count} jvid clips"
```

`other` is always required. the rules live in `plural_category` in `../j-ui/src/i18n.rs`.

## leave these alone

anything the user typed, effect names from `.wgsl` files, saved identifiers, shortcut keys (`Ctrl+Shift+Z`), codec names (`H.264`) and "jvid". logs and CLI output stay english so bug reports read the same.

## translating a registered language

the stubs only have `language.name`. copy each `en.toml`'s keys into the three `<code>.toml` files and translate. for japanese, chinese or korean, re-run `../j-ui/assets/subset-cjk.py` and commit the new font face.

## adding a new language

1. add its code to `LANGUAGES` in `../j-ui/src/i18n.rs`.
2. copy `en.toml` to `<code>.toml` in all three catalog folders and translate. set `language.name` to its own name ("Deutsch").
3. add the `include_str!` to each `Catalog::new([..])` - in `crates/jvid-app/src/i18n.rs`, `../j-ui/src/i18n.rs` and `../j-ui/examples/gallery.rs`.
4. add its plural rule to `plural_category` if it isn't plain one/other.
5. run the tests, then eyeball the editor at 100% and 150% scaling.

## tests

```sh
cargo test -p jvid-app --test i18n_guard   # no hardcoded UI text, every key exists
cargo test -p jvid-app --lib i18n          # catalogs match english, every glyph exists
cargo test -p jvid-app --test i18n         # live switching, layout with long text
cargo test --manifest-path ../j-ui/Cargo.toml --test i18n
```

the guard parses jvid-app's source and fails on any string literal that would reach the screen untranslated. fix with `tr!`, or `verbatim(..)` for things that stay as written. don't add exemptions to sneak a string through.

## fonts

jvid only draws from fonts it bundles - no system fallback. inter covers latin and cyrillic. CJK gets a subset noto face per language, capped at 1 MB each, built by `subset-cjk.py`. right-to-left scripts aren't supported yet, egui doesn't do the shaping.
