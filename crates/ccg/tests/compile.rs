//! End-to-end backend tests: lower a program, emit C, compile it with the
//! system C compiler, run it, and check the output matches the interpreter.
//!
//! These need a C compiler (`$CC`, else `cc`) and `-pthread`, both present in the
//! project dev shell.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use frontend::lowering::data::Program;
use frontend::{lower_program, Checker, Decls, Resolved};

/// The implicitly imported CORE module (defines `to_string` and the `+ - * / %`
/// operator overloads). The driver injects it into every program; the harness
/// must too, or arithmetic is unbound.
const CORE_SRC: &str = include_str!("../../../library/CORE.thx");

/// Copy every resolution a checker produced into the shared `resolved` the
/// lowering consumes. Mirrors the driver's per-module merge.
fn collect_resolved(checker: &Checker, resolved: &mut Resolved) {
    let (exprs, pats) = checker.array_nodes();
    resolved.array_exprs.extend(exprs.iter().copied());
    resolved.array_pats.extend(pats.iter().copied());
    resolved.tensor_exprs.extend(checker.tensor_nodes().iter().copied());
    for (&site, names) in checker.promotions() { resolved.promotions.insert(site, names.clone()); }
    for (&site, n) in checker.struct_lit_names() { resolved.struct_lit_names.insert(site, n.clone()); }
    let (clits, obs) = checker.codata_sites();
    resolved.codata_lits.extend(clits.iter().copied());
    resolved.observations.extend(obs.iter().copied());
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
    let module = checker.module_name().to_string();
    for (name, spec) in checker.own_externs() {
        resolved
            .externs
            .insert((module.clone(), name.to_string()), spec.clone());
    }
    for (name, layout) in checker.crepr_layouts() {
        resolved.crepr_layouts.insert(name.to_string(), layout.clone());
    }
}

/// Parse, check, and lower `src` with CORE injected. Returns the lowered modules
/// root-first (the user module, then CORE), the order `ccg::emit` expects.
fn lower(src: &str) -> Vec<Program> {
    let ast = frontend::Ast::new();
    let (ast, core_prog) = frontend::parse_into(ast, CORE_SRC).expect("parse CORE");
    let (ast, user_prog) = frontend::parse_into(ast, src).expect("parse");

    let mut core_checker = Checker::new(&ast);
    core_checker.check_program(&core_prog).expect("check CORE");
    let mut user_checker = Checker::new(&ast);
    user_checker.import_from(&core_checker);
    user_checker.check_program(&user_prog).expect("check");

    let mut resolved = Resolved::default();
    collect_resolved(&core_checker, &mut resolved);
    collect_resolved(&user_checker, &mut resolved);

    let programs = [core_prog, user_prog];
    let decls = Decls::collect(&ast, &programs);
    // Root (the user module) first, then CORE.
    vec![
        lower_program(&ast, &programs[1], &decls, &resolved),
        lower_program(&ast, &programs[0], &decls, &resolved),
    ]
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
    let code = ccg::emit(&lowered, entry, frontend::EntryKind::Value, utilities::Target::host());

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

/// Emit C for a C-style function `main` of the given kind, compile it, run it
/// with `args` (appended after the program path), and return `(exit_code,
/// stdout)`.
fn c_run_entry(src: &str, kind: frontend::EntryKind, args: &[&str]) -> (i32, String) {
    static SEQ: AtomicUsize = AtomicUsize::new(1_000_000);
    let lowered = lower(src);
    let code = ccg::emit(&lowered, "main", kind, utilities::Target::host());

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
    assert!(status.success(), "C compile failed for function main");

    let out = Command::new(&bin_path)
        .args(args)
        .output()
        .expect("run compiled program");
    let _ = std::fs::remove_file(&c_path);
    let _ = std::fs::remove_file(&bin_path);
    let exit = out.status.code().expect("exit code");
    (exit, String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// A C-style `main : {} -> <| e> Int` returns its `Int` as the process exit code,
/// and prints nothing on its own.
#[test]
fn entry_unit_fn_exit_code() {
    let src = "@mod MAIN\n$ main : {} -> <| e> Int = \\u = 42\n";
    let (exit, stdout) = c_run_entry(src, frontend::EntryKind::UnitFn, &[]);
    assert_eq!(exit, 42);
    assert_eq!(stdout, "");
}

/// A C-style `main : [n]Str -> <| e> Int` receives argv (path first) as a `[n]Str`.
#[test]
fn entry_argv_fn_string_vector() {
    let src = "@mod MAIN\n\
               $ main : [n]Str -> <| e> Int = \\args = @tensor_length args\n";
    let (exit, _stdout) = c_run_entry(src, frontend::EntryKind::ArgvFn, &["alpha", "beta"]);
    assert_eq!(exit, 3);
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
fn runtime_operators_match_interpreter_natively() {
    // Every operator whose native implementation lives in runtime.c (`arith`,
    // `compare`, `concat`). A gap in the C backend's builtin dispatch (the class
    // of bug where `^` was wired for the interpreter but not the C runtime) makes
    // the compiled program disagree with the interpreter or fail to build.
    let cases: &[&str] = &[
        "1 + 2",
        "7 - 2",
        "4 * 3",
        "13 / 4",
        "13 % 5",
        "2 ^ 10",
        "if 3 ?= 3 => 1 else 0",
        "if 5 ?> 3 => 1 else 0",
        "if 3 ?< 5 => 1 else 0",
        "if 3 <= 3 => 1 else 0",
        "if 5 >= 4 => 1 else 0",
        "\"a\" ++ \"b\"",
        // Real `^` goes through libm `pow`; the comparison keeps the result an Int
        // so no real-formatting difference can enter the comparison.
        "if 2.0 ^ 3.0 ?> 7.0 => 1 else 0",
    ];
    for body in cases {
        let src = format!("@mod M\n$ a = {body}");
        assert_matches(&src, "a");
    }
}

#[test]
fn arithmetic_intrinsics_match_interpreter() {
    // The arithmetic primitives must compute identically in the C backend and the
    // interpreter. Integer ops yield Int directly; float ops fold to an Int so no
    // real-formatting difference between the engines can enter the comparison.
    for body in [
        "@iadd 2 3",
        "@isub 10 4",
        "@imul 6 7",
        "@idiv 13 4",
        "@imod 13 4",
        "@udiv 13 4",
        "@umod 13 4",
        "@udiv (@isub 0 1) 2", // unsigned: reads the all-ones bits as u64::MAX
        "@umod (@isub 0 1) 3",
        "if @fadd 1.5 2.0 ?= 3.5 => 1 else 0",
        "if @fsub 5.0 1.5 ?= 3.5 => 1 else 0",
        "if @fmul 2.0 3.5 ?= 7.0 => 1 else 0",
        "if @fdiv 3.0 2.0 ?= 1.5 => 1 else 0",
        "if @fmod 7.0 3.0 ?= 1.0 => 1 else 0",
    ] {
        assert_matches(&format!("@mod M\n$ a = {body}"), "a");
    }
}

#[test]
fn float32_intrinsics_match_interpreter() {
    // The `@f32*` family rounds to single precision and yields a Real32. These
    // return the value directly (not folded to an Int), so the comparison also
    // pins the native `fmt_real32` shortest-round-trip display to Rust's
    // `f32::to_string`. `@f32add 16777216.0 1.0` loses the +1 (2^24 + 1 is not an
    // f32), and `0.1 + 0.2` at f32 is exactly `0.3`, unlike the f64 result.
    for body in [
        "@f32add 1.5 2.0",
        "@f32add 16777216.0 1.0",
        "@f32add 0.1 0.2",
        "@f32sub 5.0 1.5",
        "@f32mul 2.0 3.5",
        "@f32div 1.0 3.0",
        "@f32mod 7.0 3.0",
    ] {
        assert_matches(&format!("@mod M\n$ a = {body}"), "a");
    }
}

#[test]
fn scalar_serialization_matches_interpreter() {
    // `to_string` / `from_string` are pure CORE (byte-level `@array_*` plus
    // arithmetic), so the C backend must render and parse scalars identically to
    // the interpreter. `to_string` cases also pin the shared textual encoding.
    for (body, entry) in [
        ("$ a : Str = to_string 1234", "a"),
        ("$ a : Str = to_string (0 - 42)", "a"),
        ("$ a : Str = to_string (let n : Nat = 250 in n)", "a"),
        ("$ a : @bool = (from_string (to_string 1234)) ?= 1234", "a"),
        (
            "$ a : @bool = let n : @int32 = from_string \"77\" in (to_string n) ?= \"77\"",
            "a",
        ),
        ("$ a : @bool = (from_string \"true\") ?= (1 ?= 1)", "a"),
        // Floats serialize via the C runtime seam (`thx_real_to_str` /
        // `thx_f32_to_str` + libc `atof`); the shortest-decimal rule must render
        // identically on both engines.
        ("$ a : Str = to_string (let x : @float64 = 3.5 in x)", "a"),
        ("$ a : Str = to_string (let x : @float32 = from_string \"0.5\" in x)", "a"),
        (
            "$ a : @bool = let x : @float64 = 2.25 in (from_string (to_string x)) ?= x",
            "a",
        ),
    ] {
        assert_matches(&format!("@mod M\n{body}"), entry);
    }
}

#[test]
fn float_mixed_width_matches_interpreter() {
    // `@float32 + @float64` widens via the `thx_f2d` runtime conversion and must
    // produce the same `@float64` result on the C backend as the interpreter.
    let src = "@mod M\n\
               $ x : @float32 = from_string \"1.5\"\n\
               $ y : @float64 = from_string \"2.25\"\n\
               $ fwd : @float64 = x + y\n\
               $ rev : @float64 = y + x\n";
    assert_matches(src, "fwd");
    assert_matches(src, "rev");
}

#[test]
fn user_operator_overload_matches_interpreter() {
    // A user `+` overload for a struct must lower to the user global on the C
    // backend too (resolved via the runtime string-keyed global table), while the
    // builtin `Int + Int` inside stays the builtin.
    let src = "@mod M\n\
               $ V : @struct = x: Int, y: Int\n\
               $ (+) : V -> V -> V = \\a b = V.{ .x = a.x + b.x, .y = a.y + b.y }\n\
               $ r : Int = let s = V.{ .x = 1, .y = 2 } + V.{ .x = 10, .y = 20 } in s.x + s.y";
    assert_matches(src, "r");
}

#[test]
fn native_program_always_declares_libm() {
    // runtime.c's `arith` calls `pow` (real `^`), so every native binary must link
    // libm. The emitter declares it even for a program that uses no math extern;
    // without it a minimal build fails to link `pow`. This asserts the driver's
    // link path (which, unlike `c_run` here, does not hardcode `-lm`) stays sound.
    let lowered = lower("@mod M\n$ main : Int = 1 + 1");
    let emitted = ccg::emit_program(
        &lowered,
        "main",
        frontend::EntryKind::Value,
        utilities::Target::host(),
    );
    assert!(
        emitted.libraries.iter().any(|l| l == "m"),
        "emitted libraries must include libm; got {:?}",
        emitted.libraries
    );
}

#[test]
fn cast_between_integer_widths() {
    // `@cast` reinterprets an integer at another width. It is erased after type
    // checking (integers are boxed uniformly), so both engines must agree: widen
    // `@int32 -> Int`, narrow `Int -> @int32`, and use the results in arithmetic.
    let src = "@mod T\n\
               $ small : {} -> @int32 = \\u = 65\n\
               $ widened : Int = @cast (small {})\n\
               $ narrowed : @int32 = @cast (widened + 1)\n\
               $ back : Int = @cast narrowed\n\
               $ test : Int = widened + back\n";
    assert_matches(src, "widened");
    assert_matches(src, "back");
    assert_matches(src, "test");
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
fn short_circuit_and_or() {
    // `&&`/`||` desugar to a lazy `if`; the C backend matches the interpreter.
    let src = "@mod T\n\
               $ f : @bool = @false\n\
               $ t : @bool = @true\n\
               $ a : Int = if (t && 3 ?< 5) => 1 else 0\n\
               $ b : Int = if (f || 5 ?< 3) => 1 else 0\n\
               $ test : Int = a\n";
    assert_matches(src, "a");
    assert_matches(src, "b");
}

#[test]
fn same_module_overload_dispatches_by_type() {
    // Two overloads of `kind` in one module (type-mangled globals). The C backend
    // must dispatch `kind true` to the @bool body just as the interpreter does.
    let src = "@mod M\n\
               $ kind : Int -> Int = \\x = 1\n\
               $ kind : @bool -> Int = \\b = 2\n\
               $ test : Int = (kind 7) + (kind @true) * 10\n";
    assert_matches(src, "test");
}

#[test]
fn ctx_implicit_dictionary_passing() {
    // `@ctx` implicits elaborate to leading dictionary-passing arguments; the C
    // backend must inject them exactly as the interpreter does.
    let src = "@mod M\n\
               $ cmp : Int -> Int -> @bool = \\a b = a ?> b\n\
               $ lt : Int -> Int -> @bool = \\a b = a ?< b\n\
               $ max_of : a -> a -> a  @ctx cmp : a -> a -> @bool = \\x y =\n\
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
fn ffi_struct_by_value_return() {
    // libc's `ldiv_t ldiv(long, long)` returns a struct BY VALUE. The C backend
    // emits a `typedef struct` and rebuilds the Thrax struct from the C result,
    // byte-identical to the interpreter.
    let src = "@mod M\n\
        $ LDivT : @struct @extern \"C\" = quot: Int, rem: Int,\n\
        $ ldiv : {numer: Int, denom: Int} -> LDivT = @extern \"C\" \"ldiv\" \"libc\"\n\
        $ test : Int = let d = ldiv {17, 5} in d.quot * 100 + d.rem";
    assert_matches(src, "test");
}

#[test]
fn ffi_struct_by_value_argument() {
    // A struct passed BY VALUE to a C function. Compile a helper `.so`, then check
    // the C backend builds the C struct from the Thrax value and matches the
    // interpreter (which dlopens the same library).
    use std::io::Write;
    let dir = std::env::temp_dir();
    let c_path = dir.join("thx_ccg_arg_helper.c");
    let so_path = dir.join("libthx_ccg_arg_helper.so");
    std::fs::File::create(&c_path)
        .unwrap()
        .write_all(b"typedef struct { long x, y; } P;\nlong sum_p(P p) { return p.x * 1000 + p.y; }\n")
        .unwrap();
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    assert!(Command::new(&cc)
        .args(["-shared", "-fPIC", "-O2", "-o"])
        .arg(&so_path)
        .arg(&c_path)
        .status()
        .expect("cc helper")
        .success());

    let src = format!(
        "@mod M\n\
         $ P : @struct @extern \"C\" = x: Int, y: Int,\n\
         $ sum_p : P -> Int = @extern \"C\" \"sum_p\" \"{}\"\n\
         $ test : Int = sum_p (P.{{ .x = 40, .y = 2 }})",
        so_path.display()
    );

    // Interpreter (dlopens the .so via the path in the @extern).
    let expected = interp_show(&src, "test");
    assert_eq!(expected, "40002");

    // C backend: emit, compile linking the helper .so (with an rpath so the
    // binary finds it at runtime), run, compare.
    let lowered = lower(&src);
    let code = ccg::emit(&lowered, "test", frontend::EntryKind::Value, utilities::Target::host());
    let cc_path = dir.join("thx_ccg_arg_prog.c");
    let bin_path = dir.join("thx_ccg_arg_prog.bin");
    std::fs::write(&cc_path, &code).unwrap();
    assert!(Command::new(&cc)
        .args(["-w", "-O1", "-pthread", "-o"])
        .arg(&bin_path)
        .arg(&cc_path)
        .arg("-lm")
        .arg(&so_path)
        .arg(format!("-Wl,-rpath,{}", dir.display()))
        .status()
        .expect("cc prog")
        .success());
    let out = Command::new(&bin_path).output().expect("run prog");
    assert!(out.status.success(), "faulted: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), "test = 40002");
}

#[test]
fn ffi_struct_array() {
    // A `@list T` of C-repr structs passed as a contiguous `T*`. The C backend walks
    // the cons list, packs into a malloc'd buffer, and frees after. Matches interp.
    use std::io::Write;
    let dir = std::env::temp_dir();
    let c_path = dir.join("thx_ccg_arr_helper.c");
    let so_path = dir.join("libthx_ccg_arr_helper.so");
    std::fs::File::create(&c_path)
        .unwrap()
        .write_all(
            b"typedef struct { long x, y; } P;\n\
              long sum_ps(P* a, int n) { long s = 0; for (int i = 0; i < n; i++) s += a[i].x * 10 + a[i].y; return s; }\n",
        )
        .unwrap();
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    assert!(Command::new(&cc)
        .args(["-shared", "-fPIC", "-O2", "-o"])
        .arg(&so_path)
        .arg(&c_path)
        .status()
        .expect("cc helper")
        .success());

    let src = format!(
        "@mod M\n\
         $ P : @struct @extern \"C\" = x: Int, y: Int,\n\
         $ sum_ps : {{ps: @list P, n: Int}} -> Int = @extern \"C\" \"sum_ps\" \"{lib}\"\n\
         $ test : Int = sum_ps {{[P.{{ .x = 1, .y = 2 }}, P.{{ .x = 3, .y = 4 }}, P.{{ .x = 5, .y = 6 }}], 3}}",
        lib = so_path.display()
    );
    assert_eq!(interp_show(&src, "test"), "102");

    let lowered = lower(&src);
    let code = ccg::emit(&lowered, "test", frontend::EntryKind::Value, utilities::Target::host());
    let cc_path = dir.join("thx_ccg_arr_prog.c");
    let bin_path = dir.join("thx_ccg_arr_prog.bin");
    std::fs::write(&cc_path, &code).unwrap();
    assert!(Command::new(&cc)
        .args(["-w", "-O1", "-pthread", "-o"])
        .arg(&bin_path)
        .arg(&cc_path)
        .arg("-lm")
        .arg(&so_path)
        .arg(format!("-Wl,-rpath,{}", dir.display()))
        .status()
        .expect("cc prog")
        .success());
    let out = Command::new(&bin_path).output().expect("run prog");
    assert!(out.status.success(), "faulted: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), "test = 102");
}

/// Resolve libffi compile/link flags for the generated closure runtime: prefer
/// `pkg-config`, else the dev shell's `$LIBFFI_DEV`/`$LIBFFI`. `None` means libffi
/// is unavailable (a bare `cargo test` outside the dev shell), so the callback C
/// test skips rather than failing on a missing `ffi.h`.
fn libffi_flags() -> Option<(Vec<String>, Vec<String>)> {
    if let Ok(out) = Command::new("pkg-config").args(["--cflags", "libffi"]).output() {
        if out.status.success() {
            let cflags = String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .map(String::from)
                .collect();
            let libs = Command::new("pkg-config")
                .args(["--libs", "libffi"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .split_whitespace()
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_else(|| vec!["-lffi".to_string()]);
            return Some((cflags, libs));
        }
    }
    match (std::env::var("LIBFFI_DEV"), std::env::var("LIBFFI")) {
        (Ok(dev), Ok(lib)) => Some((
            vec![format!("-I{dev}/include")],
            vec![
                format!("-L{lib}/lib"),
                format!("-Wl,-rpath,{lib}/lib"),
                "-lffi".to_string(),
            ],
        )),
        _ => None,
    }
}

#[test]
fn ffi_callback() {
    let Some((ffi_cflags, ffi_libs)) = libffi_flags() else {
        eprintln!("skipping ffi_callback: libffi not found (need pkg-config or $LIBFFI)");
        return;
    };
    // A Thrax closure passed to C as a function pointer, via the generated
    // libffi-closure runtime. The helper calls it twice; the closure captures a
    // free variable. Must match the interpreter.
    use std::io::Write;
    let dir = std::env::temp_dir();
    let c_path = dir.join("thx_ccg_cb_helper.c");
    let so_path = dir.join("libthx_ccg_cb_helper.so");
    std::fs::File::create(&c_path)
        .unwrap()
        .write_all(b"int call_twice(int (*f)(int, int)) { return f(1, 2) * 100 + f(3, 4); }\n")
        .unwrap();
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    assert!(Command::new(&cc)
        .args(["-shared", "-fPIC", "-O2", "-o"])
        .arg(&so_path)
        .arg(&c_path)
        .status()
        .expect("cc helper")
        .success());

    let src = format!(
        "@mod M\n\
         $ call_twice : (Int -> Int -> Int) -> Int = @extern \"C\" \"call_twice\" \"{lib}\"\n\
         $ k : Int = 10\n\
         $ test : Int = call_twice (\\a b = a + b + k)",
        lib = so_path.display()
    );
    assert_eq!(interp_show(&src, "test"), "1317");

    let lowered = lower(&src);
    let code = ccg::emit(&lowered, "test", frontend::EntryKind::Value, utilities::Target::host());
    let cc_path = dir.join("thx_ccg_cb_prog.c");
    let bin_path = dir.join("thx_ccg_cb_prog.bin");
    std::fs::write(&cc_path, &code).unwrap();
    let mut cmd = Command::new(&cc);
    cmd.args(["-w", "-O1", "-pthread", "-o"])
        .arg(&bin_path)
        .arg(&cc_path);
    for f in &ffi_cflags {
        cmd.arg(f);
    }
    cmd.arg("-lm");
    for f in &ffi_libs {
        cmd.arg(f);
    }
    cmd.arg(&so_path)
        .arg(format!("-Wl,-rpath,{}", dir.display()));
    assert!(cmd.status().expect("cc prog").success());
    let out = Command::new(&bin_path).output().expect("run prog");
    assert!(out.status.success(), "faulted: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), "test = 1317");
}

#[test]
fn ffi_c_union_by_value() {
    // A C union: `@union @extern "C"` emits a real C `union`; a value built with
    // one member packs just that member (presence-guarded), matching the interpreter.
    use std::io::Write;
    let dir = std::env::temp_dir();
    let c_path = dir.join("thx_ccg_union_helper.c");
    let so_path = dir.join("libthx_ccg_union_helper.so");
    std::fs::File::create(&c_path)
        .unwrap()
        .write_all(
            b"typedef union { long i; double d; } U;\n\
              long u_as_long(U u) { return u.i; }\n\
              U u_from_long(long v) { U u; u.i = v; return u; }\n",
        )
        .unwrap();
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    assert!(Command::new(&cc)
        .args(["-shared", "-fPIC", "-O2", "-o"])
        .arg(&so_path)
        .arg(&c_path)
        .status()
        .expect("cc helper")
        .success());

    let src = format!(
        "@mod M\n\
         $ U : @union @extern \"C\" = i: Int, d: Real,\n\
         $ u_as_long : U -> Int = @extern \"C\" \"u_as_long\" \"{lib}\"\n\
         $ u_from_long : Int -> U = @extern \"C\" \"u_from_long\" \"{lib}\"\n\
         $ built : Int = u_as_long (U.{{ .i = 42 }})\n\
         $ back  : Int = (u_from_long 99).i\n\
         $ test  : Int = built * 1000 + back",
        lib = so_path.display()
    );
    assert_eq!(interp_show(&src, "test"), "42099");

    let lowered = lower(&src);
    let code = ccg::emit(&lowered, "test", frontend::EntryKind::Value, utilities::Target::host());
    let cc_path = dir.join("thx_ccg_union_prog.c");
    let bin_path = dir.join("thx_ccg_union_prog.bin");
    std::fs::write(&cc_path, &code).unwrap();
    assert!(Command::new(&cc)
        .args(["-w", "-O1", "-pthread", "-o"])
        .arg(&bin_path)
        .arg(&cc_path)
        .arg("-lm")
        .arg(&so_path)
        .arg(format!("-Wl,-rpath,{}", dir.display()))
        .status()
        .expect("cc prog")
        .success());
    let out = Command::new(&bin_path).output().expect("run prog");
    assert!(out.status.success(), "faulted: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), "test = 42099");
}

#[test]
fn ffi_nested_struct_by_value() {
    // A struct of structs passed and returned by value. The C backend emits nested
    // typedefs and marshals recursively; must match the interpreter.
    use std::io::Write;
    let dir = std::env::temp_dir();
    let c_path = dir.join("thx_ccg_nested_helper.c");
    let so_path = dir.join("libthx_ccg_nested_helper.so");
    std::fs::File::create(&c_path)
        .unwrap()
        .write_all(
            b"typedef struct { long x, y; } P;\n\
              typedef struct { P a; P b; } Seg;\n\
              long seg_sum(Seg s) { return s.a.x + s.a.y*10 + s.b.x*100 + s.b.y*1000; }\n\
              Seg seg_make(long v) { Seg s = {{v, v+1},{v+2, v+3}}; return s; }\n",
        )
        .unwrap();
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    assert!(Command::new(&cc)
        .args(["-shared", "-fPIC", "-O2", "-o"])
        .arg(&so_path)
        .arg(&c_path)
        .status()
        .expect("cc helper")
        .success());

    let src = format!(
        "@mod M\n\
         $ P : @struct @extern \"C\" = x: Int, y: Int,\n\
         $ Seg : @struct @extern \"C\" = a: P, b: P,\n\
         $ seg_sum : Seg -> Int = @extern \"C\" \"seg_sum\" \"{lib}\"\n\
         $ seg_make : Int -> Seg = @extern \"C\" \"seg_make\" \"{lib}\"\n\
         $ test : Int = seg_sum (seg_make 1)",
        lib = so_path.display()
    );
    assert_eq!(interp_show(&src, "test"), "4321");

    let lowered = lower(&src);
    let code = ccg::emit(&lowered, "test", frontend::EntryKind::Value, utilities::Target::host());
    let cc_path = dir.join("thx_ccg_nested_prog.c");
    let bin_path = dir.join("thx_ccg_nested_prog.bin");
    std::fs::write(&cc_path, &code).unwrap();
    assert!(Command::new(&cc)
        .args(["-w", "-O1", "-pthread", "-o"])
        .arg(&bin_path)
        .arg(&cc_path)
        .arg("-lm")
        .arg(&so_path)
        .arg(format!("-Wl,-rpath,{}", dir.display()))
        .status()
        .expect("cc prog")
        .success());
    let out = Command::new(&bin_path).output().expect("run prog");
    assert!(out.status.success(), "faulted: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), "test = 4321");
}

#[test]
fn open_range_stream() {
    // `[lo ...]` lowers to `count_from lo`, an infinite codata stream; the C
    // backend must observe it lazily just like the interpreter. `[lo ... hi]`
    // lowers to `range lo hi`, a finite list.
    // `Stream`, `count_from`, and `range` come from CORE (`[lo ...]` /
    // `[lo ... hi]` lower to them), so the test uses those and defines only `len`.
    let src = "@mod M\n\
               $ len : @list Int -> Int = \\xs = is xs | [] => 0 | h :: t => 1 + len t\n\
               $ s : Stream Int = [3 ...]\n\
               $ test : Int = s.head + s.tail.head + len [1 ... 4]\n";
    assert_matches(src, "test");
}

#[test]
fn open_range_pattern() {
    // `lo ...` is an open range pattern, matching when `lo <= x` (one test).
    let src = "@mod M\n\
               $ sign : Int -> Str = \\n = is n | 0 ... => \"nonneg\" else \"neg\"\n\
               $ test : Str = sign 3\n";
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
               $ len : @list t -> Int =\n\
               \tlet helper : @list t -> Int -> Int = \\l n =\n\
               \t\tis l | [] => n | _ :: xs => helper xs (n + 1)\n\
               \t in \\l = helper l 0\n\
               $ xs : @list Int = [1, 2, 3]\n\
               $ test : Int = len xs\n";
    assert_matches(src, "test");
}

#[test]
fn strings_and_arrays() {
    let src = "@mod T\n\
               $ s : Str = \"ab\" ++ \"cd\"\n\
               $ n : Int = @array_len s\n\
               $ g : Int = @array_get s 1\n\
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

// -- FFI marshalling (sized numerics + @ptr) --------------------------------

#[test]
fn sized_extern_marshalling() {
    // A foreign binding with sized / pointer / float32 arguments emits each
    // argument's exact C ABI type in its wrapper (not a word-size fallback).
    let src = "@mod T\n\
               $ f : {a: @int8, b: @int32, c: @nat16, d: @float32, e: @ptr} -> Real = @extern \"C\" \"f\" \"libx\"\n\
               $ test : {a: @int8, b: @int32, c: @nat16, d: @float32, e: @ptr} -> Real = \\r = f r\n";
    let lowered = lower(src);
    let code = ccg::emit(&lowered, "test", frontend::EntryKind::Value, utilities::Target::host());
    // The wrapper's symbol declaration carries the exact C ABI types, in order.
    let decl = code
        .lines()
        .find(|l| l.contains("__asm__(\"f\")"))
        .expect("the `f` wrapper declaration");
    assert!(
        decl.contains("double THx_sym_0(int8_t, int32_t, uint16_t, float, void*)"),
        "wrong C ABI signature: {decl}"
    );
    // @float32 narrows the double slot to a float; a sized int casts to its width.
    assert!(code.contains("float a3 = (float)THxVALUE_as_num(args[3]);"));
    assert!(code.contains("int8_t a0 = (int8_t)THxVALUE_as_int(args[0]);"));
}

#[test]
fn sized_extern_runs_and_matches() {
    // A real libc call through a sized signature (`strlen : Str -> @nat64`, wrapped
    // as `uint64_t(char*)`) runs and agrees with the interpreter's host table.
    let src = "@mod T\n\
               $ strlen : Str -> @nat64 = @extern \"C\" \"strlen\" \"libc\"\n\
               $ test : @nat64 = strlen \"hello\"\n";
    assert_matches(src, "test");
}
