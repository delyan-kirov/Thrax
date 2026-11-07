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

/// `\a b => op p q`, with `p` and `q` the parameters named.
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

/// `\a b => @iadd a b`, the shape every CORE arithmetic instance field has.
fn eta_add(t: &mut Terms) -> Aol<Term> {
    let field = eta(t, "@iadd", "a", "b");
    instance(t, field)
}

/// A method that reorders its arguments: `\a b => @iadd b a`.
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
/// eta-reduce at the use.
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

/// `STR.at` applied to `args` integers.
fn global_call(t: &mut Terms, args: i64) -> Aol<Term> {
    let mut out = t.add(Term::Var {
        module: Some(StrId::intern("STR")),
        name: StrId::intern("at"),
    });
    for n in 1..=args {
        let n = t.add(Term::Int(n));
        out = t.app(out, n);
    }
    out
}

#[test]
fn a_saturated_call_of_an_eta_primitive_global_folds() {
    let mut p = prog(vec![
        ("STR.at", |t| eta(t, "@array_get", "a", "b")),
        ("M.byte", |t| global_call(t, 2)),
    ]);
    fold_primitive_methods(&mut p);
    assert_eq!(rendered(&p, 1), "(App (App (Var @array_get) (Int 1)) (Int 2))");
}

/// Extra arguments apply to the primitive's result, as they did to the global's.
#[test]
fn an_over_applied_eta_primitive_global_folds() {
    let mut p = prog(vec![
        ("STR.at", |t| eta(t, "@array_get", "a", "b")),
        ("M.byte", |t| global_call(t, 3)),
    ]);
    fold_primitive_methods(&mut p);
    assert_eq!(rendered(&p, 1), "(App (App (App (Var @array_get) (Int 1)) (Int 2)) (Int 3))");
}

#[test]
fn an_under_applied_global_is_left_alone() {
    let mut p = prog(vec![
        ("STR.at", |t| eta(t, "@array_get", "a", "b")),
        ("M.partial", |t| global_call(t, 1)),
    ]);
    fold_primitive_methods(&mut p);
    assert!(rendered(&p, 1).contains("Var STR.at"));
}

/// `STR.at` applied to `args`, each argument a call `M.f n` that takes an
/// evaluation step.
fn global_call_of_calls(t: &mut Terms, args: i64) -> Aol<Term> {
    let mut out = t.add(Term::Var { module: Some(StrId::intern("STR")), name: StrId::intern("at") });
    for n in 1..=args {
        let f = t.add(Term::Var { module: Some(StrId::intern("M")), name: StrId::intern("f") });
        let n = t.add(Term::Int(n));
        let arg = t.app(f, n);
        out = t.app(out, arg);
    }
    out
}

/// A global that reorders its parameters folds with the operands swapped when
/// at most one argument takes an evaluation step.
#[test]
fn a_reordering_global_folds_over_atoms() {
    let mut p = prog(vec![
        ("STR.at", |t| eta(t, "@array_get", "b", "a")),
        ("M.byte", |t| global_call(t, 2)),
    ]);
    fold_primitive_methods(&mut p);
    assert_eq!(rendered(&p, 1), "(App (App (Var @array_get) (Int 2)) (Int 1))");
}

/// Swapping two calls would run them in the other order, so that use stays a call.
#[test]
fn a_reordering_global_over_two_calls_is_left_alone() {
    let mut p = prog(vec![
        ("STR.at", |t| eta(t, "@array_get", "b", "a")),
        ("M.byte", |t| global_call_of_calls(t, 2)),
    ]);
    fold_primitive_methods(&mut p);
    assert!(rendered(&p, 1).contains("Var STR.at"));
}

/// `\a b => not (@ilt a b)`, the shape `>=` specializes to, folds at any
/// argument: negation reorders nothing.
#[test]
fn a_negating_global_folds() {
    let mut p = prog(vec![
        ("STR.at", |t| {
            let inner = eta(t, "@ilt", "a", "b");
            let Term::Lam { body: lam_b, .. } = t.get(inner).clone() else { unreachable!() };
            let Term::Lam { body, .. } = t.get(lam_b).clone() else { unreachable!() };
            let not = t.var("not");
            let body = t.app(not, body);
            let inner = t.lam(StrId::intern("b"), body);
            t.lam(StrId::intern("a"), inner)
        }),
        ("M.byte", |t| global_call_of_calls(t, 2)),
    ]);
    fold_primitive_methods(&mut p);
    assert_eq!(
        rendered(&p, 1),
        "(App (Var not) (App (App (Var @ilt) (App (Var M.f) (Int 1))) (App (Var M.f) (Int 2))))"
    );
}

/// A saturated call of a GLOBAL that is an eta-expanded primitive folds too,
/// which is what a method lifted out of a known instance looks like.
#[test]
fn a_call_of_a_lifted_method_global_folds() {
    let mut p = prog(vec![
        ("M.add", |t| eta(t, "@iadd", "a", "b")),
        ("M.sum", |t| {
            let add = t.add(Term::Var { module: Some(StrId::intern("M")), name: StrId::intern("add") });
            let (one, two) = (t.add(Term::Int(1)), t.add(Term::Int(2)));
            let partial = t.app(add, one);
            t.app(partial, two)
        }),
    ]);
    fold_primitive_methods(&mut p);
    assert_eq!(rendered(&p, 1), "(App (App (Var @iadd) (Int 1)) (Int 2))");
}

/// The instance's method itself becomes the bare primitive, so a kernel handed
/// it as a value (`@tensor_dot e.0.add ...`) sees the builtin.
#[test]
fn an_eta_primitive_method_reduces_to_the_primitive() {
    let mut p = prog(vec![("CORE.impl_IAdd_for_int", eta_add)]);
    fold_primitive_methods(&mut p);
    assert_eq!(rendered(&p, 0), "(Struct IAdd [add] (Var @iadd))");
}

/// A method that is not an eta-expanded primitive stays a lambda.
#[test]
fn a_reordering_method_is_not_reduced() {
    let mut p = prog(vec![("CORE.impl_IAdd_for_int", flipped)]);
    fold_primitive_methods(&mut p);
    assert!(rendered(&p, 0).contains("Lam"));
}
