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
    let types = std::rc::Rc::new(crate::Types::new());
    let mut core_checker = Checker::new(&ast, types.clone());
    core_checker.check_program(&core).expect("check CORE");
    let mut checker = Checker::new(&ast, types.clone());
    checker.import_from(&core_checker);
    let results = checker
        .check_program(&prog)
        .unwrap_or_else(|e| panic!("{}", e.render(src, "test.thx")));
    let ty = results
        .iter()
        .find(|(n, _)| *n == name)
        .expect("name present")
        .1;
    checker.show(ty)
}

fn errors(src: &str) -> String {
    let (ast, core) = crate::parse_into(Ast::new(), CORE_SRC).expect("parse CORE");
    let (ast, prog) = crate::parse_into(ast, src).expect("parse");
    let types = std::rc::Rc::new(crate::Types::new());
    let mut core_checker = Checker::new(&ast, types.clone());
    core_checker.check_program(&core).expect("check CORE");
    let mut checker = Checker::new(&ast, types.clone());
    checker.import_from(&core_checker);
    match checker.check_program(&prog) {
        Ok(_) => String::new(),
        Err(e) => format!("{e}"),
    }
}

/// Like [`errors`], but rendered with a caret against `src`, so a test can
/// assert where the diagnostic points.
fn rendered_errors(src: &str) -> String {
    let (ast, core) = crate::parse_into(Ast::new(), CORE_SRC).expect("parse CORE");
    let (ast, prog) = crate::parse_into(ast, src).expect("parse");
    let types = std::rc::Rc::new(crate::Types::new());
    let mut core_checker = Checker::new(&ast, types.clone());
    core_checker.check_program(&core).expect("check CORE");
    let mut checker = Checker::new(&ast, types.clone());
    checker.import_from(&core_checker);
    match checker.check_program(&prog) {
        Ok(_) => String::new(),
        Err(e) => e.render(src, "test.thx"),
    }
}

#[test]
fn declaration_errors_carry_a_location() {
    // These are raised outside the `infer`/`check` frames that fill a span, so
    // each one has to supply its own; without it they all caret `1:1`, blaming
    // the `@mod` line for an error somewhere else entirely.
    let cases = [
        // a `with` include that names no such type
        "$ @mod M
$ P : @struct = with Nope, a: @Int,
$ x : @Int = 1",
        // a `with` cycle
        "$ @mod M
$ A : @struct = with B, a: @Int,
$ B : @struct = with A, b: @Int,
$ x : @Int = 1",
        // a member a `with` splices in twice
        "$ @mod M
$ A : @struct = a: @Int,
$ B : @struct = with A, a: @Int,
$ x : @Int = 1",
        // an undeclared type parameter
        "$ @mod M
$ P : @struct = v: a,
$ x : @Int = 1",
        // one name defined twice
        "$ @mod M
$ f : @Int = 1
$ f : @Int = 2",
        // a C-repr field that has no C representation
        "$ @mod M
$ P : @struct @extern \"C\" = v: List @Int,
$ x : @Int = 1",
        // a C-repr struct that contains itself by value
        "$ @mod M
$ P : @struct @extern \"C\" = v: P,
$ x : @Int = 1",
        // a blessed interface declared outside CORE
        "$ @mod M
$ @IRange : @struct a b = range: a -> a -> b,
$ x : @Int = 1",
    ];
    for src in cases {
        let text = rendered_errors(src);
        assert!(!text.is_empty(), "expected an error for {src:?}");
        assert!(
            text.contains("--> test.thx:2:") || text.contains("--> test.thx:3:"),
            "the error should caret the declaration, not the module header:\n{text}"
        );
    }
}

#[test]
fn undeclared_type_param_is_error() {
    // Parameters are mandatory: a free tyvar with no declared list is rejected.
    let e = errors("$ @mod M\n$ Box : @struct = val: t\n$ x : @Int = 0");
    assert!(e.contains("declares no type parameters"), "{e}");
}

#[test]
fn parameterized_alias_expands() {
    // `MapInt` fixes the key to @Int, so `MapInt @Bool` elaborates to `Map @Int @Bool`.
    let src = "$ @mod M\n\
               $ Map : @struct k v = key: k, val: v\n\
               $ MapInt : @alias v = Map @Int v\n\
               $ m : MapInt @Bool = .{ .key = 1, .val = @true }";
    assert_eq!(type_of(src, "m"), "Map @Int @Bool");
}

#[test]
fn monomorphic_arithmetic() {
    assert_eq!(type_of("$ @mod M\n$ x = 1 + 2", "x"), "@Int");
}

#[test]
fn identity_is_polymorphic() {
    // The classic let-generalization test: `id` gets `forall a. a -> a`.
    assert_eq!(type_of("$ @mod M\n$ id = \\x = x", "id"), "a -> a");
}

#[test]
fn polymorphic_id_used_at_two_types() {
    // Using a generalized `id` at @Int and @Str must not conflict.
    let src = "$ @mod M\n\
                   $ id = \\x = x\n\
                   $ pair = let a = id 1, b = id \"s\" in {a, b}";
    assert_eq!(type_of(src, "pair"), "{@Int, @Str}");
}

#[test]
fn function_application_and_arrows() {
    let src = "$ @mod M\n$ apply = \\f x = f x";
    // `(A -> B) -> A -> B`.
    assert_eq!(type_of(src, "apply"), "(a -> b) -> a -> b");
}

#[test]
fn if_unifies_branches_and_condition() {
    assert_eq!(
        type_of("$ @mod M\n$ f = \\n = if n == 0 => 1 else n", "f"),
        "@Int -> @Int"
    );
}

#[test]
fn recursion_through_predeclared_globals() {
    let src = "$ @mod M\n$ fib = \\n = if n == 0 => 0 else fib (n - 1) + fib (n - 2)";
    assert_eq!(type_of(src, "fib"), "@Int -> @Int");
}

#[test]
fn tuples_and_lists() {
    assert_eq!(type_of("$ @mod M\n$ p = {1, \"a\"}", "p"), "{@Int, @Str}");
    assert_eq!(type_of("$ @mod M\n$ xs = [1, 2, 3]", "xs"), "@Vec @Int");
}

#[test]
fn signature_is_checked() {
    assert_eq!(
        type_of("$ @mod M\n$ f : @Int -> @Int = \\x = x + 1", "f"),
        "@Int -> @Int"
    );
}

#[test]
fn occurs_check_rejects_self_application() {
    // `\x = x x` has no finite type.
    assert!(errors("$ @mod M\n$ bad = \\x = x x").contains("infinite type"));
}

#[test]
fn type_mismatch_is_reported() {
    assert!(errors("$ @mod M\n$ bad = 1 + \"s\"").contains("TYPE_MISMATCH"));
}

#[test]
fn unbound_name_is_reported() {
    assert!(errors("$ @mod M\n$ bad = nope 1").contains("TYPE_UNBOUND"));
}

#[test]
fn struct_field_access_is_typed() {
    let src = "$ @mod M\n\
                   $ P : @struct = x: @Int, y: @Str\n\
                   $ p : P = P.{ .x = 1, .y = \"s\" }\n\
                   $ gx = p.x\n\
                   $ gy = p.y";
    assert_eq!(type_of(src, "gx"), "@Int");
    assert_eq!(type_of(src, "gy"), "@Str");
}

#[test]
fn generic_struct_instantiates_per_use() {
    let src = "$ @mod M\n\
                   $ Box : @struct t = val: t\n\
                   $ b : Box @Str = Box.{ .val = \"hi\" }\n\
                   $ out = b.val";
    assert_eq!(type_of(src, "b"), "Box @Str");
    assert_eq!(type_of(src, "out"), "@Str");
}

#[test]
fn union_constructor_and_match() {
    let src = "$ @mod M\n\
                   $ Maybe : @union t = Just: t, None: {}\n\
                   $ m : Maybe @Int = Maybe.Just.{ 7 }\n\
                   $ get = \\d = \\o = is o | Maybe.Just.{x} => x | _ => d";
    assert_eq!(type_of(src, "m"), "Maybe @Int");
    assert_eq!(type_of(src, "get"), "a -> Maybe a -> a");
}

#[test]
fn mutually_recursive_globals_via_scc() {
    let src = "$ @mod M\n\
                   $ is_even : @Int -> @Int = \\n = if n == 0 => 1 else is_odd (n - 1)\n\
                   $ is_odd  : @Int -> @Int = \\n = if n == 0 => 0 else is_even (n - 1)";
    assert_eq!(type_of(src, "is_even"), "@Int -> @Int");
    assert_eq!(type_of(src, "is_odd"), "@Int -> @Int");
}

#[test]
fn recursive_let_binding() {
    let src = "$ @mod M\n\
                   $ f = \\m = let go = \\n acc = if n == 0 => acc else go (n - 1) (acc + n) \
                   in go m 0";
    assert_eq!(type_of(src, "f"), "@Int -> @Int");
}

#[test]
fn arithmetic_resolves_its_interface_per_type() {
    assert_eq!(type_of("$ @mod M\n$ a = 1 + 2", "a"), "@Int");
    assert_eq!(type_of("$ @mod M\n$ a = 1.0 + 2.0", "a"), "@Float64");
    // An operator takes ONE type for both operands, and an integer literal never
    // adopts a float type, so mixing the two is an error rather than a silent
    // promotion. Write the float literal instead.
    assert!(errors("$ @mod M\n$ a = 1 + 2.0").contains("an integer literal cannot be a"));
}

/// One name, one type. Two definitions of a name are rejected where they are
/// written, rather than making the name's printed type a lie.
#[test]
fn a_name_defined_twice_is_an_error() {
    let src = "$ @mod M\n\
                   $ f : @Int -> @Int = \\x = x + 1\n\
                   $ f : @Str -> @Str = \\x = x ++ \"!\"";
    assert!(errors(src).contains("`f` is defined twice"), "{}", errors(src));
}

/// An interface is how one operation covers several types: the wrapper has ONE
/// honest type and each instance carries its own.
#[test]
fn an_interface_covers_several_types() {
    let src = "$ @mod M\n\
                   $ IBump : @struct t = bump: t -> t,\n\
                   $ bump : @ctx IBump t -> t -> t = \\d x = d.bump x\n\
                   $ bump_int : IBump @Int = .{ .bump = \\x = x + 1 }\n\
                   $ bump_str : IBump @Str = .{ .bump = \\x = x ++ \"!\" }\n\
                   $ a = bump 3\n\
                   $ b = bump \"hi\"";
    assert_eq!(errors(src), "");
    assert_eq!(type_of(src, "a"), "@Int");
    assert_eq!(type_of(src, "b"), "@Str");
    assert_eq!(type_of(src, "bump"), "a -> a");
}

#[test]
fn overload_deferred_until_signature_pins_operands() {
    // `p1.x + p2.x` cannot resolve until the signature makes the params
    // structs; bidirectional checking + the pending fixpoint handle it.
    let src = "$ @mod M\n\
                   $ P : @struct = x: @Int, y: @Int\n\
                   $ add : P -> P -> P = \\p1 p2 = P.{ .x = p1.x + p2.x, .y = p1.y + p2.y }";
    assert_eq!(type_of(src, "add"), "P -> P -> P");
}

#[test]
fn sized_literal_arithmetic_takes_the_result_type() {
    // Two bare literals in a sized-int context resolve to that sized overload,
    // not `@Int`: the result type is propagated onto the operands before they
    // would default to `@Int` (which has no `@Int + @Int -> @Int32`).
    assert_eq!(type_of("$ @mod M\n$ x : @Int32 = 2 + 3", "x"), "@Int32");
    assert_eq!(type_of("$ @mod M\n$ x : @Int64 = 2 + 3 * 4", "x"), "@Int64");
    assert_eq!(type_of("$ @mod M\n$ x : @Nat16 = 7 - 1", "x"), "@Nat16");
    // With no sized context, bare literals still default to `@Int`.
    assert_eq!(type_of("$ @mod M\n$ x = 2 + 3", "x"), "@Int");
}

#[test]
fn a_type_with_no_instance_is_reported() {
    let src = "$ @mod M\n\
                   $ IBump : @struct t = bump: t -> t,\n\
                   $ bump : @ctx IBump t -> t -> t = \\d x = d.bump x\n\
                   $ bump_int : IBump @Int = .{ .bump = \\x = x }\n\
                   $ bad = bump 1.0";
    assert!(errors(src).contains("no value of type `IBump @Float64`"), "{}", errors(src));
}

#[test]
fn cross_module_import_brings_in_types_and_values() {
    let dep_src = "$ @mod OPT\n\
                       $ Option : @union t = Some: t, None: {}\n\
                       $ is_some : Option t -> @Bool = \\o = \
                       is o | Option.Some.{_} => @true | _ => @false\n\
                       $ unwrap_or : Option t -> t -> t = \\o d = \
                       is o | Option.Some.{x} => x | _ => d";
    let use_src = "$ @mod U\n\
                       $ with OPT\n\
                       $ o : Option @Int = Option.Some.{ 41 }\n\
                       $ present = is_some o\n\
                       $ value = unwrap_or o 0";
    // Both modules parse into one shared `Ast` so cross-module handles resolve.
    let (ast, dep) = crate::parse_into(crate::Ast::new(), dep_src).expect("parse dep");
    let (ast, program) = crate::parse_into(ast, use_src).expect("parse use");
    let types = std::rc::Rc::new(crate::Types::new());
    let mut dep_checker = Checker::new(&ast, types.clone());
    dep_checker.check_program(&dep).expect("check dep");
    let mut checker = Checker::new(&ast, types.clone());
    checker.import_from(&dep_checker);
    let results = checker
        .check_program(&program)
        .unwrap_or_else(|e| panic!("{}", e.render(use_src, "U")));
    let ty = |name: &str| checker.show(results.iter().find(|(n, _)| *n == name).unwrap().1);
    assert_eq!(ty("present"), "@Bool");
    assert_eq!(ty("value"), "@Int");
}

/// Check `use_src` after importing `dep_src`; returns the rendered error (empty
/// string on success). Both share one `Ast` so cross-module handles resolve.
fn cross_module_errors(dep_src: &str, use_src: &str) -> String {
    let (ast, core) = crate::parse_into(Ast::new(), CORE_SRC).expect("parse CORE");
    let (ast, dep) = crate::parse_into(ast, dep_src).expect("parse dep");
    let (ast, program) = crate::parse_into(ast, use_src).expect("parse use");
    let types = std::rc::Rc::new(crate::Types::new());
    let mut core_checker = Checker::new(&ast, types.clone());
    core_checker.check_program(&core).expect("check CORE");
    let mut dep_checker = Checker::new(&ast, types.clone());
    dep_checker.import_from(&core_checker);
    dep_checker.check_program(&dep).expect("check dep");
    let mut checker = Checker::new(&ast, types.clone());
    checker.import_from(&core_checker);
    checker.import_from(&dep_checker);
    match checker.check_program(&program) {
        Ok(_) => String::new(),
        Err(e) => format!("{e}"),
    }
}

const PRIVATE_DEP: &str = "$ @mod A\n\
    $ api : @Int -> @Int = \\x = helper x + 1\n\
    $ @private\n\
    $ helper : @Int -> @Int = \\x = x * x";

#[test]
fn public_symbol_above_private_marker_is_importable() {
    // `api` sits before `$ @private`, so it exports even though its body calls a
    // private helper.
    assert_eq!(cross_module_errors(PRIVATE_DEP, "$ @mod B\n$ with A\n$ r = api 3"), "");
}

#[test]
fn private_symbol_is_not_importable_bare() {
    assert!(cross_module_errors(PRIVATE_DEP, "$ @mod B\n$ with A\n$ r = helper 3")
        .contains("TYPE_UNBOUND"));
}

#[test]
fn private_symbol_is_not_reachable_qualified() {
    // A private name is withheld from the qualified namespace too, so `A.helper`
    // does not resolve either.
    assert!(!cross_module_errors(PRIVATE_DEP, "$ @mod B\n$ with A\n$ r = A.helper 3").is_empty());
}

#[test]
fn private_type_is_not_importable() {
    let dep = "$ @mod A\n\
        $ mk : @Int -> Secret = \\n = Secret.{ .v = n }\n\
        $ @private\n\
        $ Secret : @struct = v: @Int";
    // Naming the private type in the importer fails; only its public constructor's
    // result type leaks (opaquely).
    assert!(!cross_module_errors(dep, "$ @mod B\n$ with A\n$ x : Secret = mk 3").is_empty());
}

#[test]
fn private_extends_to_end_of_file() {
    // `$ @private` has no counterpart, so every declaration below it stays hidden.
    let dep = "$ @mod A\n\
        $ @private\n\
        $ hidden : @Int -> @Int = \\x = x\n\
        $ also_hidden : @Int -> @Int = \\x = x";
    assert!(cross_module_errors(dep, "$ @mod B\n$ with A\n$ r = hidden 1").contains("TYPE_UNBOUND"));
    assert!(cross_module_errors(dep, "$ @mod B\n$ with A\n$ r = also_hidden 1")
        .contains("TYPE_UNBOUND"));
}

#[test]
fn public_marker_is_rejected() {
    // There is no `$ @public`; writing one is a parse error.
    let err = crate::parse_into(Ast::new(), "$ @mod M\n$ @public\n$ x = 1")
        .err()
        .map(|e| format!("{e}"))
        .unwrap_or_default();
    assert!(err.contains("no '@public'"), "unexpected error: {err}");
}

#[test]
fn record_parameters_bind_fields_directly() {
    // A record parameter is a real record type; its fields auto-bind in the body.
    assert_eq!(
        type_of("$ @mod M\n$ add : {x: @Int, y: @Int} -> @Int = x + y", "add"),
        "{ x: @Int, y: @Int } -> @Int"
    );
    // A one-field record is a one-field record (no collapse); a scalar promotes
    // to it at the call site.
    assert_eq!(
        type_of("$ @mod M\n$ inc : {x: @Int} -> @Int = x + 1", "inc"),
        "{ x: @Int } -> @Int"
    );
}

#[test]
fn with_scopes_struct_fields() {
    let src = "$ @mod M\n\
                   $ P : @struct = x: @Int, y: @Int\n\
                   $ sum : P -> @Int = \\p = with p in x + y";
    assert_eq!(type_of(src, "sum"), "P -> @Int");
}

#[test]
fn sequence_literal_is_type_directed() {
    // The same `[..]` is an @Array in @Array context, a @Vec otherwise.
    assert_eq!(type_of("$ @mod M\n$ a : @Array = [10, 20]", "a"), "@Array");
    assert_eq!(
        type_of("$ @mod M\n$ a : @Vec @Int = [1, 2, 3]", "a"),
        "@Vec @Int"
    );
    // An @Array-typed parameter directs a `[..]` argument at the call site.
    let src = "$ @mod M\n\
                   $ len : @Array -> @Int = \\a = @array_len a\n\
                   $ n = len [1, 2]";
    assert_eq!(type_of(src, "n"), "@Int");
}

#[test]
fn effect_operation_is_bound_and_handler_types_result() {
    // `get`/`put` are bound from the effect declaration; `tick` performs State, so
    // its arrow carries the `<State>` row; the `do/ctl` handler discharges it and
    // types to the clause/`else` bodies' common type.
    let src = "$ @mod M\n\
                   $ State : @effect = get : {} -> @Int, put : @Int -> {},\n\
                   $ tick : {} -> <State> @Int = \\u = let x = get {} in let _ = put (x + 1) in x\n\
                   $ run : @Int = do tick {} ctl k | get u => k 0 | put n => k {} | x => x";
    assert_eq!(type_of(src, "tick"), "{} -> <State> @Int");
    assert_eq!(type_of(src, "run"), "@Int");
}

#[test]
fn io_is_a_builtin_effect_and_propagates() {
    // `@IO` is a builtin effect, written in `@`-form (unlike Capitalized user
    // effects like `State`) and needing no declaration. A leaf `@extern` carries
    // it in its latent row, and a caller that performs it inherits `@IO`.
    let src = "$ @mod M\n\
               $ write : @Str -> <@IO> @Int = @extern \"C\" \"write\" \"libc\"\n\
               $ p = \\s = let _ = write s in {}";
    assert_eq!(type_of(src, "write"), "@Str -> <@IO> @Int");
    assert!(type_of(src, "p").contains("<@IO"), "{}", type_of(src, "p"));
}

#[test]
fn io_in_a_pure_typed_function_is_rejected() {
    // A function declared pure that performs `@IO` is a compile error: the latent
    // `<@IO>` cannot be subsumed into the empty ambient.
    let src = "$ @mod M\n\
               $ write : @Str -> <@IO> @Int = @extern \"C\" \"write\" \"libc\"\n\
               $ bad : @Str -> @Int = \\s = write s";
    assert!(errors(src).contains("@IO"), "{}", errors(src));
}

#[test]
fn unknown_type_name_is_rejected() {
    // A bare capitalized name in type position must be a known type; a type
    // variable is a lowercase name. `Itn` is a typo, not a type variable.
    assert!(errors("$ @mod M\n$ x : Itn = 5").contains("unknown type `Itn`"));
}

#[test]
fn numeric_literal_is_not_a_bool_condition() {
    // `if` wants a `@Bool`; a bare integer literal is a number, not a truth value,
    // so `if 1` is a type error (it used to silently mean "nonzero").
    assert!(
        errors("$ @mod M\n$ x : @Int = if 1 => 2 else 3").contains("numeric literal"),
        "expected `if 1` to be rejected: {:?}",
        errors("$ @mod M\n$ x : @Int = if 1 => 2 else 3")
    );
    // A real condition (a comparison, or a `@Bool`) is fine.
    assert_eq!(errors("$ @mod M\n$ x : @Int = if 1 == 1 => 2 else 3"), "");
    assert_eq!(errors("$ @mod M\n$ x : @Int = if @true => 2 else 3"), "");
    // A numeric literal cannot masquerade as a pointer either.
    assert!(errors("$ @mod M\n$ p : @Ptr = 0").contains("numeric literal"));
}

#[test]
fn curried_extern_is_rejected() {
    // A C function has no first-class closure to curry: a multi-parameter extern
    // must group its C parameters into one record, not curry with several arrows.
    let e = errors("$ @mod M\n$ f : @Int -> @Int -> @Int = @extern \"C\" \"f\" \"lib\"");
    assert!(e.contains("takes a single argument"), "{e}");
}

#[test]
fn single_argument_externs_are_accepted() {
    // One record groups several C parameters; a lone value and unit are one
    // argument each, so all three shapes pass the extern-shape check.
    let src = "$ @mod M\n\
               $ f : {a: @Int, b: @Int} -> @Int = @extern \"C\" \"f\" \"lib\"\n\
               $ g : @Str -> @Int = @extern \"C\" \"g\" \"lib\"\n\
               $ h : {} -> @Int = @extern \"C\" \"h\" \"lib\"";
    assert_eq!(errors(src), "");
}

#[test]
fn type_variable_and_sized_and_declared_types_are_accepted() {
    // `a` is a type variable; `@Int8`/`@Ptr` are base types; a declared union
    // name is known. None of these is an "unknown type".
    let src = "$ @mod M\n\
               $ Box : @union = Wrap: { @Int },\n\
               $ id : a -> a = \\x = x\n\
               $ n : @Int8 = 5\n\
               $ p : @Ptr -> @Ptr = \\x = x\n\
               $ b : Box = Box.Wrap.{ 1 }";
    assert_eq!(errors(src), "");
    assert_eq!(type_of(src, "id"), "a -> a");
    assert_eq!(type_of(src, "n"), "@Int8");
}

#[test]
fn unhandled_effect_is_a_compile_error() {
    // A function that performs an effect but declares a pure (or too-small) type
    // is rejected: the latent `<State>` cannot be subsumed into the empty ambient.
    let src = "$ @mod M\n\
                   $ State : @effect = get : {} -> @Int, put : @Int -> {},\n\
                   $ bad : {} -> @Int = \\u = get {}";
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
    let src = "$ @mod M\n\
                   $ Yield : @effect = yield : @Int -> {},\n\
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
    let src = "$ @mod MAIN\n\
               $ Yell : @effect = shout : @Int -> {},\n\
               $ main : {} -> <| e> @Int = \\u = let _ = Yell.shout 5 in 0";
    assert_eq!(errors(src), "", "open-row main should type-check");
    assert_eq!(type_of(src, "main"), "{} -> <Yell | a> @Int");
}

#[test]
fn signed_member_recurses_at_another_type() {
    // A use inside the recursive group sees `id`'s signature, so `g` may call it
    // at `@Str` and `@Int` and neither use fixes `a`.
    let src = "$ @mod M\n\
               $ id : a -> a = \\x = let _ = g 0 in x\n\
               $ g : @Int -> @Int = \\n = let _ = id \"s\" in id n";
    assert_eq!(errors(src), "");
    assert_eq!(type_of(src, "id"), "a -> a");
}

#[test]
fn signature_row_tail_stays_open() {
    // Unlike a type variable, a row tail may be extended by the body (#270).
    let src = "$ @mod M\n\
               $ Yell : @effect = shout : @Int -> {},\n\
               $ f : {} -> <| e> @Int = \\u = let _ = Yell.shout 5 in 0";
    assert_eq!(errors(src), "");
}

#[test]
fn meta_op_outside_e_is_an_unhandled_effect() {
    // The metaprogramming ops carry `<@Meta>`. Used in ordinary code (outside an
    // `@e`), the latent `<@Meta>` cannot be subsumed into the pure ambient,
    // so it is a clean compile error rather than a runtime no-op/fault.
    let src = "$ @mod M\n$ bad : @Str = @fresh \"x\"";
    let e = errors(src);
    assert!(e.contains("@Meta") && e.contains("not handled"), "got: {e:?}");
}

#[test]
fn e_discharges_meta_and_requires_code() {
    // `@e` handles `<@Meta>`: a meta op inside it type-checks and the surrounding
    // context stays pure. It inserts code, so a non-`@Code` operand is an error.
    assert_eq!(
        errors("$ @mod M\n$ ok : @Str = @e <| @parse_str <| @fresh \"x\""),
        "",
        "meta op inside @e should type-check",
    );
    let e = errors("$ @mod M\n$ bad : @Str = @e <| @fresh \"x\"");
    assert!(e.contains("inserts code") && e.contains("`@Str`"), "got: {e:?}");
    let e = errors("$ @mod M\n$ bad : @Int = @e 1");
    assert!(e.contains("`@Code`"), "got: {e:?}");
}

#[test]
fn run_is_gone() {
    let e = crate::parse("$ @mod M\n$ @run (@parse_items \"\")\n").err().expect("`@run` is rejected");
    assert!(format!("{e}").contains("there is no '@run'"), "got: {e}");
}

#[test]
fn build_is_the_only_io_capable_compile_time_context() {
    // `@e` discharges `<@Meta>` only, so it cannot perform IO: that is what makes
    // it hermetic, and what leaves `@build` (whose own signature carries `<@IO>`)
    // as the only place a generator may touch the world.
    let io_fn = "$ shout : @Str -> <@IO> @Int = @extern \"C\" \"puts\" \"libc\"\n";
    let e = errors(&format!(
        "$ @mod M\n{io_fn}$ bad : @Int = @e <| (shout \"hi\"; @parse_str \"1\")"
    ));
    assert!(e.contains("@IO") && e.contains("not handled"), "got: {e:?}");
    // A `@build`-shaped row handles both `@Meta` and `@IO`.
    assert_eq!(
        errors(&format!(
            "$ @mod M\n{io_fn}$ b : {{}} -> <@Meta, @IO> @Code = \\_ = \
             shout \"hi\"; @parse_items \"$ x : @Int = 1\""
        )),
        "",
        "a <@Meta, @IO> row should accept both IO and meta ops",
    );
}

#[test]
fn build_has_one_signature() {
    // `@build` is a definition like any other, so the checker only has to agree
    // with its annotation; `is_build_type` is what pins the shape (see the driver).
    let types = Types::new();
    let unit = types.con(crate::typing::data::UNIT);
    let code = types.con("@Code");
    let row = {
        let empty = types.row_empty();
        let io = types.row_extend("@IO", empty);
        types.row_extend("@Meta", io)
    };
    assert!(crate::is_build_type(&types, types.arrow_eff(unit, code, row)));
    // The row is a row: the other spelling is the same type.
    let flipped = {
        let empty = types.row_empty();
        let meta = types.row_extend("@Meta", empty);
        types.row_extend("@IO", meta)
    };
    assert!(crate::is_build_type(&types, types.arrow_eff(unit, code, flipped)));
    // Too few effects, or the wrong result, is not the build function.
    let meta_only = {
        let empty = types.row_empty();
        types.row_extend("@Meta", empty)
    };
    assert!(!crate::is_build_type(&types, types.arrow_eff(unit, code, meta_only)));
    let int = types.con(crate::typing::data::INT);
    assert!(!crate::is_build_type(&types, types.arrow_eff(unit, int, row)));
}

#[test]
fn open_effects_permits_io_at_the_shell_top_level() {
    // A top-level body performing `@IO` is rejected in a file (a pure top level)...
    let src = "$ @mod M\n\
               $ shout : @Str -> <@IO> @Int = @extern \"C\" \"puts\" \"libc\"\n\
               $ shown : @Int = shout \"hi\"";
    let e = errors(src);
    assert!(
        e.contains("@IO") && e.contains("not handled"),
        "a file top level should reject IO: {e:?}"
    );

    // ...but accepted once the shell turns on open effect rows, and the discharged
    // effect does not leak into the binding's type.
    let (ast, core) = crate::parse_into(Ast::new(), CORE_SRC).expect("parse CORE");
    let (ast, prog) = crate::parse_into(ast, src).expect("parse");
    let types = std::rc::Rc::new(crate::Types::new());
    let mut core_checker = Checker::new(&ast, types.clone());
    core_checker.check_program(&core).expect("check CORE");
    let mut checker = Checker::new(&ast, types.clone());
    checker.import_from(&core_checker);
    checker.set_open_effects(true);
    let results = checker
        .check_program(&prog)
        .unwrap_or_else(|e| panic!("open effects should accept IO: {}", e.render(src, "test.thx")));
    let ty = results.iter().find(|(n, _)| *n == "shown").expect("shown").1;
    assert_eq!(checker.show(ty), "@Int");
}

#[test]
fn only_the_one_entry_signature_is_accepted() {
    use crate::is_entry_type;
    // The entry is `$ @main : @Vec @Str -> <@IO> @Int` and nothing else: no value
    // entry, no unit parameter, no other effect row.
    let entry = |src: &str| {
        let parsed = crate::parse(src).expect("parse");
        let types = std::rc::Rc::new(crate::Types::new());
        let mut checker = Checker::new(&parsed.ast, types.clone());
        let results = checker.check_program(&parsed.program).expect("check");
        let ty = results.iter().find(|(n, _)| *n == "@main").expect("@main").1;
        is_entry_type(checker.types(), ty)
    };
    assert!(entry("$ @mod MAIN\n$ @main : @Vec @Str -> <@IO> @Int = \\args = 0"));
    assert!(!entry("$ @mod MAIN\n$ @main : {} -> <@IO> @Int = \\u = 0"));
    assert!(!entry("$ @mod MAIN\n$ @main : @Vec @Str -> @Int = \\args = 0"));
    assert!(!entry("$ @mod MAIN\n$ @main : @Vec @Int -> <@IO> @Int = \\args = 0"));
    assert!(!entry("$ @mod MAIN\n$ @main : @Vec @Str -> <@IO> {} = \\args = {}"));
}

#[test]
fn same_operation_in_two_effects_resolves_by_result_type() {
    // `ask` is declared by two effects (@Int and @Str result); a bare use is an
    // overload resolved by how the result is used, and `Effect.op` disambiguates.
    let src = "$ @mod M\n\
                   $ Reader : @effect = ask : {} -> @Int,\n\
                   $ Config : @effect = ask : {} -> @Str,\n\
                   $ n : @Int = do (ask {}) + 1 ctl k | Reader.ask u => k 10\n\
                   $ s : @Str = do Config.ask {} ctl k | Config.ask u => k \"hi\"";
    assert_eq!(type_of(src, "n"), "@Int");
    assert_eq!(type_of(src, "s"), "@Str");
}

#[test]
fn array_primitives_overload_on_array_and_str() {
    let src = "$ @mod M\n\
                   $ a : @Array = @array_push (@Array.{ 0 }) 65\n\
                   $ n = @array_len a\n\
                   $ b = @array_get \"hi\" 0";
    assert_eq!(type_of(src, "a"), "@Array");
    assert_eq!(type_of(src, "n"), "@Int");
    assert_eq!(type_of(src, "b"), "@Int");
}

/// The computed C layout of a named C-repr struct, or a panic with the check error.
fn crepr_layout(src: &str, name: &str) -> utilities::CLayout {
    let parsed = crate::parse(src).expect("parse");
    let types = std::rc::Rc::new(crate::Types::new());
    let mut checker = Checker::new(&parsed.ast, types.clone());
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
    let src = "$ @mod M\n\
        $ Vector2 : @struct @extern \"C\" = x: @Float32, y: @Float32,\n\
        $ Color : @struct @extern \"C\" = r: @Nat8, g: @Nat8, b: @Nat8, a: @Nat8,\n\
        $ x : @Int = 0";
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
    let src = "$ @mod M\n\
        $ Vector4 : @struct @extern \"C\" = x: @Float32, y: @Float32, z: @Float32, w: @Float32,\n\
        $ Quaternion : @alias = Vector4\n\
        $ Transform : @struct @extern \"C\" = rotation: Quaternion, scale: @Float32,\n\
        $ x : @Int = 0";
    let t = crepr_layout(src, "Transform");
    // rotation is a 16-byte/4-align Vector4, so `scale` starts at offset 16.
    assert_eq!(t.fields[1].offset, 16);
    assert_eq!((t.size, t.align), (20, 4));
}

#[test]
fn c_union_layout_overlaps() {
    // `@union @extern "C"` is a C union: members share offset 0, size is the largest.
    let src = "$ @mod M\n\
        $ U : @union @extern \"C\" = i: @Int32, d: @Float64,\n\
        $ x : @Int = 0";
    let u = crepr_layout(src, "U");
    assert!(u.is_union);
    assert_eq!(u.fields[0].offset, 0);
    assert_eq!(u.fields[1].offset, 0);
    assert_eq!((u.size, u.align), (8, 8));
}

#[test]
fn crepr_struct_rejects_non_c_field() {
    let e = errors("$ @mod M\n$ Bad : @struct @extern \"C\" = s: @Str,");
    assert!(e.contains("not C-representable"), "{e}");
}

#[test]
fn crepr_struct_rejects_generic() {
    let e = errors("$ @mod M\n$ Bad : @struct @extern \"C\" a = v: a,");
    assert!(e.contains("may not be generic"), "{e}");
}

#[test]
fn real_literal_takes_real32_width() {
    // A `@Float64` literal checks against a `@Float32` expected type (like an integer
    // literal takes its width), so a float C struct binds with plain literals.
    let src = "$ @mod M\n\
        $ Vector2 : @struct @extern \"C\" = x: @Float32, y: @Float32,\n\
        $ v : Vector2 = Vector2.{ .x = 1.0, .y = 2.5 }\n\
        $ f : @Float32 = 3.5\n\
        $ g : @Float64 = 3.5";
    assert_eq!(type_of(src, "v"), "Vector2");
    assert_eq!(type_of(src, "f"), "@Float32");
    assert_eq!(type_of(src, "g"), "@Float64");
}

#[test]
fn crepr_struct_nested() {
    let src = "$ @mod M\n\
        $ Vector2 : @struct @extern \"C\" = x: @Float32, y: @Float32,\n\
        $ Line : @struct @extern \"C\" = a: Vector2, b: Vector2,\n\
        $ x : @Int = 0";
    let l = crepr_layout(src, "Line");
    assert_eq!((l.size, l.align), (16, 4));
    assert_eq!(l.fields[1].offset, 8);
}

#[test]
fn int_nat_friendly_spellings_are_dropped() {
    // Stage 4: word-size integers are spelled `@Int`/`@Nat`; the old friendly
    // `Int`/`Nat` are no longer type names and read as unknown types.
    assert!(errors("$ @mod M\n$ x : Int = 5").contains("unknown type `Int`"));
    assert!(errors("$ @mod M\n$ y : Nat = 5").contains("unknown type `Nat`"));
    // The `@`-spellings type and display canonically.
    assert_eq!(type_of("$ @mod M\n$ a : @Int = 5", "a"), "@Int");
    assert_eq!(type_of("$ @mod M\n$ b : @Nat = 5", "b"), "@Nat");
    // Inference still displays the word default as `@Int`.
    assert_eq!(type_of("$ @mod M\n$ c = 1 + 2", "c"), "@Int");
}

/// A user definition for a custom type EXTENDS a comparison operator rather than
/// shadowing the built-in: the custom operands reach it, primitive ones still
/// reach CORE's per-type overload.
#[test]
fn comparison_is_extended_by_an_instance() {
    let src = "$ @mod M\n$ T : @struct = v: @Int\n\
               $ ord_t : IOrd T = .{ .lt = \\a b = a.v < b.v }\n\
               $ custom : @Bool = T.{ .v = 1 } < T.{ .v = 2 }\n\
               $ prim : @Bool = 1 < 2";
    assert_eq!(errors(src), "");
    assert_eq!(type_of(src, "custom"), "@Bool");
    assert_eq!(type_of(src, "prim"), "@Bool");
}

/// There is no structural fallback: `==` on a type with no `IEq` instance is an
/// error, while a tuple composes from its components' instances (CORE has one per
/// arity), so the aggregates stay comparable without a definition of their own.
#[test]
fn equality_needs_an_instance() {
    let bare = "$ @mod M\n$ T : @struct = v: @Int\n\
                $ same : @Bool = T.{ .v = 1 } == T.{ .v = 1 }";
    assert!(errors(bare).contains("no value of type `IEq T`"), "{}", errors(bare));

    let src = "$ @mod M\n$ T : @struct = v: @Int\n\
               $ eq_t : IEq T = .{ .eq = \\a b = a.v == b.v }\n\
               $ same : @Bool = T.{ .v = 1 } == T.{ .v = 1 }\n\
               $ pair : @Bool = {1, \"a\"} == {1, \"a\"}";
    assert_eq!(errors(src), "");
    assert_eq!(type_of(src, "same"), "@Bool");
    assert_eq!(type_of(src, "pair"), "@Bool");
}

/// A generic comparison DECLARES its context, which is what makes it generic: an
/// undeclared one has nothing to resolve, and says so instead of inventing a type.
#[test]
fn generic_comparison_declares_its_context() {
    let src = "$ @mod M\n$ eq : @ctx IEq a -> a -> a -> @Bool = \\d x y = d.eq x y";
    assert_eq!(errors(src), "");
    assert_eq!(type_of(src, "eq"), "a -> a -> @Bool");

    let undeclared = errors("$ @mod M\n$ eq = \\x y = x == y");
    assert!(undeclared.contains("is not determined here"), "{undeclared}");
}

/// `<` and `>` are comparison operators in expression position and effect-row
/// brackets in type position; a signature carrying a row still parses and checks.
#[test]
fn comparison_operators_coexist_with_effect_rows() {
    let src = "$ @mod M\n$ Fail : @effect = fail : {} -> @Int\n\
               $ f : @Int -> <Fail | e> @Bool = \\n = n < 3\n\
               $ g : @Int -> <| e> @Bool = \\n = n > 3";
    assert_eq!(errors(src), "");
    assert_eq!(type_of(src, "g"), "@Int -> @Bool");
}

// -- issue #209: the checker must not fabricate a type ---------------------
//
// Each of these used to type-check and then fault at run time, because the
// checker answered "I cannot determine this" with a fresh unification variable,
// which unifies with anything.

#[test]
fn concat_on_an_unsupported_type_is_a_type_error() {
    // `++` was bound as a generic `a -> a -> a`, a signature that claimed a
    // domain the operator does not have.
    let e = errors(
        "$ @mod M\n\
         $ P : @struct = x: @Int\n\
         $ h : @Int = (P.{.x=1} ++ P.{.x=2}).x",
    );
    assert!(e.contains("no value of type `ICat P`"), "{e}");
}

#[test]
fn a_user_concat_instance_joins_the_existing_ones() {
    // An instance EXTENDS the interface, so `"a" ++ "b"` keeps working in the
    // module that adds one for its own type.
    let src = "$ @mod M\n\
               $ P : @struct = x: @Int\n\
               $ cat_p : ICat P = .{ .cat = \\a b = P.{ .x = a.x + b.x } }\n\
               $ s : @Str = \"a\" ++ \"b\"\n\
               $ p : P = P.{.x=1} ++ P.{.x=2}";
    assert_eq!(errors(src), "");
}

#[test]
fn an_unknown_variant_tag_is_a_type_error() {
    let e = errors(
        "$ @mod M\n\
         $ U : @union = A: {}, B: {@Int},\n\
         $ h : @Int = is U.A | .Nope.{q} => q | _ => 0",
    );
    assert!(e.contains("no union has a variant `Nope`"), "{e}");
}

#[test]
fn a_variant_pattern_on_a_non_union_is_a_type_error() {
    // The loose fallthrough bound `a` and `b` to fresh variables and never
    // unified with the scrutinee, so the arm could claim any type.
    let e = errors(
        "$ @mod M\n\
         $ P : @struct = x: @Int\n\
         $ h : P -> @Int = \\p = is p | P.Zzz.{a, b} => a | _ => 0",
    );
    assert!(e.contains("`P` is not a union"), "{e}");
}

#[test]
fn an_unknown_field_is_a_type_error() {
    let e = errors(
        "$ @mod M\n\
         $ P : @struct = x: @Int\n\
         $ h : P -> @Int = \\v = v.nonexistent",
    );
    assert!(e.contains("has no field `nonexistent`"), "{e}");
}

#[test]
fn a_struct_literal_with_an_unknown_field_is_a_type_error() {
    let e = errors(
        "$ @mod M\n\
         $ P : @struct = x: @Int, y: @Int\n\
         $ p : P = { .x = 1, .nope = 2 }",
    );
    assert!(e.contains("struct `P` has no field `nope`"), "{e}");
}

#[test]
fn a_field_on_an_unknown_type_becomes_a_row_constraint() {
    // Not knowing the record's type yet is a reason to CONSTRAIN it, not to
    // invent one: the parameter becomes an open row.
    assert_eq!(type_of("$ @mod M\n$ f = \\r = r.x", "f"), "{ x: a | b } -> a");
}

// -- issue #209: laziness is a property of the slot ------------------------

#[test]
fn a_recursive_slot_is_lazy_and_an_arrow_slot_is_not() {
    // `List a` leads back to `List`, so that slot is lazy. `{} -> Task` is
    // already a suspension, so thunking it again would leave callers applying a
    // thunk instead of the function (this is what `Task.Susp` relies on).
    let src = "$ @mod M\n\
               $ L : @union a = N: {}, C: {a, L a},\n\
               $ Task : @union = Fin: {}, Susp: { @Int, {} -> Task },\n\
               $ S : @struct t = head : t, tail : S t\n\
               $ P : @struct = x: @Int, y: @Int\n\
               $ z : @Int = 0";
    let (ast, core) = crate::parse_into(Ast::new(), CORE_SRC).expect("parse CORE");
    let (ast, prog) = crate::parse_into(ast, src).expect("parse");
    let types = std::rc::Rc::new(crate::Types::new());
    let mut core_checker = Checker::new(&ast, types.clone());
    core_checker.check_program(&core).expect("check CORE");
    let mut checker = Checker::new(&ast, types.clone());
    checker.import_from(&core_checker);
    checker.check_program(&prog).expect("check");
    let lazy = checker.lazy_slots();
    assert_eq!(lazy.get(&("L".into(), Some("C".into()))), Some(&vec![false, true]));
    assert_eq!(lazy.get(&("S".into(), None)), Some(&vec![false, true]));
    assert_eq!(lazy.get(&("Task".into(), Some("Susp".into()))), None);
    assert_eq!(lazy.get(&("P".into(), None)), None);
}

#[test]
fn a_literal_must_give_every_field() {
    let err = |src: &str| {
        let text = rendered_errors(src);
        assert!(!text.is_empty(), "expected an error for {src:?}");
        assert!(text.contains("--> test.thx:"), "no caret:\n{text}");
        text
    };
    let p = "$ @mod M\n$ P : @struct = a: @Int, b: @Int,\n";

    // Too few: the value would be missing a field its type promises, and reading
    // that field faults at run time.
    assert!(err(&format!("{p}$ y = P.{{1}}")).contains("missing field `b`"));
    assert!(err("$ @mod M\n$ y = List.Cons.{1}").contains("missing the one at index 1"));
    assert!(err("$ @mod M\n$ y = List.Cons.{}").contains("missing the one at index 0"));
    // Too many.
    assert!(err(&format!("{p}$ y = P.{{1, 2, 3}}")).contains("no field 2"));
    assert!(err("$ @mod M\n$ y = List.Cons.{1, List.Nil, 9}").contains("no field 2"));
    assert!(err(&format!("{p}$ y = P.{{ .c = 1 }}")).contains("has no field `c`"));

    // A complete literal, a named one, and an update are all fine.
    assert!(rendered_errors(&format!("{p}$ y = P.{{1, 2}}")).is_empty());
    assert!(rendered_errors(&format!("{p}$ y = P.{{ .a = 1, .b = 2 }}")).is_empty());
    assert!(rendered_errors(&format!("{p}$ y = P.{{ .a = 9 | P.{{1, 2}} }}")).is_empty());
    assert!(rendered_errors("$ @mod M\n$ y = List.Cons.{1, List.Nil}").is_empty());
}

#[test]
fn a_c_union_literal_picks_one_member() {
    let u = "$ @mod M\n$ U : @union @extern \"C\" = i: @Int, d: @Float64,\n";
    // Members share offset 0, so a literal names exactly one; completeness would
    // be wrong here, and two members would be nonsense.
    assert!(rendered_errors(&format!("{u}$ y = U.{{ .i = 42 }}")).is_empty());
    assert!(rendered_errors(&format!("{u}$ y = U.{{ .i = 1, .d = 2.0 }}"))
        .contains("exactly one member"));
}

#[test]
fn a_pattern_may_bind_fewer_fields_but_never_more() {
    let p = "$ @mod M\n$ P : @struct = a: @Int, b: @Int,\n";
    // Fewer is the point of a pattern.
    assert!(rendered_errors(&format!("{p}$ y = is P.{{1,2}} | P.{{x}} => x | _ => 0")).is_empty());
    assert!(rendered_errors(&format!("{p}$ y = is P.{{1,2}} | P.{{ .b = v }} => v | _ => 0")).is_empty());
    assert!(
        rendered_errors("$ @mod M\n$ y = is List.Cons.{1, List.Nil} | List.Cons.{h, t} => h | _ => 0")
            .is_empty()
    );
    // More binds a name with no field behind it, which faults when read.
    let surplus = rendered_errors(&format!("{p}$ y = is P.{{1,2}} | P.{{x, y, z}} => x | _ => 0"));
    assert!(surplus.contains("no field 2"), "{surplus}");
    let surplus = rendered_errors(
        "$ @mod M\n$ y = is List.Cons.{1, List.Nil} | List.Cons.{h, t, u} => h | _ => 0",
    );
    assert!(surplus.contains("no field 2"), "{surplus}");
    let unknown = rendered_errors(&format!("{p}$ y = is P.{{1,2}} | P.{{ .c = v }} => v | _ => 0"));
    assert!(unknown.contains("has no field `c`"), "{unknown}");
}

/// The rendered root of the check's error (empty when it passes) and the
/// messages-with-location of its warnings, for the exhaustiveness tests.
fn coverage(src: &str) -> (String, Vec<String>) {
    let (ast, core) = crate::parse_into(Ast::new(), CORE_SRC).expect("parse CORE");
    let (ast, prog) = crate::parse_into(ast, src).expect("parse");
    let types = std::rc::Rc::new(crate::Types::new());
    let mut core_checker = Checker::new(&ast, types.clone());
    core_checker.check_program(&core).expect("check CORE");
    let mut checker = Checker::new(&ast, types.clone());
    checker.import_from(&core_checker);
    let err = match checker.check_program(&prog) {
        Ok(_) => String::new(),
        Err(e) => format!("{e}"),
    };
    let warnings = checker
        .warnings()
        .iter()
        .map(|w| w.render_warning(src, "test.thx").lines().nth(1).unwrap_or("").trim().to_string())
        .collect();
    (err, warnings)
}

const L: &str = "$ @mod T\n$ L : @union = A, B, C,\n";

#[test]
fn missing_variant_is_an_error() {
    let (err, _) = coverage(&format!("{L}$ f : L -> @Int = \\l = is l | L.A => 0 | L.B => 1\n"));
    assert!(err.contains("NON_EXHAUSTIVE_MATCH"), "{err}");
    assert!(err.contains("`L.C`"), "{err}");
}

#[test]
fn every_variant_or_a_catch_all_is_exhaustive() {
    let all = format!("{L}$ f : L -> @Int = \\l = is l | L.A => 0 | L.B => 1 | .C => 2\n");
    assert_eq!(coverage(&all), (String::new(), vec![]));
    let wild = format!("{L}$ f : L -> @Int = \\l = is l | L.A => 0 | x => 1\n");
    assert_eq!(coverage(&wild), (String::new(), vec![]));
    let or = format!("{L}$ f : L -> @Int = \\l = is l | L.A | L.B | L.C => 0\n");
    assert_eq!(coverage(&or), (String::new(), vec![]));
}

#[test]
fn no_arms_cover_only_a_variantless_union() {
    let empty = "$ @mod T\n$ E : @union =\n$ f : E -> @Int = \\e = is e\n";
    assert_eq!(coverage(empty), (String::new(), vec![]));
    let (err, _) = coverage(&format!("{L}$ f : L -> @Int = \\l = is l\n"));
    assert!(err.contains("NON_EXHAUSTIVE_MATCH"), "{err}");
}

#[test]
fn guarded_arms_do_not_cover() {
    let src = format!("{L}$ f : L -> @Int = \\l = is l | L.A => 0 | L.B => 1 | L.C if @true => 2\n");
    assert!(coverage(&src).0.contains("`L.C`"));
    let src = format!("{L}$ f : L -> @Int = \\l = is l | x if @true => 0 | L.A => 1 | L.B => 1 | L.C => 2\n");
    assert_eq!(coverage(&src), (String::new(), vec![]));
}

#[test]
fn nested_witness_is_named() {
    let src = "$ @mod T\n$ M : @union a = No, Yes: {a},\n\
               $ f : M @Bool -> @Int = \\m = is m | .No => 0 | .Yes.{ @true } => 1\n";
    let (err, _) = coverage(src);
    assert!(err.contains("`M.Yes.{ false }`"), "{err}");
}

#[test]
fn infinite_domains_need_a_catch_all() {
    let (err, _) = coverage("$ @mod T\n$ f : @Int -> @Int = \\n = is n | 0 => 1 | 1 => 2\n");
    assert!(err.contains("`2 ...`"), "{err}");
    let (err, _) = coverage("$ @mod T\n$ f : @Float64 -> @Int = \\n = is n | 0.0 => 1 | 1.0 => 2\n");
    assert!(err.contains("`_`"), "{err}");
    let (err, _) = coverage("$ @mod T\n$ f : @Str -> @Int = \\s = is s | \"a\" => 1 | \"b\" ++ r => 2\n");
    assert!(err.contains("NON_EXHAUSTIVE_MATCH"), "{err}");
    let (err, _) = coverage("$ @mod T\n$ f : @Int -> @Int = \\n = is n | 0 ... 9 => 1 | 10 ... => 2\n");
    assert!(err.contains("does not cover `-1`"), "{err}");
    let (err, _) = coverage("$ @mod T\n$ f : @Int -> @Int = \\n = is n | 0 ... 9 => 1 | 20 ... => 2\n");
    assert!(err.contains("`10 ... 19`"), "{err}");
}

#[test]
fn bools_tuples_and_structs() {
    let ok = "$ @mod T\n$ f : {@Bool, @Bool} -> @Int = \\p = is p | {@true, _} => 0 | {_, @true} => 1 | {@false, @false} => 2\n";
    assert_eq!(coverage(ok), (String::new(), vec![]));
    let (err, _) = coverage("$ @mod T\n$ f : {@Bool, @Bool} -> @Int = \\p = is p | {@true, _} => 0 | {_, @true} => 1\n");
    assert!(err.contains("`{ false, false }`"), "{err}");
    let (err, _) = coverage(
        "$ @mod T\n$ P : @struct = x: @Bool, y: @Int,\n$ f : P -> @Int = \\p = is p | P.{ .x = @true } => 0\n",
    );
    assert!(err.contains("`P.{ .x = false }`"), "{err}");
}

#[test]
fn sequence_patterns_reduce_to_the_view() {
    let ok = "$ @mod T\n$ f : @Vec @Int -> @Int = \\v = is v | [] => 0 | [x] => x | x :: y :: r => y\n";
    assert_eq!(coverage(ok), (String::new(), vec![]));
    let (err, _) = coverage("$ @mod T\n$ f : @Vec @Int -> @Int = \\v = is v | [] => 0 | [x, y, ..r] => y\n");
    assert!(err.contains("`[_]`"), "{err}");
    let list = "$ @mod T\n$ f : List @Int -> @Int = \\l = is l | List.Nil => 0 | h :: t => h\n";
    assert_eq!(coverage(list), (String::new(), vec![]));
}

#[test]
fn unreachable_arms_warn() {
    let (err, w) = coverage(&format!("{L}$ f : L -> @Int = \\l = is l | L.A => 0 | m => 1 | _ => 2\n"));
    assert_eq!(err, "");
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("test.thx:3:"), "{w:?}");
    let (_, w) = coverage("$ @mod T\n$ f : @Int -> @Int = \\n = is n | 1 => 0 | 1 => 1 | _ => 2\n");
    assert_eq!(w.len(), 1, "{w:?}");
    let (_, w) = coverage("$ @mod T\n$ f : @Int -> @Int = \\n = is n | 1 ... 3 => 0 | 2 => 1 | _ => 2\n");
    assert_eq!(w.len(), 1, "{w:?}");
    let (_, w) = coverage("$ @mod T\n$ f : @Float64 -> @Int = \\n = is n | 1.0 ... 3.0 => 0 | 2.0 => 1 | _ => 2\n");
    assert!(w.is_empty(), "{w:?}");
}

#[test]
fn handler_value_arms_are_checked() {
    let src = "$ @mod T\n$ Exn : @effect = throw : @Str -> a,\n$ M : @union = Hit: {@Int}, Miss,\n\
               $ g : {} -> <Exn> M = \\u = M.Miss\n\
               $ f : @Int = do g {} ctl k | Exn.throw m => 0 | .Hit.{ v } => v\n";
    let (err, _) = coverage(src);
    assert!(err.contains("value arms do not cover `M.Miss`"), "{err}");
    assert!(!err.contains("%ret"), "{err}");
}

#[test]
fn unreachable_warnings_point_at_the_pattern() {
    let src = format!("{L}$ f : L -> @Int = \\l = is l | L.A => 0 | m => 1 | _ => 2\n");
    let (_, w) = coverage(&src);
    let col = src.lines().nth(2).unwrap().find("| _").unwrap() + 3;
    assert_eq!(w, vec![format!("--> test.thx:3:{col}")]);
}

#[test]
fn dead_alternatives_warn_separately() {
    let (err, w) = coverage(&format!("{L}$ f : L -> @Int = \\l = is l | L.A => 0 | L.B | L.A | L.C => 1\n"));
    assert_eq!(err, "");
    assert_eq!(w.len(), 1, "{w:?}");
    let (_, w) = coverage(&format!("{L}$ f : L -> @Int = \\l = is l | L.A | L.B | L.C => 0 | L.A | L.B => 1\n"));
    assert_eq!(w.len(), 1, "{w:?}");
}

#[test]
fn refutable_let_and_lambda_patterns_are_errors() {
    let m = "$ @mod T\n$ M : @union a = No, Yes: {a},\n";
    let (err, _) = coverage(&format!("{m}$ f : M @Int -> @Int = \\.Yes.{{ x }} = x\n"));
    assert!(err.contains("refutable pattern in a lambda parameter") && err.contains("`M.No`"), "{err}");
    let (err, _) = coverage(&format!("{m}$ g : M @Int -> @Int = \\m = let .Yes.{{ x }} = m in x\n"));
    assert!(err.contains("refutable pattern in a `let` binding"), "{err}");
    let (err, _) = coverage("$ @mod T\n$ h : @Nat8 -> @Int = \\0 = 1\n");
    assert!(err.contains("`1 ...`"), "{err}");
    let ok = "$ @mod T\n$ P : @struct = x: @Int, y: @Int,\n\
              $ f : P -> {@Int, @Int} -> @Int = \\P.{ x, _ } {a, b} = let {c, _} = {a, b} in x + c\n";
    assert_eq!(coverage(ok), (String::new(), vec![]));
}

const ROPE: &str = "$ @mod T\n$ Rope : @union = Leaf: {@Int}, Node: {Rope, Rope},\n\
    $ impl_ISeqView_for_Rope : @ISeqView Rope @Int = .{\n\
    \t.view = \\r = is r | Rope.Leaf.{ n } => SeqView.More.{ n, Rope.Node.{ r, r } } | _ => SeqView.Empty,\n}\n";

#[test]
fn mixing_variant_and_sequence_patterns_is_reported() {
    let (err, _) = coverage(&format!("{ROPE}$ f : Rope -> @Int = \\r = is r | Rope.Leaf.{{ n }} => 0 | [] => 1 | _ :: _ => 2\n"));
    assert!(err.contains("both its variants and sequence patterns") && err.contains("`Rope`"), "{err}");
    let only_seq = format!("{ROPE}$ f : Rope -> @Int = \\r = is r | [] => 1 | _ :: _ => 2\n");
    assert_eq!(coverage(&only_seq), (String::new(), vec![]));
    let catch_all = format!("{ROPE}$ f : Rope -> @Int = \\r = is r | Rope.Leaf.{{ n }} => 0 | [] => 1 | _ => 2\n");
    assert_eq!(coverage(&catch_all), (String::new(), vec![]));
    let apart = format!("{ROPE}$ f : {{Rope, Rope}} -> @Int = \\p = is p | {{Rope.Leaf.{{ n }}, _}} => 0 | {{Rope.Node.{{ a, b }}, []}} => 1 | {{_, _ :: _}} => 2\n");
    assert_eq!(coverage(&apart), (String::new(), vec![]));
}

#[test]
fn integer_ranges_cover_bounded_types() {
    let ok = "$ @mod T\n$ f : @Nat8 -> @Int = \\n = is n | 0 ... 127 => 0 | 128 ... 255 => 1\n";
    assert_eq!(coverage(ok), (String::new(), vec![]));
    let (err, _) = coverage("$ @mod T\n$ f : @Int8 -> @Int = \\n = is n | 0 ... => 0\n");
    assert!(err.contains("`-128 ... -1`"), "{err}");
    let signed = "$ @mod T\n$ f : @Int8 -> @Int = \\n = is n | -128 ... -1 => 1 | 0 ... => 2\n";
    assert_eq!(coverage(signed), (String::new(), vec![]));
    let (err, _) = coverage("$ @mod T\n$ f : @Nat8 -> @Int = \\n = is n | 0 ... 99 => 0 | 101 ... => 1\n");
    assert!(err.contains("does not cover `100`"), "{err}");
    let (err, _) = coverage("$ @mod T\n$ f : @Nat8 -> @Int = \\n = is n | 0 => 0 | 1 ... 9 => 1\n");
    assert!(err.contains("`10 ...`"), "{err}");
    let (_, w) = coverage("$ @mod T\n$ f : @Nat8 -> @Int = \\n = is n | 0 ... 200 => 0 | 100 ... 150 => 1 | _ => 2\n");
    assert_eq!(w.len(), 1, "{w:?}");
    let (_, w) = coverage("$ @mod T\n$ f : {@Nat8, @Bool} -> @Int = \\p = is p | {0 ... 9, @true} => 0 | {5, _} => 1 | {_, _} => 2\n");
    assert!(w.is_empty(), "{w:?}");
}

#[test]
fn sized_literals_must_fit_their_type() {
    let (err, _) = coverage("$ @mod T\n$ x : @Nat8 = 300\n");
    assert!(err.contains("INT_LITERAL_RANGE") && err.contains("300"), "{err}");
    let (err, _) = coverage("$ @mod T\n$ x : @Int8 = -129\n");
    assert!(err.contains("-129"), "{err}");
    let ok = "$ @mod T\n$ a : @Int8 = -128\n$ b : @Nat8 = 255\n$ c : @Nat8 = @cast 300\n";
    assert_eq!(coverage(ok), (String::new(), vec![]));
}

#[test]
fn a_mismatch_names_expected_and_found_the_right_way_round() {
    let text = rendered_errors("$ @mod M\n$ x : @Int = \"s\"\n");
    assert!(text.contains("expected @Int, found @Str"), "{text}");
    let text = rendered_errors("$ @mod M\n$ x = ((>): @Int -> @Int -> @Int)\n");
    assert!(text.contains("expected @Int, found @Bool"), "{text}");
    assert!(text.contains("expected because of this annotation"), "{text}");
    assert!(text.contains("found type `@Int -> @Int -> @Bool`"), "{text}");
}

#[test]
fn an_unknown_type_is_reported_before_the_errors_it_causes() {
    let ty = "$ @mod M\n$ MyTyp : @struct = a: @Int, b: @Int\n";
    for body in ["$ x = Mytype.{1, 2}\n", "$ foo : MyType = .{1, 2}\n", "$ foo : MyType = .{.a = 1, .b = 2}\n"] {
        let text = rendered_errors(&format!("{ty}{body}"));
        assert!(text.contains("unknown type `My"), "{text}");
        assert!(text.contains("help: did you mean `MyTyp`?"), "{text}");
    }
}

#[test]
fn an_unknown_qualified_name_is_a_check_error() {
    let text = rendered_errors("$ @mod M\n$ x = CORE.no_such_function 1\n");
    assert!(text.contains("`CORE` has no value named `no_such_function`"), "{text}");
    let text = rendered_errors("$ @mod M\n$ x = NOWHERE.f 1\n");
    assert!(text.contains("no module or effect `NOWHERE`"), "{text}");
    assert_eq!(errors("$ @mod M\n$ x : @Int = TARGET.int_bits\n"), "");
    assert!(errors("$ @mod M\n$ x : @Str = TARGET.int_bits\n").contains("expected @Str"));
}

#[test]
fn applying_a_non_function_names_the_argument() {
    let src = "$ @mod M\n$ f : @Int -> @Int = \\x =\n    x\n    1\n";
    let text = rendered_errors(src);
    assert!(text.contains("its type `@Int` is not a function"), "{text}");
    assert!(text.contains("- the argument"), "{text}");
    assert!(text.contains("help: if the argument is meant as a separate step"), "{text}");
}

#[test]
fn an_unbound_name_suggests_a_near_miss() {
    let text = rendered_errors("$ @mod M\n$ length : @Int = 1\n$ x = lenght\n");
    assert!(text.contains("help: did you mean `length`?"), "{text}");
}

#[test]
fn a_near_miss_instance_is_named_instead_of_none_in_scope() {
    let text = rendered_errors("$ @mod M\n$ x = for ([0 ... 100] : List @Int) \\i = i\n");
    assert!(text.contains("`impl_IFor_for_List` does not fit this use of `for`"), "{text}");
    assert!(text.contains("its second type parameter is `@Int`, but this use needs `{}`"), "{text}");
    assert!(!text.contains("in scope"), "{text}");
}

const ROW_ALIAS_EFFECTS: &str = "$ @mod M\n\
     $ Log : @effect = log : @Int -> {},\n\
     $ Gpu : @effect = draw : @Int -> {},\n\
     $ Net : @effect = fetch : @Int -> @Int,\n\
     $ App : @alias = <Gpu, Log>\n";

#[test]
fn row_alias_expands_to_its_labels_and_prints_folded() {
    let src = format!(
        "{ROW_ALIAS_EFFECTS}\
         $ frame : @Int -> <App> @Int = \\x = log x; draw x; x\n\
         $ wide : @Int -> <Log, Gpu, Net> @Int = \\x = fetch (frame x)\n\
         $ one : @Int -> <Log> @Int = \\x = log x; x\n"
    );
    assert_eq!(type_of(&src, "frame"), "@Int -> <App> @Int");
    // Every label of `App` is present, so the row prints with the alias.
    assert_eq!(type_of(&src, "wide"), "@Int -> <App, Net> @Int");
    // Not all of them: the labels print as written.
    assert_eq!(type_of(&src, "one"), "@Int -> <Log> @Int");
}

#[test]
fn row_alias_nests_and_keeps_a_label_once() {
    let src = format!(
        "{ROW_ALIAS_EFFECTS}\
         $ Big : @alias = <App, Net>\n\
         $ f : @Int -> <Big> @Int = \\x = fetch x; log x; x\n\
         $ g : @Int -> <App, Log> @Int = \\x = draw x; x\n"
    );
    assert_eq!(errors(&src), "");
    assert_eq!(type_of(&src, "f"), "@Int -> <Big> @Int");
    assert_eq!(type_of(&src, "g"), "@Int -> <App> @Int");
}

#[test]
fn parameterized_row_alias_takes_the_tail() {
    let src = format!(
        "{ROW_ALIAS_EFFECTS}\
         $ WithLog : @alias e = <Log | e>\n\
         $ f : @Int -> <WithLog e> @Int = \\x = log x; x\n\
         $ g : @Int -> <WithLog e, Net | e> @Int = \\x = fetch x\n\
         $ closed : @Int -> <WithLog> @Int = \\x = log x; x\n"
    );
    assert_eq!(type_of(&src, "f"), "@Int -> <Log | a> @Int");
    assert_eq!(type_of(&src, "g"), "@Int -> <Log, Net | a> @Int");
    // Under-applied, the tail is a fresh variable, as for a type alias.
    assert_eq!(type_of(&src, "closed"), "@Int -> <Log | a> @Int");
}

#[test]
fn row_alias_rejects_a_label_outside_it() {
    let src = format!("{ROW_ALIAS_EFFECTS}$ f : @Int -> <App> @Int = \\x = fetch x\n");
    let e = errors(&src);
    assert!(e.contains("effect `Net` is performed but not handled"), "got: {e:?}");
}

#[test]
fn unhandled_label_names_the_alias_it_came_from() {
    let src = format!(
        "{ROW_ALIAS_EFFECTS}\
         $ frame : @Int -> <App> @Int = \\x = log x; draw x; x\n\
         $ g : @Int -> <Gpu> @Int = \\x = frame x\n"
    );
    let e = errors(&src);
    assert!(
        e.contains("effect `Log` (part of `App`) is performed but not handled"),
        "got: {e:?}"
    );
}

#[test]
fn row_alias_misuse_is_reported() {
    let e = errors(&format!("{ROW_ALIAS_EFFECTS}$ x : App = 3\n"));
    assert!(e.contains("`App` is an effect row alias, not a type"), "got: {e:?}");
    let e = errors(&format!(
        "{ROW_ALIAS_EFFECTS}$ N : @alias = @Int\n$ f : @Int -> <N> @Int = \\x = x\n"
    ));
    assert!(e.contains("type alias `N` is not an effect row"), "got: {e:?}");
    let e = errors(&format!("{ROW_ALIAS_EFFECTS}$ f : @Int -> <App e> @Int = \\x = x\n"));
    assert!(e.contains("row alias `App` takes 0 parameter(s) but 1 were given"), "got: {e:?}");
    let e = errors(&format!("{ROW_ALIAS_EFFECTS}$ f : @Int -> <Log e> @Int = \\x = x\n"));
    assert!(e.contains("effect `Log` takes no parameters"), "got: {e:?}");
    let e = errors(&format!("{ROW_ALIAS_EFFECTS}$ W : @alias = <Log | e>\n"));
    assert!(e.contains("declares no type parameters"), "got: {e:?}");
    let e = errors(&format!("{ROW_ALIAS_EFFECTS}$ A1 : @alias = <A2>\n$ A2 : @alias = <A1>\n"));
    assert!(e.contains("row alias `A1` expands to itself"), "got: {e:?}");
}

// -- effect parameters (#291) ---------------------------------------------

const BOX_EFFECT: &str = "$ @mod M\n$ Box : @effect a = take : @Int -> a,\n";

#[test]
fn an_effect_parameter_is_fixed_by_its_row() {
    let src = format!("{BOX_EFFECT}$ f : {{}} -> <Box @Int> @Int = \\u = Box.take 1 + Box.take 2\n");
    assert_eq!(type_of(&src, "f"), "{} -> <Box @Int> @Int");
}

#[test]
fn one_handler_serves_one_instance() {
    let src = format!(
        "{BOX_EFFECT}\
         $ f : {{}} -> <Box> @Int = \\u = let s : @Str = Box.take 0 in Box.take 1\n"
    );
    assert!(errors(&src).contains("expected @Int, found @Str"), "got: {:?}", errors(&src));
}

#[test]
fn an_operation_variable_is_opaque_in_the_handler() {
    let ok = "$ @mod M\n$ Put : @effect = put : a -> {},\n\
              $ f : ({} -> <Put> {}) -> @Int = \\b = do b {} ctl k | Put.put x => k {} | u => 0\n";
    assert_eq!(errors(ok), "");
    let bad = "$ @mod M\n$ Put : @effect = put : a -> {},\n\
               $ f : ({} -> <Put> {}) -> @Int = \\b = do b {} ctl k | Put.put x => x + 0 | u => 0\n";
    assert!(errors(bad).contains("`a` of `Put.put`"), "got: {:?}", errors(bad));
}

#[test]
fn a_handler_cannot_resume_with_a_value_of_its_own() {
    let src = "$ @mod M\n$ Exn : @effect = throw : @Int -> a,\n\
               $ ok : @Int = do Exn.throw 1 + 2 ctl k | Exn.throw n => n\n\
               $ bad : @Int = do Exn.throw 1 + 2 ctl k | Exn.throw n => k n\n";
    let e = errors(src);
    assert!(e.contains("expected `a` of `Exn.throw`") && e.contains("found @Int"), "got: {e:?}");
}

#[test]
fn mutual_recursion_keeps_a_row_parameter_open() {
    let src = "$ @mod M\n\
               $ St : @union e = Done: @Int, More: ({} -> <e> St e),\n\
               $ serve : St e -> @Int -> <e> @Int = \\st s =\n\
               \tis st | St.Done.{ r } => r + s | St.More.{ k } => step k s\n\
               $ step : ({} -> <e> St e) -> @Int -> <e> @Int = \\k s = serve (k {}) s\n";
    // A bare row variable is not printed; a closed one would show as `St <>`.
    assert_eq!(type_of(src, "serve"), "St a -> @Int -> @Int");
    assert_eq!(type_of(src, "step"), "({} -> St a) -> @Int -> @Int");
}
