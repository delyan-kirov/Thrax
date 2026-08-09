//! End-to-end backend tests: lower a program, emit C, compile it with the
//! system C compiler, run it, and check the output matches the interpreter.
//!
//! These need a C compiler (`$CC`, else `cc`) and `-pthread`, both present in the
//! project dev shell.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use frontend::lowering::data::Program;
use frontend::{lower_program, Decls, Resolved};

/// Parse, check, and lower a single-module source. Mirrors the driver's pipeline
/// (including the checker resolutions lowering consumes).
fn lower(src: &str) -> Vec<Program> {
    let parsed = frontend::parse(src).expect("parse");
    let mut checker = frontend::Checker::new(&parsed.ast);
    checker.check_program(&parsed.program).expect("check");
    let (exprs, pats) = checker.array_nodes();
    let mut resolved = Resolved::default();
    resolved.array_exprs.extend(exprs.iter().copied());
    resolved.array_pats.extend(pats.iter().copied());
    for (&site, names) in checker.promotions() { resolved.promotions.insert(site, names.clone()); }
        for (&site, n) in checker.struct_lit_names() { resolved.struct_lit_names.insert(site, n.clone()); }
        let (clits, obs) = checker.codata_sites(); resolved.codata_lits.extend(clits.iter().copied()); resolved.observations.extend(obs.iter().copied());
    for (&site, &m) in checker.call_modules() {
        resolved.call_modules.insert(site, m.to_string());
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
    let decls = Decls::collect(&parsed.ast, std::slice::from_ref(&parsed.program));
    vec![lower_program(
        &parsed.ast,
        &parsed.program,
        &decls,
        &resolved,
    )]
}

fn interp_show(src: &str, entry: &str) -> String {
    let lowered = lower(src);
    let ir = frontend::ir::lower_modules(&lowered);
    interpreter::machine::eval(&ir, entry).expect("machine")
}

/// Emit C for `src`, compile and run it, and return its stdout (trimmed).
fn c_run(src: &str, entry: &str) -> String {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let lowered = lower(src);
    let code = ccg::emit(&lowered, entry, utilities::Target::host());

    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut c_path = std::env::temp_dir();
    c_path.push(format!("ccg_{}_{}.c", std::process::id(), n));
    let bin_path: PathBuf = c_path.with_extension("bin");
    std::fs::write(&c_path, &code).expect("write C");

    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let status = Command::new(&cc)
        .args(["-w", "-O1", "-pthread", "-o"])
        .arg(&bin_path)
        .arg(&c_path)
        .arg("-lm")
        .status()
        .expect("run C compiler");
    assert!(status.success(), "C compile failed for entry `{entry}`");

    let out = Command::new(&bin_path)
        .output()
        .expect("run compiled program");
    let _ = std::fs::remove_file(&c_path);
    let _ = std::fs::remove_file(&bin_path);
    assert!(
        out.status.success(),
        "compiled program faulted: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

/// The C program prints `entry = <show>`; assert it equals the interpreter.
fn assert_matches(src: &str, entry: &str) {
    let expected = format!("{entry} = {}", interp_show(src, entry));
    assert_eq!(c_run(src, entry), expected);
}

/// Read a single-module example from the corpus and assert `entry` matches.
fn assert_example(file: &str, entry: &str) {
    let path = format!("{}/../../examples/{file}", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).expect("read example");
    assert_matches(&src, entry);
}

#[test]
fn arithmetic_and_precedence() {
    let src = "@mod T\n\
               $ a = 1 + 2 * 3 - 4\n\
               $ b : Real = 100.123 % 7\n\
               $ test : Int = a\n";
    assert_matches(src, "a");
    assert_matches(src, "b");
    assert_matches(src, "test");
}

#[test]
fn same_module_overload_dispatches_by_type() {
    // Two overloads of `kind` in one module (type-mangled globals). The C backend
    // must dispatch `kind true` to the Bool body just as the interpreter does.
    let src = "@mod M\n\
               $ kind : Int -> Int = \\x = 1\n\
               $ kind : Bool -> Int = \\b = 2\n\
               $ test : Int = (kind 7) + (kind true) * 10\n";
    assert_matches(src, "test");
}

#[test]
fn ctx_implicit_dictionary_passing() {
    // `@ctx` implicits elaborate to leading dictionary-passing arguments; the C
    // backend must inject them exactly as the interpreter does.
    let src = "@mod M\n\
               $ cmp : Int -> Int -> Bool = \\a b = a ?> b\n\
               $ lt : Int -> Int -> Bool = \\a b = a ?< b\n\
               $ max_of : a -> a -> a  @ctx cmp : a -> a -> Bool = \\x y =\n\
               \tif cmp x y => x else y\n\
               $ test : Int = (max_of 3 7) + (max_of 3 7 @ctx lt)\n";
    assert_matches(src, "test");
}

#[test]
fn type_splice_with() {
    // `with` copies struct fields and union variants into a new type; the C
    // backend must lay them out and match them exactly as the interpreter does.
    let src = "@mod M\n\
               $ Point : @struct = x: Int, y: Int\n\
               $ Point3 : @struct = with Point, z: Int\n\
               $ Base : @union = Red: {}, Green: {}\n\
               $ Color : @union = with Base, Blue: {}\n\
               $ rank : Color -> Int = \\c =\n\
               \tis c | Color.Red => 1 | Color.Green => 2 | Color.Blue => 3\n\
               $ test : Int =\n\
               \tlet p = Point3.{ .x = 1, .y = 2, .z = 3 } in\n\
               \t(p.x + p.y + p.z) + rank Color.Blue\n";
    assert_matches(src, "test");
}

#[test]
fn open_row_record_param() {
    // Row-polymorphic record param over the C backend: field access resolves the
    // same by-name as the interpreter, regardless of the concrete struct passed.
    let src = "@mod M\n\
               $ Point  : @struct = x: Int, y: Int,\n\
               $ Point3 : @struct = x: Int, y: Int, z: Int,\n\
               $ area : { x: Int, y: Int | r } -> Int = \\p = p.x * p.y\n\
               $ test : Int = (area Point.{ .x=3, .y=4 }) + (area Point3.{ .x=5, .y=6, .z=9 })\n";
    assert_matches(src, "test");
}

#[test]
fn anonymous_record_values() {
    // Records under an open row (name-keyed) and pair decay (positional) must build
    // and read the same on the C backend as the interpreter.
    let src = "@mod M\n\
               $ area : { x: Int, y: Int | r } -> Int = \\p = p.x * p.y\n\
               $ shift : { x: Int | r } -> { x: Int | r } = \\p = { .x = p.x + 10 | p }\n\
               $ add : {x: Int, y: Int} -> Int = x + y\n\
               $ test : Int =\n\
               \t(area { .x = 2, .y = 5, .tag = 7 }) + (area (shift { .x = 1, .y = 4 })) + add { .x = 5, .y = 6 }\n";
    assert_matches(src, "test");
}

#[test]
fn record_destructuring_pattern() {
    // Record patterns lower to name-keyed struct matches; the C backend must
    // destructure them the same as the interpreter.
    let src = "@mod M\n\
               $ area : { x: Int, y: Int | r } -> Int = \\p = is p | { .x = a, .y = b, .._ } => a * b\n\
               $ sumxy : { x: Int, y: Int | r } -> Int = \\{ .x, .y } = x + y\n\
               $ test : Int = area { .x = 3, .y = 4, .tag = 9 } + sumxy { .x = 5, .y = 6 }\n";
    assert_matches(src, "test");
}

#[test]
fn codata_stream() {
    // Codata desugars to a record of thunks + apply-unit observation; the C
    // backend must drive the lazy infinite stream the same as the interpreter.
    let src = "@mod M\n\
               $ Stream : @codata t = head : t, tail : Stream t,\n\
               $ from : Int -> Stream Int = \\n = { .head = n, .tail = from (n + 1) }\n\
               $ smap : (a -> b) -> Stream a -> Stream b = \\f s = { .head = f s.head, .tail = smap f s.tail }\n\
               $ nth : Int -> Stream t -> t = \\n s = if n ?= 0 => s.head else nth (n - 1) s.tail\n\
               $ dbl : Int -> Int = \\x = x + x\n\
               $ test : Int = nth 4 (smap dbl (from 1))\n";
    assert_matches(src, "test");
}

#[test]
fn recursion_fib() {
    let src = "@mod T\n\
               $ fib : Int -> Int = \\n =\n\
               \tif n ?= 0 => 0 else if n ?= 1 => 1 else (fib (n-1)) + (fib (n-2))\n\
               $ test : Int = fib 15\n";
    assert_matches(src, "test");
}

#[test]
fn higher_order_and_let() {
    let src = "@mod T\n\
               $ apply2 : (Int -> Int) -> Int -> Int = \\f x = f (f x)\n\
               $ test : Int =\n\
               \tlet inc = \\x = x + 1\n\
               \t in apply2 inc 40\n";
    assert_matches(src, "test");
}

#[test]
fn structs_and_fields() {
    let src = "@mod T\n\
               $ Point : @struct = x: Int, y: Int,\n\
               $ p : Point = Point.{ .x = 3, .y = 4 }\n\
               $ sx : Int = p.x + p.y\n\
               $ test : Int = sx\n";
    assert_matches(src, "p");
    assert_matches(src, "test");
}

#[test]
fn variants_and_when() {
    let src = "@mod T\n\
               $ Shape : @union = Dot: {}, Seg: { Int }\n\
               $ size : Shape -> Int = \\s =\n\
               \tis s | Shape.Dot => 0 | Shape.Seg.{n} => n\n\
               $ test : Int = size (Shape.Seg.{7}) - size Shape.Dot\n";
    assert_matches(src, "test");
}

#[test]
fn lists_and_length() {
    let src = "@mod T\n\
               $ len : List t -> Int =\n\
               \tlet helper : List t -> Int -> Int = \\l n =\n\
               \t\tis l | List.Nil => n | List.Cons.{_, xs} => helper xs (n + 1)\n\
               \t in \\l = helper l 0\n\
               $ xs : List Int = List.Cons.{1, List.Cons.{2, List.Cons.{3, List.Nil}}}\n\
               $ test : Int = len xs\n";
    assert_matches(src, "test");
}

#[test]
fn strings_and_arrays() {
    let src = "@mod T\n\
               $ s : Str = \"ab\" ++ \"cd\"\n\
               $ n : Int = array_len s\n\
               $ g : Int = array_get s 1\n\
               $ test : Int = n + g\n";
    assert_matches(src, "s");
    assert_matches(src, "n");
    assert_matches(src, "test");
}

#[test]
fn tuples() {
    let src = "@mod T\n\
               $ t = {1, 2, 3}\n\
               $ mid : Int = t.1\n\
               $ test : Int = mid\n";
    assert_matches(src, "t");
    assert_matches(src, "test");
}

// -- algebraic effects (CEK driver) ----------------------------------------

#[test]
fn effects_generator_and_state() {
    // sumGen resumes once per yield (42); runState threads state (21); the
    // exception clause never resumes (-1).
    assert_example("EFFECTS.thx", "r_gen");
    assert_example("EFFECTS.thx", "r_state");
    assert_example("EFFECTS.thx", "r_exn_bad");
    assert_example("EFFECTS.thx", "test");
}

#[test]
fn effects_same_op_name_overload() {
    assert_example("EFFECT_OVERLOAD.thx", "readEnv");
    assert_example("EFFECT_OVERLOAD.thx", "cfg");
}

#[test]
fn effects_coroutines_cross_context_resume() {
    // A continuation captured in `spawn`'s handler, stored in `Susp`, and resumed
    // later from `rr` (a different dynamic context).
    assert_example("COROUTINES.thx", "scheduled");
}

#[test]
fn effects_defer_finalization() {
    // Normal completion, abort (handler drops the continuation), LIFO nesting,
    // and a stored continuation whose cleanup runs when it finally completes.
    assert_example("FINALLY.thx", "r_normal");
    assert_example("FINALLY.thx", "r_abort");
    assert_example("FINALLY.thx", "r_nested");
    assert_example("FINALLY.thx", "r_stored");
}

#[test]
fn effects_pipes_and_seq() {
    assert_example("PIPES.thx", "r_seq");
    assert_example("PIPES.thx", "test");
}

// -- FFI marshalling (sized numerics + Ptr) --------------------------------

#[test]
fn sized_extern_marshalling() {
    // A foreign binding with sized / pointer / float32 arguments emits each
    // argument's exact C ABI type in its wrapper (not a word-size fallback).
    let src = "@mod T\n\
               $ f : Int8 -> Int32 -> Nat16 -> Real32 -> Ptr -> Real = @extern \"C\" \"f\" \"libx\"\n\
               $ test : Int8 -> Int32 -> Nat16 -> Real32 -> Ptr -> Real = f\n";
    let lowered = lower(src);
    let code = ccg::emit(&lowered, "test", utilities::Target::host());
    // The wrapper's symbol declaration carries the exact C ABI types, in order.
    let decl = code
        .lines()
        .find(|l| l.contains("__asm__(\"f\")"))
        .expect("the `f` wrapper declaration");
    assert!(
        decl.contains("double THx_sym_0(int8_t, int32_t, uint16_t, float, void*)"),
        "wrong C ABI signature: {decl}"
    );
    // Real32 narrows the double slot to a float; a sized int casts to its width.
    assert!(code.contains("float a3 = (float)THxVALUE_as_num(args[3]);"));
    assert!(code.contains("int8_t a0 = (int8_t)THxVALUE_as_int(args[0]);"));
}

#[test]
fn sized_extern_runs_and_matches() {
    // A real libc call through a sized signature (`strlen : Str -> Nat64`, wrapped
    // as `uint64_t(char*)`) runs and agrees with the interpreter's host table.
    let src = "@mod T\n\
               $ strlen : Str -> Nat64 = @extern \"C\" \"strlen\" \"libc\"\n\
               $ test : Nat64 = strlen \"hello\"\n";
    assert_matches(src, "test");
}
