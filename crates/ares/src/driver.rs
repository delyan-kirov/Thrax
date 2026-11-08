//! The Ares driver (`DR`): module loading, dependency ordering, type-checking,
//! lowering, and the `lex`/`parse`/`check`/`run` subcommands.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::OnceLock;

use frontend::Lexer;
use frontend::{Item, Program};

use crate::stdlib;

mod resident;
use resident::ModuleRun;

// These shadow the std macros for this whole module, so a diagnostic printed
// while a progress line is open starts on a line of its own. Under
// [`capture_stderr`] they write to its buffer instead.
macro_rules! eprint {
    ($($arg:tt)*) => {
        match format_args!($($arg)*) {
            args => {
                if !$crate::driver::write_captured(args) {
                    utilities::progress::seal();
                    std::eprint!("{args}")
                }
            }
        }
    };
}

macro_rules! eprintln {
    ($($arg:tt)*) => {
        eprint!("{}\n", format_args!($($arg)*))
    };
}

thread_local! {
    /// What the driver's `eprint!` writes while [`capture_stderr`] runs.
    static CAPTURED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Append to the [`capture_stderr`] buffer; false when nothing is capturing.
pub(crate) fn write_captured(args: std::fmt::Arguments<'_>) -> bool {
    CAPTURED.with(|c| match c.borrow_mut().as_mut() {
        Some(buf) => {
            std::fmt::Write::write_fmt(buf, args).expect("writing to a String");
            true
        }
        None => false,
    })
}

/// Run `f` with the driver's diagnostics kept in a buffer rather than printed,
/// for a host with no stderr to read (the browser playground). A failure
/// returns what `f` would have printed.
fn capture_stderr<T>(f: impl FnOnce() -> Result<T, ExitCode>) -> Result<T, String> {
    let outer = CAPTURED.with(|c| c.borrow_mut().replace(String::new()));
    let result = f();
    let text = CAPTURED.with(|c| std::mem::replace(&mut *c.borrow_mut(), outer)).unwrap_or_default();
    result.map_err(|_| text)
}

/// How a compile finds a module it imports: the module's diagnostic path and
/// source. A file compile searches the disk ([`disk_fetch`]); a host with no
/// filesystem hands over its own ([`compile_program`]).
type Fetch = Rc<dyn Fn(&str) -> Result<(String, String), String>>;

/// The [`Fetch`] of a file compile: modules resolved around `root_dir`.
fn disk_fetch(root_dir: &Path) -> Fetch {
    let root_dir = root_dir.to_path_buf();
    Rc::new(move |name| load_module(name, &root_dir))
}

/// Report the `steps` steps of a command on stderr, as it takes them, when that
/// is a terminal.
fn start_progress(steps: usize) {
    use std::io::IsTerminal;
    if cfg!(target_arch = "wasm32")
        || !std::io::stderr().is_terminal()
        || std::env::var_os("TERM").is_some_and(|t| t == "dumb")
    {
        return;
    }
    utilities::progress::begin(steps, stderr_cols());
}

#[cfg(not(target_arch = "wasm32"))]
fn stderr_cols() -> usize {
    extern "C" {
        fn arx_term_err_cols() -> std::os::raw::c_int;
    }
    unsafe { arx_term_err_cols() }.max(0) as usize
}

#[cfg(target_arch = "wasm32")]
fn stderr_cols() -> usize {
    0
}

/// A module's name and source text, plus the name-to-slot index and the root
/// module name. Shared by `check` and `run`.
pub(crate) struct Loaded {
    /// One entry per module: `(module name, source path, source text)`. The path
    /// is what diagnostics render, so it stays a real, clickable file location.
    pub sources: Vec<(String, String, String)>,
    pub index: HashMap<String, usize>,
    pub root_name: String,
}

/// Load the root file and, transitively, every module it imports from the
/// standard library.
fn load_sources(path: &str) -> Result<Loaded, ExitCode> {
    let root_dir = Path::new(path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();

    let root_src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ares: cannot read {path}: {e}");
            return Err(ExitCode::FAILURE);
        }
    };
    let root_name = parse_mod_name(&root_src).unwrap_or_else(|| file_stem(path));

    load_core(root_name, path.to_string(), root_src, &|name| load_module(name, &root_dir)).map_err(|e| {
        eprintln!("{}", e.render().trim_end());
        ExitCode::FAILURE
    })
}

/// Why [`load_core`] failed.
pub(crate) enum LoadError {
    Message(String),
    /// A `$ @import` that names no loadable module, as a diagnostic in the
    /// importing module. `root_span` is the root's own `$ @import` that leads,
    /// possibly through other modules, to that importer; it is `None` when
    /// the importer is not reached from the root (CORE's own imports).
    Import { diag: utilities::Diagnostic, importer: String, path: String, src: String, root_span: Option<utilities::Span> },
}

impl LoadError {
    pub(crate) fn render(&self) -> String {
        match self {
            LoadError::Message(msg) => msg.clone(),
            LoadError::Import { diag, path, src, .. } => diag.render(src, path),
        }
    }
}

impl From<String> for LoadError {
    fn from(msg: String) -> LoadError {
        LoadError::Message(msg)
    }
}

impl From<LoadError> for String {
    fn from(e: LoadError) -> String {
        e.render()
    }
}

/// Transitively load the standard-library modules a given root module imports,
/// seeding the always-present `CORE` and auto-injected `C`. The root's name,
/// diagnostic path, and source are supplied by the caller, so this serves both
/// an on-disk file (`load_sources`) and an in-memory REPL session. `fetch` maps a
/// module name to its diagnostic path and source.
pub(crate) fn load_core(
    root_name: String,
    root_path: String,
    root_src: String,
    fetch: &dyn Fn(&str) -> Result<(String, String), String>,
) -> Result<Loaded, LoadError> {
    let mut sources: Vec<(String, String, String)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    // Each module reached from the root, mapped to the root's `$ @import` it came through.
    let mut via: HashMap<String, utilities::Span> = HashMap::new();
    let mut queue: Vec<(String, String, String)> =
        vec![(root_name.clone(), root_path, root_src)];

    // The implicitly imported CORE module (bare names everywhere) is an ordinary
    // standard-library module. Seed it into the load queue so it is always
    // present, even without an explicit `$ @import CORE`.
    if root_name != "CORE" {
        let (path, src) = fetch("CORE")?;
        queue.push(("CORE".to_string(), path, src));
    }
    while let Some((name, src_path, src)) = queue.pop() {
        if index.contains_key(&name) {
            continue;
        }
        let imports = parse_imports(&src);
        for (imp, span) in imports {
            if index.contains_key(&imp)
                || imp == name
                || queue.iter().any(|(n, _, _)| *n == imp)
            {
                continue;
            }
            let root_span = if name == root_name { Some(span) } else { via.get(&name).copied() };
            let fetched = if imp == BUILD_SCRIPT { Err(String::new()) } else { fetch(&imp) };
            let (path, imp_src) = fetched.map_err(|msg| LoadError::Import {
                diag: import_failure(&msg, &imp, &name, span),
                importer: name.clone(),
                path: src_path.clone(),
                src: src.clone(),
                root_span,
            })?;
            if let Some(at) = root_span {
                via.entry(imp.clone()).or_insert(at);
            }
            queue.push((imp, path, imp_src));
        }
        index.insert(name.clone(), sources.len());
        sources.push((name, src_path, src));
    }
    // Auto-inject the `C` namespace (libc + libm as `@extern` bindings),
    // reachable qualified (`C.sqrt`) with no import, like the prelude.
    if !index.contains_key("C") {
        let (path, src) = fetch("C")?;
        index.insert("C".to_string(), sources.len());
        sources.push(("C".to_string(), path, src));
    }

    Ok(Loaded {
        sources,
        index,
        root_name,
    })
}

/// Load every module an import names that is not loaded yet, transitively. The
/// initial set comes from [`load_core`]; this reopens it after an expansion round,
/// where a `@build` or a `$ @e` may have injected `$ @import M` for a module no
/// hand-written source mentions.
fn load_injected_imports(loaded: &mut Loaded, fetch: &Fetch) -> Result<(), String> {
    let mut queue: Vec<(String, utilities::Span, usize)> = Vec::new();
    for (i, (_, _, src)) in loaded.sources.iter().enumerate() {
        queue.extend(parse_imports(src).into_iter().map(|(name, span)| (name, span, i)));
    }
    while let Some((name, span, importer)) = queue.pop() {
        if loaded.index.contains_key(&name) {
            continue;
        }
        let fetched = if name == BUILD_SCRIPT { Err(String::new()) } else { fetch(&name) };
        let (path, src) = fetched.map_err(|msg| {
            let (importer, imp_path, imp_src) = &loaded.sources[importer];
            import_failure(&msg, &name, importer, span).render(imp_src, imp_path)
        })?;
        let me = loaded.sources.len();
        queue.extend(parse_imports(&src).into_iter().map(|(name, span)| (name, span, me)));
        loaded.index.insert(name.clone(), me);
        loaded.sources.push((name, path, src));
    }
    Ok(())
}

/// A module that could not be loaded, reported at the `$ @import` in `importer` that
/// names it.
fn import_failure(msg: &str, name: &str, importer: &str, span: utilities::Span) -> utilities::Diagnostic {
    if name == BUILD_SCRIPT {
        return utilities::Diagnostic::error(
            utilities::Code::BuildScript,
            span,
            0,
            "`BUILD` configures the build and cannot be imported",
        );
    }
    let msg = msg.strip_prefix("ares: ").unwrap_or(msg);
    let mut d = utilities::Diagnostic::error(utilities::Code::UnknownModule, span, 0, msg);
    if importer == BUILD_SCRIPT {
        d = d.with_note(
            "`BUILD.ares` runs before its `@import_path` directories are searched, so it sees only \
             the standard library, its own neighbours and `--import-dir`",
        );
    }
    let known = stdlib::MODULES.iter().map(|(n, _)| *n);
    if let Some(near) = utilities::error::closest(name, known) {
        d = d.with_fix(
            format!("did you mean `{near}`?"),
            vec![utilities::Edit::new(span, near)],
            utilities::Applicability::MaybeIncorrect,
        );
    }
    d
}

/// The dependency graph over parsed modules (edges point at imports).
pub(crate) fn import_graph(
    ast: &frontend::Ast,
    programs: &[Program],
    index: &HashMap<String, usize>,
) -> Vec<Vec<usize>> {
    let mut graph = vec![Vec::new(); programs.len()];
    for (i, program) in programs.iter().enumerate() {
        for item in ast.slice(program.items) {
            if let Item::Import { module, .. } = item {
                if let Some(&j) = index.get(&dotted(ast, *module)) {
                    graph[i].push(j);
                }
            }
        }
    }
    graph
}

/// Every module parsed into one shared [`frontend::Ast`], each past its broken
/// items. `errors[i]` holds module `i`'s parse errors. Fails (rendered) only when
/// a module's `$ @mod` header is unreadable, since then it has no program at all.
pub(crate) struct ParsedAll {
    pub ast: frontend::Ast,
    pub programs: Vec<Program>,
    pub errors: Vec<Vec<utilities::Diagnostic>>,
    /// The expression handles each module's parse created (a contiguous run,
    /// since modules are parsed one after another into the shared store).
    pub expr_ranges: Vec<std::ops::Range<usize>>,
}

pub(crate) fn parse_all(sources: &[(String, String, String)]) -> Result<ParsedAll, String> {
    let mut ast = frontend::Ast::new();
    let mut programs = Vec::with_capacity(sources.len());
    let mut errors = Vec::with_capacity(sources.len());
    let mut expr_ranges = Vec::with_capacity(sources.len());
    for (_name, src_path, src) in sources {
        let lo = ast.exprs.len();
        let (next_ast, program, errs) = frontend::parse_recovering(ast, src);
        ast = next_ast;
        expr_ranges.push(lo..ast.exprs.len());
        match program {
            Ok(p) => programs.push(p),
            Err(d) => return Err(d.render(src, src_path)),
        }
        errors.push(errs);
    }
    Ok(ParsedAll { ast, programs, errors, expr_ranges })
}

thread_local! {
    /// The project options of the program being compiled, set by its
    /// `BUILD.ares` once that has run (see [`lower_all`]).
    static PROJECT: std::cell::RefCell<Option<utilities::config::Config>> = const { std::cell::RefCell::new(None) };
}

fn project_config() -> utilities::config::Config {
    PROJECT.with(|c| c.borrow().clone()).unwrap_or_default()
}

/// Render the errors of every module, in the order given (dependencies first),
/// capped at the project's `[check] max_errors`.
fn render_errors(order: &[usize], errors: &[Vec<utilities::Diagnostic>], sources: &[(String, String, String)]) -> String {
    let all: Vec<(usize, &utilities::Diagnostic)> = order
        .iter()
        .flat_map(|&i| errors[i].iter().map(move |d| (i, d)))
        .collect();
    let max = project_config().int("check", "max_errors") as usize;
    let mut out = String::new();
    for (i, d) in all.iter().take(max) {
        let (_, path, src) = &sources[*i];
        out.push_str(&d.render(src, path));
    }
    if all.len() > max {
        out.push_str(&format!("ares: {} more error(s) not shown\n", all.len() - max));
    }
    out
}

/// The public value names each standard-library module defines, inverted: name
/// to the modules that define it. Read off the sources' `$ name` lines (no
/// parse), so it is cheap enough to build once per process.
fn import_hints() -> Rc<HashMap<utilities::StrId, Vec<utilities::StrId>>> {
    thread_local! {
        static HINTS: Rc<HashMap<utilities::StrId, Vec<utilities::StrId>>> = Rc::new(build_import_hints());
    }
    HINTS.with(Rc::clone)
}

fn build_import_hints() -> HashMap<utilities::StrId, Vec<utilities::StrId>> {
    let mut hints: HashMap<utilities::StrId, Vec<utilities::StrId>> = HashMap::new();
    for (module, src) in stdlib::MODULES {
        if matches!(*module, "CORE" | "C") {
            continue;
        }
        for line in src.lines() {
            if line.starts_with("$ @private") {
                break;
            }
            let Some(rest) = line.strip_prefix("$ ") else { continue };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.starts_with(|c: char| c.is_ascii_lowercase())  {
                let homes = hints.entry(name.into()).or_default();
                let module = utilities::StrId::intern(module);
                if !homes.contains(&module) {
                    homes.push(module);
                }
            }
        }
    }
    hints
}

/// Print a rendered warning once per process. A module is checked again on
/// every metaprogram-expansion round and every REPL entry, and would otherwise
/// repeat the same warning each time.
fn report_warning(rendered: String) {
    static SEEN: std::sync::Mutex<Option<HashSet<String>>> = std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.get_or_insert_with(HashSet::new).insert(rendered.clone()) {
        eprint!("{rendered}");
    }
}

/// Type-check every module in dependency order, reusing the modules an earlier
/// compile kept (see [`resident`]), and report what the check found: warnings
/// printed, errors rendered as the `Err` (or sent to the editor's sink). A header
/// that does not parse is an `Err` too.
fn check_all(loaded: &Loaded, lenient: bool, session: Option<SessionRoot<'_>>) -> Result<ModuleRun, String> {
    let mut run = ModuleRun::check(&loaded.sources, &loaded.index, lenient, session)?;
    let sources = &loaded.sources;
    let level = project_config().word("lints", "unreachable_arm").to_string();
    for &i in &run.order.clone() {
        let (_name, src_path, src) = &sources[i];
        for w in run.warnings[i].clone() {
            match level.as_str() {
                "allow" if w.root().code == utilities::Code::UnreachableArm => {}
                "deny" if w.root().code == utilities::Code::UnreachableArm => {
                    run.errors[i].push(w.with_note("`[lints] unreachable_arm = \"deny\"` makes this an error"));
                }
                _ => report_warning(w.render_warning(src, src_path)),
            }
        }
    }
    if run.errors.iter().any(|e| !e.is_empty()) {
        if sink_active() {
            for &i in &run.order {
                for d in &run.errors[i] {
                    sink_push(&sources[i].0, d.clone());
                }
            }
            return Err(String::new());
        }
        return Err(render_errors(&run.order, &run.errors, sources));
    }
    Ok(run)
}

/// Every module checked from scratch over a type store of its own, failed ones
/// included: the checkers, indexed by module, and the store. For an editor's
/// base, which keeps its own checkers.
pub(crate) fn check_modules<'a>(
    ast: &'a frontend::Ast,
    programs: &[Program],
    graph: &[Vec<usize>],
    sources: &[(String, String, String)],
    parse_errors: Vec<Vec<utilities::Diagnostic>>,
) -> (Vec<frontend::Checker<'a>>, Rc<frontend::Types>) {
    // One type store for the whole compilation: a `Type` is a handle into it, so a
    // type crossing a module boundary has to address the same store.
    let types = Rc::new(frontend::Types::new());
    let modules = ModuleSet { ast, programs, graph, sources };
    let setup = Setup { lenient: false, open_effects: false, record_types: false, probe: false };
    let over = check_modules_over(modules, &types, setup, None, parse_errors, &|_| None, &mut |_| {});
    (over.checkers.into_iter().map(|c| c.expect("nothing reused")).collect(), types)
}

/// The modules of one compilation, parsed into one tree.
#[derive(Clone, Copy)]
pub(crate) struct ModuleSet<'a, 's> {
    pub ast: &'a frontend::Ast,
    pub programs: &'s [Program],
    pub graph: &'s [Vec<usize>],
    pub sources: &'s [(String, String, String)],
}

/// What [`check_modules_over`] found, indexed by module: each checker (none for
/// a module that was reused), its definitions' types, its errors and warnings,
/// and the order the modules were checked in, dependencies first, which is also
/// the order their diagnostics are worth reading in.
pub(crate) struct CheckedOver<'a> {
    pub checkers: Vec<Option<frontend::Checker<'a>>>,
    pub results: Vec<Vec<(utilities::StrId, frontend::Type)>>,
    pub errors: Vec<Vec<utilities::Diagnostic>>,
    pub warnings: Vec<Vec<utilities::Diagnostic>>,
    pub order: Vec<usize>,
}

/// A module checked by an earlier compilation and reused as it is: its checker
/// and its definitions' types.
pub(crate) type Reused<'c, 'a> = (&'c frontend::Checker<'a>, &'c [(utilities::StrId, frontend::Type)]);

/// Type-check every module in dependency order over the type store `types`,
/// except those `reused` supplies, which their dependents import from as they
/// are. `checked` is told each module this checks, right after it does.
/// `setup` holds the options of every module but the session root's.
pub(crate) fn check_modules_over<'a, 'c>(
    modules: ModuleSet<'a, '_>,
    types: &Rc<frontend::Types>,
    setup: Setup,
    session: Option<SessionRoot<'_>>,
    parse_errors: Vec<Vec<utilities::Diagnostic>>,
    reused: &dyn Fn(usize) -> Option<Reused<'c, 'a>>,
    checked: &mut dyn FnMut(usize),
) -> CheckedOver<'a> {
    let ModuleSet { ast, programs, graph, sources } = modules;
    let mut errors = parse_errors;
    errors.resize_with(programs.len(), Vec::new);
    let mut warnings = vec![Vec::new(); programs.len()];
    let mut checkers: Vec<Option<frontend::Checker>> = (0..programs.len()).map(|_| None).collect();
    let mut results: Vec<Vec<(utilities::StrId, frontend::Type)>> = vec![Vec::new(); programs.len()];

    // The auto-injected `C` namespace and the implicitly imported `CORE` module
    // have no dependencies and are checked first: `C` made available qualified-only
    // (`C.sqrt`), `CORE` bare (its `to_string`, its instances, etc.). `CORE` is checked
    // after `C` but imports neither, so ordering the two is unconstrained.
    let (c_idx, core_idx) = prelude_indices(sources);
    let mut order: Vec<usize> = topological_order(graph);
    for &pre in [core_idx, c_idx].iter().flatten() {
        order.retain(|&i| i != pre);
        order.insert(0, pre);
    }
    let mut hollow = vec![false; programs.len()];
    for &i in &order {
        if let Some((_, defs)) = reused(i) {
            results[i] = defs.to_vec();
            continue;
        }
        let root = session.filter(|s| s.module == sources[i].0);
        let setup = Setup {
            open_effects: root.is_some_and(|s| s.open_effects),
            probe: root.is_some(),
            ..setup
        };
        let lookup = |k: usize| reused(k).map(|(c, _)| c).or(checkers[k].as_ref());
        let mut checker = prepare_checker(ast, &programs[i], types, &lookup, &graph[i], c_idx, core_idx, i, setup);
        let outcome = checker.check_program(&programs[i]);
        // A dependency that failed before exporting anything leaves every name it
        // defines unbound here, so this module's errors would only echo it.
        let echo = graph[i].iter().any(|&d| hollow[d]);
        warnings[i] = checker.warnings().to_vec();
        match outcome {
            Ok(defs) => results[i] = defs,
            Err(diag) => {
                hollow[i] = echo || checker.own_values().is_empty();
                if echo {
                } else if !checker.recovered() {
                    errors[i].push(diag);
                } else {
                    errors[i].extend(checker.errors());
                }
                results[i] = checker.own_values().to_vec();
            }
        }
        checkers[i] = Some(checker);
        checked(i);
    }
    CheckedOver { checkers, results, errors, warnings, order }
}

/// Where the auto-injected `C` and the implicit `CORE` sit among the modules.
pub(crate) fn prelude_indices(sources: &[(String, String, String)]) -> (Option<usize>, Option<usize>) {
    (
        sources.iter().position(|(n, _, _)| n == "C"),
        sources.iter().position(|(n, _, _)| n == "CORE"),
    )
}

/// The binding the shell and `@eval` wrap an expression in, the one global whose
/// type is inferred rather than written.
pub const PROBE: &str = "__it";

/// The in-memory module a tool compiles around one expression (see [`PROBE`]).
#[derive(Clone, Copy)]
pub(crate) struct SessionRoot<'s> {
    pub module: &'s str,
    /// Check the top level under an open effect row, as the shell needs to run IO.
    pub open_effects: bool,
}

/// How one module is checked.
#[derive(Clone, Copy)]
pub(crate) struct Setup {
    pub lenient: bool,
    pub open_effects: bool,
    pub record_types: bool,
    /// The module may leave [`PROBE`] unannotated.
    pub probe: bool,
}

/// A checker for module `i`, with `C` (qualified), `CORE` (bare) and its
/// direct imports `deps` brought in from their already-run checkers, each as
/// `program`'s `$ @import` items for it say (bare, aliased, or both).
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_checker<'a, 'c>(
    ast: &'a frontend::Ast,
    program: &Program,
    types: &Rc<frontend::Types>,
    checkers: &dyn Fn(usize) -> Option<&'c frontend::Checker<'a>>,
    deps: &[usize],
    c_idx: Option<usize>,
    core_idx: Option<usize>,
    i: usize,
    setup: Setup,
) -> frontend::Checker<'a> {
    let mut checker = frontend::Checker::new(ast, types.clone());
    checker.set_lenient(setup.lenient);
    checker.set_import_hints(import_hints());
    checker.set_record_types(setup.record_types);
    checker.set_open_effects(setup.open_effects);
    if setup.probe {
        checker.set_inferred(PROBE);
    }
    if let Some(c) = c_idx {
        if c != i && Some(i) != core_idx {
            checker.import_qualified(checkers(c).expect("C checked first"));
        }
    }
    if let Some(core) = core_idx {
        if core != i && Some(i) != c_idx && !deps.contains(&core) {
            checker.import_from(checkers(core).expect("CORE checked first"));
        }
    }
    let mut done = HashSet::new();
    for &dep in deps {
        if !done.insert(dep) {
            continue;
        }
        let other = checkers(dep).expect("dependency checked first");
        let (bare, aliases) = import_forms(ast, program, other.module_name().as_str());
        checker.import_as(other, bare || Some(dep) == core_idx, &aliases);
    }
    checker
}

/// How `program` imports `module`: whether bare (`$ @import M`), and under which
/// aliases (`$ @import A = M`).
fn import_forms(ast: &frontend::Ast, program: &Program, module: &str) -> (bool, Vec<utilities::StrId>) {
    let mut bare = false;
    let mut aliases = Vec::new();
    for item in ast.slice(program.items) {
        let Item::Import { module: m, alias, .. } = item else { continue };
        if dotted(ast, *m) != module {
            continue;
        }
        match alias {
            Some(a) => aliases.push(*a),
            None => bare = true,
        }
    }
    (bare, aliases)
}

/// A dotted module path as written, `A.B`.
pub(crate) fn dotted(ast: &frontend::Ast, parts: utilities::Slice<utilities::StrId>) -> String {
    ast.slice(parts).iter().map(|&p| ast.text(p)).collect::<Vec<_>>().join(".")
}

/// The full pipeline up to (but not including) execution: load, parse, check,
/// and lower every module, returning the lowered modules (root first). The
/// parse/check state is local (self-borrowing), so only the owned `lowered`
/// escapes; the expansion loop in [`lower_all`] calls this repeatedly on
/// progressively `@e`-expanded sources.
type Compiled = (
    Vec<frontend::core::CoreProgram>,
    Reflection,
    Vec<Option<u64>>,
);

/// What compile-time code can ask about a compile: the declared types and global
/// values (`@type_of`, `@lookup`), and per module that has a site to run, the
/// names in scope there (`@scope`).
struct Reflection {
    info: frontend::ReflectInfo,
    scopes: HashMap<utilities::StrId, Rc<Vec<(String, String)>>>,
}

/// The [`Reflection`] of a checked run. Rendering every global's type is only
/// worth it when some module has a site to ask: `scoped` says which do.
fn reflection(run: &ModuleRun, loaded: &Loaded, resolved: &frontend::Resolved, scoped: &dyn Fn(&str) -> bool) -> Reflection {
    let mut info = resolved.declarations.reflect();
    let mut scopes = HashMap::new();
    let modules: Vec<usize> = (0..loaded.sources.len()).filter(|&i| scoped(&loaded.sources[i].0)).collect();
    if modules.is_empty() {
        return Reflection { info, scopes };
    }
    // Each global as its own module renders it: only there does its type carry
    // its `@use` fields.
    let mut declared: HashMap<utilities::StrId, Vec<String>> = HashMap::new();
    for (i, defs) in run.results.iter().enumerate() {
        let module = utilities::StrId::intern(&loaded.sources[i].0);
        let checker = run.checker(i);
        for (name, ty) in defs {
            let shown = checker.show_decl(*name, *ty);
            declared.entry(*name).or_default().push(shown.clone());
            info.values.push((module, *name, shown));
        }
    }
    for i in modules {
        let visible = run.checker(i).visible_values().into_iter().filter(|(n, _)| *n != PROBE);
        let entries = visible
            .map(|(n, ty)| match declared.get(&utilities::StrId::intern(n)).map(Vec::as_slice) {
                Some([shown]) => (n.to_string(), shown.clone()),
                _ => (n.to_string(), ty),
            })
            .collect();
        scopes.insert(utilities::StrId::intern(&loaded.sources[i].0), Rc::new(entries));
    }
    Reflection { info, scopes }
}

/// `shell` checks the root as the shell's session, under an open effect row.
fn compile_sources(loaded: &Loaded, lenient: bool, want_entry: bool, shell: bool) -> Result<Compiled, ExitCode> {
    let session = shell.then(|| SessionRoot { module: &loaded.root_name, open_effects: true });
    // A lenient round tolerates unbound names a generator may still inject. Once
    // the round fails anyway those names are worth reporting too, so the report
    // comes from a strict check of the same sources.
    let mut run = match check_all(loaded, lenient, session) {
        Ok(run) => run,
        Err(rendered) => {
            let rendered = if lenient { check_all(loaded, false, session).err().unwrap_or(rendered) } else { rendered };
            eprint!("{rendered}");
            return Err(ExitCode::FAILURE);
        }
    };

    let resolved = frontend::collect_resolved(run.checkers());

    // Lower every module; put the root first so its names win when resolving an
    // unqualified reference defined in more than one module.
    let root = loaded.index[&loaded.root_name];
    let mut order: Vec<usize> = (0..run.programs.len()).collect();
    order.sort_by_key(|&i| i != root);
    let lowered = run.desugar(&order, &resolved);
    let with_sites: HashSet<&str> = lowered
        .iter()
        .filter(|p| {
            !(p.ct_runs.is_empty()
                && p.ct_evals.is_empty()
                && p.ct_types.is_empty()
                && p.ct_patterns.is_empty()
                && p.ct_rows.is_empty())
                || p.ct_build.is_some()
        })
        .map(|p| p.module.as_str())
        .collect();
    let reflect = reflection(&run, loaded, &resolved, &|m| with_sites.contains(m));

    // `$ @build` has one signature, like the entry: one name, one shape. Its own
    // annotation is known even in a lenient round, so this is checked every round
    // and a wrong shape is reported before the build function is ever run.
    for (i, defs) in run.results.iter().enumerate() {
        let Some((_, ty)) = defs.iter().find(|(n, _)| *n == frontend::BUILD) else {
            continue;
        };
        if !frontend::is_build_type(run.checker(i).types(), *ty) {
            eprintln!(
                "ares: `{}` of module `{}` must have the signature `{}`",
                frontend::BUILD,
                loaded.sources[i].0,
                frontend::BUILD_SIG
            );
            return Err(ExitCode::FAILURE);
        }
    }

    // Only a command that RUNS the program needs an entry (`check` type-checks a
    // library module too), and a metaprogram-expansion round does not: the entry
    // may itself be injected, so the requirement waits for the final strict round.
    if want_entry && !lenient {
        if let Err(msg) = check_entry(&lowered[0], &run.results[root], run.checker(root), &loaded.root_name) {
            eprintln!("ares: {msg}");
            return Err(ExitCode::FAILURE);
        }
    }
    let kept = run.kept_ids(&order);
    Ok((lowered, reflect, kept))
}

/// Whether the root module (lowered as `root`, checked as `results` by `checker`)
/// has an entry to run, with the one signature an entry has.
fn check_entry(
    root: &frontend::core::CoreProgram,
    results: &[(utilities::StrId, frontend::Type)],
    checker: &frontend::Checker,
    root_name: &str,
) -> Result<(), String> {
    if !root.globals.iter().any(|(n, _)| n == frontend::ENTRY) {
        return Err(format!(
            "module `{root_name}` has no `$ {} : {}` to run",
            frontend::ENTRY,
            frontend::ENTRY_SIG
        ));
    }
    let ok = results
        .iter()
        .find(|(n, _)| *n == frontend::ENTRY)
        .is_some_and(|(_, ty)| frontend::is_entry_type(checker.types(), *ty));
    if !ok {
        return Err(format!("`{}` must have the signature `{}`", frontend::ENTRY, frontend::ENTRY_SIG));
    }
    Ok(())
}

/// Render a compile-time `@e` fault at its call site: fill the fault's (sentinel)
/// span with the site's span and render against that module's source, so the
/// caret lands on the `@e` in the user's file rather than a synthetic global.
fn render_e_fault(
    diag: utilities::Diagnostic,
    module: &str,
    span: utilities::Span,
    sources: &[(String, String, String)],
) -> String {
    let (src, path) = sources
        .iter()
        .find(|(n, _, _)| n == module)
        .map(|(_, p, s)| (s.as_str(), p.as_str()))
        .unwrap_or(("", module));
    let diag = place_in_literal(diag, src, span).fill_span(span);
    if sink_active() {
        sink_push(module, diag);
        return String::new();
    }
    diag.render(src, path)
}

/// Move an error raised inside a `@parse_str` / `@parse_items` fragment into the
/// string literal that spells it: the first one inside the `@e` site that
/// decodes to the fragment, else the first anywhere in the module. A fragment
/// no literal spells (built at compile time) stays at the site, quoted in a
/// note.
fn place_in_literal(diag: utilities::Diagnostic, src: &str, site: utilities::Span) -> utilities::Diagnostic {
    let Some(text) = diag.fragment().map(str::to_owned) else { return diag };
    let mut found = None;
    let mut lx = frontend::Lexer::new(src);
    while let Ok(t) = lx.next_token() {
        if matches!(t.kind, frontend::Kind::Eof) {
            break;
        }
        if !matches!(t.kind, frontend::Kind::Str) {
            continue;
        }
        let Some((bytes, at)) = frontend::lexer::decode_string_offsets(&src[t.span.start..t.span.end]) else {
            continue;
        };
        if bytes != text.as_bytes() {
            continue;
        }
        let inside = site.start <= t.span.start && t.span.end <= site.end;
        if found.is_none() || inside {
            found = Some((t.span.start, at, inside));
        }
        if inside {
            break;
        }
    }
    match found {
        Some((start, at, inside)) => {
            let diag = diag.place_fragment(|s| {
                let (a, b) = (start + at[s.start], start + at[s.end]);
                Some(utilities::Span::new(a, b.max(a + 1)))
            });
            if inside { diag } else { diag.with_label(site, "the `@e` that ran it") }
        }
        None => diag
            .place_fragment(|_| None)
            .with_note(format!("in the generated code `{}`", text.escape_debug())),
    }
}

/// Where diagnostics go when an editor drives the pipeline: collected here as
/// `(module, diagnostic)` instead of rendered to stderr. Spans are mapped back
/// through the expansion splices to the source as written.
struct Sink {
    found: Vec<(String, utilities::Diagnostic)>,
    /// Per round, per module, the splices `(start, old_len, new_len)` in the
    /// round's own coordinates, sorted by start.
    rounds: Vec<HashMap<String, Vec<(usize, usize, usize)>>>,
}

thread_local! {
    static SINK: std::cell::RefCell<Option<Sink>> = const { std::cell::RefCell::new(None) };
}

fn sink_active() -> bool {
    SINK.with(|s| s.borrow().is_some())
}

/// Collect a diagnostic of `module` (span in the current round's text).
fn sink_push(module: &str, diag: utilities::Diagnostic) {
    SINK.with(|s| {
        if let Some(sink) = s.borrow_mut().as_mut() {
            sink.found.push((module.to_string(), diag));
        }
    });
}

/// A position in `module`'s expanded text, as a position in the text as
/// written: a position inside spliced-in code maps to the site it replaced.
fn map_back(rounds: &[HashMap<String, Vec<(usize, usize, usize)>>], module: &str, pos: usize) -> usize {
    let mut p = pos as isize;
    for round in rounds.iter().rev() {
        let Some(edits) = round.get(module) else { continue };
        let mut delta: isize = 0;
        let mut inside = None;
        for &(start, old, new) in edits {
            let post = start as isize + delta;
            if p < post {
                break;
            }
            if p < post + new as isize {
                inside = Some(start as isize);
                break;
            }
            delta += new as isize - old as isize;
        }
        p = inside.unwrap_or(p - delta);
    }
    p.max(0) as usize
}

/// Run `path`'s whole compile-time pipeline (every `$ @e` and `@build`, then the
/// strict check) and return what it reports, as `(module, diagnostic)` with
/// spans in the sources as written. For an editor's on-save check.
pub(crate) fn compile_time_diagnostics(path: &str) -> Vec<(String, utilities::Diagnostic)> {
    SINK.with(|s| *s.borrow_mut() = Some(Sink { found: Vec::new(), rounds: Vec::new() }));
    let _ = lower_all(path, false);
    let sink = SINK.with(|s| s.borrow_mut().take()).expect("installed above");
    let mut seen = HashSet::new();
    sink.found
        .into_iter()
        .filter(|(m, d)| seen.insert((m.clone(), d.root().span.start, d.root().span.end, d.root().msg.clone())))
        .map(|(module, d)| {
            let moved = d.root().span != utilities::Span::at(0)
                && map_back(&sink.rounds, &module, d.root().span.start) != d.root().span.start;
            let d = d.map_spans(|s| {
                let start = map_back(&sink.rounds, &module, s.start);
                utilities::Span::new(start, map_back(&sink.rounds, &module, s.end).max(start))
            });
            let d = if moved { d.with_note("raised in code a compile-time generator produced") } else { d };
            (module, d)
        })
        .collect()
}

/// How many expansion rounds [`lower_all`] runs before it gives up. Staging depth
/// is unbounded by design (an `@e` may inject another `@e`), so this only stops a
/// generator that injects a fresh site every round from hanging the build.
const MAX_META_ROUNDS: usize = 100;

/// Compile `path`, iteratively expanding every `@e` (compile-time code
/// insertion): each round compiles the current sources, forces every `@e` site,
/// and substitutes the code it returns in source, repeating until
/// none remain. `@link`/`@link_path` calls record build directives along the way,
/// drained into the returned `BuildPlan`. Returns the expanded modules too, so a
/// caller can report against the sources the checker actually saw. `want_entry`
/// demands a `$ @main` of the root module: set by the commands that run or build
/// the program, clear for a plain `check`.
fn lower_all(
    path: &str,
    want_entry: bool,
) -> Result<(Vec<frontend::core::CoreProgram>, Loaded, BuildPlan), ExitCode> {
    utilities::progress::step("PARSING");
    if file_stem(path) == BUILD_SCRIPT {
        return in_build_script(|| expand_all(path, want_entry));
    }
    let root_dir = Path::new(path).parent().map(Path::to_path_buf).unwrap_or_default();
    let Some(built) = project(&root_dir) else {
        return Err(ExitCode::FAILURE);
    };
    PROJECT.with(|c| *c.borrow_mut() = Some(built.config));
    expand_all(path, want_entry)
}

/// [`lower_all`] once the project's `BUILD.ares` has run.
fn expand_all(
    path: &str,
    want_entry: bool,
) -> Result<(Vec<frontend::core::CoreProgram>, Loaded, BuildPlan), ExitCode> {
    let mut loaded = load_sources(path)?;
    let root_dir = Path::new(path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let (lowered, plan) = expand_loaded(&mut loaded, &disk_fetch(&root_dir), want_entry, false)?;
    Ok((lowered, loaded, plan))
}

/// The expansion loop of [`expand_all`] over sources already loaded, which it
/// rewrites in place, finding modules a generator imports through `fetch`.
/// `shell` compiles the root as the shell's session.
fn expand_loaded(
    loaded: &mut Loaded,
    fetch: &Fetch,
    want_entry: bool,
    shell: bool,
) -> Result<(Vec<frontend::core::CoreProgram>, BuildPlan), ExitCode> {
    let mut plan = BuildPlan::default();
    // The modules whose `$ @build` has already run. A `@build` is a definition, not
    // a directive, so nothing consumes it: this is what stops the expansion loop
    // from running it again every round.
    let mut built: HashSet<String> = HashSet::new();
    let _ = interpreter::machine::take_link_directives(); // drop any stale directives
    let mut rounds = 0;
    loop {
        // A generator can inject `$ @import M` for a module nothing imported yet, so
        // the load set is reopened every round rather than fixed before the first.
        if let Err(msg) = load_injected_imports(loaded, fetch) {
            eprintln!("{}", msg.trim_end());
            return Err(ExitCode::FAILURE);
        }
        // Compile leniently while `@e` sites remain: a generator may inject a
        // definition that hand-written code forward-references, which would not
        // yet resolve. Unbound names are deferred (fresh vars) so the generators
        // can still run; the final strict compile below validates the result.
        let compiled = compile_sources(loaded, true, want_entry, shell)?;
        // Gather every `@e` site this round, as `(module, qualified global, span,
        // how its code is spliced)`: item position (`$ @e X`) replaces the
        // directive with the items, expression and type position splice the code
        // parenthesized to keep precedence, and pattern position (`@e (X)`) splices
        // it bare, since a pattern has no parentheses.
        let sites: Vec<(String, String, utilities::Span, Splice)> = compiled
            .0
            .iter()
            .flat_map(|p| {
                let kinds = [
                    (&p.ct_runs, Splice::Items),
                    (&p.ct_evals, Splice::Parenthesized),
                    (&p.ct_types, Splice::Parenthesized),
                    (&p.ct_patterns, Splice::Bare),
                    (&p.ct_rows, Splice::Bare),
                ];
                kinds.into_iter().flat_map(move |(list, how)| {
                    list.iter()
                        .map(move |(name, span)| (p.module.to_string(), format!("{}.{}", p.module, name), *span, how))
                })
            })
            .collect();
        // `$ @build` sites: the synthetic `@build {}` call of every module whose
        // build function has not run yet, as `(module, qualified global)`.
        let build_sites: Vec<(String, String)> = compiled
            .0
            .iter()
            .filter(|p| !built.contains(p.module.as_str()))
            .filter_map(|p| {
                p.ct_build
                    .as_ref()
                    .map(|name| (p.module.to_string(), format!("{}.{}", p.module, name)))
            })
            .collect();
        if sites.is_empty() && build_sites.is_empty() {
            // No `@e` remains: re-check strictly so any name still unbound after
            // all injection, or a missing/ill-typed entry, is reported now.
            let final_compiled = compile_sources(loaded, false, want_entry, shell)?;
            // Fold in any `@link`/`@link_path` directives recorded while running.
            for (is_path, arg) in interpreter::machine::take_link_directives() {
                let set = if is_path { &mut plan.lib_paths } else { &mut plan.libs };
                if !set.contains(&arg) {
                    set.push(arg);
                }
            }
            return Ok((final_compiled.0, plan));
        }
        rounds += 1;
        if rounds > MAX_META_ROUNDS {
            clear_meta_hosts();
            eprintln!("ares: compile-time expansion did not settle after {MAX_META_ROUNDS} rounds");
            let pending = build_sites
                .iter()
                .map(|(module, _)| (module, utilities::Span::at(0)))
                .chain(sites.iter().map(|(module, _, span, _)| (module, *span)));
            for (module, span) in pending {
                let diag = utilities::Diagnostic::error(
                    utilities::Code::ExpansionLimit,
                    span,
                    0,
                    "this compile-time site is still pending",
                )
                .with_note("every round ran it and it left another site to expand");
                eprint!("{}", render_e_fault(diag, module, span, &loaded.sources));
            }
            return Err(ExitCode::FAILURE);
        }
        let ir = resident::lower_reusing(&compiled.0, &compiled.2);
        let reflect = reflect_tables(compiled.1);
        let round_sources: Rc<HashMap<String, (String, String)>> = Rc::new(
            loaded
                .sources
                .iter()
                .map(|(name, path, src)| (name.clone(), (path.clone(), src.clone())))
                .collect(),
        );
        let enter = |module: &str| {
            interpreter::machine::set_meta_eval(Some(round_eval_host(&round_sources, module, fetch)));
            interpreter::machine::set_meta_check(Some(round_check_host(&round_sources, module, fetch)));
            interpreter::machine::set_meta_scope(reflect.scopes.get(&utilities::StrId::intern(module)).cloned());
            interpreter::machine::set_type_host(Some(reflect_host(Rc::clone(&reflect), module.to_string())));
        };
        // `@build` first, and alone in its round: it contributes the definitions
        // the rest of the compile-time code may depend on, so its items have to
        // land before any `$ @e` that reads them runs. Each `@build` is forced
        // once, and the items of the `@Code` it returns are APPENDED to its
        // module's source. Appending (rather than replacing a span, as an `$ @e`
        // directive does) is what makes `@build` a definition that stays put: it
        // contributes items to its module instead of becoming them. Whatever it
        // injects is expanded by the rounds that follow, so a generated `$ @e`
        // still runs.
        let ran_builds = !build_sites.is_empty();
        for (module, qualified) in build_sites {
            enter(&module);
            interpreter::machine::set_meta_site(Some(site_of(&loaded.sources, &module, utilities::Span::at(0))));
            let script = module == BUILD_SCRIPT;
            if script {
                interpreter::machine::open_build_record();
            }
            let v = interpreter::machine::eval_value(&ir, &qualified);
            report_meta_warnings(&module, utilities::Span::at(0), &loaded.sources);
            if script {
                let record = interpreter::machine::take_build_record();
                BUILT.with(|c| *c.borrow_mut() = Some(record));
            }
            let v = match v {
                Ok(v) => v,
                Err(d) => {
                    clear_meta_hosts();
                    let d = d.with_note(format!(
                        "raised while running `{}` of module `{module}`",
                        frontend::BUILD
                    ));
                    eprint!("{}", render_e_fault(d, &module, utilities::Span::at(0), &loaded.sources));
                    return Err(ExitCode::FAILURE);
                }
            };
            // The signature check in `compile_sources` already pinned the result
            // type, so this guards only a malformed `@Code` value.
            let Some(items) = as_code_src(&v) else {
                clear_meta_hosts();
                eprintln!(
                    "ares: `{}` of module `{module}` did not produce an `@Code` of \
                     top-level items (build one with `@parse_items`)",
                    frontend::BUILD
                );
                return Err(ExitCode::FAILURE);
            };
            if script && !items.trim().is_empty() {
                clear_meta_hosts();
                let d = utilities::Diagnostic::error(
                    utilities::Code::BuildScript,
                    utilities::Span::at(0),
                    0,
                    format!("the `{}` of `BUILD.ares` returned code", frontend::BUILD),
                )
                .with_help("return `@parse_items \"\"`; `BUILD.ares` configures the build and adds no items");
                eprint!("{}", render_e_fault(d, &module, utilities::Span::at(0), &loaded.sources));
                return Err(ExitCode::FAILURE);
            }
            built.insert(module.clone());
            if let Some(i) = loaded.index.get(&module) {
                let src = &mut loaded.sources[*i].2;
                src.push('\n');
                src.push_str(&items);
                src.push('\n');
            }
        }
        if ran_builds {
            clear_meta_hosts();
            continue;
        }
        let mut edits: Vec<(String, utilities::Span, String)> = Vec::new();
        // A site that faults stays in the source and runs again next round: it may
        // depend on code another site of this round injects (a check over derived
        // operators), which only exists once the round is spliced. Faults are
        // reported only when a round makes no progress at all.
        let mut faults: Vec<(utilities::Diagnostic, String, utilities::Span)> = Vec::new();
        // A site that reaches code the lenient check left unresolved waits without
        // running. See Note [Deferred sites hold back generators] in `frontend::desugar`.
        let deferred: HashSet<utilities::StrId> = compiled
            .0
            .iter()
            .flat_map(|p| p.deferred.iter().map(move |n| utilities::StrId::intern(&format!("{}.{n}", p.module))))
            .collect();
        let code_of: HashMap<utilities::StrId, usize> = ir.globals.iter().copied().collect();
        let mut held = 0;
        let mut blocked = |qualified: &str| {
            let hit = reaches_any(&ir, &code_of, qualified, &deferred);
            held += hit as usize;
            hit
        };
        for (module, qualified, span, how) in sites {
            if blocked(&qualified) {
                continue;
            }
            enter(&module);
            interpreter::machine::set_meta_site(Some(site_of(&loaded.sources, &module, span)));
            let v = interpreter::machine::eval_value(&ir, &qualified);
            report_meta_warnings(&module, span, &loaded.sources);
            let v = match v {
                Ok(v) => v,
                Err(d) => {
                    faults.push((d, module, span));
                    continue;
                }
            };
            let code = as_code_src(&v).expect("the checker requires an `@e` operand to be `@Code`");
            let text = match how {
                Splice::Items | Splice::Bare => code,
                Splice::Parenthesized => format!("({code})"),
            };
            edits.push((module, span, text));
        }
        clear_meta_hosts();
        if edits.is_empty() {
            assert!(held + faults.len() > 0, "a round with sites and no edits must have faulted or waited");
            for (diag, module, span) in faults {
                let diag = diag.with_note("raised while running this compile-time `@e`");
                eprint!("{}", render_e_fault(diag, &module, span, &loaded.sources));
            }
            // Nothing will supply what the waiting sites lack any more, so what the
            // lenient check deferred is an error: the strict check reports it.
            if held > 0 {
                compile_sources(loaded, false, want_entry, shell)?;
                eprintln!("ares: a compile-time `@e` waits on code that never resolved");
            }
            return Err(ExitCode::FAILURE);
        }
        splice_sources(&mut loaded.sources, edits);
    }
}

/// How the code an `@e` site returns replaces the site.
#[derive(Clone, Copy)]
enum Splice {
    /// `$ @e X`: the whole directive becomes the items, so empty code (a check,
    /// an `@link`) drops it.
    Items,
    /// Expression and type position: the code, parenthesized.
    Parenthesized,
    /// Pattern position: the code as written.
    Bare,
}

/// Whether global `from` of `ir` is in `targets` or can reach one, through the
/// globals and closures its code references. `code_of` maps each global to its
/// code.
fn reaches_any(
    ir: &frontend::ir::IrProgram,
    code_of: &HashMap<utilities::StrId, usize>,
    from: &str,
    targets: &HashSet<utilities::StrId>,
) -> bool {
    use frontend::ir::data::{Atom, IrExpr};
    let from = utilities::StrId::intern(from);
    if targets.contains(&from) {
        return true;
    }
    if targets.is_empty() {
        return false;
    }
    let Some(&start) = code_of.get(&from) else { return false };
    let mut seen = HashSet::from([start]);
    let mut todo = vec![start];
    while let Some(code) = todo.pop() {
        let mut hit = false;
        ir.codes[code].body.any_node(&mut |e: &IrExpr| {
            fn visit(
                a: &Atom,
                code_of: &HashMap<utilities::StrId, usize>,
                targets: &HashSet<utilities::StrId>,
                next: &mut Vec<usize>,
            ) -> bool {
                match a {
                    Atom::Glob { name } if targets.contains(name) => true,
                    Atom::Glob { name } => {
                        next.extend(code_of.get(name));
                        false
                    }
                    Atom::Clos { code, captures } => {
                        next.push(*code);
                        captures.iter().any(|c| visit(c, code_of, targets, next))
                    }
                    _ => false,
                }
            }
            let mut next = Vec::new();
            e.for_each_operand(&mut |a| hit = hit || visit(a, code_of, targets, &mut next));
            todo.extend(next.into_iter().filter(|c| seen.insert(*c)));
            hit
        });
        if hit {
            return true;
        }
    }
    false
}

/// Clear both compile-time hosts (`@eval` and type reflection) installed around an
/// expansion round, so a stray meta op at runtime faults cleanly.
fn clear_meta_hosts() {
    interpreter::machine::set_meta_eval(None);
    interpreter::machine::set_meta_check(None);
    interpreter::machine::set_meta_site(None);
    interpreter::machine::set_meta_scope(None);
    interpreter::machine::set_type_host(None);
}

/// The `(file, line, column)` `@here` reports for a site at `span` of `module`,
/// in this round's text. The sentinel span (a `@build`) is the file's start.
fn site_of(sources: &[(String, String, String)], module: &str, span: utilities::Span) -> (String, i64, i64) {
    let Some((_, path, src)) = sources.iter().find(|(n, _, _)| n == module) else {
        return (module.to_string(), 1, 1);
    };
    let at = span.start.min(src.len());
    let before = &src[..at];
    let line = before.matches('\n').count() + 1;
    let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    (path.clone(), line as i64, column as i64)
}

/// Render what `@warn` reported while the site at `span` of `module` ran.
fn report_meta_warnings(module: &str, span: utilities::Span, sources: &[(String, String, String)]) {
    for w in interpreter::machine::take_meta_warnings() {
        let w = w.with_note("raised while running this compile-time `@e`");
        eprint!("{}", render_e_fault(w, module, span, sources));
    }
}

/// Every declared type shape a round can reflect on, keyed for both lookup
/// forms: `qualified` is the exact `module.name`, and `bare` lists every module
/// declaring a given name, sorted by module so the choice never depends on hash
/// order. Types are namespaced per module, so a bare name means "mine first".
struct ReflectTables {
    qualified: HashMap<(utilities::StrId, utilities::StrId), interpreter::machine::TypeInfo>,
    bare: HashMap<utilities::StrId, Vec<(utilities::StrId, interpreter::machine::TypeInfo)>>,
    /// What `@scope` reports, per module with a site.
    scopes: HashMap<utilities::StrId, Rc<Vec<(String, String)>>>,
}

/// Index a round's collected type shapes for `$ @e` reflection.
fn reflect_tables(Reflection { info: reflect, scopes }: Reflection) -> Rc<ReflectTables> {
    use interpreter::machine::TypeInfo;
    let mut qualified = HashMap::new();
    let mut bare: HashMap<utilities::StrId, Vec<(utilities::StrId, TypeInfo)>> = HashMap::new();
    let mut put = |module, name, info: TypeInfo| {
        qualified.insert((module, name), info.clone());
        bare.entry(name).or_default().push((module, info));
    };
    for (module, name, shape) in reflect.structs {
        put(module, name, TypeInfo::Struct(shape));
    }
    for (module, name, shape) in reflect.unions {
        put(module, name, TypeInfo::Union(shape));
    }
    for (module, name, ty) in reflect.values {
        put(module, name, TypeInfo::Value { ty });
    }
    for entries in bare.values_mut() {
        entries.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    }
    Rc::new(ReflectTables { qualified, bare, scopes })
}

/// The `$ @e` reflection host for the module whose directive is running: a
/// by-name lookup answering `@type_kind`/`@type_fields`/`@type_variants`/`@type_of`.
///
/// A bare name resolves to `caller`'s OWN type first. Types are namespaced per
/// module, so two modules may each declare a `Box`; without this a derive in one
/// module could reflect the other's shape and generate field accesses that do
/// not exist on the value it runs against. A name no module of the caller's own
/// declares falls back to the sole declarer, then to the first by module name.
fn reflect_host(
    tables: Rc<ReflectTables>,
    caller: String,
) -> Box<dyn Fn(&str) -> Option<interpreter::machine::TypeInfo>> {
    let caller = utilities::StrId::intern(&caller);
    Box::new(move |name: &str| {
        if let Some((module, bare)) = name.split_once('.') {
            return tables.qualified.get(&(module.into(), bare.into())).cloned();
        }
        let entries = tables.bare.get(&name.into())?;
        entries
            .iter()
            .find(|(m, _)| *m == caller)
            .or_else(|| entries.first())
            .map(|(_, info)| info.clone())
    })
}

/// If `v` is a `@Code` fragment, its source text.
fn as_code_src(v: &interpreter::machine::data::PVal) -> Option<String> {
    use interpreter::machine::data::Value;
    if let Value::Struct { name, fields } = &*v.borrow() {
        if name == "@Code" {
            return fields.iter().find(|(k, _)| *k == "src").and_then(|(_, s)| {
                match &*s.borrow() {
                    Value::Str(b) => Some(String::from_utf8_lossy(b).into_owned()),
                    _ => None,
                }
            });
        }
    }
    None
}

/// Apply `@e` source substitutions to each module, right-to-left within a module
/// so earlier spans stay valid as later ones are replaced.
fn splice_sources(sources: &mut [(String, String, String)], mut edits: Vec<(String, utilities::Span, String)>) {
    if sink_active() {
        let mut round: HashMap<String, Vec<(usize, usize, usize)>> = HashMap::new();
        for (module, span, text) in &edits {
            round.entry(module.clone()).or_default().push((span.start, span.len(), text.len()));
        }
        for v in round.values_mut() {
            v.sort_unstable();
        }
        SINK.with(|s| {
            if let Some(sink) = s.borrow_mut().as_mut() {
                sink.rounds.push(round);
            }
        });
    }
    edits.sort_by(|a, b| b.1.start.cmp(&a.1.start));
    for (module, span, text) in edits {
        if let Some((_, _, src)) = sources.iter_mut().find(|(n, _, _)| *n == module) {
            if span.end <= src.len() {
                src.replace_range(span.start..span.end, &text);
            }
        }
    }
}

/// A compiled REPL session: the lowered modules (root `REPL` first) and the
/// root module's top-level bindings as `(name, rendered type)`.
pub struct Session {
    pub lowered: Vec<frontend::core::CoreProgram>,
    /// Per module of `lowered`, the id of the kept module it is, if it is one.
    kept: Vec<Option<u64>>,
    pub decls: Vec<(String, String)>,
    /// What the shell's compile-time operations read, for a shell session.
    meta: Option<SessionMeta>,
}

/// The compile-time hosts of a shell session: its reflection tables, and the
/// sources an `@check`/`@eval` fragment compiles against.
struct SessionMeta {
    reflect: Rc<ReflectTables>,
    sources: Rc<HashMap<String, (String, String)>>,
    /// Where the evaluated item starts, for `@here`.
    site: (String, i64, i64),
}

impl Session {
    /// The session lowered to the IR. The library modules it shares with the
    /// compile before it are not lowered again.
    pub fn ir(&self) -> frontend::ir::IrProgram {
        resident::lower_reusing(&self.lowered, &self.kept)
    }

    /// Install the compile-time hosts for running the shell's `REPL` module: the
    /// shell evaluates at compile time, so `<@Meta>` operations work in it as in
    /// a `$ @e`. Undo with [`leave_session_meta`].
    pub fn enter_meta(&self, root_dir: &Path) {
        let Some(meta) = &self.meta else { return };
        let module = SHELL_MODULE;
        let fetch = disk_fetch(root_dir);
        interpreter::machine::set_meta_eval(Some(round_eval_host(&meta.sources, module, &fetch)));
        interpreter::machine::set_meta_check(Some(round_check_host(&meta.sources, module, &fetch)));
        interpreter::machine::set_meta_scope(meta.reflect.scopes.get(&utilities::StrId::intern(module)).cloned());
        interpreter::machine::set_type_host(Some(reflect_host(Rc::clone(&meta.reflect), module.to_string())));
        interpreter::machine::set_meta_site(Some(meta.site.clone()));
    }
}

/// Clear what [`Session::enter_meta`] installed, keeping the run-time `@eval` host.
pub fn leave_session_meta(root_dir: &Path) {
    clear_meta_hosts();
    install_eval_host(root_dir.to_path_buf());
}

/// The shell's in-memory root module.
pub const SHELL_MODULE: &str = "REPL";

/// Expand every `$ @e` (and `@build`) of the shell session `source`, as a file's
/// are, and return the expanded session. Diagnostics are printed.
pub fn expand_session(source: &str, root_dir: &Path) -> Option<String> {
    let fetch = |name: &str| load_module(name, root_dir);
    let mut loaded = match load_core(SHELL_MODULE.to_string(), "<repl>".to_string(), source.to_string(), &fetch) {
        Ok(loaded) => loaded,
        Err(e) => {
            print!("{}", e.render());
            return None;
        }
    };
    expand_loaded(&mut loaded, &disk_fetch(root_dir), false, true).ok()?;
    Some(loaded.sources[loaded.index[SHELL_MODULE]].2.clone())
}

/// Compile an in-memory `$ @mod REPL` session `source` against the standard
/// library (CORE + C + any `$ @import` imports resolved from `root_dir`). Returns
/// the lowered modules and the root's typed bindings, or a rendered diagnostic.
/// Unlike the file commands this renders errors into a string instead of exiting,
/// so the shell can print them and keep going.
pub fn compile_session(source: &str, root_dir: &Path) -> Result<Session, String> {
    compile_in_memory(SHELL_MODULE, source, &|name| load_module(name, root_dir), true)
}

/// Compile a whole program held in memory: `source` is the root module, named
/// by its own `$ @mod`, and `fetch` maps every module it imports (directly or
/// not, `CORE` and `C` included) to a diagnostic path and source. The root must
/// have an entry. Returns the lowered modules, root first, or the rendered
/// diagnostics. This is the pipeline for a host with no filesystem to search,
/// such as the browser playground. It expands every `$ @e` (and `@build`) as a
/// file compile does, so a generated item or member works the same there.
pub fn compile_program(
    source: &str,
    fetch: impl Fn(&str) -> Result<(String, String), String> + 'static,
) -> Result<Vec<frontend::core::CoreProgram>, String> {
    let fetch: Fetch = Rc::new(fetch);
    let root_name = parse_mod_name(source).unwrap_or_else(|| "MAIN".to_string());
    let mut loaded = load_core(root_name.clone(), root_name, source.to_string(), &*fetch).map_err(|e| e.render())?;
    let (lowered, _plan) = capture_stderr(|| expand_loaded(&mut loaded, &fetch, true, false))?;
    Ok(lowered)
}

/// Compile an in-memory shell or `@eval` root module (see [`SessionRoot`]).
/// With `open_effects` its top level is checked under an open effect row, as
/// the shell needs to run IO; without it the module must be pure, like a
/// file's top level.
fn compile_in_memory(
    root_name: &str,
    source: &str,
    fetch: &dyn Fn(&str) -> Result<(String, String), String>,
    open_effects: bool,
) -> Result<Session, String> {
    let loaded = load_core(root_name.to_string(), "<repl>".to_string(), source.to_string(), fetch)?;

    let session = Some(SessionRoot { module: &loaded.root_name, open_effects });
    let mut run = check_all(&loaded, false, session)?;
    let resolved = frontend::collect_resolved(run.checkers());

    let root = loaded.index[&loaded.root_name];
    let mut order: Vec<usize> = (0..run.programs.len()).collect();
    order.sort_by_key(|&i| i != root);
    let lowered = run.desugar(&order, &resolved);
    let kept = run.kept_ids(&order);

    // The shell runs its items at compile time: give it the hosts a `$ @e` has.
    let meta = if open_effects {
        let reflect = reflection(&run, &loaded, &resolved, &|m| m == loaded.root_name);
        // A fragment imports the session, but not the item being evaluated,
        // whose inferred probe only the session root may hold.
        let sources = loaded
            .sources
            .iter()
            .map(|(name, path, src)| {
                let src = match src.rfind(&format!("\n$ {PROBE}")) {
                    Some(at) if *name == loaded.root_name => &src[..at + 1],
                    _ => src.as_str(),
                };
                (name.clone(), (path.clone(), src.to_string()))
            })
            .collect();
        let src = &loaded.sources[root].2;
        let at = src.rfind(&format!("\n$ {PROBE}")).map_or(0, |at| at + 1);
        let site = site_of(&loaded.sources, &loaded.root_name, utilities::Span::at(at));
        Some(SessionMeta { reflect: reflect_tables(reflect), sources: Rc::new(sources), site })
    } else {
        None
    };

    let checker = run.checker(root);
    let decls = run.results[root]
        .iter()
        .map(|(n, ty)| (n.to_string(), checker.show_decl(*n, *ty)))
        .collect();

    Ok(Session {
        lowered,
        kept,
        decls,
        meta,
    })
}

/// Libraries and search paths a program's `$ @e <| @link ..` directives added to
/// the build. Applied to the native link line; the
/// interpreter's default set already covers libc/libm and lazily `dlopen`s the
/// rest per `@extern`.
#[derive(Default)]
struct BuildPlan {
    libs: Vec<String>,
    lib_paths: Vec<String>,
}


/// Compile and run an `@Code` fragment (source text), reifying its value. This
/// is the `@eval` host, at build time and at run time alike: it re-enters the same
/// pipeline the REPL uses, wrapping the fragment as a def body of an in-memory
/// module. The fragment must be pure, which is what lets `@eval` be typed pure.
/// `root_dir` resolves the fragment's imports (it sees the same standard library
/// as the root).
pub fn eval_fragment(
    src: &str,
    root_dir: &Path,
) -> std::result::Result<interpreter::machine::OwnedValue, String> {
    eval_wrapped(src, &[], &|name| load_module(name, root_dir))
}

/// Compile `src` as the body of a definition in an in-memory module that opens
/// with the `$ @import` lines `imports`, and run it.
fn eval_wrapped(
    src: &str,
    imports: &[String],
    fetch: &dyn Fn(&str) -> Result<(String, String), String>,
) -> std::result::Result<interpreter::machine::OwnedValue, String> {
    let session = compile_wrapped(src, imports, fetch)?;
    let ir = session.ir();
    let v = interpreter::machine::eval_value(&ir, &format!("{FRAGMENT}.{PROBE}"))
        .map_err(|d| d.render("", "@eval"))?;
    interpreter::machine::reify(&v)
}

/// The in-memory module `@eval` and `@check` wrap a fragment in. It is not the
/// shell's `REPL`, which a fragment run from the shell imports.
const FRAGMENT: &str = "FRAGMENT";

/// Compile `src` as the body of a definition in an in-memory module that opens
/// with the `$ @import` lines `imports`.
fn compile_wrapped(
    src: &str,
    imports: &[String],
    fetch: &dyn Fn(&str) -> Result<(String, String), String>,
) -> Result<Session, String> {
    let mut module = format!("$ @mod {FRAGMENT}\n");
    for line in imports {
        module.push_str(line);
        module.push('\n');
    }
    module.push_str(&format!("$ {PROBE} =\n"));
    module.push_str(src);
    compile_in_memory(FRAGMENT, &module, fetch, false)
}

/// The expansion-time `@eval` host for a directive in `module`. A fragment sees
/// the imports of the module whose directive runs it, and compiles against the
/// sources this round already holds, so no loaded module is read from disk again.
fn round_eval_host(
    sources: &Rc<HashMap<String, (String, String)>>,
    module: &str,
    fetch: &Fetch,
) -> Box<dyn Fn(&str) -> std::result::Result<interpreter::machine::OwnedValue, String>> {
    let imports = sources
        .get(module)
        .map(|(_, src)| import_lines(src))
        .unwrap_or_default();
    let sources = Rc::clone(sources);
    let fetch = Rc::clone(fetch);
    Box::new(move |src| {
        eval_wrapped(src, &imports, &|name| match sources.get(name) {
            Some((path, text)) => Ok((path.clone(), text.clone())),
            None => fetch(name),
        })
    })
}

/// The expansion-time `@check` host for a directive in `module`: like
/// [`round_eval_host`], but the fragment is only checked, and its type returned.
fn round_check_host(
    sources: &Rc<HashMap<String, (String, String)>>,
    module: &str,
    fetch: &Fetch,
) -> Box<dyn Fn(&str) -> std::result::Result<String, String>> {
    let mut imports = sources
        .get(module)
        .map(|(_, src)| import_lines(src))
        .unwrap_or_default();
    imports.push(format!("$ @import {module}"));
    let sources = Rc::clone(sources);
    let fetch = Rc::clone(fetch);
    Box::new(move |src| {
        let session = compile_wrapped(src, &imports, &|name| match sources.get(name) {
            Some((path, text)) => Ok((path.clone(), text.clone())),
            None => fetch(name),
        })?;
        let ty = session.decls.iter().find(|(n, _)| n == PROBE).map(|(_, t)| t.clone());
        Ok(ty.expect("the probe of a checked fragment has a type"))
    })
}

/// Install the `@eval` host for code that runs after compilation (`ares run`,
/// the shell, a native program calling into `libares`). The compile-time
/// expansion installs and clears its own around each round.
pub fn install_eval_host(root_dir: PathBuf) {
    interpreter::machine::set_meta_eval(Some(Box::new(move |src| eval_fragment(src, &root_dir))));
}

pub fn cmd_run(path: &str, prog_args: &[String]) -> ExitCode {
    start_progress(2);
    // `lower_all` has already expanded every `$ @e` (compile-time execution and
    // code injection); the lowered program is `@e`-free.
    let (lowered, _loaded, _plan) = match lower_all(path, true) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let ir = frontend::ir::lower_modules(&lowered);
    install_eval_host(
        Path::new(path).parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")),
    );
    // `@main` takes the argument vector (`argv[0]` = the entry path, then the
    // extra args) and its `@Int` result is the process exit code.
    let mut argv = vec![path.to_string()];
    argv.extend(prog_args.iter().cloned());
    utilities::progress::end_with("RUNNING");
    match interpreter::machine::run_entry(&ir, frontend::ENTRY, argv) {
        Ok(code) => ExitCode::from((code & 0xff) as u8),
        Err(diag) => {
            eprint!("{}", diag.render("", frontend::ENTRY));
            ExitCode::FAILURE
        }
    }
}

/// Lower, then emit a standalone C program for the module to stdout, compiled
/// for `target` (default: the host).
pub fn cmd_emit_c(path: &str, target: utilities::Target) -> ExitCode {
    start_progress(2);
    interpreter::machine::set_reflected_target(Some(target));
    let (lowered, _loaded, _plan) = match lower_all(path, true) {
        Ok(x) => x,
        Err(code) => return code,
    };
    // `emit-c` prints C to stdout; the caller drives the link, so `BUILD`
    // directives (which steer linking) have nothing to apply here beyond the
    // link comment the generated source already carries.
    utilities::progress::step("GENERATING C");
    let c = ccg::emit(&lowered, frontend::ENTRY, ccg::Entry::Main, target);
    utilities::progress::end();
    print!("{c}");
    ExitCode::SUCCESS
}

/// Lower, emit C for `target`, then compile and link it with the target's
/// toolchain (`cc` natively, `emcc` for wasm). Writes `<stem>.c` and the
/// executable into a `ares-out/` directory beside the source (kept out of the
/// source tree, gitignore-friendly); prints the path built.
///
/// `libc` is the `--libc=` choice; `None` takes the target's default, which on
/// Linux is musl when a sysroot ships with this compiler and nothing shared is
/// linked.
pub fn cmd_build(
    path: &str,
    target: utilities::Target,
    libc: Option<utilities::Libc>,
) -> ExitCode {
    start_progress(3);
    interpreter::machine::set_reflected_target(Some(target));
    let (lowered, _loaded, plan) = match lower_all(path, true) {
        Ok(x) => x,
        Err(code) => return code,
    };
    utilities::progress::step("GENERATING C");
    let emitted = ccg::emit_program(&lowered, frontend::ENTRY, ccg::Entry::Main, target);

    let src = Path::new(path);
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let dir = src.parent().unwrap_or_else(|| Path::new(".")).join("ares-out");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("ares: cannot create {}: {e}", dir.display());
        return ExitCode::FAILURE;
    }

    let mut libs: Vec<&str> = emitted.libraries.iter().map(String::as_str).collect();
    libs.extend(plan.libs.iter().map(String::as_str));
    let musl = match choose_libc(target, libc, &libs, emitted.uses_compiler) {
        Ok(musl) => musl,
        Err(msg) => {
            eprintln!("ares: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let tc = match &musl {
        Some(sysroot) => {
            let specs = dir.join("musl.specs");
            if let Err(e) = std::fs::write(&specs, utilities::musl_specs(sysroot)) {
                eprintln!("ares: cannot write {}: {e}", specs.display());
                return ExitCode::FAILURE;
            }
            utilities::musl_toolchain(sysroot, &specs)
        }
        None => utilities::toolchain(target),
    };
    if tc.cc.is_empty() {
        eprintln!("ares: {}", tc.hint);
        return ExitCode::FAILURE;
    }
    let compiler_lib = if emitted.uses_compiler {
        if target != utilities::Target::host() {
            eprintln!(
                "ares: this program runs the compiler (`@lex`, `@parse_str`, `@eval`, ...), \
                 and libares is only available for the host target"
            );
            return ExitCode::FAILURE;
        }
        match libares_dir() {
            Some(dir) => Some(dir),
            None => {
                eprintln!(
                    "ares: this program runs the compiler (`@lex`, `@parse_str`, `@eval`, ...), \
                     but the compiler library was not found beside the ares executable or in \
                     its lib/"
                );
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };

    let c_path = dir.join(format!("{stem}.c"));
    let out_path = dir.join(format!("{stem}{}", tc.exe_suffix));

    if let Err(e) = std::fs::write(&c_path, &emitted.source) {
        eprintln!("ares: cannot write {}: {e}", c_path.display());
        return ExitCode::FAILURE;
    }

    let mut cmd = std::process::Command::new(&tc.cc);
    cmd.args(&tc.cflags)
        .arg(&c_path)
        .arg("-o")
        .arg(&out_path);
    for lib in &emitted.libraries {
        if let Some(flag) = target.link_flag(lib) {
            cmd.arg(flag);
        }
        // A path-named SHARED `@extern` library (e.g. "bin/libraylib.so") is
        // found by the linker at build time, but the runtime loader would not
        // find its versioned soname later. Bake an rpath (a runtime search path
        // stored in the binary) pointing at the directory that actually holds
        // the soname (canonicalize resolves the symlink), so the built program
        // just runs. A static archive (`.a`) is baked in and needs no rpath.
        if tc.rpath && !lib.ends_with(".a") && (lib.contains('/') || lib.contains('.')) {
            if let Ok(real) = std::fs::canonicalize(lib) {
                if let Some(libdir) = real.parent() {
                    cmd.arg(format!("-Wl,-rpath,{}", libdir.display()));
                }
            }
        }
    }
    // Libraries and search paths declared in Ares via `$ @e <| @link ..` /
    // `@link_path`. Skip a lib already linked from an `@extern`.
    for lib in &plan.libs {
        if emitted.libraries.iter().any(|l| l == lib) {
            continue;
        }
        if let Some(flag) = target.link_flag(lib) {
            cmd.arg(flag);
        }
    }
    for dir in &plan.lib_paths {
        cmd.arg(format!("-L{dir}"));
        if tc.rpath {
            cmd.arg(format!("-Wl,-rpath,{dir}"));
        }
    }
    // The shared library, not `libares.a`: it carries its own dependencies
    // (libffi among them, which a static link would have to name and find).
    if let Some(dir) = &compiler_lib {
        cmd.arg(format!("-L{}", dir.display()));
        if tc.rpath {
            cmd.arg(format!("-Wl,-rpath,{}", dir.display()));
        }
        cmd.arg("-lares");
    }
    utilities::progress::step("COMPILING");
    // The compiler's own messages would land on the open progress line, so
    // they are held back and printed once it is ended.
    let held = utilities::progress::active();
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(if held { std::process::Stdio::piped() } else { std::process::Stdio::inherit() });
    let ran = cmd.output();
    utilities::progress::end();
    if let Ok(out) = &ran {
        use std::io::Write;
        let _ = std::io::stderr().write_all(&out.stderr);
    }
    match ran.map(|out| out.status) {
        Ok(status) if status.success() => {
            // Without an rpath the loader looks beside the executable.
            if let (Some(dir), false) = (&compiler_lib, tc.rpath) {
                let name = target.soname("ares");
                if let Err(e) = std::fs::copy(dir.join(&name), out_path.with_file_name(&name)) {
                    eprintln!("ares: cannot copy {name} beside {}: {e}", out_path.display());
                    return ExitCode::FAILURE;
                }
            }
            println!("built {}", out_path.display());
            ExitCode::SUCCESS
        }
        Ok(status) => {
            eprintln!("ares: {} failed ({status})", tc.cc);
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("ares: cannot run {} ({e}); {}", tc.cc, tc.hint);
            ExitCode::FAILURE
        }
    }
}

/// The directory holding the compiler library (`libares.so`, `libares.dylib`,
/// `ares.dll`): beside this executable (a cargo build,
/// `target/<profile>/`), or `<prefix>/lib` for `<prefix>/bin/ares` (the Nix
/// package).
/// Pick the C library for a build: `Some(sysroot)` links statically against
/// the musl there, `None` uses the target's own toolchain.
///
/// An explicit `--libc` is honoured or refused with the reason. Without one,
/// Linux defaults to musl, but only when a musl sysroot is installed with this
/// compiler and the program links nothing shared; otherwise the build quietly
/// uses glibc. A development compiler (`target/debug`) ships no sysroot, so it
/// always builds against the system libraries.
fn choose_libc(
    target: utilities::Target,
    libc: Option<utilities::Libc>,
    libs: &[&str],
    uses_compiler: bool,
) -> Result<Option<PathBuf>, String> {
    let static_ok = !uses_compiler && libs.iter().all(|l| links_statically(l));
    match libc {
        None => {
            let wants_musl = target.default_libc() == Some(utilities::Libc::Musl);
            if wants_musl && target == utilities::Target::host() && static_ok {
                Ok(musl_sysroot())
            } else {
                Ok(None)
            }
        }
        Some(_) if target.default_libc().is_none() => Err(format!(
            "`--libc` does not apply to {}, which has one C library",
            target.name()
        )),
        Some(utilities::Libc::Gnu) => Ok(None),
        Some(utilities::Libc::Musl) => {
            if target != utilities::Target::host() {
                return Err(format!(
                    "the musl sysroot is built for the host ({}), not {}",
                    utilities::Target::host().name(),
                    target.name()
                ));
            }
            if uses_compiler {
                return Err("this program runs the compiler (`@lex`, `@parse_str`, \
                     `@eval`, ...), which links the shared libares, so it cannot \
                     be linked statically against musl; use --libc=gnu"
                    .to_string());
            }
            if let Some(lib) = libs.iter().find(|l| !links_statically(l)) {
                return Err(format!(
                    "`{lib}` is linked as a shared library, which a static musl \
                     build cannot do; point the `@extern` at its `.a` archive, \
                     or use --libc=gnu"
                ));
            }
            match musl_sysroot() {
                Some(sysroot) => Ok(Some(sysroot)),
                None => Err("no musl sysroot found: set ARES_MUSL to one (a prefix \
                     holding include/ and lib/libc.a), or use a compiler built by \
                     `thrax build release`, which ships one in its lib/musl"
                    .to_string()),
            }
        }
    }
}

/// Whether a library a program links can go into a static musl executable:
/// the C library's own parts, which musl provides, or a `.a` archive.
fn links_statically(lib: &str) -> bool {
    matches!(lib, "" | "libc" | "c" | "libm" | "m" | "pthread" | "libpthread")
        || lib.ends_with(".a")
}

/// The musl sysroot to link against: `$ARES_MUSL`, else the `lib/musl` a
/// release compiler ships beside its `bin/`.
fn musl_sysroot() -> Option<PathBuf> {
    let is_sysroot = |d: &Path| d.join("lib/libc.a").exists() && d.join("include").is_dir();
    if let Some(dir) = std::env::var_os("ARES_MUSL") {
        let dir = PathBuf::from(dir);
        return is_sysroot(&dir).then_some(dir);
    }
    let exe = exe_path()?;
    let dir = exe.parent()?.parent()?.join("lib/musl");
    is_sysroot(&dir).then_some(dir)
}

/// This executable's path with symlinks resolved (a Nix profile links it). On
/// Windows the path is already final, and canonicalizing would give it the
/// `\\?\` verbatim prefix, which a C compiler's `-L` does not understand.
fn exe_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    if utilities::Target::host().os == utilities::Os::Windows {
        return Some(exe);
    }
    exe.canonicalize().ok()
}

fn libares_dir() -> Option<PathBuf> {
    let exe = exe_path()?;
    let bin = exe.parent()?;
    let mut dirs = vec![bin.to_path_buf()];
    if let Some(prefix) = bin.parent() {
        dirs.push(prefix.join("lib"));
    }
    let name = utilities::Target::host().soname("ares");
    dirs.into_iter().find(|d| d.join(&name).exists())
}

/// Print the source as the checker finally saw it: every `@e` spliced
/// and every `@build`'s items appended. This is the only view of what a
/// metaprogram actually produced, so it is how a generator is debugged.
///
/// `only` restricts the output to one module (by `@mod` name); without it every
/// module of the program is printed, root first, each under a `# ==== NAME` banner.
/// The standard library is skipped unless asked for by name, since a program's own
/// modules are the interesting ones.
pub fn cmd_expand(path: &str, only: Option<&str>) -> ExitCode {
    start_progress(1);
    let (_lowered, loaded, _plan) = match lower_all(path, false) {
        Ok(x) => x,
        Err(code) => return code,
    };
    utilities::progress::end();
    if let Some(name) = only {
        match loaded.index.get(name) {
            Some(&i) => {
                print!("{}", loaded.sources[i].2);
                return ExitCode::SUCCESS;
            }
            None => {
                eprintln!("ares: no module `{name}` in this program");
                return ExitCode::FAILURE;
            }
        }
    }
    for (name, src_path, src) in &loaded.sources {
        // A stdlib module is read from the distribution or embedded, never written
        // by the author of the program being inspected.
        if is_stdlib_module(name) {
            continue;
        }
        println!("# ==== {name}  ({src_path})");
        print!("{src}");
        println!();
    }
    ExitCode::SUCCESS
}

/// Whether `name` is a standard-library module (so `expand` leaves it out by
/// default). `CORE` and `C` are seeded into every program, the rest arrive by
/// import; all of them ship with the compiler.
fn is_stdlib_module(name: &str) -> bool {
    stdlib::source(name).is_some()
}

pub fn cmd_check(path: &str) -> ExitCode {
    start_progress(1);
    // Expanding first means the printed types are the ones the program really has
    // (a `@e`-injected definition included), and that every `$ @e` directive has
    // run: a module's compile-time assertions are checked by checking it.
    let (_lowered, loaded, _plan) = match lower_all(path, false) {
        Ok(x) => x,
        Err(code) => return code,
    };

    let run = match check_all(&loaded, false, None) {
        Ok(run) => run,
        Err(rendered) => {
            eprint!("{rendered}");
            return ExitCode::FAILURE;
        }
    };

    utilities::progress::end();
    let root = loaded.index[&loaded.root_name];
    let checker = run.checker(root);
    for (name, ty) in &run.results[root] {
        println!("{name} : {}", checker.show_decl(*name, *ty));
    }
    ExitCode::SUCCESS
}

/// Postorder DFS over the dependency graph: a vertex appears after all vertices
/// it points at, so dependencies are checked before their importers.
pub(crate) fn topological_order(graph: &[Vec<usize>]) -> Vec<usize> {
    fn visit(v: usize, graph: &[Vec<usize>], seen: &mut [bool], order: &mut Vec<usize>) {
        if seen[v] {
            return;
        }
        seen[v] = true;
        for &w in &graph[v] {
            visit(w, graph, seen, order);
        }
        order.push(v);
    }
    let mut seen = vec![false; graph.len()];
    let mut order = Vec::with_capacity(graph.len());
    for v in 0..graph.len() {
        visit(v, graph, &mut seen, &mut order);
    }
    order
}

/// Extra module directories, from `--import-dir=DIR`, searched ahead of
/// everything else. Set once from `main` before any command runs.
static IMPORT_DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();

pub fn set_import_dirs(dirs: Vec<PathBuf>) {
    let _ = IMPORT_DIRS.set(dirs);
}

fn import_dirs() -> &'static [PathBuf] {
    IMPORT_DIRS.get().map(Vec::as_slice).unwrap_or_default()
}

/// Find the source file for a module: the `--import-dir` directories, then the
/// project's `@import_path` ones, then the program's own neighbourhood (its `library/`, the `examples/` the combined test
/// runner imports, a sibling file), then the standard libraries [`stdlib_dirs`]
/// found. First match wins, so a module a program ships beside itself shadows the
/// standard-library module of that name.
pub(crate) fn resolve_module_file(name: &str, root_dir: &Path) -> Option<PathBuf> {
    let file = format!("{name}.ares");
    let mut candidates: Vec<PathBuf> = import_dirs().iter().map(|d| d.join(&file)).collect();
    candidates.extend(project(root_dir).unwrap_or_default().paths.iter().map(|d| d.join(&file)));
    candidates.push(root_dir.join("library").join(&file));
    candidates.push(root_dir.join("examples").join(&file));
    if let Some(parent) = root_dir.parent() {
        candidates.push(parent.join("examples").join(&file));
    }
    candidates.push(root_dir.join(&file));
    candidates.extend(stdlib_dirs(root_dir).iter().map(|d| d.join(&file)));
    candidates.into_iter().find(|p| p.exists())
}

/// The module that configures a project, found as `BUILD.ares` in the root file's
/// directory or the nearest one above it.
const BUILD_SCRIPT: &str = "BUILD";

/// What a project's `BUILD.ares` configures: the module directories it adds and
/// the options it sets over the defaults.
#[derive(Clone, Default)]
struct Built {
    paths: Vec<PathBuf>,
    config: utilities::config::Config,
}

/// A `BUILD.ares` that has run, with the modification time it ran at and what it
/// configured (`None` when it failed).
struct Project {
    file: PathBuf,
    stamp: Option<std::time::SystemTime>,
    built: Option<Built>,
}

static PROJECTS: std::sync::Mutex<Vec<Project>> = std::sync::Mutex::new(Vec::new());

thread_local! {
    /// Set while a `BUILD.ares` compiles: its own imports never see its paths.
    static IN_BUILD_SCRIPT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// What the last `BUILD.ares` `@build` recorded.
    static BUILT: std::cell::RefCell<Option<interpreter::machine::BuildRecord>> = const { std::cell::RefCell::new(None) };
}

fn in_build_script<T>(f: impl FnOnce() -> T) -> T {
    let outer = IN_BUILD_SCRIPT.with(|c| c.replace(true));
    let out = f();
    IN_BUILD_SCRIPT.with(|c| c.set(outer));
    out
}

/// The nearest `BUILD.ares` at or above `root_dir`.
fn find_build_script(root_dir: &Path) -> Option<PathBuf> {
    let dir = if root_dir.as_os_str().is_empty() { std::env::current_dir().ok()? } else { root_dir.to_path_buf() };
    let dir = dir.canonicalize().ok()?;
    dir.ancestors().map(|d| d.join(format!("{BUILD_SCRIPT}.ares"))).find(|f| f.is_file())
}

/// What the project around `root_dir` configures, running its `BUILD.ares` when it
/// has not run since it last changed. `None` when it fails; the failure has been
/// reported.
fn project(root_dir: &Path) -> Option<Built> {
    if IN_BUILD_SCRIPT.with(|c| c.get()) {
        return Some(Built::default());
    }
    let Some(file) = find_build_script(root_dir) else {
        return Some(Built::default());
    };
    let stamp = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
    {
        let projects = PROJECTS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = projects.iter().find(|p| p.file == file && p.stamp == stamp) {
            return p.built.clone();
        }
    }
    // The lock is released: the run resolves modules, which comes back here.
    let built = run_build_script(&file);
    let mut projects = PROJECTS.lock().unwrap_or_else(|e| e.into_inner());
    projects.retain(|p| p.file != file);
    projects.push(Project { file, stamp, built: built.clone() });
    built
}

/// The options in force for a file in `dir`: what its project's `BUILD.ares` sets,
/// or the defaults when there is none or it fails.
pub fn project_options(dir: &Path) -> utilities::config::Config {
    project(dir).map(|b| b.config).unwrap_or_default()
}

/// Compile `file` and run its `@build`, returning what it configured. Its
/// `@import_path` directories are relative to `file`'s directory.
fn run_build_script(file: &Path) -> Option<Built> {
    let path = file.display().to_string();
    let src = match std::fs::read_to_string(file) {
        Ok(src) => src,
        Err(e) => {
            eprintln!("ares: cannot read {path}: {e}");
            return None;
        }
    };
    if parse_mod_name(&src).as_deref() != Some(BUILD_SCRIPT) {
        let d = utilities::Diagnostic::error(
            utilities::Code::BuildScript,
            utilities::Span::at(0),
            0,
            format!("`BUILD.ares` does not declare `$ @mod {BUILD_SCRIPT}`"),
        )
        .with_note("a `BUILD.ares` configures every program in the directories below it");
        eprint!("{}", d.render(&src, &path));
        return None;
    }
    BUILT.with(|c| c.borrow_mut().take());
    in_build_script(|| expand_all(&path, false)).ok()?;
    let record = BUILT.with(|c| c.take()).unwrap_or_default();
    let dir = file.parent().unwrap_or_else(|| Path::new("."));
    let mut config = utilities::config::Config::default();
    for o in record.options {
        config.set(o.section, o.name, o.value).expect("`@option` validated it");
    }
    Some(Built { paths: record.import_paths.into_iter().map(|p| dir.join(p)).collect(), config })
}

/// The standard libraries on disk, in search order: one beside the program's own
/// tree (a checkout's `library/`), then the ones this executable ships with. A
/// directory qualifies only when it holds `CORE.ares`, so an unrelated `library/`
/// above a program is never mistaken for the standard library.
fn stdlib_dirs(root_dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(parent) = root_dir.parent() {
        dirs.push(parent.join("library"));
    }
    dirs.extend(stdlib::distribution_dirs());
    dirs.retain(|dir| dir.join("CORE.ares").exists());
    dirs
}

/// A module's diagnostic path and its source: the on-disk search first, then the
/// standard library embedded in this binary.
fn load_module(name: &str, root_dir: &Path) -> Result<(String, String), String> {
    if let Some(file) = resolve_module_file(name, root_dir) {
        return match std::fs::read_to_string(&file) {
            Ok(src) => Ok((file.display().to_string(), src)),
            Err(e) => Err(format!(
                "ares: cannot read module `{name}` ({}): {e}",
                file.display()
            )),
        };
    }
    match stdlib::source(name) {
        Some(src) => Ok((stdlib::path(name), src.to_string())),
        None => Err(format!("ares: cannot find module `{name}`")),
    }
}

/// The `@mod` name declared by a source, by parsing it in a scratch arena.
pub(crate) fn parse_mod_name(src: &str) -> Option<String> {
    let (ast, program, _) = frontend::parse_recovering(frontend::Ast::new(), src);
    program.ok().map(|p| ast.text(p.module).to_string())
}

/// The module names a source imports (`$ @import MOD`) with the span of each, or
/// empty if even its header does not parse (the errors are reported later, against the
/// shared arena).
pub(crate) fn parse_imports(src: &str) -> Vec<(String, utilities::Span)> {
    thread_local! {
        /// Imports by source text: every compile loads the same library, and
        /// finding a module's imports means parsing all of it.
        static SEEN: std::cell::RefCell<HashMap<String, Vec<(String, utilities::Span)>>> =
            std::cell::RefCell::new(HashMap::new());
    }
    if let Some(found) = SEEN.with(|s| s.borrow().get(src).cloned()) {
        return found;
    }
    // Recovering, so a broken item elsewhere in the file does not hide its
    // imports (every use of an imported name would then be reported unbound).
    let (ast, program, _) = frontend::parse_recovering(frontend::Ast::new(), src);
    let found: Vec<(String, utilities::Span)> = match program {
        Ok(program) => ast
            .slice(program.items)
            .iter()
            .filter_map(|item| match item {
                Item::Import { module, span, .. } => Some((dotted(&ast, *module), *span)),
                _ => None,
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    SEEN.with(|s| {
        let mut seen = s.borrow_mut();
        if seen.len() >= 512 {
            seen.clear();
        }
        seen.insert(src.to_string(), found.clone());
    });
    found
}

/// A source's `$ @import` items, re-rendered as source lines.
fn import_lines(src: &str) -> Vec<String> {
    let Ok(parsed) = frontend::parse(src) else {
        return Vec::new();
    };
    let ast = &parsed.ast;
    ast.slice(parsed.program.items)
        .iter()
        .filter_map(|item| match item {
            Item::Import { module, alias: None, .. } => Some(format!("$ @import {}", dotted(ast, *module))),
            Item::Import { module, alias: Some(alias), .. } => {
                Some(format!("$ @import {} = {}", ast.text(*alias), dotted(ast, *module)))
            }
            _ => None,
        })
        .collect()
}

pub(crate) fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("<root>")
        .to_string()
}

pub fn cmd_parse(path: &str) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ares: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    match frontend::parse(&source) {
        Ok(parsed) => {
            println!(
                "module {} ({} items)",
                parsed.ast.text(parsed.program.module),
                parsed.program.items.len()
            );
            for item in parsed.ast.slice(parsed.program.items) {
                println!("  {item:?}");
            }
            ExitCode::SUCCESS
        }
        Err(diag) => {
            eprint!("{}", diag.render(&source, path));
            ExitCode::FAILURE
        }
    }
}

pub fn cmd_lex(path: &str) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ares: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    match Lexer::tokenize(&source) {
        Ok(tokens) => {
            for tok in &tokens {
                let text = &source[tok.span.start..tok.span.end];
                println!("{:>4}  {:?}  {:?}", tok.line, tok.kind, text);
            }
            ExitCode::SUCCESS
        }
        Err(diag) => {
            eprint!("{}", diag.render(&source, path));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::choose_libc;
    use utilities::{Libc, Os, Target};

    // The libc choice is a Linux policy, checked against the host because the
    // musl sysroot is.
    #[test]
    fn a_shared_library_keeps_the_default_build_on_glibc() {
        if Target::host().os != Os::Linux {
            return;
        }
        let linux = Target::host();
        assert_eq!(choose_libc(linux, None, &["raylib"], false), Ok(None));
        assert_eq!(choose_libc(linux, None, &["m", "pthread"], true), Ok(None));
        assert_eq!(choose_libc(linux, Some(Libc::Gnu), &["m"], false), Ok(None));
    }

    #[test]
    fn explicit_musl_refuses_what_it_cannot_link() {
        if Target::host().os != Os::Linux {
            return;
        }
        let host = Target::host();
        let shared = choose_libc(host, Some(Libc::Musl), &["m", "raylib"], false);
        assert!(shared.unwrap_err().contains("`raylib`"));
        let compiler = choose_libc(host, Some(Libc::Musl), &[], true);
        assert!(compiler.unwrap_err().contains("libares"));
        let wasm = Target::parse("wasm32-wasi").unwrap();
        assert!(choose_libc(wasm, Some(Libc::Gnu), &[], false).is_err());
    }

    #[test]
    fn link_directives_are_collected_in_order_and_drained() {
        use interpreter::machine::{push_link, take_link_directives};
        let _ = take_link_directives(); // clear any stale state
        push_link(false, "curl".to_string());
        push_link(true, "vendor".to_string());
        assert_eq!(
            take_link_directives(),
            vec![(false, "curl".to_string()), (true, "vendor".to_string())]
        );
        assert!(take_link_directives().is_empty(), "draining empties the queue");
    }
}

#[cfg(test)]
mod sink_tests {
    use super::*;

    #[test]
    fn positions_map_back_through_splices() {
        // "ab@e(x)cd" -> the 5-byte site at 2 became 1 byte: "ab1cd".
        let mut round = HashMap::new();
        round.insert("M".to_string(), vec![(2, 5, 1)]);
        let rounds = vec![round];
        assert_eq!(map_back(&rounds, "M", 0), 0);
        assert_eq!(map_back(&rounds, "M", 2), 2);
        assert_eq!(map_back(&rounds, "M", 3), 7);
        assert_eq!(map_back(&rounds, "M", 4), 8);
        assert_eq!(map_back(&rounds, "N", 4), 4);
    }
}
