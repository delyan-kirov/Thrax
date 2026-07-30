//! The Thrax driver (`DR`): module loading, dependency ordering, type-checking,
//! lowering, and the `lex`/`parse`/`check`/`run` subcommands.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use frontend::Lexer;
use frontend::{Item, Program};
use utilities::Arena;

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
    Ok(Loaded {
        sources,
        index,
        root_name,
    })
}

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
    for i in topological_order(graph) {
        let mut checker = frontend::Checker::new(ast);
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

/// Lower, then evaluate a module's entry point (`test`, else `main`).
pub fn cmd_run(path: &str) -> ExitCode {
    let loaded = match load_sources(path) {
        Ok(l) => l,
        Err(code) => return code,
    };

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
    let checkers = match check_all(&ast, &programs, &graph, &loaded.sources) {
        Ok((checkers, _)) => checkers,
        Err(_) => return ExitCode::FAILURE,
    };

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
    let entry = match entry {
        Some(e) => e.to_string(),
        None => {
            eprintln!(
                "thrax: module `{}` has no `test` or `main` to run",
                loaded.root_name
            );
            return ExitCode::FAILURE;
        }
    };

    // Evaluate on a thread with a large stack: the interpreter recurses with the
    // program, and without tail-call optimization even a tail-recursive loop
    // nests one native frame per iteration, so a deep loop needs headroom.
    let run_entry = entry.clone();
    let result = std::thread::Builder::new()
        .stack_size(4 << 30)
        .spawn(move || {
            let interp = interpreter::Interp::new(&lowered);
            interp.eval_global(&run_entry).map(|v| v.show())
        })
        .expect("spawn interpreter thread")
        .join()
        .expect("interpreter thread panicked");

    match result {
        Ok(shown) => {
            println!("{entry} = {shown}");
            ExitCode::SUCCESS
        }
        Err(diag) => {
            eprint!("{}", diag.render("", &loaded.root_name));
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

    let arena = Arena::new();
    match Lexer::tokenize(&source, &arena) {
        Ok(tokens) => {
            for tok in &tokens {
                println!("{:>4}  {:?}  {:?}", tok.line, tok.kind, tok.text);
            }
            ExitCode::SUCCESS
        }
        Err(diag) => {
            eprint!("{}", diag.render(&source, path));
            ExitCode::FAILURE
        }
    }
}
