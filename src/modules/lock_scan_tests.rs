//! No module keeps control state behind a bare `std::sync` lock, and no
//! module's `process()` takes a lock.
//!
//! A source scan, so a new module or control file is covered the moment it
//! exists. Control state the audio thread never reads, and that a control
//! thread must serialize, sits behind a [`ControlLock`], which panics in
//! debug builds if a `process()` reaches it (see
//! `crate::invention::publish::tests::catalog`). Everything else the audio
//! thread shares with control threads is atomic, or a request.
//!
//! The scan parses each file, so comments, strings and `#[cfg(test)]` code
//! are left out exactly, and the code around them is still scanned.
//!
//! [`ControlLock`]: crate::audio_thread::ControlLock

use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};

/// Files allowed to break a rule: (path from the crate root, why). Empty:
/// a file that truly cannot comply names itself here with its reason, and
/// an entry is a bug to fix rather than a place to rest.
const ALLOWED: &[(&str, &str)] = &[];

/// Lock types a control file may not hold.
const LOCK_TYPES: &[&str] = &["Mutex", "RwLock"];

/// Methods that take a lock (called with no arguments), which a
/// `process()` body may not call.
const LOCK_CALLS: &[&str] = &["lock", "try_lock", "read", "write"];

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every non-test `.rs` file under `dir`.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" && name != "test_support" {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") && !name.ends_with("tests.rs") {
            out.push(path);
        }
    }
}

/// Whether `attrs` keep their node out of a build outside tests:
/// `#[cfg(test)]` or `#[test]`.
fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("test")
            || (attr.path().is_ident("cfg")
                && attr
                    .parse_args::<syn::Path>()
                    .is_ok_and(|path| path.is_ident("test")))
    })
}

fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    use syn::Item;
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

/// What a file breaks, collected as the shipped part of it is walked.
#[derive(Default)]
struct Scan {
    control: bool,
    in_process: usize,
    processes: usize,
    found: Vec<String>,
}

impl Scan {
    fn breach(&mut self, what: String) {
        if !self.found.contains(&what) {
            self.found.push(what);
        }
    }

    fn named(&mut self, ident: &str) {
        if let Some(lock) = LOCK_TYPES.iter().find(|lock| **lock == ident) {
            if self.control {
                self.breach(format!("holds a std::sync::{lock}"));
            }
            if self.in_process > 0 {
                self.breach(format!("process() names {lock}"));
            }
        }
    }

    /// Macro bodies are tokens, not syntax: look for lock names, the body
    /// of any `fn process`, and `.lock()`-shaped calls among them.
    fn tokens(&mut self, tokens: proc_macro2::TokenStream) {
        use proc_macro2::{Delimiter, TokenTree};
        let tokens: Vec<TokenTree> = tokens.into_iter().collect();
        let ident_at = |at: usize, name: &str| matches!(tokens.get(at), Some(TokenTree::Ident(ident)) if ident == name);
        // The brace group a `fn process` at `at` opens, if any.
        let mut process_body = None;
        for (at, token) in tokens.iter().enumerate() {
            if ident_at(at, "fn") && ident_at(at + 1, "process") {
                process_body = tokens[at..]
                    .iter()
                    .position(
                        |t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace),
                    )
                    .map(|offset| at + offset);
            }
            match token {
                TokenTree::Group(group) => {
                    let process = process_body == Some(at);
                    self.in_process += usize::from(process);
                    self.processes += usize::from(process);
                    self.tokens(group.stream());
                    self.in_process -= usize::from(process);
                }
                TokenTree::Ident(ident) => {
                    let ident = ident.to_string();
                    self.named(&ident);
                    let dotted = at > 0
                        && matches!(&tokens[at - 1], TokenTree::Punct(p) if p.as_char() == '.');
                    let empty_call = matches!(tokens.get(at + 1), Some(TokenTree::Group(g))
                        if g.delimiter() == Delimiter::Parenthesis && g.stream().is_empty());
                    if self.in_process > 0 && dotted && empty_call && LOCK_CALLS.contains(&&*ident)
                    {
                        self.breach(format!("process() calls .{ident}()"));
                    }
                }
                _ => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        if !test_only(item_attrs(item)) {
            visit::visit_item(self, item);
        }
    }

    fn visit_field(&mut self, field: &'ast syn::Field) {
        if !test_only(&field.attrs) {
            visit::visit_field(self, field);
        }
    }

    fn visit_variant(&mut self, variant: &'ast syn::Variant) {
        if !test_only(&variant.attrs) {
            visit::visit_variant(self, variant);
        }
    }

    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        let attrs = match stmt {
            syn::Stmt::Local(local) => &local.attrs[..],
            syn::Stmt::Macro(mac) => &mac.attrs[..],
            _ => &[],
        };
        if !test_only(attrs) {
            visit::visit_stmt(self, stmt);
        }
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        if test_only(&function.attrs) {
            return;
        }
        let process = function.sig.ident == "process"
            && matches!(
                function.sig.receiver(),
                Some(receiver) if receiver.reference.is_some() && receiver.mutability.is_some()
            );
        self.in_process += usize::from(process);
        self.processes += usize::from(process);
        visit::visit_impl_item_fn(self, function);
        self.in_process -= usize::from(process);
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        let attrs = match item {
            syn::ImplItem::Const(item) => &item.attrs[..],
            syn::ImplItem::Type(item) => &item.attrs[..],
            syn::ImplItem::Macro(item) => &item.attrs[..],
            _ => &[],
        };
        if !test_only(attrs) {
            visit::visit_impl_item(self, item);
        }
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let method = call.method.to_string();
        if self.in_process > 0 && call.args.is_empty() && LOCK_CALLS.contains(&&*method) {
            self.breach(format!("process() calls .{method}()"));
        }
        visit::visit_expr_method_call(self, call);
    }

    fn visit_ident(&mut self, ident: &'ast proc_macro2::Ident) {
        self.named(&ident.to_string());
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        // A macro wrapping items is scanned as items, cfg(test) and all;
        // anything else (a macro_rules! body, an expression) as tokens.
        match syn::parse2::<syn::File>(mac.tokens.clone()) {
            Ok(items) if !items.items.is_empty() => self.visit_file(&items),
            _ => self.tokens(mac.tokens.clone()),
        }
        visit::visit_macro(self, mac);
    }
}

/// What `source` breaks, as a control file (`control`) and in its
/// `process()` bodies, and how many `process()` bodies it has.
fn breaches(source: &str, control: bool) -> (Vec<String>, usize) {
    let file = syn::parse_file(source).unwrap_or_else(|error| panic!("unparsed: {error}"));
    let mut scan = Scan {
        control,
        ..Scan::default()
    };
    scan.visit_file(&file);
    (scan.found, scan.processes)
}

/// A module's control file: `controls.rs`, or anything under `controls/`.
fn is_control_file(relative: &Path) -> bool {
    relative.starts_with("src/modules")
        && relative
            .iter()
            .any(|part| part == "controls.rs" || part == "controls")
}

#[test]
fn no_module_control_or_process_path_takes_a_std_lock() {
    let root = crate_root();
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    let mut misses = Vec::new();
    let mut control_files = 0;
    let mut process_files = 0;
    for path in files {
        let relative = path.strip_prefix(&root).unwrap();
        let shown = relative.to_string_lossy().into_owned();
        let control = is_control_file(relative);
        let source = fs::read_to_string(&path).unwrap();
        let (found, processes) = std::panic::catch_unwind(|| breaches(&source, control))
            .unwrap_or_else(|_| panic!("{shown} did not parse"));
        control_files += usize::from(control);
        process_files += usize::from(processes > 0);
        let allowed = ALLOWED.iter().any(|(file, _)| *file == shown);
        for breach in found {
            if !allowed {
                misses.push(format!("{shown}: {breach}"));
            }
        }
    }
    assert!(misses.is_empty(), "{misses:#?}");
    // The walk reached the module catalog.
    assert!(control_files >= 15, "only {control_files} control files");
    assert!(process_files >= 20, "only {process_files} process() files");
    for (file, reason) in ALLOWED {
        assert!(
            root.join(file).exists(),
            "allowed '{file}' ({reason}) is gone"
        );
    }
}

#[test]
fn the_scan_catches_a_lock_in_a_control_file_and_in_process() {
    let control = "use std::sync::{Arc, Mutex};\nstruct S { state: Arc<Mutex<u8>> }\n";
    assert_eq!(breaches(control, true).0, ["holds a std::sync::Mutex"]);
    assert!(breaches(control, false).0.is_empty());

    let process =
        "impl Module for M {\n    fn process(&mut self, frames: usize) -> bool {\n        \
        if frames > 0 { let _g = self.shared.lock(); }\n        \
        debug_assert!(self.other.try_lock().is_ok());\n        true\n    }\n    \
        fn other(&self) { self.shared.lock(); }\n}\n";
    assert_eq!(
        breaches(process, false).0,
        ["process() calls .lock()", "process() calls .try_lock()"]
    );

    let fine = "// a Mutex in a comment\nconst S: &str = \"Mutex\";\n\
        struct S { guard: std::sync::MutexGuard<'static, ()> }\n\
        #[cfg(test)]\nmod tests { use std::sync::Mutex; }\n";
    assert!(breaches(fine, true).0.is_empty());
}

#[test]
fn the_scan_skips_test_code_and_still_scans_what_follows_it() {
    let after_tests = "#[cfg(test)]\nmod tests;\n#[cfg(test)]\nmod more {\n    \
        fn f() { let _open = \"{\"; let _m = std::sync::Mutex::new(()); }\n}\n\
        #[test]\nfn helper() { drop(M.lock()); }\n\
        impl Module for M {\n    #[cfg(test)]\n    fn probe(&self) {}\n    \
        fn process(&mut self) { self.state.lock(); }\n}\n";
    assert_eq!(breaches(after_tests, false).0, ["process() calls .lock()"]);

    let fields = "struct State {\n    #[cfg(test)]\n    probe: std::sync::RwLock<u8>,\n    \
        shared: std::sync::Mutex<u8>,\n}\n";
    assert_eq!(breaches(fields, true).0, ["holds a std::sync::Mutex"]);

    // Inside macros: items a macro wraps, and a macro_rules! template.
    let wrapped = "passthrough! {\n    impl Module for M {\n        \
        fn process(&mut self) { self.shared.lock(); }\n    }\n}\n";
    assert_eq!(
        breaches(wrapped, false),
        (vec!["process() calls .lock()".into()], 1)
    );
    let template =
        "macro_rules! module {\n    ($m:ident) => {\n        impl Module for $m {\n            \
        fn process(&mut self) { self.shared.read(); }\n        }\n    };\n}\n";
    assert_eq!(
        breaches(template, false),
        (vec!["process() calls .read()".into()], 1)
    );
}
