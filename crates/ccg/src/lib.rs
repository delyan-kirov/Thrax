//! The C code generator (`ccg`): a backend that emits a standalone C program
//! from the Core ([`frontend::lowering::data`]), the ahead-of-time counterpart to
//! the tree-walking `interpreter`.
//!
//! [`emit`] returns a single self-contained C translation unit: a fixed runtime
//! prelude (`runtime.c`, a hand port of the interpreter's value model and
//! built-ins) followed by the program-specific functions the [`gen`] module
//! produces. Compile it with any C99 compiler; running it prints the entry
//! global exactly as `thrax run` does.
//!
//! The effect-free core compiles faithfully. Algebraic effects compile to a
//! runtime fault (see [`gen`]); a continuation-capturing backend is future work.

mod gen;

use frontend::lowering::data::Program;
use gen::{c_string, Emitter};

/// The runtime prelude, emitted verbatim ahead of the generated code.
const RUNTIME: &str = include_str!("runtime.c");

/// Emit a complete C program for `modules` (root first) whose `main` forces and
/// prints the global `entry` from the root module.
pub fn emit(modules: &[Program], entry: &str) -> String {
    let mut em = Emitter::new();

    // Assign each global an index in emission order (root module first), so the
    // globals table's bare-name lookup resolves to the root's definition first,
    // matching the interpreter.
    let mut table: Vec<(String, String, usize)> = Vec::new(); // (key, bare, idx)
    let mut entry_idx: Option<usize> = None;
    let entry_key = format!("{}.{}", modules[0].module, entry);
    let mut idx = 0;
    for program in modules {
        for (name, term) in &program.globals {
            let key = format!("{}.{}", program.module, name);
            em.emit_global(idx, &key, term);
            if key == entry_key {
                entry_idx = Some(idx);
            }
            table.push((key, name.clone(), idx));
            idx += 1;
        }
    }
    let entry_idx = entry_idx.unwrap_or_else(|| panic!("entry `{entry}` not found in root module"));

    let mut out = String::new();
    // `ucontext` (the effect engine's fibers) is obsolescent POSIX; glibc gates
    // its declarations behind _XOPEN_SOURCE, which must precede every header.
    out.push_str("#define _XOPEN_SOURCE 700\n");
    out.push_str(&format!(
        "#define THRAX_ARCH {}\n",
        c_string(std::env::consts::ARCH)
    ));
    out.push_str(&format!(
        "#define THRAX_OS {}\n",
        c_string(std::env::consts::OS)
    ));
    out.push_str(RUNTIME);

    out.push_str("\n/* -- forward declarations -- */\n");
    for d in &em.decls {
        out.push_str(d);
        out.push('\n');
    }

    out.push_str("\n/* -- definitions -- */\n");
    for f in &em.defs {
        out.push_str(f);
    }

    out.push_str("\n/* -- global table -- */\n");
    out.push_str("static Global THRAX_GLOBALS[] = {\n");
    for (key, bare, idx) in &table {
        out.push_str(&format!(
            "  {{{}, {}, thrax_g{idx}}},\n",
            c_string(key),
            c_string(bare)
        ));
    }
    out.push_str("};\n");
    out.push_str(
        "static const Global *thrax_globals(void) { return THRAX_GLOBALS; }\n\
         static size_t thrax_nglobals(void) { return sizeof(THRAX_GLOBALS) / sizeof(THRAX_GLOBALS[0]); }\n",
    );

    out.push_str("\n/* -- effect operations -- */\n");
    let ops: Vec<(&str, &str)> = modules
        .iter()
        .flat_map(|m| m.effects.iter().map(|e| (e.effect.as_str(), e.op.as_str())))
        .collect();
    if ops.is_empty() {
        out.push_str(
            "static const OpDecl *thrax_ops(void) { return NULL; }\n\
             static size_t thrax_nops(void) { return 0; }\n",
        );
    } else {
        out.push_str("static OpDecl THRAX_OPS[] = {\n");
        for (effect, op) in &ops {
            out.push_str(&format!("  {{{}, {}}},\n", c_string(effect), c_string(op)));
        }
        out.push_str("};\n");
        out.push_str(
            "static const OpDecl *thrax_ops(void) { return THRAX_OPS; }\n\
             static size_t thrax_nops(void) { return sizeof(THRAX_OPS) / sizeof(THRAX_OPS[0]); }\n",
        );
    }

    // Run on a thread with a large stack: evaluation recurses with the program,
    // and without tail-call optimization even a tail-recursive loop nests one
    // native frame per iteration, so a deep loop needs headroom (the interpreter
    // does the same). Requires linking with `-pthread`.
    out.push_str(&format!(
        "\nstatic void *thrax_main(void *unused) {{\n\
         (void)unused;\n\
         Value *v = thrax_g{entry_idx}();\n\
         printf(\"%s = %s\\n\", {entry}, thrax_show(v));\n\
         return NULL;\n\
         }}\n\
         \n\
         int main(void) {{\n\
         pthread_attr_t attr;\n\
         pthread_t t;\n\
         if (pthread_attr_init(&attr) == 0 &&\n\
         pthread_attr_setstacksize(&attr, (size_t)1 << 30) == 0 &&\n\
         pthread_create(&t, &attr, thrax_main, NULL) == 0) {{\n\
         pthread_join(t, NULL);\n\
         }} else {{\n\
         thrax_main(NULL);\n\
         }}\n\
         return 0;\n\
         }}\n",
        entry = c_string(entry),
    ));

    out
}
