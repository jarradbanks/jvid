//! Fails when user-visible text bypasses the translation layer (#210).
//!
//! Parses every non-test module of jvid-app, suite-ui and the suite-ui gallery with `syn`
//! and flags string literals that would reach the screen untranslated:
//!
//! - **Prose anywhere**: a literal with letters that isn't identifier-shaped (`png`,
//!   `settings.toml`, `JVID_LOG`), wherever it sits: UI calls, nested closures, `format!`,
//!   consts, arrays, struct fields, helper arguments, `Display` impls.
//! - **Any letters in a UI sink**: a literal passed to a method or constructor that draws
//!   text (`label`, `on_hover_text`, `RichText::new`, `Dialog::new`, `painter.text`, ..), so
//!   even a lowercase `ui.label("fps")` fails, as does `{x:?}`, which shows a Rust name. So
//!   does a literal that reaches the sink through a `let`, a `for` over it, a const or static,
//!   or the return value of a function or method defined in the same file.
//! - **Macros**: a `macro_rules!` body is checked as the code it expands to, and a macro that
//!   draws one of its arguments makes that argument a sink wherever it's used.
//! - **Keys and placeholders**: every `tr!`/`trn!` key exists in English and every
//!   `{placeholder}` of every language is passed.
//!
//! Text goes through `tr!`/`trn!`, or `verbatim("..")` for brand names and notation shown
//! as written. The exemptions below are by context, never by literal, each with its reason.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, Spacing};
use suite_ui::i18n::{Catalog, LANGUAGES, placeholders};
use syn::buffer::{Cursor, TokenBuffer};
use syn::punctuated::Punctuated;
use syn::visit_mut::{self, VisitMut};
use syn::{Attribute, Expr, Lit, Macro, Token};

/// Macros whose text never reaches the UI: logs and terminal diagnostics stay English for
/// bug reports, panics and assertions are programmer errors, and the rest take no text.
const EXEMPT_MACROS: &[&str] = &[
    "trace",
    "debug",
    "info",
    "warn",
    "error",
    "event",
    "span", // tracing logs
    "println",
    "eprintln",
    "print",
    "eprint", // terminal diagnostics
    "panic",
    "unreachable",
    "todo",
    "unimplemented",
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne", // programmer errors
    "include_bytes",
    "cfg",
    "matches",
    "pin", // no text, or patterns
];

/// Macros that make text from build data. Fine as data, but not drawn: wrap the result in
/// `verbatim(..)` to show it as written. `concat!` isn't here; its pieces are checked.
const DATA_MACROS: &[&str] = &["stringify", "include_str", "env", "option_env"];

/// Calls whose arguments are identifiers, not display text.
const EXEMPT_CALLS: &[&str] = &[
    "Id::new",       // egui ids must stay stable across languages
    "Grid::new",     // a grid's id salt
    "verbatim",      // explicitly shown as written
    "var",           // environment variable names
    "var_os",        //
    "set_var",       //
    "Path::new",     // file paths
    "PathBuf::from", //
    "from_name",     // egui key names parsed from shortcut notation
];

/// Methods whose arguments can't reach their result, so never the screen. Methods that
/// put an argument into their result (`join`, `with`, ..) are checked like any call.
const EXEMPT_METHODS: &[&str] = &[
    "expect",             // panic message: a programmer error
    "id_salt",            // egui ids must stay stable across languages
    "id_source",          //
    "make_persistent_id", //
    "on_begin_pass",      // egui plugin names
    "starts_with",        // tests on the receiver; the result is a bool or part of it
    "ends_with",          //
    "strip_prefix",       //
    "strip_suffix",       //
    "split",              //
];

/// Methods and constructors that draw their text arguments.
const SINK_METHODS: &[&str] = &[
    "label",
    "button",
    "small_button",
    "heading",
    "small",
    "strong",
    "weak",
    "monospace",
    "code",
    "colored_label",
    "selectable_label",
    "selectable_value",
    "checkbox",
    "radio",
    "radio_value",
    "toggle_value",
    "menu_button",
    "collapsing",
    "hyperlink_to",
    "on_hover_text",
    "on_disabled_hover_text",
    "on_hover_text_at_pointer",
    "hint_text",
    "selected_text",
    "add_filter",
    "set_title",
    "set_description",
    "title",
    "body",
    "buttons",
    "hint",
    "prefix",
    "error",
    "actions",
    "toast",
    "notify",
    "with_title",
    "text",   // Painter::text
    "suffix", // DragValue::suffix
];
/// Sinks whose later arguments aren't text: a file filter's extensions.
const SINK_FIRST_ONLY: &[&str] = &["add_filter"];
const SINK_CALLS: &[&str] = &[
    "RichText::new",
    "Label::new",
    "Button::new",
    "Button::primary",
    "Button::secondary",
    "Button::ghost",
    "Button::destructive",
    "Button::icon",
    "Window::new",
    "CollapsingHeader::new",
    "ComboBox::from_label",
    "Checkbox::new",
    "SectionHeader::new",
    "Dialog::new",
    "Panel::titled",
    "Panel::tabbed",
    "Dropdown::new",
    "Segmented::new",
    "MenuRow::new",
    "SelectableLabel::new",
    "Hyperlink::from_label_and_url",
    "WidgetText::from",
    "Shape::text",
    "WidgetInfo::labeled", // what a screen reader says
    "WidgetInfo::selected",
];

/// Methods whose result is their receiver's text.
const CONVERSIONS: &[&str] = &["to_owned", "to_string", "into", "as_str", "clone", "as_ref"];

/// Macros whose own definitions aren't checked: the translation macros forward to the catalog.
const TRANSLATION_MACROS: &[&str] = &["tr", "trn"];

/// Whole items exempt from the prose rule, by file and name.
const EXEMPT_ITEMS: &[(&str, &str, &str)] = &[
    (
        "jvid-app/src/keymap.rs",
        "ACTIONS",
        "default bindings in shortcut notation; names come from action.<id> keys",
    ),
    (
        "jvid-app/src/keymap.rs",
        "parse",
        "reads shortcut notation (Ctrl+Shift+Z), which stays English like the keycaps",
    ),
    (
        "jvid-app/src/keymap.rs",
        "format",
        "writes shortcut notation, the inverse of parse",
    ),
    (
        "jvid-app/src/keymap.rs",
        "save",
        "its only error goes to the log (app.rs), like the io errors beside it",
    ),
    (
        "jvid-app/src/effects.rs",
        "TEMPLATE",
        "WGSL source for a new file; its shown name and description are bin.template_* keys",
    ),
    (
        "jvid-app/src/effects.rs",
        "duplicate",
        "names the copy's file; a stem is the effect's id in project files",
    ),
    (
        "jvid-app/src/effects.rs",
        "delete",
        "its only error goes to the log (media_bin.rs)",
    ),
    (
        "jvid-app/src/export.rs",
        "headless_gpu",
        "only jvid-cli and tests call it; the CLI's output isn't localised",
    ),
    (
        "jvid-app/src/export/lossless.rs",
        "format",
        "a comparison key for stream formats, never shown",
    ),
    (
        "jvid-app/src/export/queue.rs",
        "reveal",
        "file manager arguments",
    ),
    (
        "jvid-app/src/export/queue.rs",
        "notify",
        "notifier arguments and AppleScript; callers pass the translated title and body",
    ),
    (
        "jvid-app/src/effects.rs",
        "free_path",
        "names a new effect's file; a stem is the effect's id in project files",
    ),
    (
        "jvid-app/src/autosave.rs",
        "untitled",
        "the unsaved project's autosave file; never shown, and a translated name would lose it",
    ),
    (
        "suite-ui/src/i18n.rs",
        "walk",
        "catalog errors are for translators, reported by tests",
    ),
    (
        "suite-ui/src/i18n.rs",
        "mismatches",
        "catalog drift is for translators, reported by tests",
    ),
];

/// A crate's sources and the catalog its `tr!` reads.
struct Tree {
    name: &'static str,
    roots: &'static [&'static str],
    catalog: &'static Catalog,
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn leak_catalog(dir: &str) -> &'static Catalog {
    let read = |l: &str| -> &'static str {
        let path = workspace().join(dir).join(format!("{l}.toml"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        Box::leak(text.into_boxed_str())
    };
    Box::leak(Box::new(Catalog::new(LANGUAGES.map(read))))
}

fn trees() -> Vec<Tree> {
    vec![
        Tree {
            name: "jvid-app",
            roots: &["crates/jvid-app/src/lib.rs", "crates/jvid-app/src/main.rs"],
            catalog: &jvid_app::i18n::CATALOG,
        },
        Tree {
            name: "suite-ui",
            roots: &["crates/suite-ui/src/lib.rs"],
            catalog: &suite_ui::i18n::CATALOG,
        },
        Tree {
            name: "gallery",
            roots: &["crates/suite-ui/examples/gallery.rs"],
            catalog: leak_catalog("crates/suite-ui/i18n/gallery"),
        },
    ]
}

/// `#[cfg(test)]`, `#[test]` or test-support-only code: never in a shipped UI.
fn is_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        let p = a.path();
        if p.is_ident("test") {
            return true;
        }
        if !p.is_ident("cfg") {
            return false;
        }
        let tokens = a
            .meta
            .require_list()
            .map(|l| l.tokens.to_string())
            .unwrap_or_default();
        tokens == "test" || tokens.contains("\"test-support\"")
    })
}

/// The text a literal shows once `{..}` format arguments are taken out.
fn without_placeholders(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Whether a format string has a `{..:?}` argument: Debug output names Rust items.
fn has_debug(s: &str) -> bool {
    s.split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}'))
        .any(|(spec, _)| spec.ends_with('?'))
}

/// Identifier-shaped text: file names, extensions, keys, MIME types, env var names.
fn identifier_like(s: &str) -> bool {
    let lower = s.chars().all(|c| {
        c.is_ascii_lowercase() || c.is_ascii_digit() || "_-./:+#%@=?&*,()[]$\\".contains(c)
    });
    let screaming = s.contains('_')
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    lower || screaming
}

/// What a file's items make of literals, by name, so a sink can see text that reaches it from
/// elsewhere in the file. Gathered by a first pass.
///
/// shortcut: same-file only, as names alone can't tell modules' items apart; resolve `use`
/// paths if text is found drawn from another module's const or helper.
#[derive(Default)]
struct Known {
    /// Consts and statics: the literals of their values.
    consts: BTreeMap<String, Vec<String>>,
    /// Functions and methods: the literals they can return.
    fns: BTreeMap<String, Vec<String>>,
    /// `macro_rules!` macros that draw one of their arguments.
    drawing: BTreeSet<String>,
}

struct Guard<'a> {
    file: String,
    catalog: &'a Catalog,
    item: Vec<String>,
    sink: usize,
    findings: Vec<String>,
    /// From the first pass; empty during it.
    known: &'a Known,
    /// Gathered from this file for the next pass.
    found: Known,
    /// The current function's `let` and `for` bindings and the literals they hold.
    locals: BTreeMap<String, Vec<String>>,
    /// Inside a `macro_rules!` body: its `$` variables, and whether one was drawn.
    metavars: BTreeSet<String>,
    draws_metavar: bool,
}

impl Guard<'_> {
    fn report(&mut self, what: String) {
        let item = self.item.last().map_or("", String::as_str);
        self.findings
            .push(format!("{} `{item}`: {what}", self.file));
    }

    fn literal(&mut self, value: &str) {
        if self.sink > 0 && has_debug(value) {
            return self.report(format!("{value:?} draws a Debug name"));
        }
        let text = without_placeholders(value);
        if !text.chars().any(char::is_alphabetic) {
            return;
        }
        if self.sink > 0 {
            self.report(format!("{value:?} drawn untranslated"));
        } else if !identifier_like(&text) {
            self.report(format!("{value:?} looks like UI text"));
        }
    }

    /// The literals with letters that `e` can evaluate to, as text flows: through branches,
    /// arrays, `format!`/`concat!`/`vec!`, conversions and bindings.
    fn values(&self, e: &Expr) -> Vec<String> {
        let mut out = Vec::new();
        self.values_into(e, &mut out);
        out
    }

    fn values_into(&self, e: &Expr, out: &mut Vec<String>) {
        let tail = |b: &syn::Block, out: &mut Vec<String>| {
            if let Some(syn::Stmt::Expr(e, None)) = b.stmts.last() {
                self.values_into(e, out);
            }
        };
        match e {
            Expr::Lit(l) => {
                if let Lit::Str(s) = &l.lit
                    && without_placeholders(&s.value())
                        .chars()
                        .any(char::is_alphabetic)
                {
                    out.push(s.value());
                }
            }
            Expr::Path(p) => {
                if let Some(name) = p.path.segments.last().map(|s| s.ident.to_string()) {
                    let held = match self.locals.get(&name) {
                        Some(held) => Some(held),
                        None => self.known.consts.get(&name),
                    };
                    out.extend(held.into_iter().flatten().cloned());
                }
            }
            Expr::Array(a) => a.elems.iter().for_each(|e| self.values_into(e, out)),
            Expr::Tuple(t) => t.elems.iter().for_each(|e| self.values_into(e, out)),
            Expr::Paren(p) => self.values_into(&p.expr, out),
            Expr::Group(g) => self.values_into(&g.expr, out),
            Expr::Reference(r) => self.values_into(&r.expr, out),
            Expr::Index(i) => self.values_into(&i.expr, out),
            Expr::Block(b) => tail(&b.block, out),
            Expr::If(i) => {
                tail(&i.then_branch, out);
                if let Some((_, e)) = &i.else_branch {
                    self.values_into(e, out);
                }
            }
            Expr::Match(m) => m.arms.iter().for_each(|a| self.values_into(&a.body, out)),
            Expr::MethodCall(m) if CONVERSIONS.contains(&m.method.to_string().as_str()) => {
                self.values_into(&m.receiver, out)
            }
            Expr::Macro(m) => {
                let name = m.mac.path.segments.last().map(|s| s.ident.to_string());
                if matches!(name.as_deref(), Some("format" | "concat" | "vec"))
                    && let Ok(args) = m
                        .mac
                        .parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated)
                {
                    args.iter().for_each(|e| self.values_into(e, out));
                }
            }
            _ => {}
        }
    }

    /// A literal held elsewhere, drawn here by `name`.
    fn drawn_through(&mut self, name: &str, values: Vec<String>) {
        for v in values {
            self.report(format!("{v:?} drawn untranslated through `{name}`"));
        }
    }

    /// Records what function `name` with `body` can return, then checks it.
    fn function(&mut self, name: String, attrs: &[Attribute], body: &mut syn::Block) {
        if is_test(attrs) || self.exempt_item(&name) {
            return;
        }
        let outer = std::mem::take(&mut self.locals);
        self.item.push(name.clone());
        self.visit_block_mut(body);
        let mut returned = Vec::new();
        if let Some(syn::Stmt::Expr(e, None)) = body.stmts.last() {
            self.values_into(e, &mut returned);
        }
        let mut returns = Returns(Vec::new());
        returns.visit_block_mut(body);
        for e in &returns.0 {
            self.values_into(e, &mut returned);
        }
        if !returned.is_empty() {
            self.found.fns.entry(name).or_default().extend(returned);
        }
        self.item.pop();
        self.locals = outer;
    }

    /// Checks each rule of `macro_rules! name` as the code it expands to.
    fn macro_rules(&mut self, name: &str, mac: &Macro) {
        if TRANSLATION_MACROS.contains(&name) {
            return;
        }
        let buffer = TokenBuffer::new2(mac.tokens.clone());
        let mut c = buffer.begin();
        let mut groups = 0;
        while let Some((tt, next)) = c.token_tree() {
            // Rules are `(pattern) => {body}`: every second group is a body.
            if let Some((body, ..)) = c.any_group() {
                groups += 1;
                if groups % 2 == 0 {
                    self.metavars.clear();
                    let code = transcribe(body, &mut self.metavars);
                    match syn::parse_str::<syn::Block>(&format!("{{{code}}}")) {
                        Ok(mut block) => {
                            self.item.push(format!("{name}!"));
                            self.visit_block_mut(&mut block);
                            self.item.pop();
                        }
                        Err(e) => self.report(format!("can't read `{name}!`: {e} in {tt}")),
                    }
                    self.metavars.clear();
                }
            }
            c = next;
        }
        if std::mem::take(&mut self.draws_metavar) {
            self.found.drawing.insert(name.to_owned());
        }
    }

    fn exempt_item(&self, name: &str) -> bool {
        EXEMPT_ITEMS
            .iter()
            .any(|(file, item, _)| self.file.ends_with(file) && *item == name)
    }

    fn in_item(&mut self, name: String, attrs: &[Attribute], visit: impl FnOnce(&mut Self)) {
        if is_test(attrs) || self.exempt_item(&name) {
            return;
        }
        self.item.push(name);
        visit(self);
        self.item.pop();
    }

    fn sink(&mut self, visit: impl FnOnce(&mut Self)) {
        self.sink += 1;
        visit(self);
        self.sink -= 1;
    }

    fn mac(&mut self, mac: &Macro) {
        let name = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        if EXEMPT_MACROS.contains(&name.as_str()) || name == "macro_rules" {
            return;
        }
        let drawing = self.known.drawing.contains(&name);
        if DATA_MACROS.contains(&name.as_str()) {
            if self.sink > 0 {
                self.report(format!("`{name}!` drawn untranslated"));
            }
            return;
        }
        // `vec![x; n]` repeats; everything else is a comma list.
        let repeat = |input: syn::parse::ParseStream| {
            let x: Expr = input.parse()?;
            input.parse::<Token![;]>()?;
            let n: Expr = input.parse()?;
            Ok(Punctuated::from_iter([x, n]))
        };
        let Ok(args) = mac
            .parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated)
            .or_else(|_| mac.parse_body_with(repeat))
        else {
            self.report(format!("can't read the arguments of `{name}!`"));
            return;
        };
        match name.as_str() {
            "tr" | "trn" => self.translation(&name, args),
            _ => {
                for mut arg in args {
                    if drawing {
                        self.sink(|g| g.visit_expr_mut(&mut arg));
                    } else {
                        self.visit_expr_mut(&mut arg);
                    }
                }
            }
        }
    }

    /// Checks a `tr!`/`trn!` key and its named arguments, then the argument values.
    fn translation(&mut self, name: &str, args: Punctuated<Expr, Token![,]>) {
        let mut key = None;
        let mut names = BTreeSet::new();
        for mut arg in args {
            match &mut arg {
                Expr::Lit(l) if key.is_none() => {
                    if let Lit::Str(s) = &l.lit {
                        key = Some(s.value());
                    }
                }
                Expr::Assign(a) => {
                    if let Expr::Path(p) = &*a.left {
                        names.extend(p.path.get_ident().map(|i| i.to_string()));
                    }
                    self.visit_expr_mut(&mut a.right);
                }
                // The catalog or the count.
                other => self.visit_expr_mut(other),
            }
        }
        let Some(key) = key else {
            return self.report(format!("`{name}!` without a literal key"));
        };
        if name == "trn" {
            names.insert("count".into());
        }
        let forms: Vec<String> = match name {
            "trn" => ["one", "many", "other"]
                .map(|f| format!("{key}.{f}"))
                .into(),
            _ => vec![key.clone()],
        };
        let english = forms
            .iter()
            .filter_map(|f| self.catalog.get_in("en", f))
            .count();
        if english == 0 || (name == "trn" && self.catalog.get_in("en", &forms[2]).is_none()) {
            return self.report(format!("`{name}!` key {key:?} isn't in en.toml"));
        }
        for lang in LANGUAGES {
            for form in &forms {
                let Some(text) = self.catalog.get_in(lang, form) else {
                    continue;
                };
                for p in placeholders(text) {
                    if !names.contains(p) {
                        self.report(format!("{lang} {form:?} uses {{{p}}}, not passed"));
                    }
                }
            }
        }
        let used: BTreeSet<&str> = LANGUAGES
            .iter()
            .flat_map(|l| forms.iter().filter_map(|f| self.catalog.get_in(l, f)))
            .flat_map(placeholders)
            .collect();
        for n in &names {
            if !used.contains(n.as_str()) && n != "count" {
                self.report(format!("{key:?} doesn't use the argument `{n}`"));
            }
        }
    }
}

/// The `return` values of a function body, not counting nested closures or items.
struct Returns(Vec<Expr>);

impl VisitMut for Returns {
    fn visit_expr_return_mut(&mut self, r: &mut syn::ExprReturn) {
        self.0.extend(r.expr.as_deref().cloned());
    }
    fn visit_expr_closure_mut(&mut self, _: &mut syn::ExprClosure) {}
    fn visit_item_mut(&mut self, _: &mut syn::Item) {}
}

/// A `macro_rules!` body as plain Rust: `$x` as `x` (added to `metavars`), `$crate` as
/// `crate`, and a `$(..)sep*` repetition as one copy of what it repeats.
fn transcribe(mut c: Cursor, metavars: &mut BTreeSet<String>) -> String {
    let mut out = String::new();
    while let Some((tt, next)) = c.token_tree() {
        if let Some((p, after)) = c.punct()
            && p.as_char() == '$'
        {
            if let Some((name, after)) = after.ident() {
                if name != "crate" {
                    metavars.insert(name.to_string());
                }
                out += &format!("{name} ");
                c = after;
                continue;
            }
            if let Some((inner, _, _, mut after)) = after.any_group() {
                out += &transcribe(inner, metavars);
                // An optional separator, then `*`, `+` or `?`.
                while let Some((p, n)) = after.punct() {
                    after = n;
                    if "*+?".contains(p.as_char()) {
                        break;
                    }
                }
                c = after;
                continue;
            }
        }
        if let Some((inner, delimiter, _, after)) = c.any_group() {
            let (open, close) = match delimiter {
                Delimiter::Parenthesis => ("(", ")"),
                Delimiter::Brace => ("{", "}"),
                Delimiter::Bracket => ("[", "]"),
                Delimiter::None => ("", ""),
            };
            out += &format!("{open}{}{close} ", transcribe(inner, metavars));
            c = after;
            continue;
        }
        out += &tt.to_string();
        // `::`, `=>`, `'a`: joined puncts stay joined.
        if !c
            .punct()
            .is_some_and(|(p, _)| p.spacing() == Spacing::Joint)
        {
            out.push(' ');
        }
        c = next;
    }
    out
}

fn call_name(func: &Expr) -> String {
    let Expr::Path(p) = func else {
        return String::new();
    };
    let segs: Vec<String> = p
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    segs[segs.len().saturating_sub(2)..].join("::")
}

/// An egui `Id` by its spelling: `ui.id()`, `Id::new(..)`, a salted one, or a binding or
/// field named `id`. Only its `with` salts are exempt; other `with`s may make text.
fn is_id(e: &Expr) -> bool {
    match e {
        Expr::MethodCall(m) => {
            m.method == "id"
                || m.method == "make_persistent_id"
                || (m.method == "with" && is_id(&m.receiver))
        }
        Expr::Call(c) => call_name(&c.func).ends_with("Id::new"),
        Expr::Path(p) => p
            .path
            .get_ident()
            .is_some_and(|i| i == "id" || i.to_string().ends_with("_id")),
        Expr::Field(f) => matches!(&f.member, syn::Member::Named(n) if n == "id"),
        _ => false,
    }
}

fn matches_name(list: &[&str], name: &str) -> bool {
    list.iter()
        .any(|n| name == *n || name.rsplit("::").next() == Some(n))
}

impl VisitMut for Guard<'_> {
    fn visit_attribute_mut(&mut self, _: &mut Attribute) {}

    fn visit_pat_mut(&mut self, _: &mut syn::Pat) {}

    fn visit_item_mod_mut(&mut self, m: &mut syn::ItemMod) {
        let name = m.ident.to_string();
        self.in_item(name, &m.attrs.clone(), |g| {
            visit_mut::visit_item_mod_mut(g, m)
        });
    }

    fn visit_item_fn_mut(&mut self, f: &mut syn::ItemFn) {
        self.function(f.sig.ident.to_string(), &f.attrs, &mut f.block);
    }

    fn visit_impl_item_fn_mut(&mut self, f: &mut syn::ImplItemFn) {
        self.function(f.sig.ident.to_string(), &f.attrs, &mut f.block);
    }

    fn visit_item_impl_mut(&mut self, i: &mut syn::ItemImpl) {
        if !is_test(&i.attrs) {
            visit_mut::visit_item_impl_mut(self, i);
        }
    }

    fn visit_item_const_mut(&mut self, c: &mut syn::ItemConst) {
        let name = c.ident.to_string();
        self.in_item(name.clone(), &c.attrs.clone(), |g| {
            visit_mut::visit_item_const_mut(g, c);
            let values = g.values(&c.expr);
            g.found.consts.entry(name).or_default().extend(values);
        });
    }

    fn visit_item_static_mut(&mut self, s: &mut syn::ItemStatic) {
        let name = s.ident.to_string();
        self.in_item(name.clone(), &s.attrs.clone(), |g| {
            visit_mut::visit_item_static_mut(g, s);
            let values = g.values(&s.expr);
            g.found.consts.entry(name).or_default().extend(values);
        });
    }

    fn visit_item_macro_mut(&mut self, m: &mut syn::ItemMacro) {
        match &m.ident {
            Some(name) if m.mac.path.is_ident("macro_rules") => {
                if !is_test(&m.attrs) {
                    self.macro_rules(&name.to_string(), &m.mac);
                }
            }
            _ => self.mac(&m.mac),
        }
    }

    fn visit_local_mut(&mut self, l: &mut syn::Local) {
        visit_mut::visit_local_mut(self, l);
        let pat = match &l.pat {
            syn::Pat::Type(t) => &*t.pat,
            pat => pat,
        };
        if let syn::Pat::Ident(i) = pat {
            let values = l.init.as_ref().map_or(Vec::new(), |i| self.values(&i.expr));
            self.locals.insert(i.ident.to_string(), values);
        }
    }

    fn visit_expr_for_loop_mut(&mut self, f: &mut syn::ExprForLoop) {
        self.visit_expr_mut(&mut f.expr);
        if let syn::Pat::Ident(i) = &*f.pat {
            let values = self.values(&f.expr);
            self.locals.insert(i.ident.to_string(), values);
        }
        self.visit_block_mut(&mut f.body);
    }

    fn visit_stmt_macro_mut(&mut self, m: &mut syn::StmtMacro) {
        self.mac(&m.mac);
    }

    fn visit_expr_mut(&mut self, e: &mut Expr) {
        match e {
            Expr::Lit(l) => {
                if let Lit::Str(s) = &l.lit {
                    self.literal(&s.value());
                }
            }
            Expr::Macro(m) => self.mac(&m.mac),
            Expr::Path(p) if self.sink > 0 => {
                let name = p.path.segments.last().map(|s| s.ident.to_string());
                let name = name.unwrap_or_default();
                if self.metavars.contains(&name) {
                    self.draws_metavar = true;
                }
                let values = self.values(e);
                self.drawn_through(&name, values);
            }
            Expr::Call(c) => {
                let name = call_name(&c.func);
                if matches_name(EXEMPT_CALLS, &name) {
                    return;
                }
                if self.sink > 0 {
                    let last = name.rsplit("::").next().unwrap_or_default();
                    let values = self.known.fns.get(last).cloned().unwrap_or_default();
                    self.drawn_through(&format!("{name}()"), values);
                }
                let sink = SINK_CALLS.contains(&name.as_str());
                for arg in &mut c.args {
                    if sink {
                        self.sink(|g| g.visit_expr_mut(arg));
                    } else {
                        self.visit_expr_mut(arg);
                    }
                }
            }
            Expr::MethodCall(m) => {
                let name = m.method.to_string();
                // `a.name()` draws what `name` returns, not `a`; `a.to_owned()` draws `a`.
                if !matches!(*m.receiver, Expr::Path(_)) || CONVERSIONS.contains(&name.as_str()) {
                    self.visit_expr_mut(&mut m.receiver);
                }
                if EXEMPT_METHODS.contains(&name.as_str()) || (name == "with" && is_id(&m.receiver))
                {
                    return;
                }
                if self.sink > 0 {
                    let values = self.known.fns.get(&name).cloned().unwrap_or_default();
                    self.drawn_through(&format!(".{name}()"), values);
                }
                let sink = SINK_METHODS.contains(&name.as_str());
                let first_only = SINK_FIRST_ONLY.contains(&name.as_str());
                for (i, arg) in m.args.iter_mut().enumerate() {
                    if sink && (i == 0 || !first_only) {
                        self.sink(|g| g.visit_expr_mut(arg));
                    } else {
                        self.visit_expr_mut(arg);
                    }
                }
            }
            // A UI call's closure draws its own widgets; the sink is only its direct text.
            Expr::Closure(c) => {
                let outer = std::mem::take(&mut self.sink);
                self.visit_expr_mut(&mut c.body);
                self.sink = outer;
            }
            _ => visit_mut::visit_expr_mut(self, e),
        }
    }
}

/// Every finding in one source file. A first pass learns what its items hold.
fn check_source(file: &str, source: &str, catalog: &Catalog) -> Vec<String> {
    let known = guard_pass(file, source, catalog, &Known::default()).1;
    guard_pass(file, source, catalog, &known).0
}

/// One file's findings, given what its items are `known` to hold, and what they hold.
fn guard_pass(file: &str, source: &str, catalog: &Catalog, known: &Known) -> (Vec<String>, Known) {
    let mut ast = match syn::parse_file(source) {
        Ok(ast) => ast,
        Err(e) => {
            return (
                vec![format!("{file}: doesn't parse: {e}")],
                Known::default(),
            );
        }
    };
    let mut guard = Guard {
        file: file.into(),
        catalog,
        item: Vec::new(),
        sink: 0,
        findings: Vec::new(),
        known,
        found: Known::default(),
        locals: BTreeMap::new(),
        metavars: BTreeSet::new(),
        draws_metavar: false,
    };
    guard.visit_file_mut(&mut ast);
    (guard.findings, guard.found)
}

/// The files of a module tree, following `mod x;` but not into test-only modules.
fn module_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut queue = vec![(root.to_path_buf(), true)];
    while let Some((file, is_root)) = queue.pop() {
        let source = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{file:?}: {e}"));
        let ast = syn::parse_file(&source).unwrap_or_else(|e| panic!("{file:?}: {e}"));
        let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
        let dir = file.parent().unwrap();
        let children = if is_root || stem == "mod" {
            dir.to_path_buf()
        } else {
            dir.join(&stem)
        };
        for item in &ast.items {
            let syn::Item::Mod(m) = item else { continue };
            if m.content.is_some() || is_test(&m.attrs) {
                continue;
            }
            let name = m.ident.to_string();
            let flat = children.join(format!("{name}.rs"));
            let nested = children.join(&name).join("mod.rs");
            let path = if flat.exists() { flat } else { nested };
            assert!(path.exists(), "{file:?}: no file for `mod {name};`");
            queue.push((path, false));
        }
        out.push(file);
    }
    out.sort();
    out
}

#[test]
fn ui_text_goes_through_the_translation_layer() {
    let root = workspace();
    let mut findings = Vec::new();
    let mut files = 0;
    for tree in trees() {
        assert!(
            tree.catalog.errors().is_empty(),
            "{}: {:?}",
            tree.name,
            tree.catalog.errors()
        );
        let mut seen = BTreeSet::new();
        for entry in tree.roots {
            for file in module_files(&root.join(entry)) {
                if !seen.insert(file.clone()) {
                    continue;
                }
                files += 1;
                let shown = file.strip_prefix(&root).unwrap_or(&file).to_string_lossy();
                let source = std::fs::read_to_string(&file).unwrap();
                findings.extend(check_source(&shown, &source, tree.catalog));
            }
        }
    }
    assert!(
        files > 40,
        "only {files} files scanned; is the module walk broken?"
    );
    assert!(
        findings.is_empty(),
        "{} strings bypass the translation layer. Route them through tr!/trn! (see \
         docs/translating.md) or verbatim(..) for text shown as written:\n{}",
        findings.len(),
        findings.join("\n")
    );
}

/// The guard itself: each bypass shape is caught, and the allowed shapes aren't.
#[test]
fn guard_catches_every_bypass_shape() {
    static CATALOG: Catalog = Catalog::new([
        "known = \"Known\"\nnamed = \"Hi {name}\"\n[clips]\none = \"{count} clip\"\nother = \"{count} clips\"\n",
        "known = \"Conocido\"\nnamed = \"Hola {name} {extra}\"\n[clips]\none = \"{count} clip\"\nother = \"{count} clips\"\n",
    ]);
    let bypasses = r#"
        const HINT: &str = "Drop files here";
        fn ui(ui: &mut egui::Ui, n: usize, x: bool) {
            ui.label("Direct Label");
            ui.horizontal(|ui| { ui.vertical(|ui| { ui.button("Nested Button"); }); });
            ui.label(format!("{n} things"));
            ui.label(egui::RichText::new("Rich Text").strong());
            egui::Window::new("Window Title").show(ui.ctx(), |_| {});
            Dialog::new("Prompt Title", "Body text.", &[("Okay", ButtonKind::Primary)]);
            ui.button("Go").on_hover_text("Tooltip text");
            row(ui, "Helper Label", 3);
            let tabs = ["Media Tab", "Effects"];
            ui.label("fps");
            ui.label(if x { "Yes" } else { "No" });
            let toast = Toast { text: "Saved it".into() };
            ui.label(tr!("missing.key"));
            ui.label(tr!("named", wrong = 1));
            ui.label(trn!("clips", n, extra = 2));
            ui.label(egui::WidgetText::from("accessible"));
            ui.label(tr!("known", name = "Bob"));
            let v = vec!["Repeated Text"; 2];
            ui.selectable_value(&mut mode, m, format!("{m:?}"));
            rfd::FileDialog::new().add_filter("Filter Name", &["png"]);
            ui.label(concat!("Joined ", "Label"));
            egui::Window::new(concat!("Glued ", "Window")).show(ui.ctx(), |_| {});
            ui.label(names.join(" or "));
            ui.label(builder.with("With Text"));
            ui.label(stringify!(Shown Words));
            ui.label(env!("CARGO_PKG_NAME"));
        }
        const BANNER: &str = concat!("Const ", "Concat");
        impl std::fmt::Display for E {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "Broken thing") }
        }
        const LOWER: &str = "lowerconst";
        fn helper() -> &'static str { "helperreturn" }
        fn early(x: bool) -> &'static str { if x { return "earlyreturn"; } "latereturn" }
        impl Kind {
            fn name(&self) -> &'static str { match self { Kind::A => "kinda", _ => "kindb" } }
        }
        macro_rules! show { ($ui:expr) => { $ui.label("macrobody") }; }
        macro_rules! draw { ($ui:expr, $text:expr) => { $ui.label($text) }; }
        macro_rules! many { ($ui:expr, $($t:expr),*) => { $( $ui.button($t); )* }; }
        fn painted(p: &egui::Painter, ui: &mut egui::Ui, k: Kind, i: usize, v: &mut f32) {
            p.text(pos, Align2::LEFT_TOP, "paintertext", font, color);
            egui::Shape::text(&fonts, pos, Align2::LEFT_TOP, "shapetext", font, color);
            ui.add(egui::DragValue::new(v).suffix("px"));
            r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "spoken"));
            r.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, false, "chosen"));
            let caption = "untranslated";
            ui.label(caption);
            let tabs = ["mediatab", "effectstab"];
            ui.label(tabs[i]);
            for t in ["looped"] { ui.button(t); }
            ui.label(LOWER);
            ui.label(helper());
            ui.label(early(true));
            ui.label(k.name());
            let owned = "converted".to_owned();
            ui.label(owned.as_str());
            show!(ui);
            draw!(ui, "macroarg");
            many!(ui, "repeated", "again");
        }
    "#;
    let findings = check_source("bypass.rs", bypasses, &CATALOG).join("\n");
    for caught in [
        "Drop files here",
        "Direct Label",
        "Nested Button",
        "{n} things",
        "Rich Text",
        "Window Title",
        "Prompt Title",
        "Body text.",
        "Okay",
        "Tooltip text",
        "Helper Label",
        "Media Tab",
        "\"Effects\"",
        "\"fps\" drawn",
        "\"Yes\"",
        "\"No\"",
        "Saved it",
        "\"missing.key\" isn't in en.toml",
        "es \"named\" uses {extra}",
        "en \"named\" uses {name}",
        "doesn't use the argument `wrong`",
        "\"clips\" doesn't use the argument `extra`",
        "\"accessible\" drawn",
        "\"Bob\" drawn",
        "Repeated Text",
        "\"{m:?}\" draws a Debug name",
        "Filter Name",
        "Broken thing",
        "\"Joined \" drawn",
        "\"Label\" drawn",
        "\"Glued \" drawn",
        "\" or \" drawn",
        "\"With Text\" drawn",
        "`stringify!` drawn",
        "`env!` drawn",
        "\"Const \" looks like UI text",
        "\"paintertext\" drawn",
        "\"shapetext\" drawn",
        "\"px\" drawn",
        "\"spoken\" drawn",
        "\"chosen\" drawn",
        "\"untranslated\" drawn untranslated through `caption`",
        "\"mediatab\" drawn untranslated through `tabs`",
        "\"looped\" drawn untranslated through `t`",
        "\"lowerconst\" drawn untranslated through `LOWER`",
        "\"helperreturn\" drawn untranslated through `helper()`",
        "\"earlyreturn\" drawn untranslated through `early()`",
        "\"latereturn\" drawn untranslated through `early()`",
        "\"kinda\" drawn untranslated through `.name()`",
        "\"converted\" drawn untranslated through `owned`",
        "`show!`: \"macrobody\" drawn",
        "\"macroarg\" drawn",
        "\"repeated\" drawn",
        "\"again\" drawn",
    ] {
        assert!(
            findings.contains(caught),
            "missed {caught:?} in:\n{findings}"
        );
    }

    let allowed = r#"
        //! Doc comments are fine.
        const FILE: &str = "settings.toml";
        const ENV: &str = "JVID_TEST_MEDIA";
        fn ui(ui: &mut egui::Ui, n: usize, path: &Path) {
            ui.label(tr!("known"));
            ui.label(tr!("named", name = path.display(), extra = n));
            let row = vec![0u8; n];
            rfd::FileDialog::new().add_filter(tr!("known"), &["png", "jpg"]);
            rfd::FileDialog::new().set_file_name(format!("{n}.jvid"));
            ui.label(trn!("clips", n));
            ui.label(verbatim("H.264"));
            ui.label(format!("{n} · {}", tr!("known")));
            let id = egui::Id::new("Some Id Salt");
            egui::Grid::new("grid_id");
            tracing::warn!("Logs stay English {n}");
            eprintln!("Terminal output");
            let v = std::env::var("JVID_LOG");
            let f = path.with_extension("png");
            something.expect("a programmer message");
            assert!(n > 0, "Assertion text");
            if ext == "srt" {}
            match s { "Pattern Text" => {}, _ => {} }
            let salt = ui.id().with("clip menu");
            let drags = id.with(("fade drags", n));
            let file = dir.join("settings.toml").with_extension("bak");
            storage.set_string("window_state", s);
            let rest = s.strip_prefix("Ctrl+").unwrap_or(s);
            ui.label(verbatim(env!("CARGO_PKG_VERSION")));
            let list = names.join(", ");
        }
        fn flows(ui: &mut egui::Ui, dir: &Path, a: &Action) {
            let caption = "shadowed";
            let caption = tr!("known");
            ui.label(caption);
            let ext = "png";
            let file = dir.join(FILE).with_extension(ext);
            let stem = file_stem();
            ui.label(a.name());
            ui.label(verbatim(VERSION));
            salt!("panel");
        }
        fn file_stem() -> &'static str { "autosave" }
        const VERSION: &str = "v1";
        macro_rules! salt { ($x:expr) => { egui::Id::new($x) }; }
        macro_rules! tr {
            ($($args:tt)+) => { suite_ui::tr!($crate::i18n::CATALOG, $($args)+) };
        }
        const AGENT: &str = concat!("jvid/", env!("CARGO_PKG_VERSION"));
        const SHADER: &str = include_str!("shader.wgsl");
        #[cfg(test)]
        mod tests { fn t() { ui.label("Test Text"); } }
        #[test]
        fn alone() { ui.label("Test Text"); }
    "#;
    let findings = check_source("allowed.rs", allowed, &CATALOG);
    assert!(
        findings.is_empty(),
        "false positives:\n{}",
        findings.join("\n")
    );
}
