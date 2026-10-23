//! Fails when user-visible text bypasses the translation layer (#210).
//!
//! Parses every non-test module of jvid-app, suite-ui and the suite-ui gallery with `syn`
//! and flags string literals that would reach the screen untranslated:
//!
//! - **Prose anywhere**: a literal with letters that isn't identifier-shaped (`png`,
//!   `settings.toml`, `JVID_LOG`), wherever it sits: UI calls, nested closures, `format!`,
//!   consts, arrays, struct fields, helper arguments, `Display` impls.
//! - **Any letters in a UI sink**: a literal passed to a method or constructor that draws
//!   text (`label`, `on_hover_text`, `RichText::new`, `Dialog::new`, ..), so even a lowercase
//!   `ui.label("fps")` fails, as does `{x:?}`, which shows a Rust name.
//! - **Keys and placeholders**: every `tr!`/`trn!` key exists in English and every
//!   `{placeholder}` of every language is passed.
//!
//! Text goes through `tr!`/`trn!`, or `verbatim("..")` for brand names and notation shown
//! as written. The exemptions below are by context, never by literal, each with its reason.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use suite_ui::i18n::{Catalog, LANGUAGES, placeholders};
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
];

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

struct Guard<'a> {
    file: String,
    catalog: &'a Catalog,
    item: Vec<String>,
    sink: usize,
    findings: Vec<String>,
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
                    self.visit_expr_mut(&mut arg);
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
        let name = f.sig.ident.to_string();
        self.in_item(name, &f.attrs.clone(), |g| {
            visit_mut::visit_item_fn_mut(g, f)
        });
    }

    fn visit_impl_item_fn_mut(&mut self, f: &mut syn::ImplItemFn) {
        let name = f.sig.ident.to_string();
        self.in_item(name, &f.attrs.clone(), |g| {
            visit_mut::visit_impl_item_fn_mut(g, f)
        });
    }

    fn visit_item_impl_mut(&mut self, i: &mut syn::ItemImpl) {
        if !is_test(&i.attrs) {
            visit_mut::visit_item_impl_mut(self, i);
        }
    }

    fn visit_item_const_mut(&mut self, c: &mut syn::ItemConst) {
        let name = c.ident.to_string();
        self.in_item(name, &c.attrs.clone(), |g| {
            visit_mut::visit_item_const_mut(g, c)
        });
    }

    fn visit_item_static_mut(&mut self, s: &mut syn::ItemStatic) {
        let name = s.ident.to_string();
        self.in_item(name, &s.attrs.clone(), |g| {
            visit_mut::visit_item_static_mut(g, s)
        });
    }

    fn visit_item_macro_mut(&mut self, m: &mut syn::ItemMacro) {
        self.mac(&m.mac);
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
            Expr::Call(c) => {
                let name = call_name(&c.func);
                if matches_name(EXEMPT_CALLS, &name) {
                    return;
                }
                self.visit_expr_mut(&mut c.func);
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
                self.visit_expr_mut(&mut m.receiver);
                let name = m.method.to_string();
                if EXEMPT_METHODS.contains(&name.as_str()) || (name == "with" && is_id(&m.receiver))
                {
                    return;
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

/// Every finding in one source file.
fn check_source(file: &str, source: &str, catalog: &Catalog) -> Vec<String> {
    let mut ast = match syn::parse_file(source) {
        Ok(ast) => ast,
        Err(e) => return vec![format!("{file}: doesn't parse: {e}")],
    };
    let mut guard = Guard {
        file: file.into(),
        catalog,
        item: Vec::new(),
        sink: 0,
        findings: Vec::new(),
    };
    guard.visit_file_mut(&mut ast);
    guard.findings
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
