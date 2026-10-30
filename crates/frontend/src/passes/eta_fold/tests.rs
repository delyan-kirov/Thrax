use super::*;

/// Builds one global's body in the program's store.
type Build = fn(&mut Terms) -> Aol<Term>;

/// A program of `globals`, each built in its store by its function.
fn prog(globals: Vec<(&str, Build)>) -> CoreProgram {
    let mut p = CoreProgram::new(StrId::intern("M"));
    for (name, build) in globals {
        let term = build(&mut p.terms);
        p.globals.push((StrId::intern(name), term));
    }
    p
}

/// `\a b = op p q`, with `p` and `q` the parameters named.
fn eta(t: &mut Terms, op: &str, p: &str, q: &str) -> Aol<Term> {
    let op = t.var(op);
    let (p, q) = (t.var(p), t.var(q));
    let body = t.app(op, p);
    let body = t.app(body, q);
    let inner = t.lam(StrId::intern("b"), body);
    t.lam(StrId::intern("a"), inner)
}

fn instance(t: &mut Terms, field: Aol<Term>) -> Aol<Term> {
    t.add(Term::Struct {
        name: StrId::intern("IAdd"),
        base: None,
        fields: vec![(StrId::intern("add"), field)],
    })
}

/// `\a b = @iadd a b`, the shape every CORE arithmetic instance field has.
fn eta_add(t: &mut Terms) -> Aol<Term> {
    let field = eta(t, "@iadd", "a", "b");
    instance(t, field)
}

/// A method that reorders its arguments: `\a b = @iadd b a`.
fn flipped(t: &mut Terms) -> Aol<Term> {
    let field = eta(t, "@iadd", "b", "a");
    instance(t, field)
}

/// `CORE.impl_IAdd_for_int.add` applied to `args` integers.
fn projection(t: &mut Terms, args: i64) -> Aol<Term> {
    let instance = t.add(Term::Var {
        module: Some(StrId::intern("CORE")),
        name: StrId::intern("impl_IAdd_for_int"),
    });
    let mut out = t.field(instance, "add");
    for n in 1..=args {
        let n = t.add(Term::Int(n));
        out = t.app(out, n);
    }
    out
}

fn rendered(p: &CoreProgram, i: usize) -> String {
    p.terms.render(p.globals[i].1)
}

#[test]
fn a_saturated_projection_of_an_eta_primitive_folds() {
    let mut p = prog(vec![
        ("CORE.impl_IAdd_for_int", eta_add),
        ("M.sum", |t| projection(t, 2)),
    ]);
    fold_primitive_methods(&mut p);
    assert_eq!(rendered(&p, 1), "(App (App (Var @iadd) (Int 1)) (Int 2))");
}

/// An UNDER-applied projection keeps the field read: there is nothing to
/// eta-reduce, and the primitive is not a first-class value.
#[test]
fn an_under_applied_projection_is_left_alone() {
    let mut p = prog(vec![
        ("CORE.impl_IAdd_for_int", eta_add),
        ("M.add1", |t| projection(t, 1)),
    ]);
    fold_primitive_methods(&mut p);
    assert!(rendered(&p, 1).contains("Field"));
}

/// A method that is not an eta-expanded primitive (it reorders its arguments)
/// must not be folded: the rewrite is only sound when the body applies the
/// primitive to its own parameters in order.
#[test]
fn a_method_that_reorders_its_arguments_is_left_alone() {
    let mut p = prog(vec![
        ("CORE.impl_IAdd_for_int", flipped),
        ("M.sum", |t| projection(t, 2)),
    ]);
    fold_primitive_methods(&mut p);
    assert!(rendered(&p, 1).contains("Field"));
}
