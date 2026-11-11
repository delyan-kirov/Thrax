//! A test for every rule in the theory ledger (issue #484): each calls
//! `applies` on a script where the rule fires and `blocked` on a near miss
//! where it must not. `tests/ledger.rs` fails when an implemented rule has
//! no test tagged with its name here.

use kernel::{check_script, normal_form, Signature};

/// Shared definitions, checked before every case.
const PRELUDE: &str = r"
$ Nat : @Type 1 = @data | zero | succ Nat
$ Bool : @Type 1 = @data | false | true
$ List : @ctx {u: @l} -> (a: @Type (u + 1)) -> @Type (u + 1) = @data | nil | cons (h: a) (t: List @ctx {.u = u} a)
$ add : Nat -> Nat -> Nat = \m n => Nat.elim @ctx {.m = 1} (\_ => Nat) n (\_ r => succ r) m
$ two : Nat = succ (succ zero)
$ four : Nat = succ (succ (succ (succ zero)))
$ P : Nat -> @Type 1
$ not : Bool -> Bool = \b => Bool.elim @ctx {.m = 1} (\_ => Bool) true false b
$ even : Nat -> Bool = \n => Nat.elim @ctx {.m = 1} (\_ => Nat -> Bool) (\_ => true) (\_ r => \_ => not (r zero)) n zero
$ R : Nat -> Nat -> @Type 0 = \m n => even m ~ even n
$ Parity : @Type 1 = @Quot Nat R
    (\x => @refl (even x))
    (\x y e => @transp (even x) (\b => b ~ even x) (@refl (even x)) (even y) e)
    (\x y z e f => @transp (even y) (\b => even x ~ b) e (even z) f)
$ class : Nat -> Parity = \n => @qin n
$ parity : Parity -> Bool = \q => @qelim (\_ => Bool) even (\x y e => e) q
";

/// The prelude and `src` check; the signature, to read normal forms from.
fn applies(src: &str) -> Signature {
    let all = format!("{PRELUDE}{src}");
    match check_script(&all) {
        Ok(sig) => sig,
        Err(e) => panic!("rejected:\n{}", e.render(&all, "rule.ott")),
    }
}

/// The prelude checks, `src` does not, and the error mentions `needle`.
fn blocked(src: &str, needle: &str) {
    let all = format!("{PRELUDE}{src}");
    match check_script(&all) {
        Ok(_) => panic!("accepted, expected an error containing `{needle}`:\n{src}"),
        Err(e) => {
            let shown = e.render(&all, "rule.ott");
            assert!(shown.contains(needle), "error does not mention `{needle}`:\n{shown}");
        }
    }
}

fn normal(sig: &Signature, name: &str) -> String {
    normal_form(sig, name).expect("a definition")
}

// ## Universes and functions

// Test [Univ]
#[test]
fn univ() {
    applies("$ u : @Type 2 = @Type 1\n$ o : @Type 1 = @Type 0\n");
    blocked("$ u : @Type 1 = @Type 1\n", "expected `@Type 1`, found `@Type 2`");
}

// Test [Level-Polymorphism]
#[test]
fn level_polymorphism() {
    applies("$ id : @ctx {u: @l} -> (t: @Type u) -> t -> t = \\t x => x\n$ n : Nat = id @ctx {.u = 1} Nat zero\n");
    blocked("$ id : @ctx {u: @l} -> (t: @Type u) -> t -> t = \\t x => x\n$ n : Nat = id Nat zero\n", "needs its level `u`");
}

// Test [Pi-Form]
#[test]
fn pi_form() {
    applies("$ F : @Type 2 = @Type 1 -> Nat\n$ G : @Type 0 = (A: @Type 0) -> A\n");
    blocked("$ F : @Type 1 = @Type 1 -> Nat\n", "expected `@Type 1`, found `@Type 2`");
}

// Test [Fun]
#[test]
fn fun() {
    applies("$ f : Nat -> Nat = \\n => succ n\n");
    blocked("$ f : Nat = \\(n: Nat) => n\n", "expected `Nat`, found `Nat -> Nat`");
}

// Test [App]
#[test]
fn app() {
    applies("$ n : Nat = add two zero\n");
    blocked("$ n : Nat = add true zero\n", "expected `Nat`, found `Bool`");
}

// Test [Beta]
#[test]
fn beta() {
    applies("$ p : P (succ zero)\n$ q : P ((\\(n: Nat) => succ n) zero) = p\n");
    blocked("$ p : P (succ zero)\n$ q : P ((\\(n: Nat) => n) zero) = p\n", "expected `P zero`");
}

// Test [Eta-Pi]
#[test]
fn eta_pi() {
    applies("$ f : Nat -> Nat\n$ Q : (Nat -> Nat) -> @Type 1\n$ p : Q f\n$ q : Q (\\n => f n) = p\n");
    blocked("$ f : Nat -> Nat\n$ Q : (Nat -> Nat) -> @Type 1\n$ p : Q f\n$ q : Q (\\n => f zero) = p\n", "expected `Q");
}

// Test [Let]
#[test]
fn let_() {
    applies("$ p : P (succ zero)\n$ q : P (let n: Nat = zero in succ n) = p\n");
    blocked("$ p : P (succ zero)\n$ q : P (let n: Nat = zero in n) = p\n", "expected `P zero`");
}

// Test [Proof-Irrelevance]
#[test]
fn proof_irrelevance() {
    let proofs = "$ a : A\n$ b : A\n$ Q : A -> @Type 1\n$ q : Q a\n$ r : Q b = q\n";
    applies(&format!("$ A : @Type 0\n{proofs}"));
    blocked(&format!("$ A : @Type 1\n{proofs}"), "expected `Q b`, found `Q a`");
}

// ## Pairs, truth and falsity

// Test [Sigma-Form]
#[test]
fn sigma_form() {
    applies("$ S : @Type 1 = Nat * Nat\n$ E : @Type 0 = (x: @Top) * @Top\n");
    blocked("$ S : @Type 1 = @Type 1 * Nat\n", "expected `@Type 1`, found `@Type 2`");
}

// Test [Sigma-Pair]
#[test]
fn sigma_pair() {
    applies("$ s : Nat * Nat = (zero, succ zero)\n$ p : P (succ zero)\n$ q : P s.2 = p\n");
    blocked("$ s : Nat * Nat = (zero, succ zero)\n$ p : P (succ zero)\n$ q : P s.1 = p\n", "expected `P zero`");
}

// Test [Eta-Sigma]
#[test]
fn eta_sigma() {
    let pair = "$ s : Nat * Nat\n$ Q : Nat * Nat -> @Type 1\n$ p : Q s\n";
    applies(&format!("{pair}$ q : Q (s.1, s.2) = p\n"));
    blocked(&format!("{pair}$ q : Q (s.2, s.1) = p\n"), "expected `Q");
}

// Test [Top]
#[test]
fn top() {
    applies("$ t : @Top = @tt\n");
    blocked("$ t : @Bottom = @tt\n", "expected `@Bottom`, found `@Top`");
}

// Test [Bottom]
#[test]
fn bottom() {
    applies("$ f : @Bottom -> Nat = \\e => @absurd Nat e\n");
    blocked("$ f : @Top -> Nat = \\e => @absurd Nat e\n", "expected `@Bottom`, found `@Top`");
}

// ## Equality

// Test [Eq-Form]
#[test]
fn eq_form() {
    applies("$ E : @Type 0 = zero ~ succ zero\n");
    blocked("$ E : @Type 0 = zero ~ true\n", "expected `Nat`, found `Bool`");
    blocked("$ E : @Type 0 = @tt ~ @tt\n", "equality is not formed at the proposition `@Top`");
}

// Test [Eq-Box]
#[test]
fn eq_box() {
    applies("$ t : (u v: (x: Nat) * @Top) -> u.1 ~ v.1 -> u ~ v = \\u v e => (e, @tt)\n");
    blocked("$ t : (u v: (x: Nat) * @Top) -> u ~ v = \\u v => (@tt, @tt)\n", "expected `u.1 ~ v.1`, found `@Top`");
}

// Test [Eq-Stuck]
#[test]
fn eq_stuck() {
    applies("$ r : @ctx {u: @l} -> (A: @Type u) -> (a: A) -> a ~ a = \\A a => @refl a\n");
    let lists = "(A B: @Type 1) -> List @ctx {.u = 0} A ~ List @ctx {.u = 0} B";
    blocked(&format!("$ inj : {lists} -> A ~ B = \\A B e => e\n"), "expected `A ~ B`, found `List @ctx {.u = 0} A ~ List @ctx {.u = 0} B`");
}

// Test [Refl]
#[test]
fn refl() {
    applies("$ t : add two two ~ four = @refl four\n");
    blocked("$ t : add two two ~ two = @refl two\n", "expected `@Bottom`");
}

// Test [Transport]
#[test]
fn transport() {
    let pre = "$ Q : Nat -> @Type 0\n";
    applies(&format!("{pre}$ tr : (m n: Nat) -> m ~ n -> Q m -> Q n = \\m n e q => @transp m (\\k => Q k) q n e\n"));
    blocked(&format!("{pre}$ tr : (m n: Nat) -> m ~ n -> Q m -> Q n = \\m n e q => @transp n (\\k => Q k) q m e\n"), "expected `Q n`, found `Q m`");
}

// Test [Cast]
#[test]
fn cast() {
    applies("$ c : (A B: @Type 1) -> A ~ B -> A -> B = \\A B e a => @cast A B e a\n");
    blocked("$ c : (A B: @Type 1) -> A ~ B -> A -> B = \\A B e a => @cast A B @tt a\n", "found `@Top`");
}

// Test [Eq-Fun]
#[test]
fn eq_fun() {
    applies("$ fe : (f g: Nat -> Nat) -> ((n: Nat) -> f n ~ g n) -> f ~ g = \\f g h => h\n");
    blocked("$ fe : (f g: Nat -> Nat) -> f zero ~ g zero -> f ~ g = \\f g h => h\n", "found `f zero ~ g zero`");
}

// Test [Eq-Pair]
#[test]
fn eq_pair() {
    applies("$ t : (a b: Nat) -> a ~ b -> ((a, zero) : Nat * Nat) ~ (b, zero) = \\a b e => (e, @tt)\n");
    blocked("$ t : (a b: Nat) -> a ~ b -> ((a, zero) : Nat * Nat) ~ (b, succ zero) = \\a b e => (e, @tt)\n", "expected `@Bottom`");
}

// Test [Eq-Omega]
#[test]
fn eq_omega() {
    let pre = "$ A : @Type 0\n$ B : @Type 0\n";
    applies(&format!("{pre}$ ext : (A -> B) -> (B -> A) -> A ~ B = \\f g => (f, g)\n"));
    blocked(&format!("{pre}$ ext : (A -> B) -> A ~ B = \\f => f\n"), "expected `(A -> B) * (B -> A)`, found `A -> B`");
}

// Test [Eq-Univ]
#[test]
fn eq_univ() {
    applies("$ t : @Type 1 ~ @Type 1 = @tt\n");
    blocked("$ t : @Type 1 ~ (Nat -> @Type 1) = @tt\n", "expected `@Bottom`, found `@Top`");
}

// Test [Eq-Univ-Neq]
#[test]
fn eq_univ_neq() {
    applies("$ t : Nat ~ (Nat -> Nat) -> @Bottom = \\e => e\n$ l : (@Type 1 -> Nat) ~ (Nat -> @Type 1) -> @Bottom = \\e => e\n");
    blocked("$ t : Nat ~ Nat -> @Bottom = \\e => e\n", "expected `@Bottom`, found `@Top`");
}

// Test [Eq-Pi]
#[test]
fn eq_pi() {
    applies("$ t : (Nat -> Nat) ~ (Nat -> Nat) = (@tt, \\_ => @tt)\n");
    blocked("$ t : (Nat -> Nat) ~ (Nat -> Bool) = (@tt, \\_ => @tt)\n", "expected `@Bottom`, found `@Top`");
}

// Test [Eq-Sigma]
#[test]
fn eq_sigma() {
    applies("$ t : (Nat * Nat) ~ (Nat * Nat) = (@tt, \\_ => @tt)\n");
    blocked("$ t : (Nat * Nat) ~ (Nat * Bool) = (@tt, \\_ => @tt)\n", "expected `@Bottom`, found `@Top`");
}

// Test [Eq-Ctor]
#[test]
fn eq_ctor() {
    applies("$ t : (m n: Nat) -> m ~ n -> succ m ~ succ n = \\m n e => e\n");
    blocked("$ t : zero ~ succ zero = @tt\n", "expected `@Bottom`, found `@Top`");
}

// Test [Eq-Univ-Data]
#[test]
fn eq_univ_data() {
    applies("$ t : Nat ~ Nat = @tt\n");
    blocked("$ t : Nat ~ Bool = @tt\n", "expected `@Bottom`, found `@Top`");
}

// Test [Eq-Quot-Class]
#[test]
fn eq_quot_class() {
    applies("$ t : class zero ~ class two = @tt\n");
    blocked("$ t : class zero ~ class (succ zero) = @tt\n", "expected `@Bottom`, found `@Top`");
}

// Test [Eq-Quot]
#[test]
fn eq_quot() {
    applies("$ t : Parity ~ Parity = (@tt, \\x y => (\\r => r, \\r => r))\n");
    blocked("$ t : Parity ~ Nat = @tt\n", "expected `@Bottom`, found `@Top`");
}

// ## Cast

// Test [Cast-Pi]
#[test]
fn cast_pi() {
    let sig = applies("$ f : Nat -> Nat = @cast (Nat -> Nat) (Nat -> Nat) (@tt, \\_ => @tt) (\\n => succ n)\n");
    assert!(normal(&sig, "f").starts_with('\\'), "{}", normal(&sig, "f"));
    blocked("$ f : Nat -> Nat = @cast (Nat -> Nat) (Nat -> Bool) (@tt, \\_ => @tt) (\\n => succ n)\n", "expected `@Bottom`, found `@Top`");
}

// Test [Cast-Sigma]
#[test]
fn cast_sigma() {
    let sig = applies("$ s : Nat * Nat = @cast (Nat * Nat) (Nat * Nat) (@tt, \\_ => @tt) (zero, two)\n");
    assert_eq!(normal(&sig, "s"), "(zero, succ (succ zero))");
    blocked("$ s : Nat * Nat = @cast (Nat * Nat) (Nat * Bool) (@tt, \\_ => @tt) (zero, two)\n", "expected `@Bottom`, found `@Top`");
}

// Test [Cast-Univ]
#[test]
fn cast_univ() {
    let sig = applies("$ T : @Type 1 = @cast (@Type 1) (@Type 1) @tt Nat\n$ n : T = zero\n");
    assert_eq!(normal(&sig, "T"), "Nat");
    blocked("$ T : @Type 1 = @cast (@Type 1) (@Type 1) @tt Nat\n$ n : T = true\n", "expected `Nat`, found `Bool`");
}

// Test [Cast-Ctor]
#[test]
fn cast_ctor() {
    let sig = applies("$ n : Nat = @cast Nat Nat @tt (succ zero)\n");
    assert_eq!(normal(&sig, "n"), "succ zero");
    blocked("$ n : Nat = @cast Bool Nat @tt true\n", "expected `@Bottom`, found `@Top`");
}

// Test [Cast-Quot]
#[test]
fn cast_quot() {
    let sig = applies("$ e : Parity ~ Parity\n$ c : Parity = @cast Parity Parity e (class zero)\n");
    assert_eq!(normal(&sig, "c"), "@qin zero");
    blocked("$ c : Parity = @cast Nat Parity @tt zero\n", "expected `@Bottom`, found `@Top`");
}

// Test [Cast-Refl]
#[test]
fn cast_refl() {
    applies("$ m : Nat\n$ e : Nat ~ Nat\n$ p : P m\n$ q : P (@cast Nat Nat e m) = p\n");
    let pre = "$ A : @Type 1\n$ e : A ~ Nat\n$ a : A\n$ m : Nat\n$ p : P m\n";
    blocked(&format!("{pre}$ q : P (@cast A Nat e a) = p\n"), "expected `P (@cast A Nat @irrelevant a)`, found `P m`");
}

// Test [Proof-Witness]
#[test]
fn proof_witness() {
    let sig = applies("$ t : add two two ~ four = @refl four\n");
    assert_eq!(normal(&sig, "t"), "@irrelevant");
    blocked("$ t : add two two ~ two = @refl four\n", "expected `@Bottom`");
}

// ## Inductive types

// Test [Data-Form]
#[test]
fn data_form() {
    applies("$ Tree : @Type 1 = @data | leaf | node Tree Nat Tree\n");
    blocked("$ Big : @Type 1 = @data | mk (@Type 1)\n", "too large");
    blocked("$ Yes : @Type 0 = @data | yes\n", "inductive propositions are not supported");
}

// Test [Data-Ctor]
#[test]
fn data_ctor() {
    applies("$ xs : List @ctx {.u = 0} Nat = cons @ctx {.u = 0} Nat two (nil @ctx {.u = 0} Nat)\n");
    blocked("$ n : Nat = succ true\n", "expected `Nat`, found `Bool`");
}

// Test [Data-Elim]
#[test]
fn data_elim() {
    applies("$ pred : Nat -> Nat = \\n => Nat.elim @ctx {.m = 1} (\\_ => Nat) zero (\\k _ => k) n\n");
    blocked("$ pred : Nat -> Nat = \\n => Nat.elim @ctx {.m = 1} (\\_ => Nat) zero (\\k => k) n\n", "expected `Nat -> Nat`, found `Nat`");
}

// Test [Data-Iota]
#[test]
fn data_iota() {
    applies("$ p : P (add two two)\n$ q : P four = p\n");
    blocked("$ p : P (add two two)\n$ q : P two = p\n", "expected `P (succ (succ zero))`");
}

// Test [Positivity-Restricted]
#[test]
fn positivity_restricted() {
    applies("$ Rose : @Type 1 = @data | leaf | node Rose Rose\n");
    blocked("$ Bad : @Type 1 = @data | mk (Bad -> Bad)\n", "may only occur as a whole field");
    blocked("$ Fn : @Type 1 = @data | mk (Nat -> Fn)\n", "may only occur as a whole field");
}

// ## Quotients

// Test [Quot-Form]
#[test]
fn quot_form() {
    applies("$ Q : @Type 1 = @Quot Nat (\\x y => @Top) (\\x => @tt) (\\x y e => @tt) (\\x y z e f => @tt)\n");
    blocked("$ Q : @Type 1 = @Quot Nat (\\x y => @Bottom) (\\x => @tt) (\\x y e => e) (\\x y z e f => e)\n", "expected `@Bottom`, found `@Top`");
}

// Test [Quot-Intro]
#[test]
fn quot_intro() {
    applies("$ c : Parity = @qin four\n");
    blocked("$ c : Parity = @qin true\n", "expected `Nat`, found `Bool`");
}

// Test [Quot-Elim]
#[test]
fn quot_elim() {
    applies("$ flip : Parity -> Bool = \\q => @qelim (\\_ => Bool) (\\n => not (even n)) (\\x y e => @transp (even x) (\\b => not (even x) ~ not b) (@refl (not (even x))) (even y) e) q\n");
    blocked("$ leak : Parity -> Nat = \\q => @qelim (\\_ => Nat) (\\n => n) (\\x y e => e) q\n", "~ y`, found");
}

// Test [Quot-Beta]
#[test]
fn quot_beta() {
    let sig = applies("$ b : Bool = parity (class four)\n");
    assert_eq!(normal(&sig, "b"), "true");
    blocked("$ q : Parity\n$ Q : Bool -> @Type 1\n$ t : Q true\n$ u : Q (parity q) = t\n", "expected `Q (");
}
