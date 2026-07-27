//! Type inference for Thrax: Hindley-Milner via Algorithm W with level-based
//! generalization.
//!
//! * [`ty`] is the type representation.
//! * [`engine`] is the unification engine (union-find + levels).
//! * [`infer`] is Algorithm W over the [`syntax`] AST.
//!
//! Entry point: [`check`], which infers and generalizes every global definition
//! of a [`syntax::Program`].

pub mod engine;
pub mod infer;
pub mod scc;
pub mod ty;

pub use engine::Engine;
pub use infer::Checker;
pub use ty::Type;

use diag::Result;
use syntax::Program;

/// Type-check a program, returning each global definition's generalized type.
pub fn check<'a>(program: &Program<'a>) -> Result<Vec<(&'a str, Type)>> {
    Checker::new().check_program(program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arena::Arena;

    /// Infer the type of a single global `x`, rendered as a string.
    fn type_of(src: &str, name: &str) -> String {
        let arena = Arena::new();
        let program = syntax::parse(src, &arena).expect("parse");
        let mut checker = Checker::new();
        let results = checker
            .check_program(&program)
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
        let arena = Arena::new();
        let program = syntax::parse(src, &arena).expect("parse");
        match Checker::new().check_program(&program) {
            Ok(_) => String::new(),
            Err(e) => format!("{e}"),
        }
    }

    #[test]
    fn monomorphic_arithmetic() {
        assert_eq!(type_of("@mod M\n$ x = 1 + 2", "x"), "Int");
    }

    #[test]
    fn identity_is_polymorphic() {
        // The classic let-generalization test: `id` gets `forall a. a -> a`.
        assert_eq!(type_of("@mod M\n$ id = \\x = x", "id"), "`a -> `a");
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
        // `(a -> b) -> a -> b`.
        assert_eq!(type_of(src, "apply"), "(`a -> `b) -> `a -> `b");
    }

    #[test]
    fn if_unifies_branches_and_condition() {
        assert_eq!(
            type_of("@mod M\n$ f = \\n = if n ?= 0 then 1 else n", "f"),
            "Int -> Int"
        );
    }

    #[test]
    fn recursion_through_predeclared_globals() {
        let src = "@mod M\n$ fib = \\n = if n ?= 0 then 0 else fib (n - 1) + fib (n - 2)";
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
                   $ Box : @struct = val: `T\n\
                   $ b : Box Str = Box.{ .val = \"hi\" }\n\
                   $ out = b.val";
        assert_eq!(type_of(src, "b"), "Box Str");
        assert_eq!(type_of(src, "out"), "Str");
    }

    #[test]
    fn union_constructor_and_match() {
        let src = "@mod M\n\
                   $ Maybe : @union = Just: `T, None: {}\n\
                   $ m : Maybe Int = Maybe.Just.{ 7 }\n\
                   $ get = \\d = \\o = when o is Maybe.Just.{x} then x else d";
        assert_eq!(type_of(src, "m"), "Maybe Int");
        assert_eq!(type_of(src, "get"), "`a -> Maybe `a -> `a");
    }

    #[test]
    fn mutually_recursive_globals_via_scc() {
        let src = "@mod M\n\
                   $ is_even : Int -> Int = \\n = if n ?= 0 then 1 else is_odd (n - 1)\n\
                   $ is_odd  : Int -> Int = \\n = if n ?= 0 then 0 else is_even (n - 1)";
        assert_eq!(type_of(src, "is_even"), "Int -> Int");
        assert_eq!(type_of(src, "is_odd"), "Int -> Int");
    }

    #[test]
    fn recursive_let_binding() {
        let src = "@mod M\n\
                   $ f = \\m = let go = \\n acc = if n ?= 0 then acc else go (n - 1) (acc + n) \
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
        let arena = Arena::new();
        let dep_src = "@mod OPT\n\
                       $ Option : @union = Some: `T, None: {}\n\
                       $ is_some : Option `T -> Bool = \\o = \
                       when o is Option.Some.{_} then true else false\n\
                       $ unwrap_or : Option `T -> `T -> `T = \\o d = \
                       when o is Option.Some.{x} then x else d";
        let dep = syntax::parse(dep_src, &arena).expect("parse dep");
        let mut dep_checker = Checker::new();
        dep_checker.check_program(&dep).expect("check dep");

        let use_src = "@mod U\n\
                       $ with OPT\n\
                       $ o : Option Int = Option.Some.{ 41 }\n\
                       $ present = is_some o\n\
                       $ value = unwrap_or o 0";
        let program = syntax::parse(use_src, &arena).expect("parse use");
        let mut checker = Checker::new();
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
        // Two-field record: one tuple argument, destructured to x and y.
        assert_eq!(
            type_of("@mod M\n$ add : {x: Int, y: Int} -> Int = x + y", "add"),
            "{Int, Int} -> Int"
        );
        // One-field record collapses to a bare parameter.
        assert_eq!(
            type_of("@mod M\n$ inc : {x: Int} -> Int = x + 1", "inc"),
            "Int -> Int"
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
                   $ len : Array -> Int = \\a = array_len a\n\
                   $ n = len [1, 2]";
        assert_eq!(type_of(src, "n"), "Int");
    }

    #[test]
    fn array_primitives_overload_on_array_and_str() {
        let src = "@mod M\n\
                   $ a : Array = array_push (@array.{ 0 }) 65\n\
                   $ n = array_len a\n\
                   $ b = array_get \"hi\" 0";
        assert_eq!(type_of(src, "a"), "Array");
        assert_eq!(type_of(src, "n"), "Int");
        assert_eq!(type_of(src, "b"), "Int");
    }
}
