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
    sources: Vec<(String, String)>,
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

    let mut sources: Vec<(String, String)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut queue: Vec<(String, String)> = vec![(root_name.clone(), root_src)];

    // The implicitly imported CORE module (bare names everywhere) is an ordinary
    // standard-library file, loaded from disk like the rest. Seed it into the load
    // queue so it is always present, even without an explicit `$ with CORE`.
    if root_name != "CORE" {
        match resolve_module_file("CORE", &root_dir) {
            Some(file) => match std::fs::read_to_string(&file) {
                Ok(s) => queue.push(("CORE".to_string(), s)),
                Err(e) => {
                    eprintln!("thrax: cannot read the CORE module ({}): {e}", file.display());
                    return Err(ExitCode::FAILURE);
                }
            },
            None => {
                eprintln!("thrax: cannot find the CORE standard-library module");
                return Err(ExitCode::FAILURE);
            }
        }
    }
    while let Some((name, src)) = queue.pop() {
        if index.contains_key(&name) {
            continue;
        }
        let imports = parse_imports(&src);
        index.insert(name.clone(), sources.len());
        sources.push((name, src));
        for imp in imports {
            if index.contains_key(&imp) || queue.iter().any(|(n, _)| *n == imp) {
                continue;
            }
            match resolve_module_file(&imp, &root_dir) {
                Some(file) => match std::fs::read_to_string(&file) {
                    Ok(s) => queue.push((imp, s)),
                    Err(e) => {
                        eprintln!(
                            "thrax: cannot read module `{imp}` ({}): {e}",
                            file.display()
                        );
                        return Err(ExitCode::FAILURE);
                    }
                },
                None => {
                    eprintln!("thrax: cannot find module `{imp}` imported by the program");
                    return Err(ExitCode::FAILURE);
                }
            }
        }
    }
    // Auto-inject the `C` namespace (libc + libm as `@extern` bindings),
    // reachable qualified (`C.sqrt`) with no import, like the prelude.
    if !index.contains_key("C") {
        index.insert("C".to_string(), sources.len());
        sources.push(("C".to_string(), C_SOURCE.to_string()));
    }

    Ok(Loaded {
        sources,
        index,
        root_name,
    })
}

/// The auto-injected `C` standard-library namespace (see core/C.thx).
const C_SOURCE: &str = include_str!("../../../core/C.thx");

/// The dependency graph over parsed modules (edges point at imports).
fn import_graph(
    ast: &frontend::Ast,
    programs: &[Program],
    index: &HashMap<String, usize>,
) -> Vec<Vec<usize>> {
    let mut graph = vec![Vec::new(); programs.len()];
    for (i, program) in programs.iter().enumerate() {
        for item in &program.items {
            if let Item::Import { module, .. } = item {
                let name = module
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
    sources: &[(String, String)],
) -> Result<CheckOut<'a>, ExitCode> {
    let mut checkers: Vec<Option<frontend::Checker>> = (0..programs.len()).map(|_| None).collect();
    let mut results: Vec<Vec<(&str, frontend::Type)>> = vec![Vec::new(); programs.len()];

    // The auto-injected `C` namespace and the implicitly imported `CORE` module
    // have no dependencies and are checked first: `C` made available qualified-only
    // (`C.sqrt`), `CORE` bare (its `to_string` overloads, etc.). `CORE` is checked
    // after `C` but imports neither, so ordering the two is unconstrained.
    let c_idx = sources.iter().position(|(n, _)| n == "C");
    let core_idx = sources.iter().position(|(n, _)| n == "CORE");
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
                let (name, src) = &sources[i];
                eprint!("{}", diag.render(src, name));
                return Err(ExitCode::FAILURE);
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
/// and lower every module. Returns the lowered modules (root first) and the
/// root's entry-point name (`test`, else `main`). Shared by `run` and `emit-c`.
fn lower_all(path: &str) -> Result<(Vec<frontend::lowering::data::Program>, String), ExitCode> {
    let loaded = load_sources(path)?;

    let mut ast = frontend::Ast::new();
    let mut programs: Vec<Program> = Vec::with_capacity(loaded.sources.len());
    for (name, src) in &loaded.sources {
        match frontend::parse_into(ast, src) {
            Ok((next_ast, p)) => {
                ast = next_ast;
                programs.push(p);
            }
            Err(diag) => {
                eprint!("{}", diag.render(src, name));
                return Err(ExitCode::FAILURE);
            }
        }
    }

    let graph = import_graph(&ast, &programs, &loaded.index);
    let checkers = check_all(&ast, &programs, &graph, &loaded.sources)?.0;

    // Collect the checker's resolutions lowering needs: which `[..]` nodes are
    // `Array`, and which bare calls resolved to a specific module.
    let mut resolved = frontend::Resolved::default();
    for checker in &checkers {
        let (exprs, pats) = checker.array_nodes();
        resolved.array_exprs.extend(exprs.iter().copied());
        resolved.array_pats.extend(pats.iter().copied());
        for (&site, &module) in checker.call_modules() {
            resolved.call_modules.insert(site, module.to_string());
        }
        for (&site, key) in checker.overload_calls() {
            resolved.overload_calls.insert(site, key.clone());
        }
        for (&body, key) in checker.def_keys() {
            resolved.def_keys.insert(body, key.clone());
        }
        for (&site, fields) in checker.with_fields() {
            resolved.with_fields.insert(site, fields.clone());
        }
        resolved.extern_sigs.extend(checker.extern_sigs());
    }

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
    match entry {
        Some(e) => Ok((lowered, e.to_string())),
        None => {
            eprintln!(
                "thrax: module `{}` has no `test` or `main` to run",
                loaded.root_name
            );
            Err(ExitCode::FAILURE)
        }
    }
}

/// Lower to the IR, then evaluate a module's entry point (`test`, else `main`)
/// on the reified-K machine. The machine's continuation is an explicit heap
/// stack, so no large host stack is needed for deep recursion.
pub fn cmd_run(path: &str) -> ExitCode {
    let (lowered, entry) = match lower_all(path) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let ir = frontend::ir::lower_modules(&lowered);
    match interpreter::machine::eval(&ir, &entry) {
        Ok(shown) => {
            println!("{entry} = {shown}");
            ExitCode::SUCCESS
        }
        Err(diag) => {
            eprint!("{}", diag.render("", &entry));
            ExitCode::FAILURE
        }
    }
}

/// Lower, then emit a standalone C program for the module to stdout, compiled
/// for `target` (default: the host).
pub fn cmd_emit_c(path: &str, target: utilities::Target) -> ExitCode {
    let (lowered, entry) = match lower_all(path) {
        Ok(x) => x,
        Err(code) => return code,
    };
    print!("{}", ccg::emit(&lowered, &entry, target));
    ExitCode::SUCCESS
}

/// Lower, emit C for `target`, then compile and link it with the target's
/// toolchain (`cc` natively, `emcc` for wasm). Writes `<stem>.c` and the
/// executable next to the source; prints the path built.
pub fn cmd_build(path: &str, target: utilities::Target) -> ExitCode {
    let (lowered, entry) = match lower_all(path) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let emitted = ccg::emit_program(&lowered, &entry, target);

    let tc = utilities::toolchain(target);
    if tc.cc.is_empty() {
        eprintln!("thrax: {}", tc.hint);
        return ExitCode::FAILURE;
    }

    let src = Path::new(path);
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let dir = src.parent().unwrap_or_else(|| Path::new("."));
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
    for (name, src) in &loaded.sources {
        match frontend::parse_into(ast, src) {
            Ok((next_ast, p)) => {
                ast = next_ast;
                programs.push(p);
            }
            Err(diag) => {
                eprint!("{}", diag.render(src, name));
                return ExitCode::FAILURE;
            }
        }
    }

    let graph = import_graph(&ast, &programs, &loaded.index);
    let (checkers, results) = match check_all(&ast, &programs, &graph, &loaded.sources) {
        Ok(out) => out,
        Err(code) => return code,
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
    parsed
        .program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Import { module, .. } => Some(
                module
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
            for item in parsed.program.items {
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
