//! End-to-end tests: parse and type-check a module (or several), lower it to the
//! IR, and evaluate a named global on the reified-K machine.

use frontend::{lower_program, Decls, Resolved};

/// Parse, type-check (resolving type-directed `[..]` nodes, `with` fields, and
/// cross-module overloads the lowering consumes), and lower a single module.
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
    {
        let (tex, _idx) = checker.tensor_nodes();
        resolved.tensor_exprs.extend(tex.iter().copied());
    }
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
    let decls = Decls::collect(&parsed.ast, std::slice::from_ref(&parsed.program));
    lower_program(&parsed.ast, &parsed.program, &decls, &resolved)
}

/// Lower a single module to the IR (canonical mangling + the middle-end passes)
/// and evaluate `name` on the machine.
fn run(src: &str, name: &str) -> String {
    let program = lower_checked(src, name);
    let ir = frontend::ir::lower_modules(std::slice::from_ref(&program));
    interpreter::machine::eval(&ir, name).unwrap_or_else(|e| panic!("{}", e.render(src, name)))
}

/// Type-check several modules sharing one `Ast` (the last is the root, importing
/// all earlier ones), lower every module with the merged resolutions, then
/// evaluate the root's `name` on the machine. Exercises cross-module dispatch.
fn run_modules(sources: &[&str], name: &str) -> String {
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
        for (&site, key) in c.overload_calls() {
            resolved.overload_calls.insert(site, key.clone());
        }
        for (&body, key) in c.def_keys() {
            resolved.def_keys.insert(body, key.clone());
        }
        for (&site, args) in c.implicit_calls() {
            resolved.implicit_args.insert(site, args.clone());
        }
        for (&site, fields) in c.with_fields() {
            resolved.with_fields.insert(site, fields.clone());
        }
    }

    let decls = Decls::collect(&ast, &programs);
    // Root first so its bare names win, matching the driver / `lower_modules`.
    let mut order: Vec<usize> = (0..programs.len()).collect();
    order.sort_by_key(|&i| i != programs.len() - 1);
    let lowered: Vec<_> = order
        .iter()
        .map(|&i| lower_program(&ast, &programs[i], &decls, &resolved))
        .collect();
    let ir = frontend::ir::lower_modules(&lowered);
    interpreter::machine::eval(&ir, name).unwrap_or_else(|e| panic!("{}", e.render("", name)))
}

#[test]
fn cross_module_overload_dispatches_by_type() {
    // `make` is defined in two modules with different result types. The root's
    // `make 5` must reach P.make (a Box, so `unwrap` reads its field), not Q.make
    // (an Int); the checker resolves it and the canonical `P.make` name carries it.
    let p = "@mod P\n\
             $ Box : @struct = v: Int\n\
             $ make : Int -> Box = \\n = Box.{ .v = n }\n\
             $ unwrap : Box -> Int = \\b = b.v";
    let q = "@mod Q\n$ make : Int -> Int = \\n = n + 100";
    let root = "@mod M\n\
                $ with P\n\
                $ with Q\n\
                $ r : Int = unwrap (make 5)";
    assert_eq!(run_modules(&[p, q, root], "r"), "5");
}

#[test]
fn same_module_overload_dispatches_by_type() {
    // Two overloads of `kind` in ONE module. Before type-mangling the globals both
    // collided under a single `M.kind` key and every call ran the first body
    // (giving 11); now `kind true` reaches the Bool body, so the result is 21.
    let src = "@mod M\n\
               $ kind : Int -> Int = \\x = 1\n\
               $ kind : Bool -> Int = \\b = 2\n\
               $ r : Int = (kind 7) + (kind true) * 10";
    assert_eq!(run(src, "r"), "21");
}

#[test]
fn ctx_implicit_resolves_by_name_and_type() {
    // `max_of` declares an implicit `cmp`, resolved by name from scope (the global
    // `>`-like `cmp`). The dictionary is injected as a leading argument.
    let src = "@mod M\n\
               $ cmp : Int -> Int -> Bool = \\a b = a ?> b\n\
               $ max_of : a -> a -> a  @ctx cmp : a -> a -> Bool = \\x y =\n\
               \tif cmp x y => x else y\n\
               $ r : Int = max_of 3 7";
    assert_eq!(run(src, "r"), "7");
}

#[test]
fn ctx_implicit_chains_and_overrides() {
    // `max3` passes its own `@ctx cmp` down to `max_of` (local wins), and an
    // explicit `@ctx lt` override flips `max_of` into a min.
    let src = "@mod M\n\
               $ gt : Int -> Int -> Bool = \\a b = a ?> b\n\
               $ lt : Int -> Int -> Bool = \\a b = a ?< b\n\
               $ max_of : a -> a -> a  @ctx cmp : a -> a -> Bool = \\x y =\n\
               \tif cmp x y => x else y\n\
               $ max3 : a -> a -> a -> a  @ctx cmp : a -> a -> Bool = \\x y z =\n\
               \tmax_of (max_of x y) z\n\
               $ chained : Int = max3 3 9 5 @ctx gt\n\
               $ flipped : Int = max_of 3 7 @ctx lt\n\
               $ r : Int = chained + flipped";
    assert_eq!(run(src, "r"), "12");
}

#[test]
fn struct_with_splices_included_fields() {
    // `Point3` copies `Point`'s fields ahead of its own; the positional/field
    // layout must be x, y, then z.
    let src = "@mod M\n\
               $ Point : @struct = x: Int, y: Int\n\
               $ Point3 : @struct = with Point, z: Int\n\
               $ r : Int =\n\
               \tlet p = Point3.{ .x = 1, .y = 2, .z = 3 } in p.x + p.y + p.z";
    assert_eq!(run(src, "r"), "6");
}

#[test]
fn declared_type_params_control_order() {
    // `@struct b a` declares the parameters explicitly, so `Box Int Str` binds
    // b = Int and a = Str (the declared order), not the order the fields mention
    // them. Reading `snd` back (typed `b`, i.e. Int) must give 7.
    let src = "@mod M\n\
               $ Box : @struct b a = fst: a, snd: b\n\
               $ r : Int = let x : Box Int Str = .{ .fst = \"hi\", .snd = 7 } in x.snd";
    assert_eq!(run(src, "r"), "7");
}

#[test]
fn parameterized_alias_picks_which_generic() {
    // `KeyInt` fixes the first parameter, `ValInt` the second; the alias's own
    // parameter fills the one left open. Both instantiate the same `Pair`.
    let src = "@mod M\n\
               $ Pair : @struct a b = fst: a, snd: b\n\
               $ KeyInt : @alias b = Pair Int b\n\
               $ ValInt : @alias a = Pair a Int\n\
               $ p : KeyInt Str = .{ .fst = 3, .snd = \"x\" }\n\
               $ q : ValInt Str = .{ .fst = \"y\", .snd = 9 }\n\
               $ r : Int = p.fst + q.snd";
    assert_eq!(run(src, "r"), "12");
}

#[test]
fn union_with_splices_included_variants() {
    // `Color` copies `Base`'s variants; a match over a copied and a new variant
    // both dispatch by tag.
    let src = "@mod M\n\
               $ Base : @union = Red: {}, Green: {}\n\
               $ Color : @union = with Base, Blue: {}\n\
               $ rank : Color -> Int = \\c =\n\
               \tis c | Color.Red => 1 | Color.Green => 2 | Color.Blue => 3\n\
               $ r : Int = rank Color.Red + rank Color.Blue";
    assert_eq!(run(src, "r"), "4");
}

#[test]
fn open_row_param_accepts_any_matching_struct() {
    // One row-polymorphic function `{ x:Int, y:Int | r } -> Int` accepts several
    // distinct nominal structs, as long as they carry x:Int and y:Int.
    let src = "@mod M\n\
               $ Point  : @struct = x: Int, y: Int,\n\
               $ Point3 : @struct = x: Int, y: Int, z: Int,\n\
               $ area : { x: Int, y: Int | r } -> Int = \\p = p.x * p.y\n\
               $ r : Int = (area Point.{ .x=3, .y=4 }) + (area Point3.{ .x=5, .y=6, .z=9 })";
    assert_eq!(run(src, "r"), "42");
}

#[test]
fn anonymous_records_literal_update_stack() {
    // Anonymous literal into an open row; update (`| p`) preserving shape and stack
    // (`with p`) on an open-row parameter.
    let src = "@mod M\n\
               $ area : { x: Int, y: Int | r } -> Int = \\p = p.x * p.y\n\
               $ shift : { x: Int | r } -> { x: Int | r } = \\p = { .x = p.x + 10 | p }\n\
               $ tag : { x: Int | r } -> { x: Int, tag: Int | r } = \\p = { .tag = 99, with p }\n\
               $ r : Int =\n\
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
               $ P : @struct = a: Int, b: Int, c: Int\n\
               $ base : P = P.{ .a = 1, .b = 2, .c = 3 }\n\
               $ q : P = P.{ .b = 20 | base }\n\
               $ r0 : P = .{ .a = 10 | base }\n\
               $ cl : P = .{ | base }\n\
               $ r : Int = q.a + q.b + q.c + r0.a + cl.c";
    assert_eq!(run(src, "r"), "37"); // (1+20+3) + 10 + 3
}

#[test]
fn sized_tensor_construction_and_modular_index() {
    // `[n]T` is a sized vector; a `[..]` literal's length fixes the size, and
    // `t.[i]` reads modulo the size (total: `t.[n]` wraps to `t.[0]`). Functions
    // may be size-polymorphic (`[n]a`), the size unifying at the call.
    let src = "@mod M\n\
               $ v : [3]Int = [10, 20, 30]\n\
               $ head : [n]a -> a = \\t = t.[0]\n\
               $ grid : [2][2]Int = [ [1, 2], [3, 4] ]\n\
               $ r : Int = v.[1] + v.[3] + v.[7] + head v + grid.[1].[0]";
    // 20 + v.[0]=10 + v.[1]=20 + head=10 + grid.[1].[0]=3
    assert_eq!(run(src, "r"), "63");
}

#[test]
fn tensor_size_arithmetic() {
    // `concat : [n]a -> [m]a -> [n+m]a` computes the result size forward; the
    // Z/2^64 polynomial normalizer decides `n+n == 2*n` and `n+m == m+n`.
    let src = "@mod M\n\
               $ a : [2]Int = [1, 2]\n\
               $ b : [3]Int = [3, 4, 5]\n\
               $ c : [5]Int = concat a b\n\
               $ dup : [n]x -> [2*n]x = \\t = concat t t\n\
               $ flip : [n]x -> [m]x -> [m+n]x = \\p q = concat p q\n\
               $ d : [4]Int = dup a\n\
               $ r : Int = c.[4] + d.[3]"; // 5 + (dup a = [1,2,1,2]).[3]=2
    assert_eq!(run(src, "r"), "7");
}

#[test]
fn shape_checked_linear_algebra() {
    // matmul/transpose/dot are shape-typed (matmul's shared inner dim `k` unifies);
    // multi-axis `m.[i, j]` reads an element. Over the nested-vector rep.
    let src = "@mod M\n\
               $ a : [2][3]Int = [ [1,2,3], [4,5,6] ]\n\
               $ b : [3][2]Int = [ [1,0], [0,1], [1,1] ]\n\
               $ c : [2][2]Int = matmul a b\n\
               $ t : [3][2]Int = transpose a\n\
               $ r : Int = c.[1, 1] + t.[0, 1] + dot [1,2,3] [4,5,6]"; // 11 + 4 + 32
    assert_eq!(run(src, "r"), "47");
}

#[test]
fn inclusive_range_patterns() {
    // `lo ... hi` matches when lo <= x <= hi, inclusive at both ends. Refutable, so
    // the match needs an `else`. Works on Int and Real.
    let src = "@mod M\n\
               $ grade : Int -> Str = \\n =\n\
               \tis n | 90 ... 100 => \"A\" | 60 ... 89 => \"C\" else \"F\"\n\
               $ band : Real -> Int = \\x = is x | 0.0 ... 1.0 => 1 else 0\n\
               $ r : Int =\n\
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
               $ lazy : {} -> Int = 40 + 2\n\
               $ also : {} -> Int = \\u = 40 + 3\n\
               $ r : Int = lazy {} + also {}";
    assert_eq!(run(src, "r"), "85"); // 42 + 43
}

#[test]
fn closed_record_param_named_by_a_lambda() {
    // A closed-record parameter may be named by an explicit lambda and read with
    // field access (`\q = q.y`), instead of the auto-bind sugar. The sugar only
    // fires when the body has fewer leading lambdas than the signature's arity, so
    // it still auto-binds a record ahead of a later explicit lambda parameter.
    let src = "@mod M\n\
               $ label : { y: Int, z: Int } -> Int = \\q = q.y * 100 + q.z\n\
               $ add : { x: Int, y: Int } -> Int = x + y\n\
               $ f : { x: Int, y: Int } -> Int -> Int = \\n = x + y + n\n\
               $ r : Int = label { .y = 2, .z = 3 } + add { .x = 5, .y = 6 } + f {3, 4} 5";
    assert_eq!(run(src, "r"), "226"); // 203 + 11 + 12
}

#[test]
fn record_promotion_and_named_args() {
    // A record parameter can be called positionally (promoted), by name, or by
    // name reordered; a one-field record param accepts a bare scalar.
    let src = "@mod M\n\
               $ add : {x: Int, y: Int} -> Int = x + y\n\
               $ inc : {x: Int} -> Int = x + 1\n\
               $ r : Int =\n\
               \tadd {5, 6} + add { .x = 5, .y = 6 } + add { .y = 6, .x = 5 } + inc 20 + inc { .x = 20 }";
    assert_eq!(run(src, "r"), "75"); // 11 + 11 + 11 + 21 + 21
}

#[test]
fn record_destructuring_pattern() {
    // Destructure a record by field name (match arm, and lambda shorthand), on an
    // open-row value and a nominal struct; `.._` ignores the rest.
    let src = "@mod M\n\
               $ Point : @struct = x: Int, y: Int,\n\
               $ area : { x: Int, y: Int | r } -> Int = \\p = is p | { .x = a, .y = b, .._ } => a * b\n\
               $ sumxy : { x: Int, y: Int | r } -> Int = \\{ .x, .y } = x + y\n\
               $ nx : Point -> Int = \\p = is p | { .x = a } => a\n\
               $ r : Int = area { .x = 3, .y = 4, .tag = 9 } + sumxy { .x = 5, .y = 6 } + nx Point.{ .x = 2, .y = 8 }";
    assert_eq!(run(src, "r"), "25"); // 12 + 11 + 2
}

#[test]
fn generic_struct_satisfies_an_open_row() {
    // A generic struct instance (`Box Int`, `Pair Int Str`) bridges to an open
    // record row by substituting its type arguments for the struct's parameters.
    let src = "@mod M\n\
               $ Box : @struct a = val: a\n\
               $ Pair : @struct a b = fst: a, snd: b\n\
               $ unwrap : { val: v | r } -> v = \\b = b.val\n\
               $ getfst : { fst: a | r } -> a = \\p = p.fst\n\
               $ r : Int = unwrap (Box.{ .val = 42 }) + getfst (Pair.{ .fst = 8, .snd = \"s\" })";
    assert_eq!(run(src, "r"), "50");
}

#[test]
fn record_rest_binds_the_leftover_fields() {
    // `..rest` binds the record minus the listed labels. Concrete case: matching a
    // `Point3` and binding `..rest` yields `{ y, z }`, readable and forwardable.
    // Open case: over `{ x:Int | r }`, `rest` is the polymorphic remainder.
    let src = "@mod M\n\
               $ Point3 : @struct = x: Int, y: Int, z: Int\n\
               $ sum2 : { y:Int, z:Int } -> Int = y + z\n\
               $ split : Point3 -> Int = \\p = is p | { .x = a, ..rest } => a + sum2 rest\n\
               $ drop_x : { x:Int, y:Int | r } -> Int = \\p = is p | { .x = a, ..rest } => rest.y + a\n\
               $ r : Int = split (Point3.{ .x=1, .y=2, .z=3 }) + drop_x (Point3.{ .x=10, .y=20, .z=30 })";
    assert_eq!(run(src, "r"), "36"); // (1 + (2+3)) + (20 + 10)
}

#[test]
fn codata_stream_is_lazy_and_infinite() {
    // A codata stream: construction is finite (thunks), and observing drives the
    // generative recursion lazily, so an infinite stream is fine.
    let src = "@mod M\n\
               $ Stream : @codata t = head : t, tail : Stream t,\n\
               $ from : Int -> Stream Int = \\n = { .head = n, .tail = from (n + 1) }\n\
               $ nth : Int -> Stream t -> t = \\n s = if n ?= 0 => s.head else nth (n - 1) s.tail\n\
               $ r : Int = (from 10).head + nth 5 (from 10)";
    assert_eq!(run(src, "r"), "25"); // 10 + 15
}

#[test]
fn imported_global_does_not_shadow_a_same_named_effect_op() {
    // Module A exports a plain global `get`; module B has a `State` effect whose
    // operation is also `get`. In the combined program B's bare `get` must resolve
    // to the operation, not A's imported global (which aliases into the bare-name
    // fallback). Regression for the cross-module glob-resolution gap.
    let a = "@mod A\n$ get : Int -> Int = \\x = x + 1000";
    let b = "@mod B\n\
        $ State : @effect = get : {} -> Int, put : Int -> {},\n\
        $ getit : {} -> <State> Int = \\u = get {}\n\
        $ run : Int = do getit {} ctl k | State.get u => k 42 | State.put v => k {} else r => r";
    let root = "@mod M\n$ with A\n$ with B\n$ r : Int = B.run";
    assert_eq!(run_modules(&[a, b, root], "r"), "42");
}

#[test]
fn same_named_struct_types_in_two_modules_do_not_collide() {
    // Two modules each declare a `Pair` struct with DIFFERENT fields. A positional
    // struct pattern in B must be lowered against B's own layout, not A's (whose
    // fields differ), which otherwise faults with "no field ...". Regression for
    // the shared-`Decls` type collision.
    let a = "@mod A\n\
        $ Pair : @struct = fst: Int, snd: Int\n\
        $ afst : Int = Pair.{ .fst = 7, .snd = 8 }.fst";
    let b = "@mod B\n\
        $ Pair : @struct = a: Int, b: Int\n\
        $ first : Pair -> Int = \\p = is p | Pair.{ x, y } => x\n\
        $ bfst : Int = first Pair.{ .a = 3, .b = 4 }";
    let root = "@mod M\n$ with A\n$ with B\n$ r : Int = B.bfst";
    assert_eq!(run_modules(&[a, b, root], "r"), "3");
}

#[test]
fn defer_runs_cleanup_on_completion_abort_and_nesting() {
    // A `Y` handler that sums every yielded value; the deferred cleanups perform
    // `Y.yield`, so their effects are observable in the total.
    let prelude = "@mod M\n\
        $ Y : @effect = yield : Int -> {},\n\
        $ Exn : @effect = throw : Str -> a,\n\
        $ sum : ({} -> <Y> Int) -> Int = \
          \\body = do body {} ctl k | Y.yield v => v + k {} else r => r\n";
    // Normal completion: body yields 1 and returns 100, cleanup yields 2 -> 103.
    let normal =
        format!("{prelude}$ r : Int = sum (\\_ = defer Y.yield 2 do (let _ = Y.yield 1 in 100))");
    assert_eq!(run(&normal, "r"), "103");
    // Abort: the inner handler drops the continuation, but the cleanup (yield 9)
    // still runs under the enclosing `Y` handler -> 9.
    let abort = format!(
        "{prelude}$ r : Int = sum (\\_ = \
         do (defer Y.yield 9 do (let _ = Exn.throw \"x\" in 100)) ctl k | Exn.throw e => 0)"
    );
    assert_eq!(run(&abort, "r"), "9");
    // Nested defers run innermost-first: 1 + 2 + 3 = 6.
    let nested = format!(
        "{prelude}$ r : Int = sum (\\_ = \
         defer Y.yield 3 do (defer Y.yield 2 do (let _ = Y.yield 1 in 0)))"
    );
    assert_eq!(run(&nested, "r"), "6");
}

#[test]
fn defer_cleanup_runs_when_a_stored_continuation_completes() {
    // The handler stores the continuation instead of resuming; the cleanup runs
    // only when that continuation is later driven to completion (two steps).
    let src = "@mod M\n\
        $ Co : @effect = step : Int -> {},\n\
        $ Task : @union = Fin: {}, Susp: { {} -> Task },\n\
        $ spawn : ({} -> <Co> {}) -> Task = \
          \\t = do t {} ctl k | step v => Task.Susp.{ k } else _ => Task.Fin.{}\n\
        $ drive : Task -> Int = \
          \\t = is t | Task.Fin.{} => 0 | Task.Susp.{ k } => 1 + drive (k {}) else 0\n\
        $ r : Int = drive (spawn (\\_ = defer step 2 do (let _ = step 1 in {})))";
    assert_eq!(run(src, "r"), "2");
}

#[test]
fn extern_ffi_file_roundtrip() {
    // The `@extern` FFI host table, exercised directly (the `C` namespace is the
    // same bindings injected by the driver). Open for write, put bytes, close;
    // reopen for read, read them back, then remove the file. "hi" is 104 and 105.
    let src = "@mod M\n\
        $ fopen : Str -> Str -> Int = @extern \"C\" \"fopen\" \"libc\"\n\
        $ fputs : Str -> Int -> Int = @extern \"C\" \"fputs\" \"libc\"\n\
        $ fclose : Int -> Int = @extern \"C\" \"fclose\" \"libc\"\n\
        $ fgetc : Int -> Int = @extern \"C\" \"fgetc\" \"libc\"\n\
        $ remove : Str -> Int = @extern \"C\" \"remove\" \"libc\"\n\
        $ p : Str = \"/tmp/thrax_core_roundtrip.txt\"\n\
        $ r : Int = \
          let f = fopen p \"wb\" in \
          fputs \"hi\" f ; fclose f ; \
          let g = fopen p \"rb\" in \
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
        $ iabs   : Int -> Int   = @extern \"C\" \"abs\"    \"libc\"\n\
        $ dup    : Str -> Str   = @extern \"C\" \"strdup\" \"libc\"\n\
        $ expm1r : Real -> Real = @extern \"C\" \"expm1\"  \"libm\"\n\
        $ r : Int = iabs (0 - 7) + array_len (dup \"abcde\") \
                  + (if expm1r 1.0 ?> 1.7 => 100 else 0)";
    assert_eq!(run(src, "r"), "112");
}

#[test]
fn target_reflects_the_host_consistently() {
    // Host-agnostic invariants: the word and pointer widths agree, and `name` is
    // exactly `arch-os`.
    let src = "@mod M\n$ r : Int = \
        if TARGET.int_bits ?= TARGET.ptr_bits \
        => (if (TARGET.arch ++ \"-\" ++ TARGET.os) ?= TARGET.name => 0 else 1) \
        else 1";
    assert_eq!(run(src, "r"), "0");
}

#[test]
fn array_literal_lowers_to_byte_vector() {
    // `[..]` in Array context builds a byte vector, so array_* primitives apply.
    let src = "@mod M\n$ a : Array = [10, 20, 30]\n\
               $ n = array_len a\n$ g = array_get a 1";
    assert_eq!(run(src, "n"), "3");
    assert_eq!(run(src, "g"), "20");
}

#[test]
fn array_patterns_destructure_and_guard() {
    let src = "@mod M\n\
               $ sum2 : Array -> Int = \\a = is a | [x, y] => x + y else 0\n\
               $ r = sum2 [4, 5]\n\
               $ miss = sum2 [1, 2, 3]\n\
               $ lit : Array -> Int = \\a = is a | [1, y] => y else 0\n\
               $ hit = lit [1, 42]\n\
               $ no = lit [2, 42]\n\
               $ head : Array -> Int = \\a = is a | [h, ..rest] => h + array_len rest else 0\n\
               $ hd = head [7, 8, 9]";
    assert_eq!(run(src, "r"), "9");
    assert_eq!(run(src, "miss"), "0");
    assert_eq!(run(src, "hit"), "42");
    assert_eq!(run(src, "no"), "0");
    assert_eq!(run(src, "hd"), "9");
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
    let src = "@mod M\n$ fact : Int -> Int = \\n = if n ?= 0 => 1 else n * fact (n - 1)\n\
               $ r = fact 5";
    assert_eq!(run(src, "r"), "120");
}

#[test]
fn mutual_recursion() {
    let src = "@mod M\n\
               $ is_even : Int -> Int = \\n = if n ?= 0 => 1 else is_odd (n - 1)\n\
               $ is_odd  : Int -> Int = \\n = if n ?= 0 => 0 else is_even (n - 1)\n\
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
               $ sum : List Int -> Int = \\xs = is xs | [] => 0 | h :: t => h + sum t else 0\n\
               $ a = sum [1, 2, 3, 4, 5]";
    assert_eq!(run(src, "a"), "15");
    let cons = "@mod M\n\
                $ sum : List Int -> Int = \\xs = is xs | [] => 0 | h :: t => h + sum t else 0\n\
                $ a = sum (1 :: 2 :: 3 :: [])";
    assert_eq!(run(cons, "a"), "6");
}

#[test]
fn union_construction_and_nested_match() {
    let src = "@mod M\n\
               $ Peano : @union = Zero: {}, Succ: { Peano }\n\
               $ depth : Peano -> Int = \\n = \
                 is n | Peano.Zero => 0 \
                 | Peano.Succ.{ Peano.Zero } => 1 \
                 | Peano.Succ.{ Peano.Succ.{ _ } } => 2\n\
               $ a = depth Peano.Succ.{ Peano.Succ.{ Peano.Zero } }";
    assert_eq!(run(src, "a"), "2");
}

#[test]
fn struct_field_access_and_match() {
    let src = "@mod M\n\
               $ Point : @struct = x: Int, y: Int\n\
               $ p : Point = Point.{ .x = 3, .y = 4 }\n\
               $ a = p.x + p.y";
    assert_eq!(run(src, "a"), "7");
    let m = "@mod M\n\
             $ Point : @struct = x: Int, y: Int\n\
             $ sum : Point -> Int = \\pt = is pt | Point.{ x, y } => x + y else 0\n\
             $ a = sum Point.{ .x = 10, .y = 20 }";
    assert_eq!(run(m, "a"), "30");
}

#[test]
fn with_scopes_struct_fields() {
    let src = "@mod M\n\
               $ Point : @struct = x: Int, y: Int\n\
               $ p : Point = Point.{ .x = 3, .y = 4 }\n\
               $ a = with p in x + y";
    assert_eq!(run(src, "a"), "7");
}

#[test]
fn record_parameter_destructures() {
    assert_eq!(
        run(
            "@mod M\n$ add : {x: Int, y: Int} -> Int = x + y\n$ a = add {3, 4}",
            "a"
        ),
        "7"
    );
    assert_eq!(
        run("@mod M\n$ inc : {x: Int} -> Int = x + 1\n$ a = inc 5", "a"),
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
                 $ classify : Int -> Int = \\n = is n | m if m ?> 0 => 1 | _ => 0\n\
                 $ a = classify 5";
    assert_eq!(run(guard, "a"), "1");
}

#[test]
fn string_concat_and_prefix_match() {
    assert_eq!(run("@mod M\n$ a = \"hi\" ++ \"!\"", "a"), "\"hi!\"");
    let src = "@mod M\n\
               $ verb : Str -> Int = \\s = is s | \"GET \" ++ _ => 1 else 0\n\
               $ a = verb \"GET /\"";
    assert_eq!(run(src, "a"), "1");
}

#[test]
fn sequencing_returns_last() {
    assert_eq!(run("@mod M\n$ a = 1 ; 2 ; 3", "a"), "3");
}

/// The machine's explicit continuation stack makes deep tail recursion run in
/// constant host stack (where a stack-recursive evaluator would overflow): a
/// tail-recursive countdown to a depth that would blow the native stack returns.
#[test]
fn deep_tail_recursion_is_constant_stack() {
    let src = "@mod T\n\
               $ loop : Int -> Int = \\n = if n ?= 0 => 42 else loop (n - 1)\n\
               $ test : Int = loop 1000000\n";
    assert_eq!(run(src, "test"), "42");
}
