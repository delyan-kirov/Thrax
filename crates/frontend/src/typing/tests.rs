use crate::*;

/// The implicitly imported CORE module (defines `to_string` and the `+ - * / %`
/// operator overloads). Injected so a test using arithmetic resolves it, as the
/// driver does for every real program.
const CORE_SRC: &str = include_str!("../../../../library/CORE.thx");

/// Infer the type of a single global `x`, rendered as a string. CORE is parsed
/// and imported first (so arithmetic and other CORE names resolve), as the driver
/// does for every real program.
fn type_of(src: &str, name: &str) -> String {
    let (ast, core) = crate::parse_into(Ast::new(), CORE_SRC).expect("parse CORE");
    let (ast, prog) = crate::parse_into(ast, src).expect("parse");
    let mut core_checker = Checker::new(&ast);
    core_checker.check_program(&core).expect("check CORE");
    let mut checker = Checker::new(&ast);
    checker.import_from(&core_checker);
    let results = checker
        .check_program(&prog)
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
    let (ast, core) = crate::parse_into(Ast::new(), CORE_SRC).expect("parse CORE");
    let (ast, prog) = crate::parse_into(ast, src).expect("parse");
    let mut core_checker = Checker::new(&ast);
    core_checker.check_program(&core).expect("check CORE");
    let mut checker = Checker::new(&ast);
    checker.import_from(&core_checker);
    match checker.check_program(&prog) {
        Ok(_) => String::new(),
        Err(e) => format!("{e}"),
    }
}

#[test]
fn undeclared_type_param_is_error() {
    // Parameters are mandatory: a free tyvar with no declared list is rejected.
    let e = errors("@mod M\n$ Box : @struct = val: t\n$ x : @int = 0");
    assert!(e.contains("declares no type parameters"), "{e}");
}

#[test]
fn parameterized_alias_expands() {
    // `MapInt` fixes the key to @int, so `MapInt @bool` elaborates to `Map @int @bool`.
    let src = "@mod M\n\
               $ Map : @struct k v = key: k, val: v\n\
               $ MapInt : @alias v = Map @int v\n\
               $ m : MapInt @bool = .{ .key = 1, .val = @true }";
    assert_eq!(type_of(src, "m"), "Map @int @bool");
}

#[test]
fn monomorphic_arithmetic() {
    assert_eq!(type_of("@mod M\n$ x = 1 + 2", "x"), "@int");
}

#[test]
fn identity_is_polymorphic() {
    // The classic let-generalization test: `id` gets `forall a. a -> a`.
    assert_eq!(type_of("@mod M\n$ id = \\x = x", "id"), "a -> a");
}

#[test]
fn polymorphic_id_used_at_two_types() {
    // Using a generalized `id` at @int and @str must not conflict.
    let src = "@mod M\n\
                   $ id = \\x = x\n\
                   $ pair = let a = id 1, b = id \"s\" in {a, b}";
    assert_eq!(type_of(src, "pair"), "{@int, @str}");
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
        "@int -> @int"
    );
}

#[test]
fn recursion_through_predeclared_globals() {
    let src = "@mod M\n$ fib = \\n = if n ?= 0 => 0 else fib (n - 1) + fib (n - 2)";
    assert_eq!(type_of(src, "fib"), "@int -> @int");
}

#[test]
fn tuples_and_lists() {
    assert_eq!(type_of("@mod M\n$ p = {1, \"a\"}", "p"), "{@int, @str}");
    assert_eq!(type_of("@mod M\n$ xs = [1, 2, 3]", "xs"), "@vec @int");
}

#[test]
fn signature_is_checked() {
    assert_eq!(
        type_of("@mod M\n$ f : @int -> @int = \\x = x + 1", "f"),
        "@int -> @int"
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
                   $ P : @struct = x: @int, y: @str\n\
                   $ p : P = P.{ .x = 1, .y = \"s\" }\n\
                   $ gx = p.x\n\
                   $ gy = p.y";
    assert_eq!(type_of(src, "gx"), "@int");
    assert_eq!(type_of(src, "gy"), "@str");
}

#[test]
fn generic_struct_instantiates_per_use() {
    let src = "@mod M\n\
                   $ Box : @struct t = val: t\n\
                   $ b : Box @str = Box.{ .val = \"hi\" }\n\
                   $ out = b.val";
    assert_eq!(type_of(src, "b"), "Box @str");
    assert_eq!(type_of(src, "out"), "@str");
}

#[test]
fn union_constructor_and_match() {
    let src = "@mod M\n\
                   $ Maybe : @union t = Just: t, None: {}\n\
                   $ m : Maybe @int = Maybe.Just.{ 7 }\n\
                   $ get = \\d = \\o = is o | Maybe.Just.{x} => x else d";
    assert_eq!(type_of(src, "m"), "Maybe @int");
    assert_eq!(type_of(src, "get"), "a -> Maybe a -> a");
}

#[test]
fn mutually_recursive_globals_via_scc() {
    let src = "@mod M\n\
                   $ is_even : @int -> @int = \\n = if n ?= 0 => 1 else is_odd (n - 1)\n\
                   $ is_odd  : @int -> @int = \\n = if n ?= 0 => 0 else is_even (n - 1)";
    assert_eq!(type_of(src, "is_even"), "@int -> @int");
    assert_eq!(type_of(src, "is_odd"), "@int -> @int");
}

#[test]
fn recursive_let_binding() {
    let src = "@mod M\n\
                   $ f = \\m = let go = \\n acc = if n ?= 0 => acc else go (n - 1) (acc + n) \
                   in go m 0";
    assert_eq!(type_of(src, "f"), "@int -> @int");
}

#[test]
fn arithmetic_is_overloaded_on_int_and_real() {
    assert_eq!(type_of("@mod M\n$ a = 1 + 2", "a"), "@int");
    assert_eq!(type_of("@mod M\n$ a = 1.0 + 2.0", "a"), "@float64");
    // An integer literal never adopts a float type: mixing one with a real literal
    // is an error, not a silent promotion. Write the float literal instead.
    assert!(errors("@mod M\n$ a = 1 + 2.0").contains("no viable overload"));
}

#[test]
fn user_overload_resolves_by_argument_type() {
    let src = "@mod M\n\
                   $ f : @int -> @int = \\x = x + 1\n\
                   $ f : @str -> @str = \\x = x ++ \"!\"\n\
                   $ a = f 3\n\
                   $ b = f \"hi\"";
    assert_eq!(type_of(src, "a"), "@int");
    assert_eq!(type_of(src, "b"), "@str");
}

#[test]
fn overload_deferred_until_signature_pins_operands() {
    // `p1.x + p2.x` cannot resolve until the signature makes the params
    // structs; bidirectional checking + the pending fixpoint handle it.
    let src = "@mod M\n\
                   $ P : @struct = x: @int, y: @int\n\
                   $ add : P -> P -> P = \\p1 p2 = P.{ .x = p1.x + p2.x, .y = p1.y + p2.y }";
    assert_eq!(type_of(src, "add"), "P -> P -> P");
}

#[test]
fn sized_literal_arithmetic_takes_the_result_type() {
    // Two bare literals in a sized-int context resolve to that sized overload,
    // not `@int`: the result type is propagated onto the operands before they
    // would default to `@int` (which has no `@int + @int -> @int32`).
    assert_eq!(type_of("@mod M\n$ x : @int32 = 2 + 3", "x"), "@int32");
    assert_eq!(type_of("@mod M\n$ x : @int64 = 2 + 3 * 4", "x"), "@int64");
    assert_eq!(type_of("@mod M\n$ x : @nat16 = 7 - 1", "x"), "@nat16");
    // With no sized context, bare literals still default to `@int`.
    assert_eq!(type_of("@mod M\n$ x = 2 + 3", "x"), "@int");
}

#[test]
fn no_matching_overload_is_reported() {
    let src = "@mod M\n\
                   $ f : @int -> @int = \\x = x\n\
                   $ f : @str -> @str = \\x = x\n\
                   $ bad = f 1.0";
    assert!(errors(src).contains("no viable overload"));
}

#[test]
fn cross_module_import_brings_in_types_and_values() {
    let dep_src = "@mod OPT\n\
                       $ Option : @union t = Some: t, None: {}\n\
                       $ is_some : Option t -> @bool = \\o = \
                       is o | Option.Some.{_} => @true else @false\n\
                       $ unwrap_or : Option t -> t -> t = \\o d = \
                       is o | Option.Some.{x} => x else d";
    let use_src = "@mod U\n\
                       $ with OPT\n\
                       $ o : Option @int = Option.Some.{ 41 }\n\
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
    assert_eq!(ty("present"), "@bool");
    assert_eq!(ty("value"), "@int");
}

#[test]
fn record_parameters_bind_fields_directly() {
    // A record parameter is a real record type; its fields auto-bind in the body.
    assert_eq!(
        type_of("@mod M\n$ add : {x: @int, y: @int} -> @int = x + y", "add"),
        "{ x: @int, y: @int } -> @int"
    );
    // A one-field record is a one-field record (no collapse); a scalar promotes
    // to it at the call site.
    assert_eq!(
        type_of("@mod M\n$ inc : {x: @int} -> @int = x + 1", "inc"),
        "{ x: @int } -> @int"
    );
}

#[test]
fn with_scopes_struct_fields() {
    let src = "@mod M\n\
                   $ P : @struct = x: @int, y: @int\n\
                   $ sum : P -> @int = \\p = with p in x + y";
    assert_eq!(type_of(src, "sum"), "P -> @int");
}

#[test]
fn sequence_literal_is_type_directed() {
    // The same `[..]` is an @array in @array context, a @vec otherwise.
    assert_eq!(type_of("@mod M\n$ a : @array = [10, 20]", "a"), "@array");
    assert_eq!(
        type_of("@mod M\n$ a : @vec @int = [1, 2, 3]", "a"),
        "@vec @int"
    );
    // An @array-typed parameter directs a `[..]` argument at the call site.
    let src = "@mod M\n\
                   $ len : @array -> @int = \\a = @array_len a\n\
                   $ n = len [1, 2]";
    assert_eq!(type_of(src, "n"), "@int");
}

#[test]
fn effect_operation_is_bound_and_handler_types_result() {
    // `get`/`put` are bound from the effect declaration; `tick` performs State, so
    // its arrow carries the `<State>` row; the `do/ctl` handler discharges it and
    // types to the clause/`else` bodies' common type.
    let src = "@mod M\n\
                   $ State : @effect = get : {} -> @int, put : @int -> {},\n\
                   $ tick : {} -> <State> @int = \\u = let x = get {} in let _ = put (x + 1) in x\n\
                   $ run : @int = do tick {} ctl k | get u => k 0 | put n => k {} else x => x";
    assert_eq!(type_of(src, "tick"), "{} -> <State> @int");
    assert_eq!(type_of(src, "run"), "@int");
}

#[test]
fn unknown_type_name_is_rejected() {
    // A bare capitalized name in type position must be a known type; a type
    // variable is a lowercase name. `Itn` is a typo, not a type variable.
    assert!(errors("@mod M\n$ x : Itn = 5").contains("unknown type `Itn`"));
}

#[test]
fn numeric_literal_is_not_a_bool_condition() {
    // `if` wants a `@bool`; a bare integer literal is a number, not a truth value,
    // so `if 1` is a type error (it used to silently mean "nonzero").
    assert!(
        errors("@mod M\n$ x : @int = if 1 => 2 else 3").contains("numeric literal"),
        "expected `if 1` to be rejected: {:?}",
        errors("@mod M\n$ x : @int = if 1 => 2 else 3")
    );
    // A real condition (a comparison, or a `@bool`) is fine.
    assert_eq!(errors("@mod M\n$ x : @int = if 1 ?= 1 => 2 else 3"), "");
    assert_eq!(errors("@mod M\n$ x : @int = if @true => 2 else 3"), "");
    // A numeric literal cannot masquerade as a pointer either.
    assert!(errors("@mod M\n$ p : @ptr = 0").contains("numeric literal"));
}

#[test]
fn curried_extern_is_rejected() {
    // A C function has no first-class closure to curry: a multi-parameter extern
    // must group its C parameters into one record, not curry with several arrows.
    let e = errors("@mod M\n$ f : @int -> @int -> @int = @extern \"C\" \"f\" \"lib\"");
    assert!(e.contains("SINGLE argument"), "{e}");
}

#[test]
fn single_argument_externs_are_accepted() {
    // One record groups several C parameters; a lone value and unit are one
    // argument each, so all three shapes pass the extern-shape check.
    let src = "@mod M\n\
               $ f : {a: @int, b: @int} -> @int = @extern \"C\" \"f\" \"lib\"\n\
               $ g : @str -> @int = @extern \"C\" \"g\" \"lib\"\n\
               $ h : {} -> @int = @extern \"C\" \"h\" \"lib\"";
    assert_eq!(errors(src), "");
}

#[test]
fn type_variable_and_sized_and_declared_types_are_accepted() {
    // `a` is a type variable; `@int8`/`@ptr` are base types; a declared union
    // name is known. None of these is an "unknown type".
    let src = "@mod M\n\
               $ Box : @union = Wrap: { @int },\n\
               $ id : a -> a = \\x = x\n\
               $ n : @int8 = 5\n\
               $ p : @ptr -> @ptr = \\x = x\n\
               $ b : Box = Box.Wrap.{ 1 }";
    assert_eq!(errors(src), "");
    assert_eq!(type_of(src, "id"), "a -> a");
    assert_eq!(type_of(src, "n"), "@int8");
}

#[test]
fn unhandled_effect_is_a_compile_error() {
    // A function that performs an effect but declares a pure (or too-small) type
    // is rejected: the latent `<State>` cannot be subsumed into the empty ambient.
    let src = "@mod M\n\
                   $ State : @effect = get : {} -> @int, put : @int -> {},\n\
                   $ bad : {} -> @int = \\u = get {}";
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
                   $ Yield : @effect = yield : @int -> {},\n\
                   $ oops : {} = yield 1";
    assert!(
        errors(src).contains("not handled"),
        "expected an unhandled-effect error, got: {:?}",
        errors(src)
    );
}

#[test]
fn open_row_entry_may_perform_any_effect() {
    // The program entry `main` carries an OPEN effect row `<| e>`, so it may
    // perform any effect without a handler (the runtime is the top handler). A
    // pure `<>` would reject this (see `unhandled_effect_is_a_compile_error`).
    let src = "@mod MAIN\n\
               $ Yell : @effect = shout : @int -> {},\n\
               $ main : {} -> <| e> @int = \\u = let _ = Yell.shout 5 in 0";
    assert_eq!(errors(src), "", "open-row main should type-check");
    assert_eq!(type_of(src, "main"), "{} -> <Yell | a> @int");
}

#[test]
fn classify_entry_recognizes_the_entry_forms() {
    use crate::{classify_entry, EntryKind};
    let kind = |src: &str| {
        let parsed = crate::parse(src).expect("parse");
        let mut checker = Checker::new(&parsed.ast);
        let results = checker.check_program(&parsed.program).expect("check");
        let ty = results.iter().find(|(n, _)| *n == "main").expect("main").1.clone();
        classify_entry(&ty)
    };
    assert_eq!(kind("@mod MAIN\n$ main : {} -> <| e> @int = \\u = 0"), EntryKind::UnitFn);
    assert_eq!(
        kind("@mod MAIN\n$ main : [n]@str -> <| e> @int = \\a = 0"),
        EntryKind::ArgvFn
    );
    assert_eq!(kind("@mod MAIN\n$ main : @int = 0"), EntryKind::Value);
    assert_eq!(kind("@mod MAIN\n$ main : @int -> <| e> @int = \\n = n"), EntryKind::BadFn);
}

#[test]
fn same_operation_in_two_effects_resolves_by_result_type() {
    // `ask` is declared by two effects (@int and @str result); a bare use is an
    // overload resolved by how the result is used, and `Effect.op` disambiguates.
    let src = "@mod M\n\
                   $ Reader : @effect = ask : {} -> @int,\n\
                   $ Config : @effect = ask : {} -> @str,\n\
                   $ n : @int = do (ask {}) + 1 ctl k | Reader.ask u => k 10\n\
                   $ s : @str = do Config.ask {} ctl k | Config.ask u => k \"hi\"";
    assert_eq!(type_of(src, "n"), "@int");
    assert_eq!(type_of(src, "s"), "@str");
}

#[test]
fn array_primitives_overload_on_array_and_str() {
    let src = "@mod M\n\
                   $ a : @array = @array_push (@array.{ 0 }) 65\n\
                   $ n = @array_len a\n\
                   $ b = @array_get \"hi\" 0";
    assert_eq!(type_of(src, "a"), "@array");
    assert_eq!(type_of(src, "n"), "@int");
    assert_eq!(type_of(src, "b"), "@int");
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
        $ Vector2 : @struct @extern \"C\" = x: @float32, y: @float32,\n\
        $ Color : @struct @extern \"C\" = r: @nat8, g: @nat8, b: @nat8, a: @nat8,\n\
        $ x : @int = 0";
    let v = crepr_layout(src, "Vector2");
    assert_eq!((v.size, v.align), (8, 4));
    assert_eq!(v.fields[1].offset, 4);
    let c = crepr_layout(src, "Color");
    assert_eq!((c.size, c.align), (4, 1));
    assert_eq!(c.fields[3].offset, 3);
}

#[test]
fn crepr_field_follows_alias_to_a_struct() {
    // A nullary alias to a C-repr struct (`Quaternion = Vector4`) is itself
    // C-representable as a field: the layout follows the alias to the struct.
    let src = "@mod M\n\
        $ Vector4 : @struct @extern \"C\" = x: @float32, y: @float32, z: @float32, w: @float32,\n\
        $ Quaternion : @alias = Vector4\n\
        $ Transform : @struct @extern \"C\" = rotation: Quaternion, scale: @float32,\n\
        $ x : @int = 0";
    let t = crepr_layout(src, "Transform");
    // rotation is a 16-byte/4-align Vector4, so `scale` starts at offset 16.
    assert_eq!(t.fields[1].offset, 16);
    assert_eq!((t.size, t.align), (20, 4));
}

#[test]
fn c_union_layout_overlaps() {
    // `@union @extern "C"` is a C union: members share offset 0, size is the largest.
    let src = "@mod M\n\
        $ U : @union @extern \"C\" = i: @int32, d: @float64,\n\
        $ x : @int = 0";
    let u = crepr_layout(src, "U");
    assert!(u.is_union);
    assert_eq!(u.fields[0].offset, 0);
    assert_eq!(u.fields[1].offset, 0);
    assert_eq!((u.size, u.align), (8, 8));
}

#[test]
fn crepr_struct_rejects_non_c_field() {
    let e = errors("@mod M\n$ Bad : @struct @extern \"C\" = s: @str,");
    assert!(e.contains("not C-representable"), "{e}");
}

#[test]
fn crepr_struct_rejects_generic() {
    let e = errors("@mod M\n$ Bad : @struct @extern \"C\" a = v: a,");
    assert!(e.contains("may not be generic"), "{e}");
}

#[test]
fn real_literal_takes_real32_width() {
    // A `@float64` literal checks against a `@float32` expected type (like an integer
    // literal takes its width), so a float C struct binds with plain literals.
    let src = "@mod M\n\
        $ Vector2 : @struct @extern \"C\" = x: @float32, y: @float32,\n\
        $ v : Vector2 = Vector2.{ .x = 1.0, .y = 2.5 }\n\
        $ f : @float32 = 3.5\n\
        $ g : @float64 = 3.5";
    assert_eq!(type_of(src, "v"), "Vector2");
    assert_eq!(type_of(src, "f"), "@float32");
    assert_eq!(type_of(src, "g"), "@float64");
}

#[test]
fn crepr_struct_nested() {
    let src = "@mod M\n\
        $ Vector2 : @struct @extern \"C\" = x: @float32, y: @float32,\n\
        $ Line : @struct @extern \"C\" = a: Vector2, b: Vector2,\n\
        $ x : @int = 0";
    let l = crepr_layout(src, "Line");
    assert_eq!((l.size, l.align), (16, 4));
    assert_eq!(l.fields[1].offset, 8);
}

#[test]
fn int_nat_friendly_spellings_are_dropped() {
    // Stage 4: word-size integers are spelled `@int`/`@nat`; the old friendly
    // `Int`/`Nat` are no longer type names and read as unknown types.
    assert!(errors("@mod M\n$ x : Int = 5").contains("unknown type `Int`"));
    assert!(errors("@mod M\n$ y : Nat = 5").contains("unknown type `Nat`"));
    // The `@`-spellings type and display canonically.
    assert_eq!(type_of("@mod M\n$ a : @int = 5", "a"), "@int");
    assert_eq!(type_of("@mod M\n$ b : @nat = 5", "b"), "@nat");
    // Inference still displays the word default as `@int`.
    assert_eq!(type_of("@mod M\n$ c = 1 + 2", "c"), "@int");
}
