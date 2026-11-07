use super::*;

fn id(s: &str) -> StrId {
    StrId::intern(s)
}

/// `M.name`.
fn global(t: &mut Terms, name: &str) -> Aol<Term> {
    t.add(Term::Var { module: Some(id("M")), name: id(name) })
}

fn apps(t: &mut Terms, head: Aol<Term>, args: &[Aol<Term>]) -> Aol<Term> {
    args.iter().fold(head, |f, &x| t.app(f, x))
}

fn lams(t: &mut Terms, params: &[&str], body: Aol<Term>) -> Aol<Term> {
    params.iter().rev().fold(body, |b, p| t.lam(id(p), b))
}

/// `IEq.{ .eq = \a b => @ieq a b }`.
fn int_eq(t: &mut Terms) -> Aol<Term> {
    let (ieq, a, b) = (t.var("@ieq"), t.var("a"), t.var("b"));
    let call = apps(t, ieq, &[a, b]);
    let method = lams(t, &["a", "b"], call);
    t.add(Term::Struct { name: id("IEq"), base: None, fields: vec![(id("eq"), method)] })
}

/// `\d x => d.eq x x`.
fn same(t: &mut Terms) -> Aol<Term> {
    let d = t.var("d");
    let eq = t.field(d, "eq");
    let x = t.var("x");
    let call = apps(t, eq, &[x, x]);
    lams(t, &["d", "x"], call)
}

/// A program of `globals`; `ctx` names the `@use` ones.
fn prog(globals: Vec<(&str, fn(&mut Terms) -> Aol<Term>)>, ctx: &[&str]) -> CoreProgram {
    let mut p = CoreProgram::new(id("M"));
    for (name, build) in globals {
        let term = build(&mut p.terms);
        p.globals.push((id(&format!("M.{name}")), term));
    }
    p.ctx_globals = ctx.iter().map(|n| id(&format!("M.{n}"))).collect();
    p
}

fn body(p: &CoreProgram, key: &str) -> String {
    let (_, term) = p.globals.iter().find(|(k, _)| k.as_str() == key).expect("no such global");
    p.terms.render(*term)
}

#[test]
fn a_call_with_a_known_instance_calls_a_clone_with_direct_methods() {
    let mut p = prog(
        vec![
            ("inst", int_eq),
            ("same", same),
            ("main", |t| {
                let (f, k, one) = (global(t, "same"), global(t, "inst"), t.add(Term::Int(1)));
                apps(t, f, &[k, one])
            }),
        ],
        &["same"],
    );
    specialize_contexts(&mut p);
    assert_eq!(body(&p, "M.main"), "(App (Var M.same#s0) (Int 1))");
    assert_eq!(body(&p, "M.same#s0"), "(Lam x (App (App (Var M.inst#f_eq) (Var x)) (Var x)))");
    assert_eq!(body(&p, "M.inst#f_eq"), "(Lam a (Lam b (App (App (Var @ieq) (Var a)) (Var b))))");
    // The literal references the lifted method instead of holding a copy.
    assert_eq!(body(&p, "M.inst"), "(Struct IEq [eq] (Var M.inst#f_eq))");
}

#[test]
fn a_context_that_is_not_known_is_left_alone() {
    let mut p = prog(
        vec![
            ("same", same),
            ("main", |t| {
                let (f, k, one) = (global(t, "same"), t.var("k"), t.add(Term::Int(1)));
                let call = apps(t, f, &[k, one]);
                t.lam(id("k"), call)
            }),
        ],
        &["same"],
    );
    specialize_contexts(&mut p);
    assert_eq!(body(&p, "M.main"), "(Lam k (App (App (Var M.same) (Var k)) (Int 1)))");
    assert_eq!(p.globals.len(), 2);
}

/// The recursive call passes the context on, so the clone calls itself.
#[test]
fn a_recursive_function_specializes_to_a_self_calling_clone() {
    let mut p = prog(
        vec![
            ("inst", int_eq),
            ("go", |t| {
                let (go, d, x) = (global(t, "go"), t.var("d"), t.var("x"));
                let call = apps(t, go, &[d, x]);
                lams(t, &["d", "x"], call)
            }),
            ("main", |t| {
                let (f, k, one) = (global(t, "go"), global(t, "inst"), t.add(Term::Int(1)));
                apps(t, f, &[k, one])
            }),
        ],
        &["go"],
    );
    specialize_contexts(&mut p);
    assert_eq!(body(&p, "M.go#s0"), "(Lam x (App (Var M.go#s0) (Var x)))");
}

/// `impl_IEq_for_box impl_IEq_for_int` is itself known: its clone is a literal.
#[test]
fn an_instance_function_applied_to_a_known_instance_is_known() {
    let mut p = prog(
        vec![
            ("inst", int_eq),
            ("boxed", |t| {
                let inner = same(t);
                let lit = t.add(Term::Struct { name: id("IEq"), base: None, fields: vec![(id("eq"), inner)] });
                t.lam(id("d"), lit)
            }),
            ("same", same),
            ("main", |t| {
                let (f, b, k, one) = (global(t, "same"), global(t, "boxed"), global(t, "inst"), t.add(Term::Int(1)));
                let inst = t.app(b, k);
                apps(t, f, &[inst, one])
            }),
        ],
        &["boxed", "same"],
    );
    specialize_contexts(&mut p);
    assert_eq!(body(&p, "M.main"), "(App (Var M.same#s1) (Int 1))");
    assert_eq!(body(&p, "M.same#s1"), "(Lam x (App (App (Var M.boxed#s0#f_eq) (Var x)) (Var x)))");
}

/// `\ds x => ds.1.eq x x` over a bundle built at the call site.
#[test]
fn a_bundle_of_known_instances_projects_its_components() {
    let mut p = prog(
        vec![
            ("inst", int_eq),
            ("pick", |t| {
                let ds = t.var("ds");
                let second = t.field(ds, "1");
                let eq = t.field(second, "eq");
                let x = t.var("x");
                let call = apps(t, eq, &[x, x]);
                lams(t, &["ds", "x"], call)
            }),
            ("main", |t| {
                let (f, a, b, one) = (global(t, "pick"), global(t, "inst"), global(t, "inst"), t.add(Term::Int(1)));
                let bundle = t.add(Term::Tuple(vec![a, b]));
                apps(t, f, &[bundle, one])
            }),
        ],
        &["pick"],
    );
    specialize_contexts(&mut p);
    assert_eq!(body(&p, "M.main"), "(App (Var M.pick#s1) (Int 1))");
    assert_eq!(body(&p, "M.pick#s1"), "(Lam x (App (App (Var M.inst#f_eq) (Var x)) (Var x)))");
}

/// A body that is not a value would run once as a global instead of per call.
#[test]
fn a_clone_whose_body_is_not_a_value_is_not_made() {
    let mut p = prog(
        vec![
            ("inst", int_eq),
            ("eager", |t| {
                let (h, d) = (global(t, "h"), t.var("d"));
                let call = t.app(h, d);
                t.lam(id("d"), call)
            }),
            ("main", |t| {
                let (f, k) = (global(t, "eager"), global(t, "inst"));
                t.app(f, k)
            }),
        ],
        &["eager"],
    );
    specialize_contexts(&mut p);
    assert_eq!(body(&p, "M.main"), "(App (Var M.eager) (Var M.inst))");
}

/// A binder that reuses the context's name hides it from the substitution.
#[test]
fn a_shadowing_binder_stops_the_substitution() {
    let mut p = prog(
        vec![
            ("inst", int_eq),
            ("shadow", |t| {
                let d = t.var("d");
                let inner = t.lam(id("d"), d);
                lams(t, &["d", "x"], inner)
            }),
            ("main", |t| {
                let (f, k) = (global(t, "shadow"), global(t, "inst"));
                t.app(f, k)
            }),
        ],
        &["shadow"],
    );
    specialize_contexts(&mut p);
    assert_eq!(body(&p, "M.shadow#s0"), "(Lam x (Lam d (Var d)))");
}

/// Each level wraps the instance once more: the depth cap stops it.
#[test]
fn polymorphic_recursion_terminates() {
    let mut p = prog(
        vec![
            ("inst", int_eq),
            ("wrap", |t| {
                let inner = same(t);
                let lit = t.add(Term::Struct { name: id("IEq"), base: None, fields: vec![(id("eq"), inner)] });
                t.lam(id("d"), lit)
            }),
            ("grow", |t| {
                let (grow, wrap, d, x) = (global(t, "grow"), global(t, "wrap"), t.var("d"), t.var("x"));
                let bigger = t.app(wrap, d);
                let call = apps(t, grow, &[bigger, x]);
                lams(t, &["d", "x"], call)
            }),
            ("main", |t| {
                let (f, k, one) = (global(t, "grow"), global(t, "inst"), t.add(Term::Int(1)));
                apps(t, f, &[k, one])
            }),
        ],
        &["wrap", "grow"],
    );
    specialize_contexts(&mut p);
    assert!(p.globals.len() < 3 + 4 * MAX_DEPTH);
}
