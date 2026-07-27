//! The Thrax driver. Frontend for now: `thrax <lex|parse|check> <file>`.
//! `check` resolves the file's `$ with MOD` imports from the standard library,
//! type-checks every module in dependency order, and reports the root module's
//! definition types.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arena::Arena;
use lexer::Lexer;
use syntax::{Item, Program};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match (args.get(1).map(String::as_str), args.get(2)) {
        (Some("lex"), Some(path)) => cmd_lex(path),
        (Some("parse"), Some(path)) => cmd_parse(path),
        (Some("check"), Some(path)) => cmd_check(path),
        _ => {
            eprintln!("usage: thrax <lex|parse|check> <file.thx>");
            ExitCode::FAILURE
        }
    }
}

fn cmd_check(path: &str) -> ExitCode {
    let root_dir = Path::new(path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();

    // Load the root file and, transitively, every module it imports.
    let root_src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("thrax: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let root_name = parse_mod_name(&root_src).unwrap_or_else(|| file_stem(path));

    // sources[i] = (module name, source text); `index` maps a name to its slot.
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
                        return ExitCode::FAILURE;
                    }
                },
                None => {
                    eprintln!("thrax: cannot find module `{imp}` imported by the program");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    // Parse every module into one shared arena.
    let arena = Arena::new();
    let mut programs: Vec<Program> = Vec::with_capacity(sources.len());
    for (name, src) in &sources {
        match syntax::parse(src, &arena) {
            Ok(p) => programs.push(p),
            Err(diag) => {
                eprint!("{}", diag.render(src, name));
                return ExitCode::FAILURE;
            }
        }
    }

    // Build the import graph (edges point at dependencies) and order it so a
    // module is checked after every module it imports.
    let mut graph = vec![Vec::new(); programs.len()];
    for (i, program) in programs.iter().enumerate() {
        for item in program.items {
            if let Item::Import { module, .. } = item {
                if let Some(&j) = index.get(&module.join(".")) {
                    graph[i].push(j);
                }
            }
        }
    }

    // Check each module, importing its already-checked dependencies first.
    let mut checkers: Vec<Option<types::Checker>> = (0..programs.len()).map(|_| None).collect();
    let mut results: Vec<Vec<(&str, types::Type)>> = vec![Vec::new(); programs.len()];
    for i in topological_order(&graph) {
        let mut checker = types::Checker::new();
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
                return ExitCode::FAILURE;
            }
        }
    }

    let root = index[&root_name];
    let checker = checkers[root].as_ref().expect("root checked");
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

/// Find the source file for a module, searching the sibling `library` directory
/// (where the standard library lives) and a few nearby fallbacks.
fn resolve_module_file(name: &str, root_dir: &Path) -> Option<PathBuf> {
    let file = format!("{name}.thx");
    let candidates = [
        root_dir.join("..").join("library").join(&file),
        root_dir.join("library").join(&file),
        PathBuf::from("library").join(&file),
        root_dir.join(&file),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// The `@mod` name declared by a source, by parsing it in a scratch arena.
fn parse_mod_name(src: &str) -> Option<String> {
    let arena = Arena::new();
    syntax::parse(src, &arena)
        .ok()
        .map(|p| p.module.to_string())
}

/// The module names a source imports (`$ with MOD`), or empty if it does not
/// parse (the parse error is reported later, against the shared arena).
fn parse_imports(src: &str) -> Vec<String> {
    let arena = Arena::new();
    let Ok(program) = syntax::parse(src, &arena) else {
        return Vec::new();
    };
    program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Import { module, .. } => Some(module.join(".")),
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

fn cmd_parse(path: &str) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("thrax: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let arena = Arena::new();
    match syntax::parse(&source, &arena) {
        Ok(program) => {
            println!("module {} ({} items)", program.module, program.items.len());
            for item in program.items {
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

fn cmd_lex(path: &str) -> ExitCode {
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
