//! The Thrax driver (`DR`): module loading, dependency ordering, type-checking,
//! lowering, and the `lex`/`parse`/`check`/`run` subcommands.

use std::collections::HashMap;
use std::rc::Rc;
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
    lenient: bool,
    open_effects_module: Option<&str>,
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
        checker.set_lenient(lenient);
        if open_effects_module == Some(sources[i].0.as_str()) {
            checker.set_open_effects(true);
        }
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

/// The full pipeline up to (but not including) execution: load, parse, check,
/// and lower every module, returning the lowered modules (root first). The
/// parse/check state is local (self-borrowing), so only the owned `lowered`
/// escapes; the expansion loop in [`lower_all`] calls this repeatedly on
/// progressively `@e`-expanded sources.
type Compiled = (
    Vec<frontend::lowering::data::Program>,
    frontend::lowering::ReflectInfo,
);

fn compile_sources(loaded: &Loaded, lenient: bool, want_entry: bool) -> Result<Compiled, ExitCode> {
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
    let (checkers, results) = check_all(&ast, &programs, &graph, &loaded.sources, lenient, None)
        .map_err(|rendered| {
            eprint!("{rendered}");
            ExitCode::FAILURE
        })?;

    let resolved = frontend::collect_resolved(&checkers);

    // Lower every module; put the root first so its names win when resolving an
    // unqualified reference defined in more than one module.
    let decls = frontend::Decls::collect(&ast, &programs);
    let reflect = decls.reflect();
    let root = loaded.index[&loaded.root_name];
    let mut order: Vec<usize> = (0..programs.len()).collect();
    order.sort_by_key(|&i| i != root);
    let lowered: Vec<frontend::lowering::data::Program> = order
        .iter()
        .map(|&i| frontend::lower_program(&ast, &programs[i], &decls, &resolved))
        .collect();

    // Only a command that RUNS the program needs an entry (`check` type-checks a
    // library module too), and a metaprogram-expansion round does not: the entry
    // may itself be injected, so the requirement waits for the final strict round.
    if want_entry && !lenient {
        let entry = lowered[0]
            .globals
            .iter()
            .any(|(n, _)| n == frontend::ENTRY);
        if !entry {
            eprintln!(
                "thrax: module `{}` has no `$ {} : {}` to run",
                loaded.root_name,
                frontend::ENTRY,
                frontend::ENTRY_SIG
            );
            return Err(ExitCode::FAILURE);
        }
        let ok = results[root]
            .iter()
            .find(|(n, _)| *n == frontend::ENTRY)
            .is_some_and(|(_, ty)| frontend::is_entry_type(ty));
        if !ok {
            eprintln!(
                "thrax: `{}` must have the signature `{}`",
                frontend::ENTRY,
                frontend::ENTRY_SIG
            );
            return Err(ExitCode::FAILURE);
        }
    }
    Ok((lowered, reflect))
}

/// Render a forced expression-position `@e X` value back into Thrax source to
/// splice at the site. A `@code` result contributes the fragment it holds; a
/// scalar contributes its literal; aggregates (vectors, tuples, structs,
/// variants) render as their type-directed literals, recursively.
fn render_meta_value(v: &interpreter::machine::data::PVal) -> std::result::Result<String, String> {
    render_owned(&interpreter::machine::reify(v)?)
}

/// Render a reified compile-time value as the Thrax literal that reconstructs it.
/// Aggregate literals (`[..]`, `.{ .. }`) are type-directed, so the rendered text
/// must land where its type is pinned (an annotation or an unambiguous use site).
fn render_owned(
    v: &interpreter::machine::OwnedValue,
) -> std::result::Result<String, String> {
    use interpreter::machine::OwnedValue;
    let each = |items: &[OwnedValue]| -> std::result::Result<Vec<String>, String> {
        items.iter().map(render_owned).collect()
    };
    match v {
        OwnedValue::Int(n) => Ok(n.to_string()),
        OwnedValue::Real(r) => Ok(format!("{r:?}")),
        OwnedValue::Real32(r) => Ok(format!("{r:?}")),
        OwnedValue::Bool(b) => Ok(if *b { "@true" } else { "@false" }.to_string()),
        OwnedValue::Unit => Ok("{}".to_string()),
        OwnedValue::Str(b) => Ok(thrax_str_literal(b)),
        OwnedValue::Vector(items) => Ok(format!("[{}]", each(items)?.join(", "))),
        OwnedValue::Tuple(items) => Ok(format!("{{{}}}", each(items)?.join(", "))),
        // A `@code` fragment reifies to a `{ src = "..." }` struct: splice its
        // held source text rather than rebuilding it as a record literal.
        OwnedValue::Struct { name, fields } if name == "@code" => fields
            .iter()
            .find(|(k, _)| k == "src")
            .map(|(_, s)| match s {
                OwnedValue::Str(b) => Ok(String::from_utf8_lossy(b).into_owned()),
                _ => Err("malformed @code value".to_string()),
            })
            .unwrap_or_else(|| Err("malformed @code value".to_string())),
        OwnedValue::Struct { name, fields } => {
            let body = fields
                .iter()
                .map(|(k, val)| Ok(format!(".{k} = {}", render_owned(val)?)))
                .collect::<std::result::Result<Vec<_>, String>>()?
                .join(", ");
            Ok(format!("{name}.{{ {body} }}"))
        }
        // Variant payloads are positional (`Ty.Tag.{ a, b }`); a nullary
        // constructor renders bare as `Ty.Tag`.
        OwnedValue::Variant { ty, tag, fields } if fields.is_empty() => Ok(format!("{ty}.{tag}")),
        OwnedValue::Variant { ty, tag, fields } => {
            Ok(format!("{ty}.{tag}.{{ {} }}", each(fields)?.join(", ")))
        }
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

/// Compile `path`, iteratively expanding every `$ @e` (compile-time execution,
/// value-fold, and code injection): each round compiles the current sources,
/// forces every `@e` site, and substitutes the result in source, repeating until
/// none remain. `@link`/`@link_path` calls record build directives along the way,
/// drained into the returned `BuildPlan`. Returns the expanded modules too, so a
/// caller can report against the sources the checker actually saw. `want_entry`
/// demands a `$ @main` of the root module: set by the commands that run or build
/// the program, clear for a plain `check`.
fn lower_all(
    path: &str,
    want_entry: bool,
) -> Result<(Vec<frontend::lowering::data::Program>, Loaded, BuildPlan), ExitCode> {
    let mut loaded = load_sources(path)?;
    let root_dir = Path::new(path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut plan = BuildPlan::default();
    let _ = interpreter::machine::take_link_directives(); // drop any stale directives
    loop {
        // Compile leniently while `@e` sites remain: a generator may inject a
        // definition that hand-written code forward-references, which would not
        // yet resolve. Unbound names are deferred (fresh vars) so the generators
        // can still run; the final strict compile below validates the result.
        let compiled = compile_sources(&loaded, true, want_entry)?;
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
        let type_sites: Vec<(String, String, utilities::Span)> = compiled
            .0
            .iter()
            .flat_map(|p| {
                p.ct_types
                    .iter()
                    .map(move |(name, span)| (p.module.clone(), format!("{}.{}", p.module, name), *span))
            })
            .collect();
        if expr_sites.is_empty() && item_sites.is_empty() && type_sites.is_empty() {
            // No `@e` remains: re-check strictly so any name still unbound after
            // all injection, or a missing/ill-typed entry, is reported now.
            let final_compiled = compile_sources(&loaded, false, want_entry)?;
            // Fold in any `@link`/`@link_path` directives recorded while running.
            for (is_path, arg) in interpreter::machine::take_link_directives() {
                let set = if is_path { &mut plan.lib_paths } else { &mut plan.libs };
                if !set.contains(&arg) {
                    set.push(arg);
                }
            }
            return Ok((final_compiled.0, loaded, plan));
        }
        let ir = frontend::ir::lower_modules(&compiled.0);
        let rd = root_dir.clone();
        interpreter::machine::set_meta_eval(Some(Box::new(move |src| meta_eval_source(src, &rd))));
        let reflect = reflect_tables(compiled.1);
        let mut edits: Vec<(String, utilities::Span, String)> = Vec::new();
        let fail = |diag, module: &str, span| -> ExitCode {
            clear_meta_hosts();
            eprintln!("thrax: a compile-time `@e` failed:");
            eprint!("{}", render_e_fault(diag, module, span, &loaded.sources));
            ExitCode::FAILURE
        };
        // Expression-position: replace the `@e X` with its value's source
        // (parenthesized to keep precedence).
        for (module, qualified, span) in expr_sites {
            interpreter::machine::set_type_host(Some(reflect_host(
                Rc::clone(&reflect),
                module.clone(),
            )));
            let v = interpreter::machine::eval_value(&ir, &qualified)
                .map_err(|d| fail(d, &module, span))?;
            match render_meta_value(&v) {
                Ok(text) => edits.push((module, span, format!("({text})"))),
                Err(msg) => {
                    clear_meta_hosts();
                    eprintln!("thrax: a compile-time `@e` failed: {msg}");
                    return Err(ExitCode::FAILURE);
                }
            }
        }
        // Item-position `$ @e X`: an `@code` result injects its item(s) in place;
        // otherwise apply a `BUILD` directive (a no-op for any other value) and
        // drop the directive. Either way the whole `$ @e X` item is replaced.
        for (module, qualified, span) in item_sites {
            interpreter::machine::set_type_host(Some(reflect_host(
                Rc::clone(&reflect),
                module.clone(),
            )));
            let v = interpreter::machine::eval_value(&ir, &qualified)
                .map_err(|d| fail(d, &module, span))?;
            // `span` is the whole `$ @e X` directive, so replacing it drops the
            // directive: an `@code` result becomes its item(s), anything else (a
            // `@link` call, whose effect was already recorded, or a discarded
            // value) becomes nothing.
            match as_code_src(&v) {
                Some(items) => edits.push((module, span, items)),
                None => edits.push((module, span, String::new())),
            }
        }
        // Type-position `@e X` (`Foo : @e X = ...`): the result must be an `@code`
        // denoting a type; splice its source in place of the `@e X`.
        for (module, qualified, span) in type_sites {
            interpreter::machine::set_type_host(Some(reflect_host(
                Rc::clone(&reflect),
                module.clone(),
            )));
            let v = interpreter::machine::eval_value(&ir, &qualified)
                .map_err(|d| fail(d, &module, span))?;
            match as_code_src(&v) {
                Some(ty_src) => edits.push((module, span, format!("({ty_src})"))),
                None => {
                    clear_meta_hosts();
                    eprintln!(
                        "thrax: a compile-time `@e` failed: a type-position `@e` must \
                         produce an `@code` (build one with `@parse_str`)"
                    );
                    return Err(ExitCode::FAILURE);
                }
            }
        }
        clear_meta_hosts();
        splice_sources(&mut loaded.sources, edits);
    }
}

/// Clear both compile-time hosts (`@eval` and type reflection) installed around a
/// `$ @e` expansion round, so a stray meta op at runtime faults cleanly.
fn clear_meta_hosts() {
    interpreter::machine::set_meta_eval(None);
    interpreter::machine::set_type_host(None);
}

/// Every declared type shape a round can reflect on, keyed for both lookup
/// forms: `qualified` is the exact `module.name`, and `bare` lists every module
/// declaring a given name, sorted by module so the choice never depends on hash
/// order. Types are namespaced per module, so a bare name means "mine first".
struct ReflectTables {
    qualified: HashMap<(String, String), interpreter::machine::TypeInfo>,
    bare: HashMap<String, Vec<(String, interpreter::machine::TypeInfo)>>,
}

/// Index a round's collected type shapes for `$ @e` reflection.
fn reflect_tables(reflect: frontend::lowering::ReflectInfo) -> Rc<ReflectTables> {
    use interpreter::machine::TypeInfo;
    let mut qualified = HashMap::new();
    let mut bare: HashMap<String, Vec<(String, TypeInfo)>> = HashMap::new();
    let mut put = |module: String, name: String, info: TypeInfo| {
        qualified.insert((module.clone(), name.clone()), info.clone());
        bare.entry(name).or_default().push((module, info));
    };
    for (module, name, params, fields) in reflect.structs {
        put(module, name, TypeInfo::Struct { params, fields });
    }
    for (module, name, params, variants) in reflect.unions {
        put(module, name, TypeInfo::Union { params, variants });
    }
    for entries in bare.values_mut() {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
    }
    Rc::new(ReflectTables { qualified, bare })
}

/// The `$ @e` reflection host for the module whose directive is running: a
/// by-name lookup answering `@type_kind`/`@type_fields`/`@type_variants`.
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
    Box::new(move |name: &str| {
        if let Some((module, bare)) = name.split_once('.') {
            return tables
                .qualified
                .get(&(module.to_string(), bare.to_string()))
                .cloned();
        }
        let entries = tables.bare.get(name)?;
        entries
            .iter()
            .find(|(m, _)| *m == caller)
            .or_else(|| entries.first())
            .map(|(_, info)| info.clone())
    })
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
    // The shell forces each entered symbol and permits IO, so the REPL root module is
    // checked under an open effect row (a file's top level stays pure).
    let (checkers, results) = check_all(
        &ast,
        &programs,
        &graph,
        &loaded.sources,
        false,
        Some(&loaded.root_name),
    )?;
    let resolved = frontend::collect_resolved(&checkers);

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
    let (lowered, _loaded, _plan) = match lower_all(path, true) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let ir = frontend::ir::lower_modules(&lowered);
    // `@main` takes the argument vector (`argv[0]` = the entry path, then the
    // extra args) and its `@int` result is the process exit code.
    let mut argv = vec![path.to_string()];
    argv.extend(prog_args.iter().cloned());
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
    let (lowered, _loaded, _plan) = match lower_all(path, true) {
        Ok(x) => x,
        Err(code) => return code,
    };
    // `emit-c` prints C to stdout; the caller drives the link, so `BUILD`
    // directives (which steer linking) have nothing to apply here beyond the
    // link comment the generated source already carries.
    print!(
        "{}",
        ccg::emit(&lowered, frontend::ENTRY, ccg::Entry::Main, target)
    );
    ExitCode::SUCCESS
}

/// Lower, emit C for `target`, then compile and link it with the target's
/// toolchain (`cc` natively, `emcc` for wasm). Writes `<stem>.c` and the
/// executable into a `thrax-out/` directory beside the source (kept out of the
/// source tree, gitignore-friendly); prints the path built.
pub fn cmd_build(path: &str, target: utilities::Target) -> ExitCode {
    let (lowered, _loaded, plan) = match lower_all(path, true) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let emitted = ccg::emit_program(&lowered, frontend::ENTRY, ccg::Entry::Main, target);

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
    // Expanding first means the printed types are the ones the program really has
    // (a `@e`-injected definition included), and that every `$ @run` directive has
    // run: a module's compile-time assertions are checked by checking it.
    let (_lowered, loaded, _plan) = match lower_all(path, false) {
        Ok(x) => x,
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
    let (checkers, results) = match check_all(&ast, &programs, &graph, &loaded.sources, false, None) {
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
