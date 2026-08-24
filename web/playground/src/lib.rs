//! The browser playground: the Thrax compiler (frontend + interpreter) built to
//! `wasm32-unknown-unknown` as a `cdylib` and driven from hand-written JS over
//! linear memory (no wasm-bindgen, no external crates).
//!
//! [`thx_eval`] takes UTF-8 Thrax source, runs the same pipeline the native
//! `thrax run` does (parse -> check -> lower -> IR -> interpret) against an
//! in-memory copy of the standard library, and returns the program's output (or
//! a rendered diagnostic). The `@extern` FFI is stubbed on wasm (no host libc),
//! so pure programs run fully and a foreign call reports a clear message.

use frontend::lowering::data::Program as LoweredProgram;
use frontend::{Ast, Item, Program};

/// The bundled standard library, keyed by module name. `C` is the auto-injected
/// libc namespace; the rest mirror `library/*.thx`. Included at build time so the
/// wasm module needs no filesystem.
fn stdlib_source(name: &str) -> Option<&'static str> {
    Some(match name {
        "C" => include_str!("../../../library/C.thx"),
        "LIST" => include_str!("../../../library/LIST.thx"),
        "MAP" => include_str!("../../../library/MAP.thx"),
        "SET" => include_str!("../../../library/SET.thx"),
        "VEC" => include_str!("../../../library/VEC.thx"),
        "STR" => include_str!("../../../library/STR.thx"),
        "OPT" => include_str!("../../../library/OPT.thx"),
        "RESULT" => include_str!("../../../library/RESULT.thx"),
        "MATH" => include_str!("../../../library/MATH.thx"),
        "PATH" => include_str!("../../../library/PATH.thx"),
        "RANDOM" => include_str!("../../../library/RANDOM.thx"),
        "IO" => include_str!("../../../library/IO.thx"),
        // Playground-only: routes I/O to JavaScript host imports (no libc on wasm).
        "HOST" => include_str!("host.thx"),
        _ => return None,
    })
}

/// The module name a source declares (`@mod NAME`), or `MAIN` if it will not
/// parse far enough to tell.
fn module_name(src: &str) -> String {
    frontend::parse(src)
        .ok()
        .map(|p| p.ast.text(p.program.module).to_string())
        .unwrap_or_else(|| "MAIN".to_string())
}

/// The modules a source imports (`$ with MOD`).
fn imports_of(src: &str) -> Vec<String> {
    let Ok(parsed) = frontend::parse(src) else {
        return Vec::new();
    };
    parsed
        .ast
        .slice(parsed.program.items)
        .iter()
        .filter_map(|item| match item {
            Item::Import { module, .. } => Some(
                parsed
                    .ast
                    .slice(*module)
                    .iter()
                    .map(|&part| parsed.ast.text(part))
                    .collect::<Vec<_>>()
                    .join("."),
            ),
            _ => None,
        })
        .collect()
}

/// Postorder DFS: a module appears after every module it imports.
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

/// Resolve the user source plus every standard-library module it imports
/// (transitively) into a `(name, source)` list, with `C` last. Mirrors the
/// driver's `load_sources`, but every dependency comes from [`stdlib_source`].
fn gather_sources(root_src: &str) -> Result<Vec<(String, String)>, String> {
    let root_name = module_name(root_src);
    let mut sources: Vec<(String, String)> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut queue: Vec<(String, String)> = vec![(root_name, root_src.to_string())];
    while let Some((name, src)) = queue.pop() {
        if names.contains(&name) {
            continue;
        }
        let imports = imports_of(&src);
        names.push(name.clone());
        sources.push((name, src));
        for imp in imports {
            if names.contains(&imp) || queue.iter().any(|(n, _)| *n == imp) {
                continue;
            }
            match stdlib_source(&imp) {
                Some(s) => queue.push((imp, s.to_string())),
                None => return Err(format!("cannot find module `{imp}`")),
            }
        }
    }
    if !names.iter().any(|n| n == "C") {
        sources.push(("C".to_string(), stdlib_source("C").unwrap().to_string()));
    }
    Ok(sources)
}

/// What to produce from a source, matching the site's mode selector.
/// 0 = run (default), 1 = generated C, 2 = IR, 3 = AST.
pub fn compile(user_src: &str, mode: i32) -> String {
    let result = match mode {
        1 => emit_c(user_src),
        2 => dump_ir(user_src),
        3 => dump_ast(user_src),
        _ => run(user_src),
    };
    match result {
        Ok(out) => out,
        Err(msg) => msg,
    }
}

/// Compile and run `user_src` (mode 0), returning `entry = value` or a rendered
/// diagnostic.
pub fn run_source(user_src: &str) -> String {
    compile(user_src, 0)
}

fn run(user_src: &str) -> Result<String, String> {
    let (lowered, entry) = pipeline(user_src)?;
    let ir = frontend::ir::lower_modules(&lowered);
    match interpreter::machine::eval(&ir, &entry) {
        Ok(shown) => Ok(format!("{entry} = {shown}")),
        Err(diag) => Err(diag.render("", &entry)),
    }
}

fn emit_c(user_src: &str) -> Result<String, String> {
    let (lowered, entry) = pipeline(user_src)?;
    // A conventional host target so the generated C reads normally, independent
    // of the wasm build the playground itself runs as.
    let target = utilities::Target {
        os: utilities::Os::Linux,
        arch: utilities::Arch::X86_64,
    };
    Ok(ccg::emit(&lowered, &entry, frontend::EntryKind::Value, target))
}

fn dump_ir(user_src: &str) -> Result<String, String> {
    let (lowered, _entry) = pipeline(user_src)?;
    Ok(format!("{:#?}", frontend::ir::lower_modules(&lowered)))
}

fn dump_ast(user_src: &str) -> Result<String, String> {
    match frontend::parse(user_src) {
        Ok(p) => {
            let mut out = format!(
                "module {} ({} items)\n",
                p.ast.text(p.program.module),
                p.program.items.len()
            );
            for item in p.ast.slice(p.program.items) {
                out.push_str(&format!("  {item:?}\n"));
            }
            Ok(out)
        }
        Err(diag) => Err(diag.render(user_src, "MAIN")),
    }
}

/// The pipeline up to (not including) execution: load, parse, check, and lower
/// every module against the in-memory standard library. Returns the lowered
/// modules (root first) and the entry-point name (`test`, else `main`).
fn pipeline(user_src: &str) -> Result<(Vec<LoweredProgram>, String), String> {
    let sources = gather_sources(user_src)?;
    let root_name = module_name(user_src);

    let mut index = std::collections::HashMap::new();
    for (i, (name, _)) in sources.iter().enumerate() {
        index.insert(name.clone(), i);
    }

    // Parse every module into one shared arena.
    let mut ast = Ast::new();
    let mut programs: Vec<Program> = Vec::with_capacity(sources.len());
    for (name, src) in &sources {
        match frontend::parse_into(ast, src) {
            Ok((next_ast, p)) => {
                ast = next_ast;
                programs.push(p);
            }
            Err(diag) => return Err(diag.render(src, name)),
        }
    }

    // Dependency graph (edges point at imports).
    let mut graph = vec![Vec::new(); programs.len()];
    for (i, program) in programs.iter().enumerate() {
        for item in ast.slice(program.items) {
            if let Item::Import { module, .. } = item {
                let name = ast
                    .slice(*module)
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

    // Type-check in dependency order, `C` first (qualified-only elsewhere).
    let c_idx = sources.iter().position(|(n, _)| n == "C");
    let mut order = topological_order(&graph);
    if let Some(c) = c_idx {
        order.retain(|&i| i != c);
        order.insert(0, c);
    }
    let mut checkers: Vec<Option<frontend::Checker>> = (0..programs.len()).map(|_| None).collect();
    for i in order {
        let mut checker = frontend::Checker::new(&ast);
        if let Some(c) = c_idx {
            if c != i {
                checker.import_qualified(checkers[c].as_ref().expect("C checked first"));
            }
        }
        for &dep in &graph[i] {
            checker.import_from(checkers[dep].as_ref().expect("dependency checked first"));
        }
        match checker.check_program(&programs[i]) {
            Ok(_) => checkers[i] = Some(checker),
            Err(diag) => {
                let (name, src) = &sources[i];
                return Err(diag.render(src, name));
            }
        }
    }
    let checkers: Vec<frontend::Checker> = checkers.into_iter().map(|c| c.expect("all checked")).collect();

    // Gather the checker resolutions lowering needs.
    let mut resolved = frontend::Resolved::default();
    for checker in &checkers {
        let (exprs, pats) = checker.array_nodes();
        resolved.array_exprs.extend(exprs.iter().copied());
        resolved.array_pats.extend(pats.iter().copied());
    resolved.tensor_exprs.extend(checker.tensor_nodes().iter().copied());
        for (&site, names) in checker.promotions() { resolved.promotions.insert(site, names.clone()); }
        for (&site, n) in checker.struct_lit_names() { resolved.struct_lit_names.insert(site, n.clone()); }
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
    }

    // Lower every module (root first so its names win the unqualified fallback).
    let decls = frontend::Decls::collect(&ast, &programs);
    let root = index[&root_name];
    let mut lower_order: Vec<usize> = (0..programs.len()).collect();
    lower_order.sort_by_key(|&i| i != root);
    let lowered: Vec<LoweredProgram> = lower_order
        .iter()
        .map(|&i| frontend::lower_program(&ast, &programs[i], &decls, &resolved))
        .collect();

    let entry = ["test", "main"]
        .into_iter()
        .find(|name| lowered[0].globals.iter().any(|(n, _)| n == name));
    let entry = match entry {
        Some(e) => e.to_string(),
        None => return Err(format!("module `{root_name}` has no `test` or `main` to run")),
    };

    Ok((lowered, entry))
}

// -- the wasm C-ABI seam ----------------------------------------------------

/// Reserve `n` bytes in the module's linear memory and return the offset; the JS
/// host writes the source there before calling [`thx_eval`].
#[no_mangle]
pub extern "C" fn thx_alloc(n: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(n.max(1));
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// The result of the most recent [`thx_eval`], kept alive for the host to read
/// via [`thx_out_ptr`]/[`thx_out_len`]. Single-threaded (wasm), so a plain
/// static is sound.
static mut OUTPUT: Vec<u8> = Vec::new();

/// Compile the `len` source bytes at `ptr` in `mode` (0 run, 1 C, 2 IR, 3 AST);
/// the output is staged for [`thx_out_ptr`]/[`thx_out_len`]. Returns the output
/// length for convenience.
#[no_mangle]
pub extern "C" fn thx_compile(ptr: *const u8, len: usize, mode: i32) -> usize {
    let src = unsafe { std::slice::from_raw_parts(ptr, len) };
    let source = String::from_utf8_lossy(src).into_owned();
    let out = compile(&source, mode).into_bytes();
    let n = out.len();
    unsafe {
        *std::ptr::addr_of_mut!(OUTPUT) = out;
    }
    n
}

/// The offset of the staged output in linear memory.
#[no_mangle]
pub extern "C" fn thx_out_ptr() -> *const u8 {
    unsafe { (*std::ptr::addr_of!(OUTPUT)).as_ptr() }
}

/// The length of the staged output.
#[no_mangle]
pub extern "C" fn thx_out_len() -> usize {
    unsafe { (*std::ptr::addr_of!(OUTPUT)).len() }
}

#[cfg(test)]
mod tests {
    use super::run_source;

    #[test]
    fn runs_a_pure_program() {
        let out = run_source("@mod MAIN\n$ main : Int = 6 * 7\n");
        assert_eq!(out, "main = 42");
    }

    #[test]
    fn imports_the_bundled_stdlib() {
        let src = "@mod MAIN\n$ with STR\n$ main : Str = STR.from_int 123\n";
        assert_eq!(run_source(src), "main = \"123\"");
    }

    #[test]
    fn reports_a_type_error() {
        let out = run_source("@mod MAIN\n$ main : Int = \"x\" + 1\n");
        assert!(out.contains("error") || out.contains("mismatch"), "{out}");
    }

    #[test]
    fn bundles_the_host_module() {
        // `HOST.print` reaches a JS import only on wasm, so it faults if run
        // natively. Checking the IR still exercises parsing, resolution, and
        // type-checking of the bundled `HOST` module end to end.
        let src = "@mod MAIN\n$ with HOST\n$ main : Int = HOST.print \"hi\"; 0\n";
        let ir = super::compile(src, 2);
        assert!(!ir.to_lowercase().contains("error"), "{ir}");
        assert!(ir.contains("WASM"), "{ir}");
        assert!(ir.contains("\"print\""), "{ir}");
    }
}
