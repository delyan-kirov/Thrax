//! End-to-end tests: parse a single module, lower it, and evaluate a named
//! global with the tree-walking interpreter.

use core::{lower_program, Decls, Interp};

/// Parse `src`, lower it, and evaluate the global `name`, returning its display.
fn eval(src: &str, name: &str) -> String {
    let parsed = syntax::parse(src).expect("parse");
    let decls = Decls::collect(&parsed.ast, std::slice::from_ref(&parsed.program));
    let lowered = vec![lower_program(&parsed.ast, &parsed.program, &decls)];
    let interp = Interp::new(&lowered);
    interp
        .eval_global(name)
        .unwrap_or_else(|e| panic!("{}", e.render(src, name)))
        .show()
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
    assert_eq!(eval(src, "a"), "7");
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
