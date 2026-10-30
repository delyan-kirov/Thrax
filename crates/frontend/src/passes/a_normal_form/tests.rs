use crate::core::Terms;
use super::ANormalizer;

/// A nested application names its inner call: `f (g x)` => `let %a0 = g x in f %a0`.
#[test]
fn names_nested_application() {
    let mut t = Terms::new();
    let (f, g, x) = (t.var("f"), t.var("g"), t.var("x"));
    let gx = t.app(g, x);
    let e = t.app(f, gx);
    let out = ANormalizer::default().term(&mut t, e);
    assert_eq!(t.render(out), "(Let %a0 (App (Var g) (Var x)) (App (Var f) (Var %a0)))");
}

/// An application of two atoms is already in A-normal form.
#[test]
fn atoms_pass_through() {
    let mut t = Terms::new();
    let (f, x) = (t.var("f"), t.var("x"));
    let e = t.app(f, x);
    let out = ANormalizer::default().term(&mut t, e);
    assert_eq!(t.render(out), "(App (Var f) (Var x))");
}

/// Both operands of a call are named left-to-right: `f (g a) (h b)`.
#[test]
fn both_operands_named_in_order() {
    let mut t = Terms::new();
    let (f, g, a, h, b) = (t.var("f"), t.var("g"), t.var("a"), t.var("h"), t.var("b"));
    let ga = t.app(g, a);
    let inner = t.app(f, ga);
    let hb = t.app(h, b);
    let e = t.app(inner, hb);
    let out = ANormalizer::default().term(&mut t, e);
    assert_eq!(
        t.render(out),
        "(Let %a0 (App (Var g) (Var a)) (Let %a1 (App (Var f) (Var %a0)) \
         (Let %a2 (App (Var h) (Var b)) (App (Var %a1) (Var %a2)))))"
    );
}

/// A field access on a computation names the record first.
#[test]
fn field_names_its_record() {
    let mut t = Terms::new();
    let (mk, z) = (t.var("mk"), t.var("z"));
    let rec = t.app(mk, z);
    let e = t.field(rec, "x");
    let out = ANormalizer::default().term(&mut t, e);
    assert_eq!(t.render(out), "(Let %a0 (App (Var mk) (Var z)) (Field x (Var %a0)))");
}
