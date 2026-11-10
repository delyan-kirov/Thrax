use crate::{check_script, normal_form, Signature};

fn accept(src: &str) -> Signature {
    match check_script(src) {
        Ok(sig) => sig,
        Err(e) => panic!("rejected:\n{}", e.render(src, "test.ott")),
    }
}

fn reject(src: &str, needle: &str) {
    match check_script(src) {
        Ok(_) => panic!("accepted, expected an error containing `{needle}`"),
        Err(e) => {
            let shown = e.render(src, "test.ott");
            assert!(shown.contains(needle), "error does not mention `{needle}`:\n{shown}");
        }
    }
}

// Church numerals over a proposition-free universe, so conversion has some
// computation to do.
const CHURCH: &str = r"
$ Num : @Type 2 = (t: @Type 1) -> (t -> t) -> t -> t
$ zero : Num = \t s z => z
$ succ : Num -> Num = \n t s z => s (n t s z)
$ add : Num -> Num -> Num = \m n t s z => m t s (n t s z)
$ two : Num = succ (succ zero)
$ four : Num = succ (succ two)
$ P : Num -> @Type 1
";

#[test]
fn identity_at_every_level() {
    accept(
        r"
$ id : @ctx {u: @l} -> (t: @Type u) -> t -> t = \t x => x
$ id1 : (t: @Type 1) -> t -> t = id @ctx {.u = 1}
$ selfapp : (t: @Type 1) -> t -> t = id @ctx {.u = 2} ((t: @Type 1) -> t -> t) id1
",
    );
}

#[test]
fn conversion_evaluates() {
    let src = format!("{CHURCH}$ p : P (add two two)\n$ q : P four = p\n");
    accept(&src);
    let src = format!("{CHURCH}$ p : P (add two two)\n$ q : P two = p\n");
    reject(&src, "expected `P");
}

#[test]
fn normal_forms_read_back() {
    let sig = accept(CHURCH);
    assert_eq!(normal_form(&sig, "two").unwrap(), r"\t => \s => \z => s (s z)");
}

#[test]
fn functions_are_compared_up_to_eta() {
    accept(
        r"
$ A : @Type 1
$ f : A -> A
$ Q : (A -> A) -> @Type 1
$ q : Q f
$ r : Q (\x => f x) = q
",
    );
}

#[test]
fn proofs_of_a_proposition_are_equal() {
    accept(
        r"
$ P : @Type 0
$ p : P
$ p' : P
$ Q : P -> @Type 1
$ q : Q p
$ r : Q p' = q
",
    );
    // Data are not proofs.
    reject(
        r"
$ A : @Type 1
$ a : A
$ a' : A
$ Q : A -> @Type 1
$ q : Q a
$ r : Q a' = q
",
        "expected `Q a'`",
    );
}

#[test]
fn propositions_are_impredicative() {
    accept(
        r"
$ True : @Type 0 = (p: @Type 0) -> p -> p
$ tt : True = \p x => x
",
    );
}

#[test]
fn universes_are_cumulative_from_data_up() {
    accept("$ A : @Type 1\n$ B : @Type 2 = A\n");
    reject("$ A : @Type 0\n$ B : @Type 1 = A\n", "expected `@Type 1`, found `@Type 0`");
    reject("$ A : @Type 2\n$ B : @Type 1 = A\n", "expected `@Type 1`, found `@Type 2`");
    accept("$ F : @Type 1 -> @Type 1\n$ G : @Type 1 -> @Type 2 = F\n");
}

#[test]
fn levels_must_be_given_and_bound() {
    reject("$ id : @ctx {u: @l} -> (t: @Type u) -> t -> t = \\t x => x\n$ f : @Type 1 = id\n", "needs its level `u`");
    reject("$ A : @Type v\n", "unknown level `v`");
}

#[test]
fn names_are_resolved() {
    reject("$ A : @Type 1\n$ B : @Type 1 = Aa\n", "unknown name `Aa`");
    reject("$ A : @Type 1\n$ A : @Type 1\n", "already defined");
}

#[test]
fn let_is_transparent() {
    let src = format!("{CHURCH}$ p : P four\n$ q : P (let n = two in add n n) = p\n");
    accept(&src);
}

#[test]
fn unannotated_lambdas_need_an_expected_type() {
    reject("$ f = \\x => x\n", "expected `:`");
    reject("$ A : @Type 1\n$ f : @Type 1 = (\\x => x) A\n", "cannot infer the type of `x`");
}

#[test]
fn pairs_are_dependent() {
    let sig = accept(
        r"
$ A : @Type 1
$ B : A -> @Type 1
$ a : A
$ b : B a
$ p : (x: A) * B x = (a, b)
$ second : B p.1 = p.2
$ swap : (A * A) -> (A * A) = \q => (q.2, q.1)
$ again : A * A = swap (swap (a, a))
",
    );
    assert_eq!(normal_form(&sig, "again").unwrap(), "(a, a)");
    reject("$ A : @Type 1\n$ a : A\n$ p : A * A = (a, @tt)\n", "expected `A`, found `@Top`");
}

#[test]
fn pairs_are_compared_up_to_eta() {
    accept(
        r"
$ A : @Type 1
$ p : A * A
$ Q : (A * A) -> @Type 1
$ q : Q p
$ r : Q (p.1, p.2) = q
",
    );
}

#[test]
fn a_pair_of_proofs_is_a_proposition() {
    accept(
        r"
$ P : @Type 0
$ Q : @Type 0
$ And : @Type 0 = P * Q
$ p : P
$ q : Q
$ both : And = (p, q)
$ swap : (P * Q) -> (Q * P) = \h => (h.2, h.1)
",
    );
}

#[test]
fn refined_values_are_data() {
    // A pair of data and a proof (a refinement, `{x: A if p x}`) is data,
    // and two of them are equal when their data parts are.
    accept(
        r"
$ A : @Type 1
$ P : A -> @Type 0
$ Refined : @Type 1 = (x: A) * P x
$ a : A
$ p : P a
$ p' : P a
$ Q : Refined -> @Type 1
$ q : Q (a, p)
$ r : Q (a, p') = q
",
    );
}

#[test]
fn bottom_proves_anything() {
    accept(
        r"
$ A : @Type 1
$ Not : @Type 0 -> @Type 0 = \p => p -> @Bottom
$ explode : @Bottom -> A = \e => @absurd A e
$ trivial : @Top = @tt
$ contradiction : (P: @Type 0) -> P -> Not P -> @Bottom = \P p n => n p
",
    );
    reject("$ A : @Type 1\n$ a : A\n$ bad : A = @absurd A @tt\n", "expected `@Bottom`, found `@Top`");
}
