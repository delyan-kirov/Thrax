use crate::*;

/// Infer the type of a single global `x`, rendered as a string.
fn type_of(src: &str, name: &str) -> String {
    let parsed = crate::parse(src).expect("parse");
    let mut checker = Checker::new(&parsed.ast);
    let results = checker
        .check_program(&parsed.program)
        .unwrap_or_else(|e| panic!("{}", e.render(src, "test.thx")));
    let ty = results
        .iter()
        .find(|(n, _)| *n == name)
        .expect("name present")
        .1
        .clone();
    checker.show(&ty)
}

fn errors(src: &str) -> String {
    let parsed = crate::parse(src).expect("parse");
    match Checker::new(&parsed.ast).check_program(&parsed.program) {
        Ok(_) => String::new(),
        Err(e) => format!("{e}"),
    }
}

#[test]
fn undeclared_type_param_is_error() {
    // Parameters are mandatory: a free tyvar with no declared list is rejected.
    let e = errors("@mod M\n$ Box : @struct = val: t\n$ x : Int = 0");
    assert!(e.contains("declares no type parameters"), "{e}");
}

#[test]
fn parameterized_alias_expands() {
    // `MapInt` fixes the key to Int, so `MapInt Bool` elaborates to `Map Int Bool`.
    let src = "@mod M\n\
               $ Map : @struct k v = key: k, val: v\n\
               $ MapInt : @alias v = Map Int v\n\
               $ m : MapInt Bool = .{ .key = 1, .val = true }";
    assert_eq!(type_of(src, "m"), "Map Int Bool");
}

#[test]
fn monomorphic_arithmetic() {
    assert_eq!(type_of("@mod M\n$ x = 1 + 2", "x"), "Int");
}

#[test]
fn identity_is_polymorphic() {
    // The classic let-generalization test: `id` gets `forall a. a -> a`.
    assert_eq!(type_of("@mod M\n$ id = \\x = x", "id"), "a -> a");
}

#[test]
fn polymorphic_id_used_at_two_types() {
    // Using a generalized `id` at Int and Str must not conflict.
    let src = "@mod M\n\
                   $ id = \\x = x\n\
                   $ pair = let a = id 1, b = id \"s\" in {a, b}";
    assert_eq!(type_of(src, "pair"), "{Int, Str}");
}

#[test]
fn function_application_and_arrows() {
    let src = "@mod M\n$ apply = \\f x = f x";
    // `(A -> B) -> A -> B`.
    assert_eq!(type_of(src, "apply"), "(a -> b) -> a -> b");
}

#[test]
fn if_unifies_branches_and_condition() {
    assert_eq!(
        type_of("@mod M\n$ f = \\n = if n ?= 0 => 1 else n", "f"),
        "Int -> Int"
    );
}

#[test]
fn recursion_through_predeclared_globals() {
    let src = "@mod M\n$ fib = \\n = if n ?= 0 => 0 else fib (n - 1) + fib (n - 2)";
    assert_eq!(type_of(src, "fib"), "Int -> Int");
}

#[test]
fn tuples_and_lists() {
    assert_eq!(type_of("@mod M\n$ p = {1, \"a\"}", "p"), "{Int, Str}");
    assert_eq!(type_of("@mod M\n$ xs = [1, 2, 3]", "xs"), "List Int");
}

#[test]
fn signature_is_checked() {
    assert_eq!(
        type_of("@mod M\n$ f : Int -> Int = \\x = x + 1", "f"),
        "Int -> Int"
    );
}

#[test]
fn occurs_check_rejects_self_application() {
    // `\x = x x` has no finite type.
    assert!(errors("@mod M\n$ bad = \\x = x x").contains("infinite type"));
}

#[test]
fn type_mismatch_is_reported() {
    assert!(errors("@mod M\n$ bad = 1 + \"s\"").contains("TYPE_MISMATCH"));
}

#[test]
fn unbound_name_is_reported() {
    assert!(errors("@mod M\n$ bad = nope 1").contains("TYPE_UNBOUND"));
}

#[test]
fn struct_field_access_is_typed() {
    let src = "@mod M\n\
                   $ P : @struct = x: Int, y: Str\n\
                   $ p : P = P.{ .x = 1, .y = \"s\" }\n\
                   $ gx = p.x\n\
                   $ gy = p.y";
    assert_eq!(type_of(src, "gx"), "Int");
    assert_eq!(type_of(src, "gy"), "Str");
}

#[test]
fn generic_struct_instantiates_per_use() {
    let src = "@mod M\n\
                   $ Box : @struct t = val: t\n\
                   $ b : Box Str = Box.{ .val = \"hi\" }\n\
                   $ out = b.val";
    assert_eq!(type_of(src, "b"), "Box Str");
    assert_eq!(type_of(src, "out"), "Str");
}

#[test]
fn union_constructor_and_match() {
    let src = "@mod M\n\
                   $ Maybe : @union t = Just: t, None: {}\n\
                   $ m : Maybe Int = Maybe.Just.{ 7 }\n\
                   $ get = \\d = \\o = is o | Maybe.Just.{x} => x else d";
    assert_eq!(type_of(src, "m"), "Maybe Int");
    assert_eq!(type_of(src, "get"), "a -> Maybe a -> a");
}

#[test]
fn mutually_recursive_globals_via_scc() {
    let src = "@mod M\n\
                   $ is_even : Int -> Int = \\n = if n ?= 0 => 1 else is_odd (n - 1)\n\
                   $ is_odd  : Int -> Int = \\n = if n ?= 0 => 0 else is_even (n - 1)";
    assert_eq!(type_of(src, "is_even"), "Int -> Int");
    assert_eq!(type_of(src, "is_odd"), "Int -> Int");
}

#[test]
fn recursive_let_binding() {
    let src = "@mod M\n\
                   $ f = \\m = let go = \\n acc = if n ?= 0 => acc else go (n - 1) (acc + n) \
                   in go m 0";
    assert_eq!(type_of(src, "f"), "Int -> Int");
}

#[test]
fn arithmetic_is_overloaded_on_int_and_real() {
    assert_eq!(type_of("@mod M\n$ a = 1 + 2", "a"), "Int");
    assert_eq!(type_of("@mod M\n$ a = 1.0 + 2.0", "a"), "Real");
    // A real anywhere in the chain makes an integer literal Real.
    assert_eq!(type_of("@mod M\n$ a = 1 + 2.0", "a"), "Real");
}

#[test]
fn user_overload_resolves_by_argument_type() {
    let src = "@mod M\n\
                   $ f : Int -> Int = \\x = x + 1\n\
                   $ f : Str -> Str = \\x = x ++ \"!\"\n\
                   $ a = f 3\n\
                   $ b = f \"hi\"";
    assert_eq!(type_of(src, "a"), "Int");
    assert_eq!(type_of(src, "b"), "Str");
}

#[test]
fn overload_deferred_until_signature_pins_operands() {
    // `p1.x + p2.x` cannot resolve until the signature makes the params
    // structs; bidirectional checking + the pending fixpoint handle it.
    let src = "@mod M\n\
                   $ P : @struct = x: Int, y: Int\n\
                   $ add : P -> P -> P = \\p1 p2 = P.{ .x = p1.x + p2.x, .y = p1.y + p2.y }";
    assert_eq!(type_of(src, "add"), "P -> P -> P");
}

#[test]
fn no_matching_overload_is_reported() {
    let src = "@mod M\n\
                   $ f : Int -> Int = \\x = x\n\
                   $ f : Str -> Str = \\x = x\n\
                   $ bad = f 1.0";
    assert!(errors(src).contains("no overload"));
}

#[test]
fn cross_module_import_brings_in_types_and_values() {
    let dep_src = "@mod OPT\n\
                       $ Option : @union t = Some: t, None: {}\n\
                       $ is_some : Option t -> Bool = \\o = \
                       is o | Option.Some.{_} => true else false\n\
                       $ unwrap_or : Option t -> t -> t = \\o d = \
                       is o | Option.Some.{x} => x else d";
    let use_src = "@mod U\n\
                       $ with OPT\n\
                       $ o : Option Int = Option.Some.{ 41 }\n\
                       $ present = is_some o\n\
                       $ value = unwrap_or o 0";
    // Both modules parse into one shared `Ast` so cross-module handles resolve.
    let (ast, dep) = crate::parse_into(crate::Ast::new(), dep_src).expect("parse dep");
    let (ast, program) = crate::parse_into(ast, use_src).expect("parse use");
    let mut dep_checker = Checker::new(&ast);
    dep_checker.check_program(&dep).expect("check dep");
    let mut checker = Checker::new(&ast);
    checker.import_from(&dep_checker);
    let results = checker
        .check_program(&program)
        .unwrap_or_else(|e| panic!("{}", e.render(use_src, "U")));
    let ty = |name: &str| checker.show(&results.iter().find(|(n, _)| *n == name).unwrap().1);
    assert_eq!(ty("present"), "Bool");
    assert_eq!(ty("value"), "Int");
}

#[test]
fn record_parameters_bind_fields_directly() {
    // A record parameter is a real record type; its fields auto-bind in the body.
    assert_eq!(
        type_of("@mod M\n$ add : {x: Int, y: Int} -> Int = x + y", "add"),
        "{ x: Int, y: Int } -> Int"
    );
    // A one-field record is a one-field record (no collapse); a scalar promotes
    // to it at the call site.
    assert_eq!(
        type_of("@mod M\n$ inc : {x: Int} -> Int = x + 1", "inc"),
        "{ x: Int } -> Int"
    );
}

#[test]
fn with_scopes_struct_fields() {
    let src = "@mod M\n\
                   $ P : @struct = x: Int, y: Int\n\
                   $ sum : P -> Int = \\p = with p in x + y";
    assert_eq!(type_of(src, "sum"), "P -> Int");
}

#[test]
fn sequence_literal_is_type_directed() {
    // The same `[..]` is an Array in Array context, a List otherwise.
    assert_eq!(type_of("@mod M\n$ a : Array = [10, 20]", "a"), "Array");
    assert_eq!(
        type_of("@mod M\n$ a : List Int = [1, 2, 3]", "a"),
        "List Int"
    );
    // An Array-typed parameter directs a `[..]` argument at the call site.
    let src = "@mod M\n\
                   $ len : Array -> Int = \\a = @array_len a\n\
                   $ n = len [1, 2]";
    assert_eq!(type_of(src, "n"), "Int");
}

#[test]
fn effect_operation_is_bound_and_handler_types_result() {
    // `get`/`put` are bound from the effect declaration; `tick` performs State, so
    // its arrow carries the `<State>` row; the `do/ctl` handler discharges it and
    // types to the clause/`else` bodies' common type.
    let src = "@mod M\n\
                   $ State : @effect = get : {} -> Int, put : Int -> {},\n\
                   $ tick : {} -> <State> Int = \\u = let x = get {} in let _ = put (x + 1) in x\n\
                   $ run : Int = do tick {} ctl k | get u => k 0 | put n => k {} else x => x";
    assert_eq!(type_of(src, "tick"), "{} -> <State> Int");
    assert_eq!(type_of(src, "run"), "Int");
}

#[test]
fn unknown_type_name_is_rejected() {
    // A bare capitalized name in type position must be a known type; a type
    // variable is a lowercase name. `Itn` is a typo, not a type variable.
    assert!(errors("@mod M\n$ x : Itn = 5").contains("unknown type `Itn`"));
}

#[test]
fn type_variable_and_sized_and_declared_types_are_accepted() {
    // `a` is a type variable; `Int8`/`Ptr` are base types; a declared union
    // name is known. None of these is an "unknown type".
    let src = "@mod M\n\
               $ Box : @union = Wrap: { Int },\n\
               $ id : a -> a = \\x = x\n\
               $ n : Int8 = 5\n\
               $ p : Ptr = 0\n\
               $ b : Box = Box.Wrap.{ 1 }";
    assert_eq!(errors(src), "");
    assert_eq!(type_of(src, "id"), "a -> a");
    assert_eq!(type_of(src, "n"), "Int8");
}

#[test]
fn unhandled_effect_is_a_compile_error() {
    // A function that performs an effect but declares a pure (or too-small) type
    // is rejected: the latent `<State>` cannot be subsumed into the empty ambient.
    let src = "@mod M\n\
                   $ State : @effect = get : {} -> Int, put : Int -> {},\n\
                   $ bad : {} -> Int = \\u = get {}";
    assert!(
        errors(src).contains("not handled"),
        "expected an unhandled-effect error, got: {:?}",
        errors(src)
    );
}

#[test]
fn top_level_unhandled_effect_is_rejected() {
    // Performing an effect at the top level (no handler in scope) is a compile
    // error, since a top-level body is typed under the empty closed row.
    let src = "@mod M\n\
                   $ Yield : @effect = yield : Int -> {},\n\
                   $ oops : {} = yield 1";
    assert!(
        errors(src).contains("not handled"),
        "expected an unhandled-effect error, got: {:?}",
        errors(src)
    );
}

#[test]
fn same_operation_in_two_effects_resolves_by_result_type() {
    // `ask` is declared by two effects (Int and Str result); a bare use is an
    // overload resolved by how the result is used, and `Effect.op` disambiguates.
    let src = "@mod M\n\
                   $ Reader : @effect = ask : {} -> Int,\n\
                   $ Config : @effect = ask : {} -> Str,\n\
                   $ n : Int = do (ask {}) + 1 ctl k | Reader.ask u => k 10\n\
                   $ s : Str = do Config.ask {} ctl k | Config.ask u => k \"hi\"";
    assert_eq!(type_of(src, "n"), "Int");
    assert_eq!(type_of(src, "s"), "Str");
}

#[test]
fn array_primitives_overload_on_array_and_str() {
    let src = "@mod M\n\
                   $ a : Array = @array_push (@array.{ 0 }) 65\n\
                   $ n = @array_len a\n\
                   $ b = @array_get \"hi\" 0";
    assert_eq!(type_of(src, "a"), "Array");
    assert_eq!(type_of(src, "n"), "Int");
    assert_eq!(type_of(src, "b"), "Int");
}

/// The computed C layout of a named C-repr struct, or a panic with the check error.
fn crepr_layout(src: &str, name: &str) -> utilities::CLayout {
    let parsed = crate::parse(src).expect("parse");
    let mut checker = Checker::new(&parsed.ast);
    checker
        .check_program(&parsed.program)
        .unwrap_or_else(|e| panic!("{}", e.render(src, "test.thx")));
    checker
        .crepr_layouts()
        .get(name)
        .cloned()
        .expect("crepr layout present")
}

#[test]
fn crepr_struct_layout_vector2_and_color() {
    let src = "@mod M\n\
        $ Vector2 : @struct @extern \"C\" = x: Real32, y: Real32,\n\
        $ Color : @struct @extern \"C\" = r: Nat8, g: Nat8, b: Nat8, a: Nat8,\n\
        $ x : Int = 0";
    let v = crepr_layout(src, "Vector2");
    assert_eq!((v.size, v.align), (8, 4));
    assert_eq!(v.fields[1].offset, 4);
    let c = crepr_layout(src, "Color");
    assert_eq!((c.size, c.align), (4, 1));
    assert_eq!(c.fields[3].offset, 3);
}

#[test]
fn c_union_layout_overlaps() {
    // `@union @extern "C"` is a C union: members share offset 0, size is the largest.
    let src = "@mod M\n\
        $ U : @union @extern \"C\" = i: Int32, d: Real64,\n\
        $ x : Int = 0";
    let u = crepr_layout(src, "U");
    assert!(u.is_union);
    assert_eq!(u.fields[0].offset, 0);
    assert_eq!(u.fields[1].offset, 0);
    assert_eq!((u.size, u.align), (8, 8));
}

#[test]
fn crepr_struct_rejects_non_c_field() {
    let e = errors("@mod M\n$ Bad : @struct @extern \"C\" = s: Str,");
    assert!(e.contains("not C-representable"), "{e}");
}

#[test]
fn crepr_struct_rejects_generic() {
    let e = errors("@mod M\n$ Bad : @struct @extern \"C\" a = v: a,");
    assert!(e.contains("may not be generic"), "{e}");
}

#[test]
fn real_literal_takes_real32_width() {
    // A `Real` literal checks against a `Real32` expected type (like an integer
    // literal takes its width), so a float C struct binds with plain literals.
    let src = "@mod M\n\
        $ Vector2 : @struct @extern \"C\" = x: Real32, y: Real32,\n\
        $ v : Vector2 = Vector2.{ .x = 1.0, .y = 2.5 }\n\
        $ f : Real32 = 3.5\n\
        $ g : Real = 3.5";
    assert_eq!(type_of(src, "v"), "Vector2");
    assert_eq!(type_of(src, "f"), "Real32");
    assert_eq!(type_of(src, "g"), "Real");
}

#[test]
fn crepr_struct_nested() {
    let src = "@mod M\n\
        $ Vector2 : @struct @extern \"C\" = x: Real32, y: Real32,\n\
        $ Line : @struct @extern \"C\" = a: Vector2, b: Vector2,\n\
        $ x : Int = 0";
    let l = crepr_layout(src, "Line");
    assert_eq!((l.size, l.align), (16, 4));
    assert_eq!(l.fields[1].offset, 8);
}
