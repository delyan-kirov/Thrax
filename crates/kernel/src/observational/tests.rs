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
