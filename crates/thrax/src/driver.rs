//! The Thrax driver (`DR`): module loading, dependency ordering, type-checking,
//! lowering, and the `lex`/`parse`/`check`/`run` subcommands.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use frontend::Lexer;
use frontend::{Item, Program};

/// A module's name and source text, plus the name-to-slot index and the root
/// module name. Shared by `check` and `run`.
struct Loaded {
    /// One entry per module: `(module name, source path, source text)`. The path
    /// is what diagnostics render, so it stays a real, clickable file location.
    sources: Vec<(String, String, String)>,
    index: HashMap<String, usize>,
    root_name: String,
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
            eprintln!("thrax: cannot read {path}: {e}");
            return Err(ExitCode::FAILURE);
        }
    };
    let root_name = parse_mod_name(&root_src).unwrap_or_else(|| file_stem(path));

    load_core(root_name, path.to_string(), root_src, &root_dir).map_err(|msg| {
        eprintln!("{msg}");
        ExitCode::FAILURE
    })
}

/// Transitively load the standard-library modules a given root module imports,
/// seeding the always-present `CORE` and auto-injected `C`. The root's name,
/// diagnostic path, and source are supplied by the caller, so this serves both
/// an on-disk file (`load_sources`) and an in-memory REPL session. `root_dir` is
/// where imported modules are resolved from. Returns a plain message on error.
fn load_core(
    root_name: String,
    root_path: String,
    root_src: String,
    root_dir: &Path,
) -> Result<Loaded, String> {
    let mut sources: Vec<(String, String, String)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut queue: Vec<(String, String, String)> =
        vec![(root_name.clone(), root_path, root_src)];

    // The implicitly imported CORE module (bare names everywhere) is an ordinary
    // standard-library file, loaded from disk like the rest. Seed it into the load
    // queue so it is always present, even without an explicit `$ with CORE`.
    if root_name != "CORE" {
        match resolve_module_file("CORE", root_dir) {
            Some(file) => match std::fs::read_to_string(&file) {
                Ok(s) => queue.push(("CORE".to_string(), file.display().to_string(), s)),
                Err(e) => {
                    return Err(format!(
                        "thrax: cannot read the CORE module ({}): {e}",
                        file.display()
                    ))
                }
            },
            None => return Err("thrax: cannot find the CORE standard-library module".to_string()),
        }
    }
    while let Some((name, src_path, src)) = queue.pop() {
        if index.contains_key(&name) {
            continue;
        }
        let imports = parse_imports(&src);
        index.insert(name.clone(), sources.len());
        sources.push((name, src_path, src));
        for imp in imports {
            if index.contains_key(&imp) || queue.iter().any(|(n, _, _)| *n == imp) {
                continue;
            }
            match resolve_module_file(&imp, root_dir) {
                Some(file) => match std::fs::read_to_string(&file) {
                    Ok(s) => queue.push((imp, file.display().to_string(), s)),
                    Err(e) => {
                        return Err(format!(
                            "thrax: cannot read module `{imp}` ({}): {e}",
                            file.display()
                        ))
                    }
                },
                None => {
                    return Err(format!(
                        "thrax: cannot find module `{imp}` imported by the program"
                    ))
                }
            }
        }
    }
    // Auto-inject the `C` namespace (libc + libm as `@extern` bindings),
    // reachable qualified (`C.sqrt`) with no import, like the prelude.
    if !index.contains_key("C") {
        index.insert("C".to_string(), sources.len());
        sources.push((
            "C".to_string(),
            "library/C.thx".to_string(),
            C_SOURCE.to_string(),
        ));
    }

    Ok(Loaded {
        sources,
        index,
        root_name,
    })
}

/// The auto-injected `C` standard-library namespace (see library/C.thx).
const C_SOURCE: &str = include_str!("../../../library/C.thx");

/// The dependency graph over parsed modules (edges point at imports).
fn import_graph(
    ast: &frontend::Ast,
    programs: &[Program],
    index: &HashMap<String, usize>,
) -> Vec<Vec<usize>> {
    let mut graph = vec![Vec::new(); programs.len()];
    for (i, program) in programs.iter().enumerate() {
        for item in ast.slice(program.items) {
            if let Item::Import { module, .. } = item {
                let name = ast.slice(*module)
                    .iter()
                    .map(|&part| ast.text(part))
                    .collect::<Vec<_>>()
                    .join(".");
                if let Some(&j) = index.get(&name) {
                    graph[i].push(j);
                }
            }
        }
    }
    graph
}

/// Type-check every module in dependency order, returning the per-module
/// checkers (or the first error, rendered).
type CheckOut<'a> = (
    Vec<frontend::Checker<'a>>,
    Vec<Vec<(&'a str, frontend::Type)>>,
);

fn check_all<'a>(
    ast: &'a frontend::Ast,
    programs: &[Program],
    graph: &[Vec<usize>],
    sources: &[(String, String, String)],
) -> Result<CheckOut<'a>, String> {
    let mut checkers: Vec<Option<frontend::Checker>> = (0..programs.len()).map(|_| None).collect();
    let mut results: Vec<Vec<(&str, frontend::Type)>> = vec![Vec::new(); programs.len()];

    // The auto-injected `C` namespace and the implicitly imported `CORE` module
    // have no dependencies and are checked first: `C` made available qualified-only
    // (`C.sqrt`), `CORE` bare (its `to_string` overloads, etc.). `CORE` is checked
    // after `C` but imports neither, so ordering the two is unconstrained.
    let c_idx = sources.iter().position(|(n, _, _)| n == "C");
    let core_idx = sources.iter().position(|(n, _, _)| n == "CORE");
    let mut order: Vec<usize> = topological_order(graph);
    for &pre in [core_idx, c_idx].iter().flatten() {
        order.retain(|&i| i != pre);
        order.insert(0, pre);
    }
    for i in order {
        let mut checker = frontend::Checker::new(ast);
        if let Some(c) = c_idx {
            if c != i && Some(i) != core_idx {
                checker.import_qualified(checkers[c].as_ref().expect("C checked first"));
            }
        }
        if let Some(core) = core_idx {
            if core != i && Some(i) != c_idx {
                checker.import_from(checkers[core].as_ref().expect("CORE checked first"));
            }
        }
        for &dep in &graph[i] {
            let dep_checker = checkers[dep].as_ref().expect("dependency checked first");
            checker.import_from(dep_checker);
        }
        match checker.check_program(&programs[i]) {
            Ok(defs) => {
                results[i] = defs;
                checkers[i] = Some(checker);
            }
            Err(diag) => {
                let (_name, src_path, src) = &sources[i];
                return Err(diag.render(src, src_path));
            }
        }
    }
    Ok((
        checkers
            .into_iter()
            .map(|c| c.expect("all checked"))
            .collect(),
        results,
    ))
}

/// Gather the checkers' resolutions that lowering needs (`[..]` array/tensor
/// nodes, resolved bare calls, overload keys, literal/pattern hooks, extern
/// specs, C-repr layouts, ...) into one [`frontend::Resolved`].
fn collect_resolved(checkers: &[frontend::Checker]) -> frontend::Resolved {
    let mut resolved = frontend::Resolved::default();
    for checker in checkers {
        let (exprs, pats) = checker.array_nodes();
        resolved.array_exprs.extend(exprs.iter().copied());
        resolved.array_pats.extend(pats.iter().copied());
        resolved.tensor_exprs.extend(checker.tensor_nodes().iter().copied());
        for (&site, names) in checker.promotions() { resolved.promotions.insert(site, names.clone()); }
        for (&site, n) in checker.struct_lit_names() { resolved.struct_lit_names.insert(site, n.clone()); }
        for (&site, (m, n)) in checker.literal_hooks() { resolved.literal_hooks.insert(site, (m.map(str::to_string), n.clone())); }
        for (&site, ((bm, bn), (em, en))) in checker.literal_pattern_hooks() { resolved.literal_pattern_hooks.insert(site, ((bm.map(str::to_string), bn.clone()), (em.map(str::to_string), en.clone()))); }
        for (&site, (m, n)) in checker.sequence_pattern_hooks() { resolved.sequence_pattern_hooks.insert(site, (m.map(str::to_string), n.clone())); }
        let (clits, obs) = checker.codata_sites(); resolved.codata_lits.extend(clits.iter().copied()); resolved.observations.extend(obs.iter().copied());
        for (&site, &module) in checker.call_modules() {
            resolved.call_modules.insert(site, module.to_string());
        }
        for (&site, key) in checker.overload_calls() {
            resolved.overload_calls.insert(site, key.clone());
        }
        for (&body, key) in checker.def_keys() {
            resolved.def_keys.insert(body, key.clone());
        }
        for (&site, args) in checker.implicit_calls() {
            resolved.implicit_args.insert(site, args.clone());
        }
        for (&site, fields) in checker.with_fields() {
            resolved.with_fields.insert(site, fields.clone());
        }
        resolved.extern_sigs.extend(checker.extern_sigs());
        let module = checker.module_name().to_string();
        for (name, spec) in checker.own_externs() {
            resolved
                .externs
                .insert((module.clone(), name.to_string()), spec.clone());
        }
        for (name, layout) in checker.crepr_layouts() {
            resolved
                .crepr_layouts
                .insert(name.to_string(), layout.clone());
        }
    }
    resolved
}

/// The full pipeline up to (but not including) execution: load, parse, check,
/// and lower every module. Returns the lowered modules (root first) and the
/// root's entry-point name (`test`, else `main`). Shared by `run` and `emit-c`.
/// Parse, check, and lower the loaded modules once, returning the lowered
/// programs and the entry point. The parse/check state is local (self-borrowing),
/// so only the owned `lowered` escapes; the expansion loop in [`lower_all`] calls
/// this repeatedly on progressively `@e`-expanded sources.
fn compile_sources(
    loaded: &Loaded,
) -> Result<(Vec<frontend::lowering::data::Program>, String, frontend::EntryKind), ExitCode> {
    let mut ast = frontend::Ast::new();
    let mut programs: Vec<Program> = Vec::with_capacity(loaded.sources.len());
    for (_name, src_path, src) in &loaded.sources {
        match frontend::parse_into(ast, src) {
            Ok((next_ast, p)) => {
                ast = next_ast;
                programs.push(p);
            }
            Err(diag) => {
                eprint!("{}", diag.render(src, src_path));
                return Err(ExitCode::FAILURE);
            }
        }
    }

    let graph = import_graph(&ast, &programs, &loaded.index);
    let (checkers, results) = check_all(&ast, &programs, &graph, &loaded.sources)
        .map_err(|rendered| {
            eprint!("{rendered}");
            ExitCode::FAILURE
        })?;

    let resolved = collect_resolved(&checkers);

    // Lower every module; put the root first so its names win when resolving an
    // unqualified reference defined in more than one module.
    let decls = frontend::Decls::collect(&ast, &programs);
    let root = loaded.index[&loaded.root_name];
    let mut order: Vec<usize> = (0..programs.len()).collect();
    order.sort_by_key(|&i| i != root);
    let lowered: Vec<frontend::lowering::data::Program> = order
        .iter()
        .map(|&i| frontend::lower_program(&ast, &programs[i], &decls, &resolved))
        .collect();

    let entry = ["test", "main"]
        .into_iter()
        .find(|name| lowered[0].globals.iter().any(|(n, _)| n == name));
    let Some(e) = entry else {
        eprintln!(
            "thrax: module `{}` has no `test` or `main` to run",
            loaded.root_name
        );
        return Err(ExitCode::FAILURE);
    };
    // The entry's type decides how it is invoked: a value is forced, `{} -> Int`
    // is applied to unit, `[n]Str -> Int` to the argument vector (C-style `main`).
    let kind = results[root]
        .iter()
        .find(|(n, _)| *n == e)
        .map(|(_, ty)| frontend::classify_entry(ty))
        .unwrap_or(frontend::EntryKind::Value);
    if kind == frontend::EntryKind::BadFn {
        eprintln!(
            "thrax: `{e}` must be a value, `{{}} -> Int`, or `[n]Str -> Int` (a C-style main)"
        );
        return Err(ExitCode::FAILURE);
    }
    Ok((lowered, e.to_string(), kind))
}

/// Render a forced expression-position `@e X` value back into Thrax source to
/// splice at the site. A `@code` result contributes the fragment it holds; a
/// scalar contributes its literal. Aggregates are not renderable yet.
fn render_meta_value(v: &interpreter::machine::data::PVal) -> std::result::Result<String, String> {
    use interpreter::machine::data::Value;
    // A `@code` fragment: splice its source text.
    if let Value::Struct { name, fields } = &*v.borrow() {
        if name == "@code" {
            let src = fields
                .iter()
                .find(|(k, _)| k == "src")
                .and_then(|(_, s)| match &*s.borrow() {
                    Value::Str(b) => Some(String::from_utf8_lossy(b).into_owned()),
                    _ => None,
                })
                .ok_or("malformed @code value")?;
            return Ok(src);
        }
    }
    // Otherwise a first-order value: render its Thrax literal.
    match interpreter::machine::reify(v)? {
        interpreter::machine::OwnedValue::Int(n) => Ok(n.to_string()),
        interpreter::machine::OwnedValue::Real(r) => Ok(format!("{r:?}")),
        interpreter::machine::OwnedValue::Bool(b) => {
            Ok(if b { "@true" } else { "@false" }.to_string())
        }
        interpreter::machine::OwnedValue::Unit => Ok("{}".to_string()),
        interpreter::machine::OwnedValue::Str(b) => Ok(thrax_str_literal(&b)),
        _ => Err(
            "an expression-position `@e` must currently produce `@code` or a scalar \
             (@int/@float64/@str/@bool/{}); aggregates are not rendered yet"
                .to_string(),
        ),
    }
}

/// Render bytes as a Thrax string literal that lexes back to those bytes: quote
/// it and escape `\`, `"`, and a `?(` interpolation opener.
fn thrax_str_literal(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut out = String::from("\"");
    let cs: Vec<char> = s.chars().collect();
    for (i, &c) in cs.iter().enumerate() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            // `?(` opens an interpolation; escape the `?` so it stays literal.
            '?' if cs.get(i + 1) == Some(&'(') => out.push_str("\\?"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
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
    diag.fill_span(span).render(src, path)
}

/// The sources that back a compiled program (`(module, path, text)`), returned by
/// [`lower_all`] so later passes can render `@e` diagnostics at real locations.
type Sources = Vec<(String, String, String)>;

/// Compile `path`, iteratively expanding expression-position `@e X`: each round
/// compiles the current sources, forces every `@e` site, renders the result to
/// source, and substitutes it in place; it repeats until no `@e` sites remain
/// (so `@e` that generates more `@e` keeps expanding). Top-level `$ @e`
/// directives are left for [`compile_and_run_ct`]. Returns the final sources too.
fn lower_all(
    path: &str,
) -> Result<
    (Vec<frontend::lowering::data::Program>, String, frontend::EntryKind, Sources, BuildPlan),
    ExitCode,
> {
    let mut loaded = load_sources(path)?;
    let root_dir = Path::new(path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut plan = BuildPlan::default();
    loop {
        let compiled = compile_sources(&loaded)?;
        // Gather every `@e` site this round: expression-position (`ct_evals`,
        // embed the value) and item-position (`ct_runs`, `$ @e X`: inject `@code`,
        // apply a `BUILD` directive, or discard).
        let expr_sites: Vec<(String, String, utilities::Span)> = compiled
            .0
            .iter()
            .flat_map(|p| {
                p.ct_evals
                    .iter()
                    .map(move |(name, span)| (p.module.clone(), format!("{}.{}", p.module, name), *span))
            })
            .collect();
        let item_sites: Vec<(String, String, utilities::Span)> = compiled
            .0
            .iter()
            .flat_map(|p| {
                p.ct_runs
                    .iter()
                    .map(move |(name, span)| (p.module.clone(), format!("{}.{}", p.module, name), *span))
            })
            .collect();
        if expr_sites.is_empty() && item_sites.is_empty() {
            return Ok((compiled.0, compiled.1, compiled.2, loaded.sources, plan));
        }
        let ir = frontend::ir::lower_modules(&compiled.0);
        let rd = root_dir.clone();
        interpreter::machine::set_meta_eval(Some(Box::new(move |src| meta_eval_source(src, &rd))));
        let mut edits: Vec<(String, utilities::Span, String)> = Vec::new();
        let fail = |diag, module: &str, span| -> ExitCode {
            interpreter::machine::set_meta_eval(None);
            eprintln!("thrax: a compile-time `@e` failed:");
            eprint!("{}", render_e_fault(diag, module, span, &loaded.sources));
            ExitCode::FAILURE
        };
        // Expression-position: replace the `@e X` with its value's source
        // (parenthesized to keep precedence).
        for (module, qualified, span) in expr_sites {
            let v = interpreter::machine::eval_value(&ir, &qualified)
                .map_err(|d| fail(d, &module, span))?;
            match render_meta_value(&v) {
                Ok(text) => edits.push((module, span, format!("({text})"))),
                Err(msg) => {
                    interpreter::machine::set_meta_eval(None);
                    eprintln!("thrax: a compile-time `@e` failed: {msg}");
                    return Err(ExitCode::FAILURE);
                }
            }
        }
        // Item-position `$ @e X`: an `@code` result injects its item(s) in place;
        // otherwise apply a `BUILD` directive (a no-op for any other value) and
        // drop the directive. Either way the whole `$ @e X` item is replaced.
        for (module, qualified, span) in item_sites {
            let v = interpreter::machine::eval_value(&ir, &qualified)
                .map_err(|d| fail(d, &module, span))?;
            // `span` is the whole `$ @e X` directive, so replacing it drops the
            // directive: an `@code` result becomes its item(s), anything else
            // (a `BUILD` directive, or a discarded value) becomes nothing.
            match as_code_src(&v) {
                Some(items) => edits.push((module, span, items)),
                None => {
                    build_directive(&v, &mut plan);
                    edits.push((module, span, String::new()));
                }
            }
        }
        interpreter::machine::set_meta_eval(None);
        splice_sources(&mut loaded.sources, edits);
    }
}

/// If `v` is a `@code` fragment, its source text.
fn as_code_src(v: &interpreter::machine::data::PVal) -> Option<String> {
    use interpreter::machine::data::Value;
    if let Value::Struct { name, fields } = &*v.borrow() {
        if name == "@code" {
            return fields.iter().find(|(k, _)| k == "src").and_then(|(_, s)| {
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
pub(crate) struct Session {
    pub lowered: Vec<frontend::lowering::data::Program>,
    pub decls: Vec<(String, String)>,
}

/// Compile an in-memory `@mod REPL` session `source` against the standard
/// library (CORE + C + any `$ with` imports resolved from `root_dir`). Returns
/// the lowered modules and the root's typed bindings, or a rendered diagnostic.
/// Unlike the file commands this renders errors into a string instead of exiting,
/// so the shell can print them and keep going.
pub(crate) fn compile_session(source: &str, root_dir: &Path) -> Result<Session, String> {
    let loaded = load_core(
        "REPL".to_string(),
        "<repl>".to_string(),
        source.to_string(),
        root_dir,
    )?;

    let mut ast = frontend::Ast::new();
    let mut programs: Vec<Program> = Vec::with_capacity(loaded.sources.len());
    for (_name, src_path, src) in &loaded.sources {
        match frontend::parse_into(ast, src) {
            Ok((next_ast, p)) => {
                ast = next_ast;
                programs.push(p);
            }
            Err(diag) => return Err(diag.render(src, src_path)),
        }
    }

    let graph = import_graph(&ast, &programs, &loaded.index);
    let (checkers, results) = check_all(&ast, &programs, &graph, &loaded.sources)?;
    let resolved = collect_resolved(&checkers);

    let module_decls = frontend::Decls::collect(&ast, &programs);
    let root = loaded.index[&loaded.root_name];
    let mut order: Vec<usize> = (0..programs.len()).collect();
    order.sort_by_key(|&i| i != root);
    let lowered = order
        .iter()
        .map(|&i| frontend::lower_program(&ast, &programs[i], &module_decls, &resolved))
        .collect();

    let checker = &checkers[root];
    let decls = results[root]
        .iter()
        .map(|(n, ty)| (n.to_string(), checker.show(ty)))
        .collect();

    Ok(Session { lowered, decls })
}

/// Lower to the IR, then evaluate a module's entry point (`test`, else `main`)
/// on the reified-K machine. 
/// Build the interpreter IR and force every `$ @run <expr>` directive through it
/// at compile time (Jai's `#run`). The value is discarded; a trap fails the
/// build. This runs on every backend, so compile-time execution is
/// engine-independent, and returns the IR so the interpreter path can reuse it.
/// Libraries and search paths a program's `$ @run BUILD.*` directives added to
/// the build (see library/BUILD.thx). Applied to the native link line; the
/// interpreter's default set already covers libc/libm and lazily `dlopen`s the
/// rest per `@extern`.
#[derive(Default)]
struct BuildPlan {
    libs: Vec<String>,
    lib_paths: Vec<String>,
}

/// The first byte string reachable in a value, drilling through the record/tuple
/// that wraps a single-field variant payload (a `BUILD` directive's `{@str}`).
fn first_str(v: &interpreter::machine::data::PVal) -> Option<String> {
    use interpreter::machine::data::Value;
    match &*v.borrow() {
        Value::Str(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        Value::Tuple(items) | Value::Variant { fields: items, .. } => {
            items.iter().find_map(first_str)
        }
        Value::Struct { fields, .. } => fields.iter().find_map(|(_, f)| first_str(f)),
        _ => None,
    }
}

/// Read a `BUILD.Directive` value produced by a compile-time `@run`, if that is
/// what it is. Any other value is plain compile-time execution (discarded).
fn build_directive(v: &interpreter::machine::data::PVal, plan: &mut BuildPlan) {
    use interpreter::machine::data::Value;
    if let Value::Variant { ty, tag, .. } = &*v.borrow() {
        if ty != "Directive" {
            return;
        }
        if let Some(s) = first_str(v) {
            let set = match tag.as_str() {
                "Lib" => &mut plan.libs,
                "LibPath" => &mut plan.lib_paths,
                _ => return,
            };
            if !set.contains(&s) {
                set.push(s);
            }
        }
    }
}

/// Compile and run an `@code` fragment (source text) at build time, reifying its
/// value. This is the `@eval` host: it re-enters the same pipeline the REPL uses,
/// wrapping the fragment as a def body of an in-memory module. `root_dir` resolves
/// the fragment's imports (it sees the same standard library as the root).
fn meta_eval_source(
    src: &str,
    root_dir: &Path,
) -> std::result::Result<interpreter::machine::OwnedValue, String> {
    let session = compile_session(&format!("@mod REPL\n$ _thrax_meta =\n{src}"), root_dir)?;
    let ir = frontend::ir::lower_modules(&session.lowered);
    let v = interpreter::machine::eval_value(&ir, "REPL._thrax_meta")
        .map_err(|d| d.render("", "@eval"))?;
    interpreter::machine::reify(&v)
}

pub fn cmd_run(path: &str, prog_args: &[String]) -> ExitCode {
    // `lower_all` has already expanded every `$ @e` (compile-time execution and
    // code injection); the lowered program is `@e`-free.
    let (lowered, entry, kind, _sources, _plan) = match lower_all(path) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let ir = frontend::ir::lower_modules(&lowered);
    use frontend::EntryKind::*;
    match kind {
        // A value: force it and print `entry = <value>` (the test-harness form).
        Value => match interpreter::machine::eval(&ir, &entry) {
            Ok(shown) => {
                println!("{entry} = {shown}");
                ExitCode::SUCCESS
            }
            Err(diag) => {
                eprint!("{}", diag.render("", &entry));
                ExitCode::FAILURE
            }
        },
        // A C-style `main`: apply it (to unit, or the argument vector `argv[0]` =
        // the entry path, then the extra args) and use its `Int` result as the
        // process exit code.
        UnitFn | ArgvFn => {
            let argv = (kind == ArgvFn).then(|| {
                let mut v = vec![path.to_string()];
                v.extend(prog_args.iter().cloned());
                v
            });
            match interpreter::machine::run_entry(&ir, &entry, argv) {
                Ok(code) => ExitCode::from((code & 0xff) as u8),
                Err(diag) => {
                    eprint!("{}", diag.render("", &entry));
                    ExitCode::FAILURE
                }
            }
        }
        BadFn => unreachable!("rejected in lower_all"),
    }
}

/// Lower, then emit a standalone C program for the module to stdout, compiled
/// for `target` (default: the host).
pub fn cmd_emit_c(path: &str, target: utilities::Target) -> ExitCode {
    let (lowered, entry, kind, _sources, _plan) = match lower_all(path) {
        Ok(x) => x,
        Err(code) => return code,
    };
    // `emit-c` prints C to stdout; the caller drives the link, so `BUILD`
    // directives (which steer linking) have nothing to apply here beyond the
    // link comment the generated source already carries.
    print!("{}", ccg::emit(&lowered, &entry, kind, target));
    ExitCode::SUCCESS
}

/// Lower, emit C for `target`, then compile and link it with the target's
/// toolchain (`cc` natively, `emcc` for wasm). Writes `<stem>.c` and the
/// executable into a `thrax-out/` directory beside the source (kept out of the
/// source tree, gitignore-friendly); prints the path built.
pub fn cmd_build(path: &str, target: utilities::Target) -> ExitCode {
    let (lowered, entry, kind, _sources, plan) = match lower_all(path) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let emitted = ccg::emit_program(&lowered, &entry, kind, target);

    let tc = utilities::toolchain(target);
    if tc.cc.is_empty() {
        eprintln!("thrax: {}", tc.hint);
        return ExitCode::FAILURE;
    }

    let src = Path::new(path);
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let dir = src.parent().unwrap_or_else(|| Path::new(".")).join("thrax-out");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("thrax: cannot create {}: {e}", dir.display());
        return ExitCode::FAILURE;
    }
    let c_path = dir.join(format!("{stem}.c"));
    let out_path = dir.join(format!("{stem}{}", tc.exe_suffix));

    if let Err(e) = std::fs::write(&c_path, &emitted.source) {
        eprintln!("thrax: cannot write {}: {e}", c_path.display());
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
    // Libraries and search paths declared in Thrax via `$ @run BUILD.lib` /
    // `BUILD.lib_path`. Skip a lib already linked from an `@extern`.
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
    match cmd.status() {
        Ok(status) if status.success() => {
            println!("built {}", out_path.display());
            ExitCode::SUCCESS
        }
        Ok(status) => {
            eprintln!("thrax: {} failed ({status})", tc.cc);
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("thrax: cannot run {} ({e}); {}", tc.cc, tc.hint);
            ExitCode::FAILURE
        }
    }
}

pub fn cmd_check(path: &str) -> ExitCode {
    let loaded = match load_sources(path) {
        Ok(l) => l,
        Err(code) => return code,
    };

    // Parse every module into one shared arena.
    let mut ast = frontend::Ast::new();
    let mut programs: Vec<Program> = Vec::with_capacity(loaded.sources.len());
    for (_name, src_path, src) in &loaded.sources {
        match frontend::parse_into(ast, src) {
            Ok((next_ast, p)) => {
                ast = next_ast;
                programs.push(p);
            }
            Err(diag) => {
                eprint!("{}", diag.render(src, src_path));
                return ExitCode::FAILURE;
            }
        }
    }

    let graph = import_graph(&ast, &programs, &loaded.index);
    let (checkers, results) = match check_all(&ast, &programs, &graph, &loaded.sources) {
        Ok(out) => out,
        Err(rendered) => {
            eprint!("{rendered}");
            return ExitCode::FAILURE;
        }
    };

    let root = loaded.index[&loaded.root_name];
    let checker = &checkers[root];
    for (name, ty) in &results[root] {
        println!("{name} : {}", checker.show(ty));
    }
    ExitCode::SUCCESS
}

/// Postorder DFS over the dependency graph: a vertex appears after all vertices
/// it points at, so dependencies are checked before their importers.
fn topological_order(graph: &[Vec<usize>]) -> Vec<usize> {
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

/// Find the source file for a module, searching the sibling standard-library and
/// example directories, then a few nearby fallbacks. The combined test runner
/// imports example modules from `tests/`, while ordinary programs import the
/// standard library.
fn resolve_module_file(name: &str, root_dir: &Path) -> Option<PathBuf> {
    let file = format!("{name}.thx");
    let candidates = [
        root_dir.join("..").join("library").join(&file),
        root_dir.join("..").join("examples").join(&file),
        root_dir.join("library").join(&file),
        root_dir.join("examples").join(&file),
        PathBuf::from("library").join(&file),
        root_dir.join(&file),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// The `@mod` name declared by a source, by parsing it in a scratch arena.
fn parse_mod_name(src: &str) -> Option<String> {
    frontend::parse(src)
        .ok()
        .map(|p| p.ast.text(p.program.module).to_string())
}

/// The module names a source imports (`$ with MOD`), or empty if it does not
/// parse (the parse error is reported later, against the shared arena).
fn parse_imports(src: &str) -> Vec<String> {
    let Ok(parsed) = frontend::parse(src) else {
        return Vec::new();
    };
    parsed.ast
        .slice(parsed.program.items)
        .iter()
        .filter_map(|item| match item {
            Item::Import { module, .. } => Some(
                parsed.ast.slice(*module)
                    .iter()
                    .map(|&part| parsed.ast.text(part))
                    .collect::<Vec<_>>()
                    .join("."),
            ),
            _ => None,
        })
        .collect()
}

fn file_stem(path: &str) -> String {
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
            eprintln!("thrax: cannot read {path}: {e}");
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
            eprintln!("thrax: cannot read {path}: {e}");
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
    use super::*;
    use interpreter::machine::data::{mk, Value};
    use std::rc::Rc;

    fn str_val(s: &str) -> interpreter::machine::data::PVal<'static> {
        mk(Value::Str(Rc::new(s.as_bytes().to_vec())))
    }

    #[test]
    fn build_directive_reads_lib_and_lib_path() {
        // `BUILD.lib "curl"` -> a `Directive.Lib` whose payload holds the name.
        let lib = mk(Value::Variant {
            ty: "Directive".into(),
            tag: "Lib".into(),
            fields: vec![str_val("curl")],
        });
        // `BUILD.lib_path "vendor"` where the payload arrives wrapped in a record,
        // exercising the drill-through in `first_str`.
        let path = mk(Value::Variant {
            ty: "Directive".into(),
            tag: "LibPath".into(),
            fields: vec![mk(Value::Struct {
                name: String::new(),
                fields: vec![("p".into(), str_val("vendor"))],
            })],
        });
        let mut plan = BuildPlan::default();
        build_directive(&lib, &mut plan);
        build_directive(&path, &mut plan);
        assert_eq!(plan.libs, vec!["curl".to_string()]);
        assert_eq!(plan.lib_paths, vec!["vendor".to_string()]);
    }

    #[test]
    fn build_directive_ignores_non_directive_values() {
        // A plain compile-time `@run` value (not a `Directive`) steers nothing.
        let mut plan = BuildPlan::default();
        build_directive(&mk(Value::Int(42)), &mut plan);
        build_directive(
            &mk(Value::Variant {
                ty: "Option".into(),
                tag: "Some".into(),
                fields: vec![str_val("x")],
            }),
            &mut plan,
        );
        assert!(plan.libs.is_empty() && plan.lib_paths.is_empty());
    }
}
