//! End-to-end tests: parse a single module, lower it, and evaluate a named
//! global with the tree-walking interpreter.

use frontend::{lower_program, Decls, Resolved};
use interpreter::Interp;

/// Parse `src`, lower it, and evaluate the global `name`, returning its display.
fn eval(src: &str, name: &str) -> String {
    let parsed = frontend::parse(src).expect("parse");
    let decls = Decls::collect(&parsed.ast, std::slice::from_ref(&parsed.program));
    let resolved = Resolved::default();
    let lowered = vec![lower_program(
        &parsed.ast,
        &parsed.program,
        &decls,
        &resolved,
    )];
    let interp = Interp::new(&lowered);
    interp
        .eval_global(name)
        .unwrap_or_else(|e| panic!("{}", e.render(src, name)))
        .show()
}

/// Parse, type-check (to resolve type-directed `[..]` nodes), lower with that
/// resolution, and evaluate `name`. Exercises the checker -> lowering side table.
fn lower_checked(src: &str, name: &str) -> frontend::lowering::data::Program {
    let parsed = frontend::parse(src).expect("parse");
    let mut checker = frontend::Checker::new(&parsed.ast);
    checker
        .check_program(&parsed.program)
        .unwrap_or_else(|e| panic!("{}", e.render(src, name)));
    let (exprs, pats) = checker.array_nodes();
    let mut resolved = Resolved::default();
    resolved.array_exprs.extend(exprs.iter().copied());
    resolved.array_pats.extend(pats.iter().copied());
    for (&site, &module) in checker.call_modules() {
        resolved.call_modules.insert(site, module.to_string());
    }
    for (&site, fields) in checker.with_fields() {
        resolved.with_fields.insert(site, fields.clone());
    }
    let decls = Decls::collect(&parsed.ast, std::slice::from_ref(&parsed.program));
    lower_program(&parsed.ast, &parsed.program, &decls, &resolved)
}

fn eval_checked(src: &str, name: &str) -> String {
    Interp::new(&[lower_checked(src, name)])
        .eval_global(name)
        .unwrap_or_else(|e| panic!("{}", e.render(src, name)))
        .show()
}

/// Lower, then run the ANF and De-Bruijn passes (the IR-pipeline front) before
/// evaluating. The passes are semantics-preserving, so this must match
/// [`eval_checked`] exactly.
fn eval_anf(src: &str, name: &str) -> String {
    let mut program = lower_checked(src, name);
    frontend::lowering::anf::normalize_program(&mut program);
    frontend::lowering::debruijn::assign_program(&mut program);
    Interp::new(&[program])
        .eval_global(name)
        .unwrap_or_else(|e| panic!("{}", e.render(src, name)))
        .show()
}

/// Type-check several modules sharing one `Ast` (the last is the root, importing
/// all earlier ones), then lower every module with the merged resolutions and
/// evaluate the root's `name`. Exercises cross-module overload dispatch.
fn eval_modules(sources: &[&str], name: &str) -> String {
    let mut ast = frontend::Ast::new();
    let mut programs = Vec::new();
    for src in sources {
        let (next, program) = frontend::parse_into(ast, src).expect("parse");
        ast = next;
        programs.push(program);
    }
    let (root, deps) = programs.split_last().expect("at least one module");

    let mut dep_checkers = Vec::new();
    for dep in deps {
        let mut c = frontend::Checker::new(&ast);
        c.check_program(dep).expect("check dep");
        dep_checkers.push(c);
    }
    let mut root_checker = frontend::Checker::new(&ast);
    for dep in &dep_checkers {
        root_checker.import_from(dep);
    }
    root_checker.check_program(root).expect("check root");

    let mut resolved = Resolved::default();
    for c in dep_checkers.iter().chain(std::iter::once(&root_checker)) {
        for (&site, &module) in c.call_modules() {
            resolved.call_modules.insert(site, module.to_string());
        }
        for (&site, fields) in c.with_fields() {
            resolved.with_fields.insert(site, fields.clone());
        }
    }

    let decls = Decls::collect(&ast, &programs);
    // The root is lowered first so its names win, matching the driver.
    let mut order: Vec<usize> = (0..programs.len()).collect();
    order.sort_by_key(|&i| i != programs.len() - 1);
    let lowered: Vec<_> = order
        .iter()
        .map(|&i| lower_program(&ast, &programs[i], &decls, &resolved))
        .collect();
    Interp::new(&lowered)
        .eval_global(name)
        .unwrap_or_else(|e| panic!("{}", e.render("", name)))
        .show()
}

#[test]
fn cross_module_overload_dispatches_by_type() {
    // `make` is defined in two modules with different result types. The root's
    // `make 5` must reach P.make (a Box, so `unwrap` reads its field), not Q.make
    // (an Int), which the flat runtime name-map might list first.
    let p = "@mod P\n\
             $ Box : @struct = v: Int\n\
             $ make : Int -> Box = \\n = Box.{ .v = n }\n\
             $ unwrap : Box -> Int = \\b = b.v";
    let q = "@mod Q\n$ make : Int -> Int = \\n = n + 100";
    let root = "@mod M\n\
                $ with P\n\
                $ with Q\n\
                $ r : Int = unwrap (make 5)";
    assert_eq!(eval_modules(&[p, q, root], "r"), "5");
}

#[test]
fn defer_runs_cleanup_on_completion_abort_and_nesting() {
    // A `Y` handler that sums every yielded value; the deferred cleanups perform
    // `Y.yield`, so their effects are observable in the total.
    let prelude = "@mod M\n\
        $ Y : @effect = yield : Int -> {},\n\
        $ Exn : @effect = throw : Str -> `a,\n\
        $ sum : ({} -> <Y> Int) -> Int = \
          \\body = do body {} ctl k is Y.yield v = v + k {} else r = r\n";
    // Normal completion: body yields 1 and returns 100, cleanup yields 2 -> 103.
    let normal =
        format!("{prelude}$ r : Int = sum (\\_ = defer Y.yield 2 do (let _ = Y.yield 1 in 100))");
    assert_eq!(eval(&normal, "r"), "103");
    // Abort: the inner handler drops the continuation, but the cleanup (yield 9)
    // still runs under the enclosing `Y` handler -> 9.
    let abort = format!(
        "{prelude}$ r : Int = sum (\\_ = \
         do (defer Y.yield 9 do (let _ = Exn.throw \"x\" in 100)) ctl k is Exn.throw e = 0)"
    );
    assert_eq!(eval(&abort, "r"), "9");
    // Nested defers run innermost-first: 1 + 2 + 3 = 6.
    let nested = format!(
        "{prelude}$ r : Int = sum (\\_ = \
         defer Y.yield 3 do (defer Y.yield 2 do (let _ = Y.yield 1 in 0)))"
    );
    assert_eq!(eval(&nested, "r"), "6");
}

#[test]
fn defer_cleanup_runs_when_a_stored_continuation_completes() {
    // The handler stores the continuation instead of resuming; the cleanup runs
    // only when that continuation is later driven to completion (two steps).
    let src = "@mod M\n\
        $ Co : @effect = step : Int -> {},\n\
        $ Task : @union = Fin: {}, Susp: { {} -> Task },\n\
        $ spawn : ({} -> <Co> {}) -> Task = \
          \\t = do t {} ctl k is step v = Task.Susp.{ k } else _ = Task.Fin.{}\n\
        $ drive : Task -> Int = \
          \\t = when t is Task.Fin.{} then 0 is Task.Susp.{ k } then 1 + drive (k {}) else 0\n\
        $ r : Int = drive (spawn (\\_ = defer step 2 do (let _ = step 1 in {})))";
    assert_eq!(eval(src, "r"), "2");
}

#[test]
fn c_namespace_file_roundtrip() {
    // Open for write, put bytes, close; reopen for read, read them back, then
    // remove the file. "hi" is bytes 104 and 105.
    let src = "@mod M\n\
        $ p : Str = \"/tmp/thrax_core_roundtrip.txt\"\n\
        $ r : Int = \
          let f = C.fopen p \"wb\" in \
          C.fputs \"hi\" f ; C.fclose f ; \
          let g = C.fopen p \"rb\" in \
          let a = C.fgetc g in let b = C.fgetc g in \
          C.fclose g ; C.remove p ; a + b";
    assert_eq!(eval(src, "r"), "209");
}

#[test]
fn target_reflects_the_host_consistently() {
    // Host-agnostic invariants: the word and pointer widths agree, and `name` is
    // exactly `arch-os`.
    let src = "@mod M\n$ r : Int = \
        if TARGET.int_bits ?= TARGET.ptr_bits \
        then (if (TARGET.arch ++ \"-\" ++ TARGET.os) ?= TARGET.name then 0 else 1) \
        else 1";
    assert_eq!(eval(src, "r"), "0");
}

#[test]
fn array_literal_lowers_to_byte_vector() {
    // `[..]` in Array context builds a byte vector, so array_* primitives apply.
    let src = "@mod M\n$ a : Array = [10, 20, 30]\n\
               $ n = array_len a\n$ g = array_get a 1";
    assert_eq!(eval_checked(src, "n"), "3");
    assert_eq!(eval_checked(src, "g"), "20");
}

#[test]
fn array_patterns_destructure_and_guard() {
    let src = "@mod M\n\
               $ sum2 : Array -> Int = \\a = when a is [x, y] then x + y else 0\n\
               $ r = sum2 [4, 5]\n\
               $ miss = sum2 [1, 2, 3]\n\
               $ lit : Array -> Int = \\a = when a is [1, y] then y else 0\n\
               $ hit = lit [1, 42]\n\
               $ no = lit [2, 42]\n\
               $ head : Array -> Int = \\a = when a is [h, ..rest] then h + array_len rest else 0\n\
               $ hd = head [7, 8, 9]";
    assert_eq!(eval_checked(src, "r"), "9");
    assert_eq!(eval_checked(src, "miss"), "0");
    assert_eq!(eval_checked(src, "hit"), "42");
    assert_eq!(eval_checked(src, "no"), "0");
    assert_eq!(eval_checked(src, "hd"), "9");
}

#[test]
fn arithmetic_and_precedence() {
    assert_eq!(eval("@mod M\n$ a = 1 + 2 * 3 - 4", "a"), "3");
    assert_eq!(eval("@mod M\n$ a = (1 + 2) * (3 - 4)", "a"), "-3");
    assert_eq!(eval("@mod M\n$ a = 100 / 7", "a"), "14");
    assert_eq!(eval("@mod M\n$ a = 100 % 7", "a"), "2");
    assert_eq!(eval("@mod M\n$ a = - (-7)", "a"), "7");
}

#[test]
fn reals_and_mixed() {
    assert_eq!(eval("@mod M\n$ a = 1.0 + 2.0", "a"), "3");
    assert_eq!(eval("@mod M\n$ a : Real = 3.0 / 2.0", "a"), "1.5");
}

#[test]
fn self_recursion_factorial() {
    let src = "@mod M\n$ fact : Int -> Int = \\n = if n ?= 0 then 1 else n * fact (n - 1)\n\
               $ r = fact 5";
    assert_eq!(eval(src, "r"), "120");
}

#[test]
fn mutual_recursion() {
    let src = "@mod M\n\
               $ is_even : Int -> Int = \\n = if n ?= 0 then 1 else is_odd (n - 1)\n\
               $ is_odd  : Int -> Int = \\n = if n ?= 0 then 0 else is_even (n - 1)\n\
               $ r = is_even 10";
    assert_eq!(eval(src, "r"), "1");
}

#[test]
fn let_bindings_chain_and_destructure() {
    assert_eq!(eval("@mod M\n$ a = let x = 6 in x * 7", "a"), "42");
    assert_eq!(
        eval(
            "@mod M\n$ a = let x = 1, y = x + 10, z = y * 2 in x + y + z",
            "a"
        ),
        "34"
    );
    assert_eq!(eval("@mod M\n$ a = let {p, q} = {3, 4} in p + q", "a"), "7");
}

#[test]
fn tuples_and_indexing() {
    assert_eq!(eval("@mod M\n$ a = {1, {2, 3}}.1.0", "a"), "2");
    let swap = "@mod M\n$ swap = \\t = {t.1, t.0}\n$ a = (swap {1, 2}).0";
    assert_eq!(eval(swap, "a"), "2");
}

#[test]
fn list_sum_and_map() {
    let src = "@mod M\n\
               $ sum : List Int -> Int = \\xs = when xs is [] then 0 is h :: t then h + sum t else 0\n\
               $ a = sum [1, 2, 3, 4, 5]";
    assert_eq!(eval(src, "a"), "15");
    let cons = "@mod M\n\
                $ sum : List Int -> Int = \\xs = when xs is [] then 0 is h :: t then h + sum t else 0\n\
                $ a = sum (1 :: 2 :: 3 :: [])";
    assert_eq!(eval(cons, "a"), "6");
}

#[test]
fn union_construction_and_nested_match() {
    let src = "@mod M\n\
               $ Peano : @union = Zero: {}, Succ: { Peano }\n\
               $ depth : Peano -> Int = \\n = \
                 when n is Peano.Zero then 0 \
                 is Peano.Succ.{ Peano.Zero } then 1 \
                 is Peano.Succ.{ Peano.Succ.{ _ } } then 2\n\
               $ a = depth Peano.Succ.{ Peano.Succ.{ Peano.Zero } }";
    assert_eq!(eval(src, "a"), "2");
}

#[test]
fn struct_field_access_and_match() {
    let src = "@mod M\n\
               $ Point : @struct = x: Int, y: Int\n\
               $ p : Point = Point.{ .x = 3, .y = 4 }\n\
               $ a = p.x + p.y";
    assert_eq!(eval(src, "a"), "7");
    let m = "@mod M\n\
             $ Point : @struct = x: Int, y: Int\n\
             $ sum : Point -> Int = \\pt = when pt is Point.{ x, y } then x + y else 0\n\
             $ a = sum Point.{ .x = 10, .y = 20 }";
    assert_eq!(eval(m, "a"), "30");
}

#[test]
fn with_scopes_struct_fields() {
    let src = "@mod M\n\
               $ Point : @struct = x: Int, y: Int\n\
               $ p : Point = Point.{ .x = 3, .y = 4 }\n\
               $ a = with p in x + y";
    assert_eq!(eval_checked(src, "a"), "7");
}

#[test]
fn record_parameter_destructures() {
    assert_eq!(
        eval(
            "@mod M\n$ add : {x: Int, y: Int} -> Int = x + y\n$ a = add {3, 4}",
            "a"
        ),
        "7"
    );
    assert_eq!(
        eval("@mod M\n$ inc : {x: Int} -> Int = x + 1\n$ a = inc 5", "a"),
        "6"
    );
}

#[test]
fn higher_order_and_guards() {
    let src = "@mod M\n\
               $ twice = \\f x = f (f x)\n\
               $ a = twice (\\n = n + 3) 1";
    assert_eq!(eval(src, "a"), "7");
    let guard = "@mod M\n\
                 $ classify : Int -> Int = \\n = when n is m if m ?> 0 then 1 is _ then 0\n\
                 $ a = classify 5";
    assert_eq!(eval(guard, "a"), "1");
}

#[test]
fn string_concat_and_prefix_match() {
    assert_eq!(eval("@mod M\n$ a = \"hi\" ++ \"!\"", "a"), "\"hi!\"");
    let src = "@mod M\n\
               $ verb : Str -> Int = \\s = when s is \"GET \" ++ _ then 1 else 0\n\
               $ a = verb \"GET /\"";
    assert_eq!(eval(src, "a"), "1");
}

#[test]
fn sequencing_returns_last() {
    assert_eq!(eval("@mod M\n$ a = 1 ; 2 ; 3", "a"), "3");
}

/// The ANF + De-Bruijn passes preserve behavior across the core constructs:
/// arithmetic, recursion, closures/let, structs/fields, variants/`when`, lists,
/// tuples, strings, and an effect handler. The normalized program must evaluate
/// to exactly what the plain lowering does.
#[test]
fn anf_preserves_semantics() {
    let cases: &[(&str, &str)] = &[
        ("@mod T\n$ test : Int = 1 + 2 * 3 - 4\n", "test"),
        (
            "@mod T\n\
             $ fib : Int -> Int = \\n =\n\
             \tif n ?< 2 then n else fib (n - 1) + fib (n - 2)\n\
             $ test : Int = fib 12\n",
            "test",
        ),
        (
            "@mod T\n\
             $ apply2 : (Int -> Int) -> Int -> Int = \\f x = f (f x)\n\
             $ test : Int =\n\
             \tlet inc = \\x = x + 1\n\
             \t in apply2 inc 40\n",
            "test",
        ),
        (
            "@mod T\n\
             $ Point : @struct = x: Int, y: Int\n\
             $ p : Point = Point.{ .x = 3, .y = 4 }\n\
             $ test : Int = p.x + p.y\n",
            "test",
        ),
        (
            "@mod T\n\
             $ Shape : @union = Dot: {}, Seg: { Int }\n\
             $ size : Shape -> Int = \\s =\n\
             \twhen s is Shape.Dot then 0 is Shape.Seg.{n} then n\n\
             $ test : Int = size (Shape.Seg.{7}) - size Shape.Dot\n",
            "test",
        ),
        (
            "@mod T\n\
             $ len : List `T -> Int =\n\
             \tlet helper : List `T -> Int -> Int = \\l n =\n\
             \t\twhen l is List.Nil then n is List.Cons.{_, xs} then helper xs (n + 1)\n\
             \t in \\l = helper l 0\n\
             $ xs : List Int = List.Cons.{1, List.Cons.{2, List.Cons.{3, List.Nil}}}\n\
             $ test : Int = len xs\n",
            "test",
        ),
        (
            "@mod T\n$ t = {1, 2, 3}\n$ test : Int = t.0 + t.1 + t.2\n",
            "test",
        ),
        (
            "@mod T\n$ s : Str = \"ab\" ++ \"cd\"\n$ test : Int = array_len s + array_get s 1\n",
            "test",
        ),
    ];
    for (src, name) in cases {
        assert_eq!(
            eval_anf(src, name),
            eval_checked(src, name),
            "ANF changed the result of `{name}` in:\n{src}"
        );
    }
}

/// ANF preservation on the effects corpus (handlers, resume, state, `defer`).
#[test]
fn anf_preserves_effects() {
    for (file, name) in [
        ("EFFECTS.thx", "test"),
        ("FINALLY.thx", "r_normal"),
        ("FINALLY.thx", "r_abort"),
    ] {
        let path = format!("{}/../../examples/{file}", env!("CARGO_MANIFEST_DIR"));
        let src = std::fs::read_to_string(&path).expect("read example");
        assert_eq!(
            eval_anf(&src, name),
            eval_checked(&src, name),
            "ANF changed the result of `{name}` in {file}"
        );
    }
}
