//! The paradox suite (issue #484): every known way to prove `@Bottom` in a
//! system like the kernel, as a script the kernel must reject, and for the
//! right reason. A paradox needs one of these steps; rejecting the step
//! rejects the paradox.
//!
//! Also in the suite: `type_formers_are_not_injective` in `tests/scripts.rs`
//! (Miquel's injection of a large type into a small one, CIC^obs Section 2.3,
//! #477) and `the_rationals_reject_wrong_claims` (a quotient eliminator that
//! does not respect the relation). Abel and Coquand's looping casts are not a
//! proof of `@Bottom` but a loss of normalisation; the kernel must accept them
//! and still terminate.

/// Check `src` and require an error whose rendering contains `needle`.
fn rejects(name: &str, src: &str, needle: &str) {
    match kernel::check_script(src) {
        Ok(_) => panic!("{name}: accepted\n{src}"),
        Err(e) => {
            let shown = e.render(src, name);
            assert!(shown.contains(needle), "{name}: rejected, but not for `{needle}`:\n{shown}");
        }
    }
}

/// Girard's paradox (1972), simplified by Hurkens (1995), needs a universe
/// that contains itself.
#[test]
fn no_universe_contains_itself() {
    rejects("type-in-type", "$ t : @Type 1 = @Type 1\n", "expected `@Type 1`, found `@Type 2`");
    rejects("prop-in-prop", "$ t : @Type 0 = @Type 0\n", "expected `@Type 0`, found `@Type 1`");
}

/// The same paradox through data: an inductive type with a field as large as
/// its own universe is a universe containing itself.
#[test]
fn no_field_is_as_large_as_its_type() {
    rejects("large-field", "$ Big : @Type 1 = @data | mk (@Type 1)\n", "this field is too large");
    rejects(
        "large-field-polymorphic",
        "$ Big : @ctx {u: @l} -> @Type (u + 1) = @data | mk (@Type (u + 1))\n",
        "this field is too large",
    );
}

/// A negative occurrence gives a fixpoint combinator, and with it a proof of
/// anything (the standard counterexample to dropping strict positivity).
#[test]
fn no_negative_occurrence() {
    rejects("negative", "$ Bad : @Type 1 = @data | mk (Bad -> @Bottom)\n", "may only occur as a whole field");
    rejects("negative-nested", "$ Bad : @Type 1 = @data | mk ((Bad -> @Top) -> @Top)\n", "may only occur as a whole field");
}

/// An inductive proposition with two constructors and large elimination
/// distinguishes two proofs that proof irrelevance makes equal, so
/// `true ~ false`. The kernel has no inductive propositions at all (survey,
/// question 2).
#[test]
fn no_inductive_propositions() {
    rejects("inductive-prop", "$ P : @Type 0 = @data | a | b\n", "inductive propositions are not supported");
}

/// A proposition used as data would let one proof stand for many values.
/// Cumulativity starts at `@Type 1`.
#[test]
fn a_proposition_is_not_data() {
    rejects("prop-as-data", "$ f : @Type 0 -> @Type 1 = \\X => X\n", "expected `@Type 1`, found `@Type 0`");
}

/// A strong sum with a large first component, in the impredicative universe,
/// gives Girard's paradox through impredicativity (Coquand, 1986). The sort of
/// a Sigma type is the maximum of its sides', so it cannot be a proposition.
#[test]
fn no_strong_impredicative_sum() {
    rejects("strong-impredicative-sigma", "$ s : @Type 0 = (X: @Type 1) * @Top\n", "expected `@Type 0`, found `@Type 2`");
}

/// A quotient eliminator whose method does not respect the relation tells
/// two equal classes apart.
#[test]
fn quotient_elimination_needs_respect() {
    let src = "\
$ Nat : @Type 1 = @data | zero | succ Nat
$ R : Nat -> Nat -> @Type 0 = \\x y => @Top
$ Q : @Type 1 = @Quot Nat R (\\x => @tt) (\\x y e => @tt) (\\x y z e f => @tt)
$ bad : Q -> Nat = \\q => @qelim (\\_ => Nat) (\\x => x) (\\x y e => @refl x) q
";
    rejects("unrespected-quotient", src, "~ y`, found `x ~ x`");
}

/// Check `src` and then normalise each of `names`, failing if either does
/// not finish within a few seconds.
fn terminates(name: &'static str, src: &'static str, names: &'static [&'static str]) {
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let outcome = kernel::check_script(src).map(|sig| {
            names.iter().map(|n| kernel::normal_form(&sig, n).expect("a definition")).collect::<Vec<_>>()
        });
        let _ = done.send(outcome.map_err(|e| e.render(src, name)));
    });
    match wait.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(Ok(_)) => {}
        Ok(Err(shown)) => panic!("{name}: rejected:\n{shown}"),
        Err(_) => panic!("{name}: did not terminate"),
    }
}

/// Abel and Coquand (LMCS 2020): a cast that reduces to its argument when its
/// two types are equal loops in an impredicative universe, given that all
/// propositions are equal, or given only propositional extensionality. The
/// kernel never reduces a cast of a proof (it is the irrelevant witness), and
/// it uses `@cast A A e t ~ t` only when comparing, not when evaluating.
#[test]
fn casts_of_proofs_do_not_loop() {
    terminates(
        "all-propositions-equal",
        r"
$ Bot : @Type 0 = (A: @Type 0) -> A
$ Tru : @Type 0 = Bot -> Bot
$ delta : Tru = \z => z Tru z
$ omega : ((A B: @Type 0) -> A ~ B) -> Bot = \h A => @cast Tru A (h Tru A) delta
$ Omega : ((A B: @Type 0) -> A ~ B) -> Bot = \h => delta (omega h)
$ Q : (((A B: @Type 0) -> A ~ B) -> Bot) -> @Type 1
$ q : Q Omega
$ r : Q (\h => omega h) = q
",
        &["omega", "Omega"],
    );
    terminates(
        "propositional-extensionality",
        r"
$ taut : (A B: @Type 0) -> A -> B -> A ~ B = \A B a b => (\_ => b, \_ => a)
$ T : @Type 0 = (A: @Type 0) -> A -> A
$ id : T -> T = \x => x
$ delta : T -> T = \z => z (T -> T) id z
$ omega : T = \A a => @cast (T -> T) A (taut (T -> T) A id a) delta
$ Omega : T = delta omega
$ Q : T -> @Type 1
$ q : Q Omega
$ r : Q (delta (delta omega)) = q
",
        &["omega", "Omega"],
    );
}
