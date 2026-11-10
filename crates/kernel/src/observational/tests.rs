use crate::check::tests::{accept, reject, NAT};
use crate::normal_form;

const EQUALITY: &str = r"
$ sym : @ctx {u: @l} -> (A: @Type (u + 1)) -> (x y: A) -> x ~ y -> y ~ x =
    \A x y e => @transp x (\z => z ~ x) (@refl x) y e
$ trans : @ctx {u: @l} -> (A: @Type (u + 1)) -> (x y z: A) -> x ~ y -> y ~ z -> x ~ z =
    \A x y z e f => @transp y (\w => x ~ w) e z f
$ ap : @ctx {u: @l, v: @l} -> (A: @Type (u + 1)) -> (B: @Type (v + 1)) -> (f: A -> B) -> (x y: A) -> x ~ y -> f x ~ f y =
    \A B f x y e => @transp x (\z => f x ~ f z) (@refl (f x)) y e
$ subst : @ctx {u: @l, v: @l} -> (A: @Type (u + 1)) -> (P: A -> @Type (v + 1)) -> (x y: A) -> x ~ y -> P x -> P y =
    \A P x y e p => @cast (P x) (P y) (@transp x (\z => P x ~ P z) (@refl (P x)) y e) p
";

#[test]
fn equality_is_reflexive_by_computation() {
    accept(&format!("{NAT}$ t : add two two ~ four = @refl four\n"));
    reject(&format!("{NAT}$ t : add two two ~ two = @refl two\n"), "expected `@Bottom`");
}

#[test]
fn constructors_are_disjoint_and_injective() {
    accept(&format!(
        r"{NAT}
$ disjoint : zero ~ succ zero -> @Bottom = \e => e
$ injective : (m n: Nat) -> succ m ~ succ n -> m ~ n = \m n e => e
"
    ));
}

#[test]
fn function_extensionality_holds_by_computation() {
    accept(&format!(
        r"{NAT}
$ funext : (f g: Nat -> Nat) -> ((x: Nat) -> f x ~ g x) -> f ~ g = \f g h => h
$ plus_zero : (n: Nat) -> add zero n ~ n = \n => @refl n
$ same : (\(n: Nat) => add zero n) ~ (\n => n) = \n => @refl n
"
    ));
}

#[test]
fn propositions_are_extensional() {
    accept(
        r"
$ P : @Type 0
$ Q : @Type 0
$ propext : (P -> Q) -> (Q -> P) -> P ~ Q = \f g => (f, g)
$ proofs : (p q: P) -> p ~ q = \p q => @tt
",
    );
}

#[test]
fn pairs_are_equal_componentwise() {
    accept(&format!(
        r"{NAT}
$ annotated : (a b: Nat) -> a ~ b -> ((a, zero) : Nat * Nat) ~ (b, zero) = \a b e => (e, @tt)
"
    ));
}

#[test]
fn equality_of_types_compares_formers() {
    accept(&format!(
        r"{NAT}
$ same : Nat ~ Nat = @tt
$ different : Nat ~ (Nat -> Nat) -> @Bottom = \e => e
$ universes : @Type 1 ~ @Type 1 = @tt
$ levels : (@Type 1 : @Type 3) ~ @Type 2 -> @Bottom = \e => e
$ arrows : (Nat -> Nat) ~ (Nat -> Nat) = (@tt, \_ => @tt)
"
    ));
}

#[test]
fn transport_derives_the_usual_lemmas() {
    accept(&format!("{NAT}{EQUALITY}"));
}

#[test]
fn cast_computes_on_canonical_types() {
    let sig = accept(&format!(
        r"{NAT}{EQUALITY}
$ moved : Nat = subst @ctx {{.u = 0, .v = 0}} Nat (\_ => Nat) two two (@refl two) four
$ pair : Nat * Nat = @cast (Nat * Nat) (Nat * Nat) (@tt, \_ => @tt) (two, four)
$ fn : Nat -> Nat = @cast (Nat -> Nat) (Nat -> Nat) (@tt, \_ => @tt) (\n => succ n)
"
    ));
    assert_eq!(normal_form(&sig, "moved").unwrap(), "succ (succ (succ (succ zero)))");
    assert_eq!(normal_form(&sig, "pair").unwrap(), "(succ (succ zero), succ (succ (succ (succ zero))))");
    assert_eq!(normal_form(&sig, "fn").unwrap(), r"\_ => succ _");
}

#[test]
fn cast_is_stuck_on_variables() {
    let sig = accept(&format!(
        r"{NAT}
$ A : @Type 1
$ e : A ~ Nat
$ a : A
$ n : Nat = @cast A Nat e a
"
    ));
    assert_eq!(normal_form(&sig, "n").unwrap(), "@cast A Nat e a");
}

#[test]
fn equality_proofs_must_typecheck() {
    reject(&format!("{NAT}$ bad : (m n: Nat) -> m ~ n = \\m n => @refl m\n"), "expected `m ~ n`, found `m ~ m`");
    reject(&format!("{NAT}$ bad : @cast Nat (Nat -> Nat) @tt zero ~ zero\n"), "expected `@Bottom`, found `@Top`");
}

// Naturals up to parity: `R m n` is `even m ~ even n`, an equality of
// booleans, so it computes.
const PARITY: &str = r"
$ Bool : @Type 1 = @data | false | true
$ not : Bool -> Bool = \b => Bool.elim @ctx {.m = 1} (\_ => Bool) true false b
$ even : Nat -> Bool = \n => Nat.elim @ctx {.m = 1} (\_ => Nat -> Bool) (\_ => true) (\_ r => \_ => not (r zero)) n zero
$ R : Nat -> Nat -> @Type 0 = \m n => even m ~ even n
$ Parity : @Type 1 = @Quot Nat R
    (\x => @refl (even x))
    (\x y e => @transp (even x) (\b => b ~ even x) (@refl (even x)) (even y) e)
    (\x y z e f => @transp (even y) (\b => even x ~ b) e (even z) f)
$ class : Nat -> Parity = \n => @qin n
";

#[test]
fn classes_are_equal_by_the_relation() {
    accept(&format!(
        r"{NAT}{PARITY}
$ same : class zero ~ class two = @tt
$ apart : class zero ~ class (succ zero) -> @Bottom = \e => e
$ related : (m n: Nat) -> R m n -> class m ~ class n = \m n e => e
"
    ));
}

#[test]
fn eliminating_a_class_needs_respect() {
    let sig = accept(&format!(
        r"{NAT}{PARITY}
$ parity : Parity -> Bool = \q => @qelim (\_ => Bool) even (\x y e => e) q
$ of_four : Bool = parity (class four)
$ classes_agree : (p q: Parity) -> p ~ q -> parity p ~ parity q =
    \p q e => @transp p (\r => parity p ~ parity r) (@refl (parity p)) q e
"
    ));
    assert_eq!(normal_form(&sig, "of_four").unwrap(), "true");
    // The identity on representatives does not respect parity.
    reject(
        &format!("{NAT}{PARITY}$ leak : Parity -> Nat = \\q => @qelim (\\_ => Nat) (\\n => n) (\\x y e => e) q\n"),
        "expected `x ~ y`, found",
    );
}

#[test]
fn quotients_need_an_equivalence() {
    reject(
        &format!("{NAT}$ Lt : @Type 1 = @Quot Nat (\\m n => @Bottom) (\\x => @tt) (\\x y e => e) (\\x y z e f => e)\n"),
        "expected `@Bottom`, found `@Top`",
    );
    reject(
        "$ P : @Type 0\n$ Q : @Type 1 = @Quot P (\\x y => @Top) (\\x => @tt) (\\x y e => e) (\\x y z e f => e)\n",
        "only data can be quotiented",
    );
}

#[test]
fn casts_move_classes_through_representatives() {
    let sig = accept(&format!(
        r"{NAT}{PARITY}
$ moved : Parity = @cast Parity Parity (@tt, \a b => (\h => h, \h => h)) (class two)
"
    ));
    assert_eq!(normal_form(&sig, "moved").unwrap(), "@qin (succ (succ zero))");
}
