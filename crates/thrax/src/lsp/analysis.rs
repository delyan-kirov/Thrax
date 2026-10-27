//! One analysis of a document, made incremental the cheap way.
//!
//! A document is checked over its whole import closure. The closure minus the
//! document itself (its dependencies, `CORE`, `C`) is the [`Base`]: parsed and
//! checked once, then kept while none of those sources changes. Each edit then
//! costs one module: the base's tree is copied, the document is parsed onto the
//! copy, and only it is checked, importing from the base's checkers. The type
//! store is shared, so before each check it is rewound to the base's watermark,
//! dropping the previous revision's types (the arena-reset discipline).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use frontend::{Ast, Checker, Expr, Item, Program, Types};
use utilities::{Aol, Code, Diagnostic, Severity, Span};

use crate::driver;

/// A diagnostic of the analyzed document.
pub struct Found {
    pub module: usize,
    pub diag: Diagnostic,
    /// Phase-then-dependency order: lower reads first.
    pub rank: usize,
}

/// How a base source is known to be unchanged.
#[derive(PartialEq)]
enum Stamp {
    Embedded,
    File { path: PathBuf, modified: Option<SystemTime>, len: u64 },
    Overlay { path: PathBuf, text: String },
}

/// Everything the analyzed document depends on, checked.
///
/// `checkers` borrow `ast`. The borrow is erased to `'static` because both live
/// here: `ast` is boxed (so it never moves), is never mutated once the checkers
/// exist, and `checkers` is declared first so it drops first.
pub struct Base {
    checkers: Vec<Checker<'static>>,
    ast: Box<Ast>,
    programs: Vec<Program>,
    sources: Vec<(String, String, String)>,
    index: HashMap<String, usize>,
    expr_ranges: Vec<std::ops::Range<usize>>,
    types: Rc<Types>,
    mark: frontend::typing::data::TypesMark,
    stamps: Vec<Stamp>,
    root_name: String,
    root_imports: Vec<String>,
    /// Bumped by each check over this base: it rewinds the type store, which
    /// invalidates the snapshot of the check before.
    generation: std::cell::Cell<u64>,
}

/// One check of a document over its [`Base`].
///
/// `checker` borrows `ast` and the base, erased to `'static` like the base's
/// own: `ast` is boxed and frozen, the base is held by `base`, and the fields
/// drop in declaration order, `checker` first.
pub struct Snapshot {
    checker: Checker<'static>,
    ast: Box<Ast>,
    base: Rc<Base>,
    generation: u64,
    pub programs: Vec<Program>,
    /// `(module name, path, source)` per module; the document is `root`.
    pub sources: Vec<(String, String, String)>,
    pub root: usize,
    pub expr_ranges: Vec<std::ops::Range<usize>>,
    pub found: Vec<Found>,
}

impl Snapshot {
    pub fn ast(&self) -> &Ast {
        &self.ast
    }

    pub fn checker(&self, module: usize) -> &Checker<'_> {
        assert_eq!(
            self.generation,
            self.base.generation.get(),
            "a snapshot was used after a newer check rewound its types"
        );
        if module == self.root {
            &self.checker
        } else {
            &self.base.checkers[module]
        }
    }

    /// The module whose source is the file at `path`.
    pub fn module_of(&self, path: &Path) -> Option<usize> {
        self.sources.iter().position(|(_, p, _)| Path::new(p) == path)
    }

    /// The innermost expression of `module` whose span holds `offset`.
    pub fn expr_at(&self, module: usize, offset: usize) -> Option<(Aol<Expr>, Span)> {
        let ast = self.ast();
        let mut best: Option<(Aol<Expr>, Span)> = None;
        for i in self.expr_ranges[module].clone() {
            let e = ast.exprs.handle(i);
            let Some(span) = ast.expr_span(e) else { continue };
            if span.start <= offset
                && offset < span.end.max(span.start + 1)
                && best.is_none_or(|(_, b)| span.len() < b.len())
            {
                best = Some((e, span));
            }
        }
        best
    }

    /// The definition whose name `offset` is on, and the name's span.
    pub fn name_at(&self, module: usize, offset: usize) -> Option<(String, Span)> {
        let ast = self.ast();
        let p = &self.programs[module];
        ast.slice(p.items).iter().zip(ast.slice(p.spans)).find_map(|(item, span)| {
            let name = span.name?;
            let Item::Def { name: n, .. } = item else { return None };
            (name.start <= offset && offset <= name.end).then(|| (ast.text(*n).to_string(), name))
        })
    }

    /// Where global `name` of `module` is defined: its name span.
    pub fn definition(&self, module: usize, name: &str) -> Option<Span> {
        let ast = self.ast();
        let p = &self.programs[module];
        ast.slice(p.items).iter().zip(ast.slice(p.spans)).find_map(|(item, span)| {
            let defined = match item {
                Item::Def { name: n, .. }
                | Item::Struct { name: n, .. }
                | Item::Union { name: n, .. }
                | Item::Alias { name: n, .. }
                | Item::Effect { name: n, .. } => *n,
                _ => return None,
            };
            (ast.text(defined) == name).then_some(span.name.unwrap_or(span.full))
        })
    }
}

/// The analyses of one document, keeping its [`Base`] between them.
#[derive(Default)]
pub struct Analyzer {
    base: Option<Rc<Base>>,
}

impl Analyzer {
    /// Check the document at `root_path`, with `overlay` (open buffers by path)
    /// standing in for files on disk. Fails only when it cannot be loaded.
    /// Any earlier [`Snapshot`] from this analyzer is invalid afterwards.
    pub fn analyze(&mut self, root_path: &Path, overlay: &HashMap<PathBuf, String>) -> Result<Snapshot, String> {
        let root_src = match overlay.get(root_path) {
            Some(s) => s.clone(),
            None => std::fs::read_to_string(root_path)
                .map_err(|e| format!("cannot read {}: {e}", root_path.display()))?,
        };
        let (root_name, root_imports) = header(&root_src, root_path);
        let fresh = self.base.as_ref().is_some_and(|b| {
            b.root_name == root_name && b.root_imports == root_imports && b.stamps_hold(overlay)
        });
        if !fresh {
            self.base = None;
            self.base = Some(Rc::new(Base::build(root_path, &root_src, &root_name, root_imports, overlay)?));
        }
        let base = Rc::clone(self.base.as_ref().expect("just built"));
        Ok(check_root(base, root_path, root_src, true))
    }
}

/// Check `src` in place of `current`'s document, over the same base, and
/// return its errors, leaving `current` valid: the trial's types are rewound
/// to a watermark taken after `current`'s, so `current`'s own survive. This is
/// what verified fixes and counterfactual ranking are built on.
pub fn trial(current: &Snapshot, src: &str) -> Vec<Diagnostic> {
    let base = Rc::clone(&current.base);
    let mark = base.types.mark();
    let path = PathBuf::from(&current.sources[current.root].1);
    let snap = check_root(Rc::clone(&base), &path, src.to_string(), false);
    let out = snap.found.iter().filter(|f| f.diag.severity() == Severity::Error).map(|f| f.diag.clone()).collect();
    drop(snap);
    base.types.truncate(&mark);
    out
}

/// A one-off check, with no base kept.
pub fn analyze(root_path: &Path, overlay: &HashMap<PathBuf, String>) -> Result<Snapshot, String> {
    Analyzer::default().analyze(root_path, overlay)
}

/// The module name a source declares, and the modules it imports (sorted).
fn header(src: &str, path: &Path) -> (String, Vec<String>) {
    let (ast, program, _) = frontend::parse_recovering(Ast::new(), src);
    let name = match &program {
        Ok(p) => ast.text(p.module).to_string(),
        Err(_) => driver::file_stem(&path.display().to_string()),
    };
    let mut imports: Vec<String> = match &program {
        Ok(p) => ast
            .slice(p.items)
            .iter()
            .filter_map(|item| match item {
                Item::Import { module, .. } => {
                    Some(ast.slice(*module).iter().map(|&s| ast.text(s)).collect::<Vec<_>>().join("."))
                }
                _ => None,
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    imports.sort();
    imports.dedup();
    (name, imports)
}

impl Base {
    fn build(
        root_path: &Path,
        root_src: &str,
        root_name: &str,
        root_imports: Vec<String>,
        overlay: &HashMap<PathBuf, String>,
    ) -> Result<Base, String> {
        let root_dir = root_path.parent().map(Path::to_path_buf).unwrap_or_default();
        let fetch = |name: &str| -> Result<(String, String), String> {
            if let Some(file) = driver::resolve_module_file(name, &root_dir) {
                let file = file.canonicalize().unwrap_or(file);
                if let Some(src) = overlay.get(&file) {
                    return Ok((file.display().to_string(), src.clone()));
                }
                return std::fs::read_to_string(&file)
                    .map(|src| (file.display().to_string(), src))
                    .map_err(|e| format!("cannot read module `{name}` ({}): {e}", file.display()));
            }
            match crate::stdlib::source(name) {
                Some(src) => Ok((crate::stdlib::path(name), src.to_string())),
                None => Err(format!("cannot find module `{name}`")),
            }
        };
        let loaded =
            driver::load_core(root_name.to_string(), root_path.display().to_string(), root_src.to_string(), &fetch)
                .map_err(|m| m.trim_end().to_string())?;
        let root = loaded.index[&loaded.root_name];
        let sources: Vec<(String, String, String)> =
            loaded.sources.into_iter().enumerate().filter(|(i, _)| *i != root).map(|(_, s)| s).collect();
        let index: HashMap<String, usize> =
            sources.iter().enumerate().map(|(i, (n, _, _))| (n.clone(), i)).collect();
        let parsed = driver::parse_all(&sources)?;
        let graph = driver::import_graph(&parsed.ast, &parsed.programs, &index);
        let ast = Box::new(parsed.ast);
        // SAFETY: see `Base`: the box outlives the checkers and is not mutated.
        let ast_ref: &'static Ast = unsafe { &*(ast.as_ref() as *const Ast) };
        let checked =
            driver::check_modules_with(ast_ref, &parsed.programs, &graph, &sources, false, None, parsed.errors, false);
        let mark = checked.types.mark();
        let stamps = sources.iter().map(|(_, path, src)| stamp(path, src, overlay)).collect();
        Ok(Base {
            checkers: checked.checkers,
            ast,
            programs: parsed.programs,
            sources,
            index,
            expr_ranges: parsed.expr_ranges,
            types: checked.types,
            mark,
            stamps,
            root_name: root_name.to_string(),
            root_imports,
            generation: std::cell::Cell::new(0),
        })
    }

    /// Whether every base source still reads as it did when the base was built.
    fn stamps_hold(&self, overlay: &HashMap<PathBuf, String>) -> bool {
        self.sources.iter().zip(&self.stamps).all(|((_, path, src), s)| stamp(path, src, overlay) == *s)
    }
}

fn stamp(path: &str, src: &str, overlay: &HashMap<PathBuf, String>) -> Stamp {
    let p = PathBuf::from(path);
    if let Some(text) = overlay.get(&p) {
        return Stamp::Overlay { path: p, text: text.clone() };
    }
    match std::fs::metadata(&p) {
        Ok(m) => Stamp::File { path: p, modified: m.modified().ok(), len: m.len() },
        Err(_) if crate::stdlib::MODULES.iter().any(|(_, s)| *s == src) => Stamp::Embedded,
        Err(_) => Stamp::File { path: p, modified: None, len: 0 },
    }
}

/// Check the document over `base`. A `fresh` check starts a new revision: it
/// rewinds the base's types (invalidating the last snapshot); a trial does not.
fn check_root(base: Rc<Base>, root_path: &Path, root_src: String, fresh: bool) -> Snapshot {
    if fresh {
        base.generation.set(base.generation.get() + 1);
        base.types.truncate(&base.mark);
    }
    let mut ast = Box::new((*base.ast).clone());
    let lo = ast.exprs.len();
    let (next, program, mut errors) = frontend::parse_recovering(std::mem::take(&mut *ast), &root_src);
    *ast = next;
    let program = match program {
        Ok(p) => p,
        Err(d) => {
            errors.push(d);
            let (next, empty, _) = frontend::parse_recovering(std::mem::take(&mut *ast), "$ @mod EMPTY\n");
            *ast = next;
            empty.expect("a bare module header parses")
        }
    };
    let range = lo..ast.exprs.len();
    let root = base.sources.len();
    let deps: Vec<usize> = ast
        .slice(program.items)
        .iter()
        .filter_map(|item| match item {
            Item::Import { module, .. } => {
                let name = ast.slice(*module).iter().map(|&s| ast.text(s)).collect::<Vec<_>>().join(".");
                base.index.get(&name).copied()
            }
            _ => None,
        })
        .collect();
    let mut programs = base.programs.clone();
    programs.push(program.clone());
    let mut sources = base.sources.clone();
    sources.push((base.root_name.clone(), root_path.display().to_string(), root_src.clone()));
    let mut expr_ranges = base.expr_ranges.clone();
    expr_ranges.push(range);

    // SAFETY: see `Snapshot`: `ast` is boxed and frozen from here on, and the
    // base, whose tree the imported names borrow, is held by the snapshot.
    let ast_ref: &'static Ast = unsafe { &*(ast.as_ref() as *const Ast) };
    let base_ref: &'static Base = unsafe { &*Rc::as_ptr(&base) };
    let (c_idx, core_idx) = driver::prelude_indices(&base_ref.sources);
    let setup = driver::Setup { lenient: false, open_effects: false, record_types: true };
    let mut checker =
        driver::prepare_checker(ast_ref, &program, &base_ref.types, &|k| base_ref.checkers.get(k), &deps, c_idx, core_idx, root, setup);
    if let Err(d) = checker.check_program(&program) {
        if checker.recovered() {
            errors.extend(checker.errors());
        } else {
            errors.push(d);
        }
    }

    let generative = is_generative(&root_src);
    let mut found = Vec::new();
    for mut d in errors {
        // Names a generator injects only exist once it runs (on save).
        if generative && d.root().code == Code::TypeUnbound {
            d = d
                .with_severity(Severity::Hint)
                .with_note("this module runs generators, which may define it; saving checks it fully");
        }
        let phase = usize::from(d.root().code != Code::UnexpectedToken);
        found.push(Found { module: root, diag: d, rank: phase * 1_000_000 });
    }
    for w in checker.warnings() {
        found.push(Found { module: root, diag: w.clone().with_severity(Severity::Warning), rank: 3_000_000 });
    }
    for (i, f) in found.iter_mut().enumerate() {
        f.rank += i;
    }
    let generation = base.generation.get();
    Snapshot { checker, ast, base, generation, programs, sources, root, expr_ranges, found }
}

/// Whether a module has compile-time code that may define names: a `@build`, or
/// an `@e` other than an `assert` check.
fn is_generative(src: &str) -> bool {
    if src.contains("@build") {
        return true;
    }
    src.lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .flat_map(|l| l.match_indices("@e").map(move |(i, _)| &l[i + 2..]))
        .filter(|rest| !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
        .any(|rest| {
            let rest = rest.trim_start();
            let rest = rest.strip_prefix("<|").or_else(|| rest.strip_prefix('(')).unwrap_or(rest).trim_start();
            !rest.starts_with("assert")
        })
}
