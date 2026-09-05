//! End-to-end tests: parse and type-check a module (or several), lower it to the
//! IR, and evaluate a named global on the reified-K machine.

use frontend::{lower_program, Checker, Decls, Resolved};

/// The implicitly imported CORE module (defines `to_string`, and now the `+ - *
/// / %` operator overloads). The driver injects it into every program; the test
/// harness must do the same, or arithmetic is unbound.
const CORE_SRC: &str = include_str!("../../../library/CORE.thx");

/// Copy every resolution a checker produced into the shared `resolved` the
/// lowering consumes (type-directed `[..]` nodes, `with` fields, overload/call
/// module qualifiers, externs, ...). Mirrors the driver's per-module merge.
fn collect_resolved(checker: &Checker, resolved: &mut Resolved) {
    let (exprs, pats) = checker.array_nodes();
    resolved.array_exprs.extend(exprs.iter().copied());
    resolved.array_pats.extend(pats.iter().copied());
    resolved.tensor_exprs.extend(checker.tensor_nodes().iter().copied());
    for (&site, names) in checker.promotions() { resolved.promotions.insert(site, names.clone()); }
    for (&site, n) in checker.struct_lit_names() { resolved.struct_lit_names.insert(site, n.clone()); }
    for (&site, (m, n)) in checker.literal_hooks() { resolved.literal_hooks.insert(site, (m.map(str::to_string), n.clone())); }
    for (&site, ((bm, bn), (em, en))) in checker.literal_pattern_hooks() { resolved.literal_pattern_hooks.insert(site, ((bm.map(str::to_string), bn.clone()), (em.map(str::to_string), en.clone()))); }
    for (&site, (m, n)) in checker.sequence_pattern_hooks() { resolved.sequence_pattern_hooks.insert(site, (m.map(str::to_string), n.clone())); }
    let (clits, obs) = checker.codata_sites();
    resolved.codata_lits.extend(clits.iter().copied());
    resolved.observations.extend(obs.iter().copied());
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
        resolved.crepr_layouts.insert(name.to_string(), layout.clone());
    }
}

/// Parse, check, and lower a single module WITHOUT CORE (for entry-point tests
/// that use no CORE names). See `run` for the CORE-injecting path.
fn lower_checked(src: &str, name: &str) -> frontend::lowering::data::Program {
    let parsed = frontend::parse(src).expect("parse");
    let mut checker = Checker::new(&parsed.ast);
    checker
        .check_program(&parsed.program)
        .unwrap_or_else(|e| panic!("{}", e.render(src, name)));
    let mut resolved = Resolved::default();
    collect_resolved(&checker, &mut resolved);
    let decls = Decls::collect(&parsed.ast, std::slice::from_ref(&parsed.program));
    lower_program(&parsed.ast, &parsed.program, &decls, &resolved)
}

/// Lower `src` to the IR with CORE injected and evaluate `name` on the machine.
fn run(src: &str, name: &str) -> String {
    run_modules(&[src], name)
}

/// Like [`run`] for several user modules sharing one `Ast`: CORE is imported into
/// every module, the last user module is the root (also importing the earlier
/// user deps), and all are lowered with the merged resolutions. Mirrors the
/// driver so operators (CORE overloads) and cross-module dispatch resolve.
fn run_modules(user_sources: &[&str], name: &str) -> String {
    let mut ast = frontend::Ast::new();
    let mut programs = Vec::new();
    for src in std::iter::once(&CORE_SRC).chain(user_sources.iter()) {
        let (next, program) = frontend::parse_into(ast, src).expect("parse");
        ast = next;
        programs.push(program);
    }

    let mut core_checker = Checker::new(&ast);
    core_checker
        .check_program(&programs[0])
        .unwrap_or_else(|e| panic!("{}", e.render(CORE_SRC, "CORE")));
    let user_count = programs.len() - 1;
    let mut user_checkers: Vec<Checker> = Vec::new();
    for i in 0..user_count {
        let mut c = Checker::new(&ast);
        c.import_from(&core_checker);
        if i + 1 == user_count {
            for dep in &user_checkers {
                c.import_from(dep);
            }
        }
        c.check_program(&programs[i + 1])
            .unwrap_or_else(|e| panic!("{}", e.render(user_sources[i], name)));
        user_checkers.push(c);
    }

    let mut resolved = Resolved::default();
    collect_resolved(&core_checker, &mut resolved);
    for c in &user_checkers {
        collect_resolved(c, &mut resolved);
    }

    let decls = Decls::collect(&ast, &programs);
    // Root (last) first so its bare names win, matching the driver.
    let root = programs.len() - 1;
    let mut order: Vec<usize> = (0..programs.len()).collect();
    order.sort_by_key(|&i| i != root);
    let lowered: Vec<_> = order
        .iter()
        .map(|&i| lower_program(&ast, &programs[i], &decls, &resolved))
        .collect();
    let ir = frontend::ir::lower_modules(&lowered);
    interpreter::machine::eval(&ir, name).unwrap_or_else(|e| panic!("{}", e.render("", name)))
}

/// Compile a tiny C source to a shared library in a temp dir, returning its path.
fn compile_helper_so(basename: &str, c_src: &str) -> std::path::PathBuf {
    use std::io::Write;
    let dir = std::env::temp_dir();
    let c_path = dir.join(format!("{basename}.c"));
    let so_path = dir.join(format!("lib{basename}.so"));
    std::fs::File::create(&c_path)
        .unwrap()
        .write_all(c_src.as_bytes())
        .unwrap();
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let status = std::process::Command::new(cc)
        .args(["-shared", "-fPIC", "-O2", "-o"])
        .arg(&so_path)
        .arg(&c_path)
        .status()
        .expect("run cc");
    assert!(status.success(), "cc failed for {basename}");
    so_path
}

#[test]
fn ffi_struct_by_value_argument() {
    // A C-repr struct passed BY VALUE to a C function: pack the fields into the
    // flat memory image, hand it to libffi as an aggregate argument. A helper `.so`
    // gives a function that takes a small struct by value.
    let so = compile_helper_so(
        "thx_ffi_arg_helper",
        "typedef struct { long x, y; } P;\n\
         long sum_p(P p) { return p.x * 1000 + p.y; }\n",
    );
    let src = format!(
        "@mod M\n\
         $ P : @struct @extern \"C\" = x: @int, y: @int,\n\
         $ sum_p : P -> @int = @extern \"C\" \"sum_p\" \"{}\"\n\
         $ test : @int = sum_p (P.{{ .x = 40, .y = 2 }})",
        so.display()
    );
    assert_eq!(run(&src, "test"), "40002");
}

#[test]
fn ffi_struct_array() {
    // A `@list T` of C-repr structs passed to C as a contiguous `T*` buffer (with a
    // separate count), the raylib vertex/point/color-buffer shape.
    let so = compile_helper_so(
        "thx_ffi_arr_helper",
        "typedef struct { long x, y; } P;\n\
         long sum_ps(P* a, int n) { long s = 0; for (int i = 0; i < n; i++) s += a[i].x * 10 + a[i].y; return s; }\n",
    );
    let src = format!(
        "@mod M\n\
         $ P : @struct @extern \"C\" = x: @int, y: @int,\n\
         $ sum_ps : {{ps: @list P, n: @int}} -> @int = @extern \"C\" \"sum_ps\" \"{lib}\"\n\
         $ test : @int = sum_ps {{[P.{{ .x = 1, .y = 2 }}, P.{{ .x = 3, .y = 4 }}, P.{{ .x = 5, .y = 6 }}], 3}}",
        lib = so.display()
    );
    // (1*10+2) + (3*10+4) + (5*10+6) = 12 + 34 + 56 = 102.
    assert_eq!(run(&src, "test"), "102");
}

#[test]
fn ffi_callback() {
    // A Thrax closure passed to C as a function pointer. The helper calls it (twice,
    // to exercise repeated invocation); the closure captures a free variable.
    let so = compile_helper_so(
        "thx_ffi_cb_helper",
        "int call_twice(int (*f)(int, int)) { return f(1, 2) * 100 + f(3, 4); }\n",
    );
    let src = format!(
        "@mod M\n\
         $ call_twice : (@int -> @int -> @int) -> @int = @extern \"C\" \"call_twice\" \"{lib}\"\n\
         $ k : @int = 10\n\
         $ test : @int = call_twice (\\a b = a + b + k)",
        lib = so.display()
    );
    // f(1,2)=13, f(3,4)=17 -> 13*100 + 17 = 1317.
    assert_eq!(run(&src, "test"), "1317");
}

#[test]
fn ffi_c_union_by_value() {
    // A C `union` passed and returned by value: build with one member (packed at
    // offset 0), read a member back from a returned union (reinterpreted bytes).
    let so = compile_helper_so(
        "thx_ffi_union_helper",
        "typedef union { long i; double d; } U;\n\
         long u_as_long(U u) { return u.i; }\n\
         U u_from_long(long v) { U u; u.i = v; return u; }\n",
    );
    let src = format!(
        "@mod M\n\
         $ U : @union @extern \"C\" = i: @int, d: Real,\n\
         $ u_as_long : U -> @int = @extern \"C\" \"u_as_long\" \"{lib}\"\n\
         $ u_from_long : @int -> U = @extern \"C\" \"u_from_long\" \"{lib}\"\n\
         $ built : @int = u_as_long (U.{{ .i = 42 }})\n\
         $ back  : @int = (u_from_long 99).i\n\
         $ test  : @int = built * 1000 + back",
        lib = so.display()
    );
    // built = 42, back = 99.
    assert_eq!(run(&src, "test"), "42099");
}

#[test]
fn ffi_nested_struct_by_value() {
    // A struct with struct fields, passed and returned BY VALUE (raylib's
    // `Camera2D`/`Rectangle` shape). `seg_make` returns a nested struct; `seg_sum`
    // takes one; the round-trip exercises nested pack and unpack.
    let so = compile_helper_so(
        "thx_ffi_nested_helper",
        "typedef struct { long x, y; } P;\n\
         typedef struct { P a; P b; } Seg;\n\
         long seg_sum(Seg s) { return s.a.x + s.a.y*10 + s.b.x*100 + s.b.y*1000; }\n\
         Seg seg_make(long v) { Seg s = {{v, v+1},{v+2, v+3}}; return s; }\n",
    );
    let src = format!(
        "@mod M\n\
         $ P : @struct @extern \"C\" = x: @int, y: @int,\n\
         $ Seg : @struct @extern \"C\" = a: P, b: P,\n\
         $ seg_sum : Seg -> @int = @extern \"C\" \"seg_sum\" \"{lib}\"\n\
         $ seg_make : @int -> Seg = @extern \"C\" \"seg_make\" \"{lib}\"\n\
         $ test : @int = seg_sum (seg_make 1)",
        lib = so.display()
    );
    // seg_make 1 = {{1,2},{3,4}}; seg_sum = 1 + 2*10 + 3*100 + 4*1000 = 4321.
    assert_eq!(run(&src, "test"), "4321");
}

#[test]
fn ffi_struct_by_value_return() {
    // libc's `div_t div(int, int)` returns a small struct BY VALUE. A C-repr
    // struct return exercises the whole struct-marshalling path (layout, libffi
    // aggregate return, unpack back to a Thrax struct value).
    let src = "@mod M\n\
        $ LDivT : @struct @extern \"C\" = quot: @int, rem: @int,\n\
        $ ldiv : {numer: @int, denom: @int} -> LDivT = @extern \"C\" \"ldiv\" \"libc\"\n\
        $ test : @int = let d = ldiv {17, 5} in d.quot * 100 + d.rem";
    // 17 / 5 = 3 remainder 2 -> 3*100 + 2.
    assert_eq!(run(src, "test"), "302");
}

#[test]
fn cross_module_overload_dispatches_by_type() {
    // `make` is defined in two modules with different result types. The root's
    // `make 5` must reach P.make (a Box, so `unwrap` reads its field), not Q.make
    // (an @int); the checker resolves it and the canonical `P.make` name carries it.
    let p = "@mod P\n\
             $ Box : @struct = v: @int\n\
             $ make : @int -> Box = \\n = Box.{ .v = n }\n\
             $ unwrap : Box -> @int = \\b = b.v";
    let q = "@mod Q\n$ make : @int -> @int = \\n = n + 100";
    let root = "@mod M\n\
                $ with P\n\
                $ with Q\n\
                $ r : @int = unwrap (make 5)";
    assert_eq!(run_modules(&[p, q, root], "r"), "5");
}

#[test]
fn literal_hook_from_imported_module() {
    // A library module provides a user type and its construction hook; a string
    // literal in the importing module builds that type via the IMPORTED hook.
    let lib = "@mod LIBSTR\n\
               $ Text : @struct = bytes: @array\n\
               $ @compiler_interface_string_literal : @array -> Text = \\b = Text.{ .bytes = b }\n\
               $ size : Text -> @int = \\t = @array_len t.bytes";
    let root = "@mod M\n\
                $ with LIBSTR\n\
                $ greeting : Text = \"hello\"\n\
                $ r : @int = size greeting";
    assert_eq!(run_modules(&[lib, root], "r"), "5");
}

#[test]
fn same_module_overload_dispatches_by_type() {
    // Two overloads of `kind` in ONE module. Before type-mangling the globals both
    // collided under a single `M.kind` key and every call ran the first body
    // (giving 11); now `kind true` reaches the @bool body, so the result is 21.
    let src = "@mod M\n\
               $ kind : @int -> @int = \\x = 1\n\
               $ kind : @bool -> @int = \\b = 2\n\
               $ r : @int = (kind 7) + (kind @true) * 10";
    assert_eq!(run(src, "r"), "21");
}

#[test]
fn ctx_implicit_resolves_by_name_and_type() {
    // `max_of` declares an implicit `cmp`, resolved by name from scope (the global
    // `>`-like `cmp`). The dictionary is injected as a leading argument.
    let src = "@mod M\n\
               $ cmp : @int -> @int -> @bool = \\a b = a ?> b\n\
               $ max_of : a -> a -> a  @ctx cmp : a -> a -> @bool = \\x y =\n\
               \tif cmp x y => x else y\n\
               $ r : @int = max_of 3 7";
    assert_eq!(run(src, "r"), "7");
}

#[test]
fn ctx_implicit_chains_and_overrides() {
    // `max3` passes its own `@ctx cmp` down to `max_of` (local wins), and an
    // explicit `@ctx lt` override flips `max_of` into a min.
    let src = "@mod M\n\
               $ gt : @int -> @int -> @bool = \\a b = a ?> b\n\
               $ lt : @int -> @int -> @bool = \\a b = a ?< b\n\
               $ max_of : a -> a -> a  @ctx cmp : a -> a -> @bool = \\x y =\n\
               \tif cmp x y => x else y\n\
               $ max3 : a -> a -> a -> a  @ctx cmp : a -> a -> @bool = \\x y z =\n\
               \tmax_of (max_of x y) z\n\
               $ chained : @int = max3 3 9 5 @ctx gt\n\
               $ flipped : @int = max_of 3 7 @ctx lt\n\
               $ r : @int = chained + flipped";
    assert_eq!(run(src, "r"), "12");
}

#[test]
fn struct_with_splices_included_fields() {
    // `Point3` copies `Point`'s fields ahead of its own; the positional/field
    // layout must be x, y, then z.
    let src = "@mod M\n\
               $ Point : @struct = x: @int, y: @int\n\
               $ Point3 : @struct = with Point, z: @int\n\
               $ r : @int =\n\
               \tlet p = Point3.{ .x = 1, .y = 2, .z = 3 } in p.x + p.y + p.z";
    assert_eq!(run(src, "r"), "6");
}

#[test]
fn declared_type_params_control_order() {
    // `@struct b a` declares the parameters explicitly, so `Box @int Str` binds
    // b = @int and a = Str (the declared order), not the order the fields mention
    // them. Reading `snd` back (typed `b`, i.e. @int) must give 7.
    let src = "@mod M\n\
               $ Box : @struct b a = fst: a, snd: b\n\
               $ r : @int = let x : Box @int Str = .{ .fst = \"hi\", .snd = 7 } in x.snd";
    assert_eq!(run(src, "r"), "7");
}

#[test]
fn parameterized_alias_picks_which_generic() {
    // `KeyInt` fixes the first parameter, `ValInt` the second; the alias's own
    // parameter fills the one left open. Both instantiate the same `Pair`.
    let src = "@mod M\n\
               $ Pair : @struct a b = fst: a, snd: b\n\
               $ KeyInt : @alias b = Pair @int b\n\
               $ ValInt : @alias a = Pair a @int\n\
               $ p : KeyInt Str = .{ .fst = 3, .snd = \"x\" }\n\
               $ q : ValInt Str = .{ .fst = \"y\", .snd = 9 }\n\
               $ r : @int = p.fst + q.snd";
    assert_eq!(run(src, "r"), "12");
}

#[test]
fn union_with_splices_included_variants() {
    // `Color` copies `Base`'s variants; a match over a copied and a new variant
    // both dispatch by tag.
    let src = "@mod M\n\
               $ Base : @union = Red: {}, Green: {}\n\
               $ Color : @union = with Base, Blue: {}\n\
               $ rank : Color -> @int = \\c =\n\
               \tis c | Color.Red => 1 | Color.Green => 2 | Color.Blue => 3\n\
               $ r : @int = rank Color.Red + rank Color.Blue";
    assert_eq!(run(src, "r"), "4");
}

#[test]
fn open_row_param_accepts_any_matching_struct() {
    // One row-polymorphic function `{ x:@int, y:@int | r } -> @int` accepts several
    // distinct nominal structs, as long as they carry x:@int and y:@int.
    let src = "@mod M\n\
               $ Point  : @struct = x: @int, y: @int,\n\
               $ Point3 : @struct = x: @int, y: @int, z: @int,\n\
               $ area : { x: @int, y: @int | r } -> @int = \\p = p.x * p.y\n\
               $ r : @int = (area Point.{ .x=3, .y=4 }) + (area Point3.{ .x=5, .y=6, .z=9 })";
    assert_eq!(run(src, "r"), "42");
}

#[test]
fn anonymous_records_literal_update_stack() {
    // Anonymous literal into an open row; update (`| p`) preserving shape and stack
    // (`with p`) on an open-row parameter.
    let src = "@mod M\n\
               $ area : { x: @int, y: @int | r } -> @int = \\p = p.x * p.y\n\
               $ shift : { x: @int | r } -> { x: @int | r } = \\p = { .x = p.x + 10 | p }\n\
               $ tag : { x: @int | r } -> { x: @int, tag: @int | r } = \\p = { .tag = 99, with p }\n\
               $ r : @int =\n\
               \t(area { .x = 2, .y = 5, .tag = 7 })\n\
               \t+ (area (shift { .x = 1, .y = 4 }))\n\
               \t+ (tag { .x = 3, .y = 6 }).tag";
    assert_eq!(run(src, "r"), "153"); // 10 + 44 + 99
}

#[test]
fn nominal_struct_update_with_pipe() {
    // Record update on a nominal struct literal uses `| base`: listed fields
    // override, the rest come from `base`. Qualified, bare-inferred, and clone forms.
    let src = "@mod M\n\
               $ P : @struct = a: @int, b: @int, c: @int\n\
               $ base : P = P.{ .a = 1, .b = 2, .c = 3 }\n\
               $ q : P = P.{ .b = 20 | base }\n\
               $ r0 : P = .{ .a = 10 | base }\n\
               $ cl : P = .{ | base }\n\
               $ r : @int = q.a + q.b + q.c + r0.a + cl.c";
    assert_eq!(run(src, "r"), "37"); // (1+20+3) + 10 + 3
}

#[test]
fn sized_tensor_construction_and_modular_index() {
    // `[n]T` is a sized vector; a `[..]` literal's length fixes the size, and
    // `@tensor_index` reads modulo the size (total: index n wraps to 0). Functions may be
    // size-polymorphic (`[n]a`), the size unifying at the call. (`.[..]` surface
    // sugar, which routes through the LA `index`, is covered by the TENSORS corpus.)
    let src = "@mod M\n\
               $ v : [3]@int = [10, 20, 30]\n\
               $ head : [n]a -> a = \\t = @tensor_index t 0\n\
               $ grid : [2][2]@int = [ [1, 2], [3, 4] ]\n\
               $ r : @int = @tensor_index v 1 + @tensor_index v 3 + @tensor_index v 7 + head v + @tensor_index (@tensor_index grid 1) 0";
    // 20 + v[0]=10 + v[1]=20 + head=10 + grid[1][0]=3
    assert_eq!(run(src, "r"), "63");
}

#[test]
fn tensor_size_arithmetic() {
    // `@tensor_concat : [n]a -> [m]a -> [n+m]a` computes the result size forward; the
    // Z/2^64 polynomial normalizer decides `n+n == 2*n` and `n+m == m+n`. (Full
    // library `concat`/`matmul`/`dot`/`transpose` are exercised end-to-end, both
    // engines, by the TENSORS corpus example, which imports `LA`.)
    let src = "@mod M\n\
               $ a : [2]@int = [1, 2]\n\
               $ b : [3]@int = [3, 4, 5]\n\
               $ c : [5]@int = @tensor_concat a b\n\
               $ dup : [n]x -> [2*n]x = \\t = @tensor_concat t t\n\
               $ flip : [n]x -> [m]x -> [m+n]x = \\p q = @tensor_concat p q\n\
               $ d : [4]@int = dup a\n\
               $ r : @int = @tensor_index c 4 + @tensor_index d 3"; // 5 + (dup a = [1,2,1,2])[3]=2
    assert_eq!(run(src, "r"), "7");
}

#[test]
fn generate_length_build_tensors_in_source() {
    // The `@tensor_create`/`@tensor_length` primitives (higher-order: the runtime applies the
    // closure per index) let a tensor op be written in source: here, transpose.
    let src = "@mod M\n\
               $ myT : [m][n]a -> [n][m]a = \\t =\n\
               \t@tensor_create (@tensor_index t 0) (\\j = @tensor_create t (\\i = @tensor_index (@tensor_index t i) j))\n\
               $ a : [2][3]@int = [ [1,2,3], [4,5,6] ]\n\
               $ at : [3][2]@int = myT a\n\
               $ r : @int = @tensor_length a + @tensor_index (@tensor_index at 0) 1 + @tensor_index (@tensor_index at 2) 0"; // 2 + 4 + 3
    assert_eq!(run(src, "r"), "9");
}

#[test]
fn overloadable_index_and_shape_sugar() {
    // `.[..]` desugars to the overloadable `@compiler_interface_indexing` hook, so two
    // local overloads (a tensor one and a custom-type one) both drive `.[..]`,
    // dispatched by receiver type. Also exercises `[m, n]T` shape sugar and `t.[i, j]`.
    let src = "@mod M\n\
               $ @compiler_interface_indexing : [n]a -> @int -> a = \\t i = @tensor_index t i\n\
               $ Box : @struct = base: @int\n\
               $ @compiler_interface_indexing : Box -> @int -> @int = \\b i = b.base + i\n\
               $ g : [2, 2]@int = [ [1, 2], [3, 4] ]\n\
               $ bx : Box = .{ .base = 100 }\n\
               $ r : @int = g.[1, 0] + g.[1].[1] + bx.[5]"; // 3 + 4 + 105
    assert_eq!(run(src, "r"), "112");
}

#[test]
fn multi_axis_slice_syntax() {
    // `..` keeps an axis, a range narrows it, an index reduces it, mixed freely.
    // The checker computes the result shape; all are O(1) strided views.
    let src = "@mod M\n\
               $ @compiler_interface_indexing : [n]a -> @int -> a = \\t i = @tensor_index t i\n\
               $ m : [3, 4]@int = [ [1,2,3,4], [5,6,7,8], [9,10,11,12] ]\n\
               $ colv : [3]@int = m.[.., 1]\n\
               $ blk : [2, 2]@int = m.[1 ... 2, 1 ... 2]\n\
               $ r : @int = colv.[2] + blk.[0, 0] + blk.[1, 1] + m.[0, 1 ... 2].[1]";
    // colv = col1 = [2,6,10], colv[2]=10 ; blk = [[6,7],[10,11]], [0,0]=6, [1,1]=11 ;
    // m.[0,1...2] = [2,3], [1]=3 -> 10+6+11+3 = 30
    assert_eq!(run(src, "r"), "30");
}

#[test]
fn inclusive_range_slice_syntax() {
    // `t.[p ... q]` is an INCLUSIVE leading-axis slice (a view), matching the range
    // pattern syntax `...`. `v.[1 ... 3]` keeps v[1], v[2], v[3].
    let src = "@mod M\n\
               $ @compiler_interface_indexing : [n]a -> @int -> a = \\t i = @tensor_index t i\n\
               $ v : [5]@int = [10, 20, 30, 40, 50]\n\
               $ s : [3]@int = v.[1 ... 3]\n\
               $ r : @int = s.[0] + s.[1] + s.[2]"; // 20+30+40
    assert_eq!(run(src, "r"), "90");
}

#[test]
fn expression_ascription() {
    // `(e : T)` checks `e` against `T` and passes the value through unchanged; the
    // annotation pins an otherwise-ambiguous literal.
    let src = "@mod M\n\
               $ r : @int = (40 : @int) + 2";
    assert_eq!(run(src, "r"), "42");
}

#[test]
fn indexing_hook_returns_non_element() {
    // The `.[..]` hook may return any type, not just the element type: a map-style
    // lookup returns an `Option`-shaped union. Exercises `@compiler_interface_indexing`
    // with a non-element result, dispatched on the receiver type.
    let src = "@mod M\n\
               $ Maybe : @union a = Nada: {}, Just: {a}\n\
               $ Dict : @struct = base: @int\n\
               $ @compiler_interface_indexing : Dict -> @int -> Maybe @int =\n\
               \t\\d k = if k ?< d.base => Maybe.Just.{ d.base + k } else Maybe.Nada\n\
               $ d : Dict = .{ .base = 10 }\n\
               $ r : @int = is d.[3] | Maybe.Just.{v} => v else 0"; // 10 + 3
    assert_eq!(run(src, "r"), "13");
}

#[test]
fn literal_construction_hooks() {
    // A user type opts into each literal kind by defining the matching
    // `@compiler_interface_*` hook; the literal (driven by the expected type) then
    // builds that user type instead of the built-in default.
    let src = "@mod M\n\
        $ MyStr : @struct = bytes: @array\n\
        $ @compiler_interface_string_literal : @array -> MyStr = \\b = MyStr.{ .bytes = b }\n\
        $ Wrap : @struct = n: @int\n\
        $ @compiler_interface_integer_literal : @int -> Wrap = \\x = Wrap.{ .n = x }\n\
        $ RWrap : @struct = r: @float64\n\
        $ @compiler_interface_real_literal : @float64 -> RWrap = \\x = RWrap.{ .r = x }\n\
        $ Bag : @union a = Items: {@vec a}\n\
        $ @compiler_interface_sequence_literal : @vec a -> Bag a = \\v = Bag.Items.{ v }\n\
        $ s : MyStr = \"hi\"\n\
        $ w : Wrap = 42\n\
        $ rw : RWrap = 3.5\n\
        $ bag : Bag @int = [1, 2, 3]\n\
        $ r : @int = w.n + @array_len s.bytes\n\
        \t+ (is bag | Bag.Items.{v} => @vec_len v else 0)"; // 42 + 2 + 3
    assert_eq!(run(src, "r"), "47");
}

#[test]
fn literal_hook_via_ascription() {
    // `(e : T)` also drives a construction hook.
    let src = "@mod M\n\
        $ Wrap : @struct = n: @int\n\
        $ @compiler_interface_integer_literal : @int -> Wrap = \\x = Wrap.{ .n = x }\n\
        $ r : @int = (41 : Wrap).n + 1";
    assert_eq!(run(src, "r"), "42");
}

#[test]
fn literal_hook_does_not_hijack_default() {
    // A string hook is in scope, but a `Str`-typed literal still builds a plain Str:
    // the hook fires only when the expected type is the user type, so the default
    // path (folded to a constant) is untouched.
    let src = "@mod M\n\
        $ MyStr : @struct = bytes: @array\n\
        $ @compiler_interface_string_literal : @array -> MyStr = \\b = MyStr.{ .bytes = b }\n\
        $ plain  : Str   = \"abc\"\n\
        $ custom : MyStr = \"xy\"\n\
        $ r : @int = @array_len plain + @array_len custom.bytes"; // 3 + 2
    assert_eq!(run(src, "r"), "5");
}

#[test]
fn literal_pattern_via_equality_hook() {
    // A literal PATTERN on a user type routes through its construction + equality
    // hooks: `is s | "hi" => ...` builds "hi" into the user type and compares it with
    // `@compiler_interface_equality`.
    let src = "@mod M\n\
        $ MyStr : @struct = bytes: @array\n\
        $ @compiler_interface_string_literal : @array -> MyStr = \\b = MyStr.{ .bytes = b }\n\
        $ @compiler_interface_equality : MyStr -> MyStr -> @bool = \\a b = a.bytes ?= b.bytes\n\
        $ classify : MyStr -> @int = \\s = is s | \"hi\" => 1 | \"bye\" => 2 else 0\n\
        $ r : @int = classify \"hi\" * 100 + classify \"bye\" * 10 + classify \"x\""; // 120
    assert_eq!(run(src, "r"), "120");
}

#[test]
fn sequence_pattern_via_view_hook() {
    // A sequence PATTERN on a user type unfolds its `@compiler_interface_sequence_view`
    // hook: `[]`, `[x]` (fixed length: tail must be empty), and `h :: t` all match.
    let src = "@mod M\n\
        $ Stack : @struct a = items: @list a\n\
        $ @compiler_interface_sequence_view : Stack a -> SeqView (Stack a) a = \\s =\n\
        \tis s.items | h :: t => SeqView.More.{ h, Stack.{ .items = t } } else SeqView.Empty\n\
        $ classify : Stack @int -> @int = \\s = is s\n\
        \t| [] => 0 | [x] => x | h :: t => 100 + h else 999\n\
        $ s0 : Stack @int = Stack.{ .items = [] }\n\
        $ s1 : Stack @int = Stack.{ .items = [7] }\n\
        $ s2 : Stack @int = Stack.{ .items = [3, 4] }\n\
        $ r : @int = classify s0 * 1000 + classify s1 * 100 + classify s2"; // 0+700+103
    assert_eq!(run(src, "r"), "803");
}

#[test]
fn strided_views_transpose_row_col_slice() {
    // Over the flat strided rep, transpose/index/slice are O(1) VIEWS sharing the
    // buffer. A transposed column is a strided view; a slice narrows an axis.
    let src = "@mod M\n\
               $ mA : [3][3]@int = [ [1,2,3], [4,5,6], [7,8,9] ]\n\
               $ tA : [3][3]@int = @tensor_transpose mA\n\
               $ colv : [3]@int = @tensor_index tA 2\n\
               $ sl : [2]@int = @tensor_slice (@tensor_index mA 0) 1 3\n\
               $ r : @int = @tensor_index (@tensor_index tA 0) 2\n\
               \t+ @tensor_index colv 0 + @tensor_index sl 1";
    // tA[0][2] = mA[2][0] = 7 ; colv = column 2 = [3,6,9], colv[0]=3 ; sl=[2,3], sl[1]=3
    assert_eq!(run(src, "r"), "13");
}

#[test]
fn inclusive_range_patterns() {
    // `lo ... hi` matches when lo <= x <= hi, inclusive at both ends. Refutable, so
    // the match needs an `else`. Works on @int and Real.
    let src = "@mod M\n\
               $ grade : @int -> Str = \\n =\n\
               \tis n | 90 ... 100 => \"A\" | 60 ... 89 => \"C\" else \"F\"\n\
               $ band : Real -> @int = \\x = is x | 0.0 ... 1.0 => 1 else 0\n\
               $ r : @int =\n\
               \t(if grade 100 ?= \"A\" => 1 else 0)\n\
               \t+ (if grade 60 ?= \"C\" => 2 else 0)\n\
               \t+ (if grade 40 ?= \"F\" => 4 else 0)\n\
               \t+ (band 0.5) * 8";
    assert_eq!(run(src, "r"), "15"); // 1 + 2 + 4 + 1*8
}

#[test]
fn unit_parameter_thunks_without_a_lambda() {
    // A `{} -> T` definition needs no explicit `\u =`: the unit parameter is
    // introduced automatically (a thunk), so the body runs when it is applied. An
    // explicit `\u =` still works (the arity guard leaves it alone).
    let src = "@mod M\n\
               $ lazy : {} -> @int = 40 + 2\n\
               $ also : {} -> @int = \\u = 40 + 3\n\
               $ r : @int = lazy {} + also {}";
    assert_eq!(run(src, "r"), "85"); // 42 + 43
}

#[test]
fn closed_record_param_named_by_a_lambda() {
    // A closed-record parameter may be named by an explicit lambda and read with
    // field access (`\q = q.y`), instead of the auto-bind sugar. The sugar only
    // fires when the body has fewer leading lambdas than the signature's arity, so
    // it still auto-binds a record ahead of a later explicit lambda parameter.
    let src = "@mod M\n\
               $ label : { y: @int, z: @int } -> @int = \\q = q.y * 100 + q.z\n\
               $ add : { x: @int, y: @int } -> @int = x + y\n\
               $ f : { x: @int, y: @int } -> @int -> @int = \\n = x + y + n\n\
               $ r : @int = label { .y = 2, .z = 3 } + add { .x = 5, .y = 6 } + f {3, 4} 5";
    assert_eq!(run(src, "r"), "226"); // 203 + 11 + 12
}

#[test]
fn record_promotion_and_named_args() {
    // A record parameter can be called positionally (promoted), by name, or by
    // name reordered; a one-field record param accepts a bare scalar.
    let src = "@mod M\n\
               $ add : {x: @int, y: @int} -> @int = x + y\n\
               $ inc : {x: @int} -> @int = x + 1\n\
               $ r : @int =\n\
               \tadd {5, 6} + add { .x = 5, .y = 6 } + add { .y = 6, .x = 5 } + inc 20 + inc { .x = 20 }";
    assert_eq!(run(src, "r"), "75"); // 11 + 11 + 11 + 21 + 21
}

#[test]
fn record_destructuring_pattern() {
    // Destructure a record by field name (match arm, and lambda shorthand), on an
    // open-row value and a nominal struct; `.._` ignores the rest.
    let src = "@mod M\n\
               $ Point : @struct = x: @int, y: @int,\n\
               $ area : { x: @int, y: @int | r } -> @int = \\p = is p | { .x = a, .y = b, .._ } => a * b\n\
               $ sumxy : { x: @int, y: @int | r } -> @int = \\{ .x, .y } = x + y\n\
               $ nx : Point -> @int = \\p = is p | { .x = a } => a\n\
               $ r : @int = area { .x = 3, .y = 4, .tag = 9 } + sumxy { .x = 5, .y = 6 } + nx Point.{ .x = 2, .y = 8 }";
    assert_eq!(run(src, "r"), "25"); // 12 + 11 + 2
}

#[test]
fn generic_struct_satisfies_an_open_row() {
    // A generic struct instance (`Box @int`, `Pair @int Str`) bridges to an open
    // record row by substituting its type arguments for the struct's parameters.
    let src = "@mod M\n\
               $ Box : @struct a = val: a\n\
               $ Pair : @struct a b = fst: a, snd: b\n\
               $ unwrap : { val: v | r } -> v = \\b = b.val\n\
               $ getfst : { fst: a | r } -> a = \\p = p.fst\n\
               $ r : @int = unwrap (Box.{ .val = 42 }) + getfst (Pair.{ .fst = 8, .snd = \"s\" })";
    assert_eq!(run(src, "r"), "50");
}

#[test]
fn record_rest_binds_the_leftover_fields() {
    // `..rest` binds the record minus the listed labels. Concrete case: matching a
    // `Point3` and binding `..rest` yields `{ y, z }`, readable and forwardable.
    // Open case: over `{ x:@int | r }`, `rest` is the polymorphic remainder.
    let src = "@mod M\n\
               $ Point3 : @struct = x: @int, y: @int, z: @int\n\
               $ sum2 : { y:@int, z:@int } -> @int = y + z\n\
               $ split : Point3 -> @int = \\p = is p | { .x = a, ..rest } => a + sum2 rest\n\
               $ drop_x : { x:@int, y:@int | r } -> @int = \\p = is p | { .x = a, ..rest } => rest.y + a\n\
               $ r : @int = split (Point3.{ .x=1, .y=2, .z=3 }) + drop_x (Point3.{ .x=10, .y=20, .z=30 })";
    assert_eq!(run(src, "r"), "36"); // (1 + (2+3)) + (20 + 10)
}

#[test]
fn codata_stream_is_lazy_and_infinite() {
    // A codata stream: construction is finite (thunks), and observing drives the
    // generative recursion lazily, so an infinite stream is fine.
    let src = "@mod M\n\
               $ Stream : @codata t = head : t, tail : Stream t,\n\
               $ from : @int -> Stream @int = \\n = { .head = n, .tail = from (n + 1) }\n\
               $ nth : @int -> Stream t -> t = \\n s = if n ?= 0 => s.head else nth (n - 1) s.tail\n\
               $ r : @int = (from 10).head + nth 5 (from 10)";
    assert_eq!(run(src, "r"), "25"); // 10 + 15
}

#[test]
fn imported_global_does_not_shadow_a_same_named_effect_op() {
    // Module A exports a plain global `get`; module B has a `State` effect whose
    // operation is also `get`. In the combined program B's bare `get` must resolve
    // to the operation, not A's imported global (which aliases into the bare-name
    // fallback). Regression for the cross-module glob-resolution gap.
    let a = "@mod A\n$ get : @int -> @int = \\x = x + 1000";
    let b = "@mod B\n\
        $ State : @effect = get : {} -> @int, put : @int -> {},\n\
        $ getit : {} -> <State> @int = \\u = get {}\n\
        $ run : @int = do getit {} ctl k | State.get u => k 42 | State.put v => k {} else r => r";
    let root = "@mod M\n$ with A\n$ with B\n$ r : @int = B.run";
    assert_eq!(run_modules(&[a, b, root], "r"), "42");
}

#[test]
fn same_named_struct_types_in_two_modules_do_not_collide() {
    // Two modules each declare a `Pair` struct with DIFFERENT fields. A positional
    // struct pattern in B must be lowered against B's own layout, not A's (whose
    // fields differ), which otherwise faults with "no field ...". Regression for
    // the shared-`Decls` type collision.
    let a = "@mod A\n\
        $ Pair : @struct = fst: @int, snd: @int\n\
        $ afst : @int = Pair.{ .fst = 7, .snd = 8 }.fst";
    let b = "@mod B\n\
        $ Pair : @struct = a: @int, b: @int\n\
        $ first : Pair -> @int = \\p = is p | Pair.{ x, y } => x\n\
        $ bfst : @int = first Pair.{ .a = 3, .b = 4 }";
    let root = "@mod M\n$ with A\n$ with B\n$ r : @int = B.bfst";
    assert_eq!(run_modules(&[a, b, root], "r"), "3");
}

#[test]
fn defer_runs_cleanup_on_completion_abort_and_nesting() {
    // A `Y` handler that sums every yielded value; the deferred cleanups perform
    // `Y.yield`, so their effects are observable in the total.
    let prelude = "@mod M\n\
        $ Y : @effect = yield : @int -> {},\n\
        $ Exn : @effect = throw : Str -> a,\n\
        $ sum : ({} -> <Y> @int) -> @int = \
          \\body = do body {} ctl k | Y.yield v => v + k {} else r => r\n";
    // Normal completion: body yields 1 and returns 100, cleanup yields 2 -> 103.
    let normal =
        format!("{prelude}$ r : @int = sum (\\_ = defer Y.yield 2 do (let _ = Y.yield 1 in 100))");
    assert_eq!(run(&normal, "r"), "103");
    // Abort: the inner handler drops the continuation, but the cleanup (yield 9)
    // still runs under the enclosing `Y` handler -> 9.
    let abort = format!(
        "{prelude}$ r : @int = sum (\\_ = \
         do (defer Y.yield 9 do (let _ = Exn.throw \"x\" in 100)) ctl k | Exn.throw e => 0)"
    );
    assert_eq!(run(&abort, "r"), "9");
    // Nested defers run innermost-first: 1 + 2 + 3 = 6.
    let nested = format!(
        "{prelude}$ r : @int = sum (\\_ = \
         defer Y.yield 3 do (defer Y.yield 2 do (let _ = Y.yield 1 in 0)))"
    );
    assert_eq!(run(&nested, "r"), "6");
}

#[test]
fn defer_cleanup_runs_when_a_stored_continuation_completes() {
    // The handler stores the continuation instead of resuming; the cleanup runs
    // only when that continuation is later driven to completion (two steps).
    let src = "@mod M\n\
        $ Co : @effect = step : @int -> {},\n\
        $ Task : @union = Fin: {}, Susp: { {} -> Task },\n\
        $ spawn : ({} -> <Co> {}) -> Task = \
          \\t = do t {} ctl k | step v => Task.Susp.{ k } else _ => Task.Fin.{}\n\
        $ drive : Task -> @int = \
          \\t = is t | Task.Fin.{} => 0 | Task.Susp.{ k } => 1 + drive (k {}) else 0\n\
        $ r : @int = drive (spawn (\\_ = defer step 2 do (let _ = step 1 in {})))";
    assert_eq!(run(src, "r"), "2");
}

#[test]
fn extern_ffi_file_roundtrip() {
    // The `@extern` FFI host table, exercised directly (the `C` namespace is the
    // same bindings injected by the driver). Open for write, put bytes, close;
    // reopen for read, read them back, then remove the file. "hi" is 104 and 105.
    let src = "@mod M\n\
        $ fopen : {path: Str, mode: Str} -> @int = @extern \"C\" \"fopen\" \"libc\"\n\
        $ fputs : {s: Str, stream: @int} -> @int = @extern \"C\" \"fputs\" \"libc\"\n\
        $ fclose : @int -> @int = @extern \"C\" \"fclose\" \"libc\"\n\
        $ fgetc : @int -> @int = @extern \"C\" \"fgetc\" \"libc\"\n\
        $ remove : Str -> @int = @extern \"C\" \"remove\" \"libc\"\n\
        $ p : Str = \"/tmp/thrax_core_roundtrip.txt\"\n\
        $ r : @int = \
          let f = fopen {p, \"wb\"} in \
          fputs {\"hi\", f} ; fclose f ; \
          let g = fopen {p, \"rb\"} in \
          let a = fgetc g in let b = fgetc g in \
          fclose g ; remove p ; a + b";
    assert_eq!(run(src, "r"), "209");
}

#[test]
fn extern_ffi_dynamic_dlopen() {
    // Symbols OUTSIDE the compiled-in host table resolve at runtime via
    // dlopen/dlsym and call through the hand-rolled SysV trampoline. `abs`,
    // `strdup` (libc) and `expm1` (libm) are none of them curated, so each
    // exercises the dynamic path: an integer arg/return, a string arg with a
    // `char*` return decoded back to bytes, and a floating arg/return in xmm.
    let src = "@mod M\n\
        $ iabs   : @int -> @int   = @extern \"C\" \"abs\"    \"libc\"\n\
        $ dup    : Str -> Str   = @extern \"C\" \"strdup\" \"libc\"\n\
        $ expm1r : Real -> Real = @extern \"C\" \"expm1\"  \"libm\"\n\
        $ r : @int = iabs (0 - 7) + @array_len (dup \"abcde\") \
                  + (if expm1r 1.0 ?> 1.7 => 100 else 0)";
    assert_eq!(run(src, "r"), "112");
}

#[test]
fn target_reflects_the_host_consistently() {
    // Host-agnostic invariants: the word and pointer widths agree, and `name` is
    // exactly `arch-os`.
    let src = "@mod M\n$ r : @int = \
        if TARGET.int_bits ?= TARGET.ptr_bits \
        => (if (TARGET.arch ++ \"-\" ++ TARGET.os) ?= TARGET.name => 0 else 1) \
        else 1";
    assert_eq!(run(src, "r"), "0");
}

#[test]
fn array_literal_lowers_to_byte_vector() {
    // `[..]` in @array context builds a byte vector, so array_* primitives apply.
    let src = "@mod M\n$ a : @array = [10, 20, 30]\n\
               $ n = @array_len a\n$ g = @array_get a 1";
    assert_eq!(run(src, "n"), "3");
    assert_eq!(run(src, "g"), "20");
}

#[test]
fn array_patterns_destructure_and_guard() {
    let src = "@mod M\n\
               $ sum2 : @array -> @int = \\a = is a | [x, y] => x + y else 0\n\
               $ r = sum2 [4, 5]\n\
               $ miss = sum2 [1, 2, 3]\n\
               $ lit : @array -> @int = \\a = is a | [1, y] => y else 0\n\
               $ hit = lit [1, 42]\n\
               $ no = lit [2, 42]\n\
               $ head : @array -> @int = \\a = is a | [h, ..rest] => h + @array_len rest else 0\n\
               $ hd = head [7, 8, 9]";
    assert_eq!(run(src, "r"), "9");
    assert_eq!(run(src, "miss"), "0");
    assert_eq!(run(src, "hit"), "42");
    assert_eq!(run(src, "no"), "0");
    assert_eq!(run(src, "hd"), "9");
}

#[test]
fn arithmetic_intrinsics() {
    // The monomorphic primitives the operator overloads will be built on.
    assert_eq!(run("@mod M\n$ a = @iadd 2 3", "a"), "5");
    assert_eq!(run("@mod M\n$ a = @isub 10 4", "a"), "6");
    assert_eq!(run("@mod M\n$ a = @imul 6 7", "a"), "42");
    assert_eq!(run("@mod M\n$ a = @idiv 13 4", "a"), "3");
    assert_eq!(run("@mod M\n$ a = @imod 13 4", "a"), "1");
    // Unsigned div/mod agree with signed for small values, but read the all-ones
    // bit pattern (`@isub 0 1` = every bit set) as u64::MAX, not -1.
    assert_eq!(run("@mod M\n$ a = @udiv 13 4", "a"), "3");
    assert_eq!(run("@mod M\n$ a = @umod 13 4", "a"), "1");
    assert_eq!(
        run("@mod M\n$ a = @udiv (@isub 0 1) 2", "a"),
        "9223372036854775807" // (2^64 - 1) / 2
    );
    assert_eq!(run("@mod M\n$ a = @umod (@isub 0 1) 3", "a"), "0"); // (2^64 - 1) % 3
    assert_eq!(run("@mod M\n$ a = @fadd 1.5 2.0", "a"), "3.5");
    assert_eq!(run("@mod M\n$ a = @fsub 5.0 1.5", "a"), "3.5");
    assert_eq!(run("@mod M\n$ a = @fmul 2.0 3.5", "a"), "7");
    assert_eq!(run("@mod M\n$ a = @fdiv 3.0 2.0", "a"), "1.5");
    assert_eq!(run("@mod M\n$ a = @fmod 7.0 3.0", "a"), "1");
    // The `@f32*` family rounds to single precision. Adding 1 to 2^24 is lost in
    // f32 (the next representable float is 2^24 + 2), where `@fadd` keeps it.
    assert_eq!(run("@mod M\n$ a = @f32add 1.5 2.0", "a"), "3.5");
    assert_eq!(run("@mod M\n$ a = @f32add 16777216.0 1.0", "a"), "16777216");
    assert_eq!(run("@mod M\n$ a = @fadd 16777216.0 1.0", "a"), "16777217");
    assert_eq!(run("@mod M\n$ a = @f32sub 5.0 1.5", "a"), "3.5");
    assert_eq!(run("@mod M\n$ a = @f32mul 2.0 3.5", "a"), "7");
    assert_eq!(run("@mod M\n$ a = @f32div 3.0 2.0", "a"), "1.5");
    assert_eq!(run("@mod M\n$ a = @f32mod 7.0 3.0", "a"), "1");
}

#[test]
fn float32_arithmetic_is_single_precision() {
    // `+` on `@float32` routes through `@f32*`, so arithmetic rounds to single
    // precision: 2^24 + 1 is not representable and falls back to 2^24. The `@fadd`
    // (f64) counterpart in `arithmetic_intrinsics` keeps the extra digit. Operands
    // are `let`-bound at `@float32` (a bare literal defaults to its width only for
    // `@int`/`Real`, an unrelated resolution limitation).
    let f32 = |lhs: &str, rhs: &str| {
        run(
            &format!(
                "@mod M\n$ a : @float32 = \
                 let x : @float32 = {lhs} in let y : @float32 = {rhs} in x + y"
            ),
            "a",
        )
    };
    assert_eq!(f32("16777216.0", "1.0"), "16777216");
    // A single-precision result displays at f32 precision, not the widened f64
    // digits a naive narrow-then-store would show (f64 `0.1 + 0.2` is
    // `0.30000000000000004`).
    assert_eq!(f32("0.1", "0.2"), "0.3");
}

#[test]
fn scalar_serialization_round_trips() {
    // `to_string` / `from_string` are inverse over the integer, `@nat`, sized
    // int/nat, and boolean scalars. They are defined entirely in CORE (byte-level
    // `@array_*` plus arithmetic), not as compiler primitives, so a round trip
    // through text recovers the original value.
    let ok = |src: &str| run(&format!("@mod M\n$ a : @bool = {src}"), "a");

    // @int, including the negative branch.
    assert_eq!(ok("(from_string (to_string 1234)) ?= 1234"), "true");
    assert_eq!(ok("(from_string (to_string (0 - 99))) ?= (0 - 99)"), "true");
    // @nat prints unsigned; round-trip at the same type.
    assert_eq!(
        ok("let n : @nat = from_string (to_string (let m : @nat = 250 in m)) in n ?= 250"),
        "true"
    );
    // A sized width (`@int32`), checked by comparing the rendered text.
    assert_eq!(
        ok("let n : @int32 = from_string \"77\" in (to_string n) ?= \"77\""),
        "true"
    );
    // Booleans render as `true`/`false` and parse back.
    assert_eq!(ok("(from_string \"true\") ?= (1 ?= 1)"), "true");
    assert_eq!(ok("(from_string \"false\") ?= (1 ?= 0)"), "true");
    // A `Str` serializes as itself.
    assert_eq!(ok("(from_string (to_string \"hi\")) ?= \"hi\""), "true");
    // Floats round-trip through their shortest decimal (via the C runtime seam:
    // `thx_real_to_str`/`thx_f32_to_str` + libc `atof`), at both widths.
    assert_eq!(
        ok("let x : @float64 = 3.5 in (from_string (to_string x)) ?= x"),
        "true"
    );
    assert_eq!(
        ok("let x : @float32 = from_string \"0.5\" in (to_string x) ?= \"0.5\""),
        "true"
    );
}

#[test]
fn float_mixed_width_widens_to_float64() {
    // `@float32 + @float64` widens the single-precision operand to double (via the
    // `thx_f2d` runtime conversion) and yields `@float64`; both argument orders
    // resolve. `Real` deliberately does NOT mix (target-dependent width), so
    // operands are the explicit sized types.
    let ok = |src: &str| run(&format!("@mod M\n$ a : @bool = {src}"), "a");
    assert_eq!(
        ok("let x : @float32 = from_string \"1.5\" in \
            let y : @float64 = from_string \"2.25\" in \
            let e : @float64 = 3.75 in (x + y) ?= e"),
        "true"
    );
    assert_eq!(
        ok("let x : @float32 = from_string \"1.5\" in \
            let y : @float64 = from_string \"2.25\" in \
            let e : @float64 = 3.75 in (y + x) ?= e"),
        "true"
    );
}

#[test]
fn int_word_mixes_with_sized() {
    // `@int` (the word default) mixes with a sized signed int: the sized type wins
    // and the word operand casts to it, so a bare literal (which defaults to
    // `@int`) drops into sized arithmetic. Both argument orders resolve. `@nat`
    // mirrors this over the unsigned `@nat*` types.
    let ok = |src: &str| run(&format!("@mod M\n$ a : @bool = {src}"), "a");
    assert_eq!(
        ok("let p : @int32 = from_string \"3\" in \
            let x : @int = 5 in \
            let r : @int32 = x * p in r ?= from_string \"15\""),
        "true"
    );
    assert_eq!(
        ok("let p : @int32 = from_string \"3\" in \
            let x : @int = 5 in \
            let r : @int32 = p * x in r ?= from_string \"15\""),
        "true"
    );
    // The reported real-world case: a bare literal times a sized value.
    assert_eq!(
        ok("let p : @int32 = from_string \"3\" in \
            let r : @int32 = 2 * p in r ?= from_string \"6\""),
        "true"
    );
    assert_eq!(
        ok("let n : @nat16 = from_string \"7\" in \
            let two : @nat = 2 in \
            let r : @nat16 = two * n in r ?= from_string \"14\""),
        "true"
    );
}

#[test]
fn sized_literal_arithmetic_evaluates() {
    // Two bare literals in a sized-int context resolve to that sized type and
    // compute the right value (the operand types are pinned to the result type,
    // not defaulted to `@int`).
    let ok = |src: &str| run(&format!("@mod M\n$ a : @bool = {src}"), "a");
    assert_eq!(
        ok("let r : @int32 = 2 + 3 * 4 in r ?= from_string \"14\""),
        "true"
    );
    assert_eq!(
        ok("let r : @int64 = 100 - 1 in r ?= from_string \"99\""),
        "true"
    );
}

#[test]
fn operator_table_every_entry() {
    // Iterate `lexer::data::OPERATORS` and check each entry: run a snippet and
    // check the result. Entries with no standalone value can be `Skip`ped
    // explicitly. An entry with NO arm hits the `_ =>` panic, so a new operator
    // added to the table without a test fails here, naming the lexeme.
    use frontend::lexer::data::OPERATORS;

    enum Check {
        Runs(&'static str),
        Skip,
    }
    use Check::*;

    for d in OPERATORS {
        // Per-entry snippet (module body after `@mod M\n`), evaluating global `a`.
        let (body, check): (&str, Check) = match d.lexeme {
            // Structural punctuation.
            "\\" => ("$ a = (\\x = x) 5", Runs("5")),
            "=" => ("$ a = 5", Runs("5")),
            "=>" => ("$ a = if @true => 1 else 0", Runs("1")),
            "->" => ("$ f : @int -> @int = \\x = x\n$ a = f 5", Runs("5")),
            ":" => ("$ a : @int = 5", Runs("5")),
            "$" => ("$ a = 5", Runs("5")),
            // Arithmetic and prefix.
            "+" => ("$ a = 1 + 2", Runs("3")),
            "-" => ("$ a = 0 - (- 5)", Runs("5")), // infix and prefix `-`
            "*" => ("$ a = 4 * 3", Runs("12")),
            "/" => ("$ a = 13 / 4", Runs("3")),
            "%" => ("$ a = 13 % 5", Runs("3")),
            "^" => ("$ a = 2 ^ 3", Runs("8")),
            "!" => ("$ a = if !@false => 1 else 0", Runs("1")),
            // Comparison.
            "?=" => ("$ a = if 3 ?= 3 => 1 else 0", Runs("1")),
            "?>" => ("$ a = if 5 ?> 3 => 1 else 0", Runs("1")),
            "?<" => ("$ a = if 3 ?< 5 => 1 else 0", Runs("1")),
            "<=" => ("$ a = if 3 <= 3 => 1 else 0", Runs("1")),
            ">=" => ("$ a = if 5 >= 4 => 1 else 0", Runs("1")),
            // Effect-row delimiters. `<`/`>` only mean something inside a `<E>`
            // row (no standalone value), so skip them; `<>` is the pure row and
            // `|` also alternates patterns, so those run.
            "<" | ">" => ("", Skip),
            "<>" => ("$ f : @int -> <> @int = \\x = x\n$ a = f 5", Runs("5")),
            "|" => ("$ a = is 1 | 1 => 10 else 0", Runs("10")),
            // Pipes, short-circuit, sequencing, cons, concat.
            "<|" => ("$ a = (\\n = n + 1) <| 5", Runs("6")),
            "&&" => ("$ a = if @true && @true => 1 else 0", Runs("1")),
            "||" => ("$ a = if @false || @true => 1 else 0", Runs("1")),
            ";" => ("$ a = 1 ; 2 ; 3", Runs("3")),
            "|>" => ("$ a = 5 |> (\\n = n + 1)", Runs("6")),
            "::" => ("$ a = is 1 :: 2 :: [] | h :: _ => h else 0", Runs("1")),
            "++" => ("$ a = \"x\" ++ \"y\"", Runs("\"xy\"")),
            other => panic!("OPERATORS entry `{other}` has no test; add an arm"),
        };
        if let Runs(expect) = check {
            let src = format!("@mod M\n{body}");
            assert_eq!(run(&src, "a").as_str(), expect, "operator `{}`", d.lexeme);
        }
    }
}

#[test]
fn user_operator_overload_dispatches_by_type() {
    // `+` overloaded for a user struct via `$ (+) : …`. `V + V` reaches the user
    // definition (componentwise); the `@int + @int` inside its body, and in `r`,
    // still resolves to the builtin. Proves symbolic-name defs join the operator's
    // overload set and dispatch by type.
    let src = "@mod M\n\
               $ V : @struct = x: @int, y: @int\n\
               $ (+) : V -> V -> V = \\a b = V.{ .x = a.x + b.x, .y = a.y + b.y }\n\
               $ r : @int = let s = V.{ .x = 1, .y = 2 } + V.{ .x = 10, .y = 20 } in s.x + s.y";
    assert_eq!(run(src, "r"), "33");
}

#[test]
fn arithmetic_and_precedence() {
    assert_eq!(run("@mod M\n$ a = 1 + 2 * 3 - 4", "a"), "3");
    assert_eq!(run("@mod M\n$ a = (1 + 2) * (3 - 4)", "a"), "-3");
    assert_eq!(run("@mod M\n$ a = 100 / 7", "a"), "14");
    assert_eq!(run("@mod M\n$ a = 100 % 7", "a"), "2");
    assert_eq!(run("@mod M\n$ a = - (-7)", "a"), "7");
}

#[test]
fn reals_and_mixed() {
    assert_eq!(run("@mod M\n$ a = 1.0 + 2.0", "a"), "3");
    assert_eq!(run("@mod M\n$ a : Real = 3.0 / 2.0", "a"), "1.5");
}

#[test]
fn self_recursion_factorial() {
    let src = "@mod M\n$ fact : @int -> @int = \\n = if n ?= 0 => 1 else n * fact (n - 1)\n\
               $ r = fact 5";
    assert_eq!(run(src, "r"), "120");
}

#[test]
fn mutual_recursion() {
    let src = "@mod M\n\
               $ is_even : @int -> @int = \\n = if n ?= 0 => 1 else is_odd (n - 1)\n\
               $ is_odd  : @int -> @int = \\n = if n ?= 0 => 0 else is_even (n - 1)\n\
               $ r = is_even 10";
    assert_eq!(run(src, "r"), "1");
}

#[test]
fn let_bindings_chain_and_destructure() {
    assert_eq!(run("@mod M\n$ a = let x = 6 in x * 7", "a"), "42");
    assert_eq!(
        run(
            "@mod M\n$ a = let x = 1, y = x + 10, z = y * 2 in x + y + z",
            "a"
        ),
        "34"
    );
    assert_eq!(run("@mod M\n$ a = let {p, q} = {3, 4} in p + q", "a"), "7");
}

#[test]
fn tuples_and_indexing() {
    assert_eq!(run("@mod M\n$ a = {1, {2, 3}}.1.0", "a"), "2");
    let swap = "@mod M\n$ swap = \\t = {t.1, t.0}\n$ a = (swap {1, 2}).0";
    assert_eq!(run(swap, "a"), "2");
}

#[test]
fn list_sum_and_map() {
    let src = "@mod M\n\
               $ sum : @list @int -> @int = \\xs = is xs | [] => 0 | h :: t => h + sum t else 0\n\
               $ a = sum [1, 2, 3, 4, 5]";
    assert_eq!(run(src, "a"), "15");
    let cons = "@mod M\n\
                $ sum : @list @int -> @int = \\xs = is xs | [] => 0 | h :: t => h + sum t else 0\n\
                $ a = sum (1 :: 2 :: 3 :: [])";
    assert_eq!(run(cons, "a"), "6");
}

#[test]
fn union_construction_and_nested_match() {
    let src = "@mod M\n\
               $ Peano : @union = Zero: {}, Succ: { Peano }\n\
               $ depth : Peano -> @int = \\n = \
                 is n | Peano.Zero => 0 \
                 | Peano.Succ.{ Peano.Zero } => 1 \
                 | Peano.Succ.{ Peano.Succ.{ _ } } => 2\n\
               $ a = depth Peano.Succ.{ Peano.Succ.{ Peano.Zero } }";
    assert_eq!(run(src, "a"), "2");
}

#[test]
fn struct_field_access_and_match() {
    let src = "@mod M\n\
               $ Point : @struct = x: @int, y: @int\n\
               $ p : Point = Point.{ .x = 3, .y = 4 }\n\
               $ a = p.x + p.y";
    assert_eq!(run(src, "a"), "7");
    let m = "@mod M\n\
             $ Point : @struct = x: @int, y: @int\n\
             $ sum : Point -> @int = \\pt = is pt | Point.{ x, y } => x + y else 0\n\
             $ a = sum Point.{ .x = 10, .y = 20 }";
    assert_eq!(run(m, "a"), "30");
}

#[test]
fn with_scopes_struct_fields() {
    let src = "@mod M\n\
               $ Point : @struct = x: @int, y: @int\n\
               $ p : Point = Point.{ .x = 3, .y = 4 }\n\
               $ a = with p in x + y";
    assert_eq!(run(src, "a"), "7");
}

#[test]
fn record_parameter_destructures() {
    assert_eq!(
        run(
            "@mod M\n$ add : {x: @int, y: @int} -> @int = x + y\n$ a = add {3, 4}",
            "a"
        ),
        "7"
    );
    assert_eq!(
        run("@mod M\n$ inc : {x: @int} -> @int = x + 1\n$ a = inc 5", "a"),
        "6"
    );
}

#[test]
fn higher_order_and_guards() {
    let src = "@mod M\n\
               $ twice = \\f x = f (f x)\n\
               $ a = twice (\\n = n + 3) 1";
    assert_eq!(run(src, "a"), "7");
    let guard = "@mod M\n\
                 $ classify : @int -> @int = \\n = is n | m if m ?> 0 => 1 | _ => 0\n\
                 $ a = classify 5";
    assert_eq!(run(guard, "a"), "1");
}

#[test]
fn string_concat_and_prefix_match() {
    assert_eq!(run("@mod M\n$ a = \"hi\" ++ \"!\"", "a"), "\"hi!\"");
    let src = "@mod M\n\
               $ verb : Str -> @int = \\s = is s | \"GET \" ++ _ => 1 else 0\n\
               $ a = verb \"GET /\"";
    assert_eq!(run(src, "a"), "1");
}

#[test]
fn sequencing_returns_last() {
    assert_eq!(run("@mod M\n$ a = 1 ; 2 ; 3", "a"), "3");
}

#[test]
fn short_circuit_and_or() {
    // `&&`/`||` desugar to a lazy `if`, so the right operand is skipped when the
    // result is already decided: a `1 / 0` on the skipped side must not fault.
    // Precedence: `&&`/`||` bind looser than comparison (`a ?< b && c ?< d`).
    let src = "@mod M\n\
               $ f : @bool = @false\n\
               $ t : @bool = @true\n\
               $ sc_and : @int = if (f && (1 / 0 ?= 0)) => 1 else 0\n\
               $ sc_or  : @int = if (t || (1 / 0 ?= 0)) => 0 else 1\n\
               $ prec   : @int = if (3 ?< 5 && 5 ?< 9) => 0 else 1\n\
               $ test : @int = sc_and + sc_or + prec";
    assert_eq!(run(src, "test"), "0");
}

/// A C-style `main : {} -> <| e> @int` is applied to unit; its `@int` result is the
/// exit code (no `entry = <value>` print). The open row lets it perform effects.
#[test]
fn entry_unit_fn_returns_exit_code() {
    let src = "@mod MAIN\n$ main : {} -> <| e> @int = \\u = 42\n";
    let program = lower_checked(src, "main");
    let ir = frontend::ir::lower_modules(std::slice::from_ref(&program));
    let code = interpreter::machine::run_entry(&ir, "main", None)
        .unwrap_or_else(|e| panic!("{}", e.render(src, "main")));
    assert_eq!(code, 42);
}

/// A C-style `main : [n]Str -> <| e> @int` receives argv as a sized tensor of
/// strings; `argv[0]` is the program path, so `main` sees the whole vector.
#[test]
fn entry_argv_fn_receives_string_vector() {
    let src = "@mod MAIN\n\
               $ main : [n]Str -> <| e> @int = \\args = @tensor_length args\n";
    let program = lower_checked(src, "main");
    let ir = frontend::ir::lower_modules(std::slice::from_ref(&program));
    let argv = vec!["prog".to_string(), "alpha".to_string(), "beta".to_string()];
    let code = interpreter::machine::run_entry(&ir, "main", Some(argv))
        .unwrap_or_else(|e| panic!("{}", e.render(src, "main")));
    assert_eq!(code, 3);
}

/// The machine's explicit continuation stack makes deep tail recursion run in
/// constant host stack (where a stack-recursive evaluator would overflow): a
/// tail-recursive countdown to a depth that would blow the native stack returns.
#[test]
fn deep_tail_recursion_is_constant_stack() {
    let src = "@mod T\n\
               $ loop : @int -> @int = \\n = if n ?= 0 => 42 else loop (n - 1)\n\
               $ test : @int = loop 1000000\n";
    assert_eq!(run(src, "test"), "42");
}
