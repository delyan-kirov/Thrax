use crate::{check_script, normal_form, Signature};

pub(crate) fn accept(src: &str) -> Signature {
    match check_script(src) {
        Ok(sig) => sig,
        Err(e) => panic!("rejected:\n{}", e.render(src, "test.ott")),
    }
}

pub(crate) fn reject(src: &str, needle: &str) {
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
fn universes_are_not_cumulative() {
    accept("$ A : @Type 1\n$ B : @Type 1 = A\n");
    reject("$ A : @Type 1\n$ B : @Type 2 = A\n", "expected `@Type 2`, found `@Type 1`");
    reject("$ A : @Type 0\n$ B : @Type 1 = A\n", "expected `@Type 1`, found `@Type 0`");
    reject("$ A : @Type 2\n$ B : @Type 1 = A\n", "expected `@Type 1`, found `@Type 2`");
    reject("$ F : @Type 1 -> @Type 1\n$ G : @Type 1 -> @Type 2 = F\n", "expected");
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

pub(crate) const NAT: &str = r"
$ Nat : @Type 1 = @data | zero | succ Nat
$ add : Nat -> Nat -> Nat = \m n => Nat.elim @ctx {.m = 1} (\_ => Nat) n (\_ r => succ r) m
$ two : Nat = succ (succ zero)
$ four : Nat = succ (succ (succ (succ zero)))
$ P : Nat -> @Type 1
";

#[test]
fn recursors_compute_on_constructors() {
    let src = format!("{NAT}$ p : P (add two two)\n$ q : P four = p\n");
    let sig = accept(&src);
    assert_eq!(normal_form(&sig, "q").unwrap(), "p");
    reject(&format!("{NAT}$ p : P (add two two)\n$ q : P two = p\n"), "expected `P (succ (succ zero))`");
}

#[test]
fn recursors_are_stuck_on_variables() {
    // `add n zero` does not reduce: the recursion is on the first argument.
    let src = format!("{NAT}$ n : Nat\n$ p : P (add zero n)\n$ q : P n = p\n");
    accept(&src);
    reject(&format!("{NAT}$ n : Nat\n$ p : P (add n zero)\n$ q : P n = p\n"), "expected `P n`");
}

#[test]
fn recursor_types_are_generated() {
    let sig = accept(NAT);
    let elim = sig.lookup("Nat.elim").unwrap();
    let ty = crate::term::TermDisplay {
        sig: &sig,
        names: &[],
        level_names: &sig.global(elim).level_params,
        term: &sig.global(elim).ty,
    }
    .to_string();
    assert_eq!(
        ty,
        "(P: Nat -> @Type m) -> P zero -> ((f0: Nat) -> P f0 -> P (succ f0)) -> (x: Nat) -> P x"
    );
}

#[test]
fn parameters_and_dependent_motives() {
    accept(&format!(
        r"{NAT}
$ List : @ctx {{u: @l}} -> (a: @Type (u + 1)) -> @Type (u + 1) = @data | nil | cons (h: a) (t: List @ctx {{.u = u}} a)
$ length : List @ctx {{.u = 0}} Nat -> Nat =
    \xs => List.elim @ctx {{.u = 0, .m = 1}} Nat (\_ => Nat) zero (\_ _ r => succ r) xs
$ xs : List @ctx {{.u = 0}} Nat = cons @ctx {{.u = 0}} Nat two (cons @ctx {{.u = 0}} Nat four (nil @ctx {{.u = 0}} Nat))
$ p : P (length xs)
$ q : P two = p
$ Unit : @Type 1 = @data | unit
$ Vec : Nat -> @Type 1 = \n => Nat.elim @ctx {{.m = 2}} (\_ => @Type 1) Unit (\_ t => Nat * t) n
$ v : Vec two = (zero, (four, unit))
"
    ));
}

#[test]
fn constructors_are_injective_by_conversion() {
    accept(&format!(
        r"{NAT}
$ n : Nat
$ p : P (succ (add zero n))
$ q : P (succ n) = p
"
    ));
}

#[test]
fn inductive_declarations_are_checked() {
    reject("$ Bad : @Type 1 = @data | mk (Bad -> Bad)\n", "may only occur as a whole field");
    reject("$ Prop : @Type 0 = @data | yes\n", "inductive propositions are not supported");
    reject("$ Big : @Type 1 = @data | mk (@Type 1)\n", "too large");
    reject("$ Idx : @Type 1 -> @Type 2 = @data | mk\n$ I : @Type 1 = @data | a | a\n", "`a` is already defined");
    reject("$ F : @Type 1 = @data | mk\n$ G : F = @data | mk\n", "must end in `@Type l`");
}

const VEC: &str = r"
$ Vec : (a: @Type 1) -> (n: Nat) -> @Type 1 = @data
    | vnil : Vec a zero
    | vcons (m: Nat) (h: a) (t: Vec a m) : Vec a (succ m)
";

#[test]
fn indexed_families_have_an_indexed_recursor() {
    let sig = accept(&format!("{NAT}{VEC}"));
    let elim = sig.lookup("Vec.elim").unwrap();
    let shown = crate::term::TermDisplay {
        sig: &sig,
        names: &[],
        level_names: &sig.global(elim).level_params,
        term: &sig.global(elim).ty,
    };
    assert_eq!(
        shown.to_string(),
        "(a: @Type 1) -> (P: (n: Nat) -> Vec a n -> @Type m) -> P zero (vnil a) \
         -> ((m: Nat) -> (h: a) -> (t: Vec a m) -> P m t -> P (succ m) (vcons a m h t)) \
         -> (n: Nat) -> (x: Vec a n) -> P n x"
    );
}

#[test]
fn the_index_rules_out_constructors() {
    // `head` needs no real case for `vnil`: its motive is `{}`-like at zero.
    let src = format!(
        r"{NAT}{VEC}
$ Unit : @Type 1 = @data | unit
$ Head : Nat -> @Type 1 = \k => Nat.elim @ctx {{.m = 2}} (\_ => @Type 1) Unit (\_ _ => Nat) k
$ head : (n: Nat) -> Vec Nat (succ n) -> Nat =
    \n v => Vec.elim @ctx {{.m = 1}} Nat (\k _ => Head k) unit (\m h t r => h) (succ n) v
$ h : Nat = head zero (vcons Nat zero four (vnil Nat))
"
    );
    let sig = accept(&src);
    assert_eq!(normal_form(&sig, "h").unwrap(), "succ (succ (succ (succ zero)))");
}

#[test]
fn a_value_at_propositionally_equal_indices_is_a_cast() {
    let src = format!(
        r"{NAT}{VEC}
$ length : (a: @Type 1) -> (n: Nat) -> Vec a n -> Nat =
    \a n v => Vec.elim @ctx {{.m = 1}} a (\_ _ => Nat) zero (\m h t r => succ r) n v
$ k : Nat
$ e : succ zero ~ k
$ one : Vec Nat (succ zero) = vcons Nat zero two (vnil Nat)
$ moved : Vec Nat k =
    @cast (Vec Nat (succ zero)) (Vec Nat k) (@transp (succ zero) (\j => Vec Nat (succ zero) ~ Vec Nat j) (@refl (Vec Nat (succ zero))) k e) one
$ len : Nat = length Nat k moved
"
    );
    let sig = accept(&src);
    assert!(normal_form(&sig, "moved").unwrap().starts_with("@cast (Vec Nat (succ zero)) (Vec Nat k) @irrelevant (vcons Nat zero"));
    assert_eq!(normal_form(&sig, "len").unwrap(), "succ zero");
}

#[test]
fn the_identity_type_is_an_indexed_family() {
    let src = format!(
        r"{NAT}
$ Id : (A: @Type 1) -> (x: A) -> (y: A) -> @Type 1 = @data | idrefl : Id A x x
$ J : (A: @Type 1) -> (x: A) -> (P: (y: A) -> Id A x y -> @Type 1) -> P x (idrefl A x) -> (y: A) -> (p: Id A x y) -> P y p =
    \A x P d y p => Id.elim @ctx {{.m = 1}} A x P d y p
$ p : P two
$ q : P (J Nat two (\_ _ => Nat) two two (idrefl Nat two)) = p
"
    );
    accept(&src);
}

#[test]
fn indexed_families_are_restricted() {
    reject(&format!("{NAT}$ V : (n: Nat) -> @Type 1 = @data | a : V zero | b\n"), "must state its result");
    reject(&format!("{NAT}$ V : (n: Nat) -> @Type 1 = @data | mk : Nat\n"), "must be `V` at its own parameters");
    reject(
        &format!("{NAT}$ V : (n: Nat) -> (f: P n) -> @Type 1 = @data | mk (p: P zero) : V zero p\n"),
        "may depend on the parameters, not on earlier indices",
    );
    reject("$ V : (p: @Top) -> @Type 1 = @data | mk : V @tt\n", "must be data");
}
