use super::*;

fn id(s: &str) -> StrId {
    StrId::intern(s)
}

fn lam(t: &mut Terms, param: &str, body: Aol<Term>) -> Aol<Term> {
    t.lam(id(param), body)
}

fn let_(t: &mut Terms, name: &str, rec: bool, val: Aol<Term>, body: Aol<Term>) -> Aol<Term> {
    t.add(Term::Let { name: id(name), rec, val, body })
}

/// Each child's binders, as owned strings, in visiting order.
fn child_binders(terms: &Terms, t: Aol<Term>) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    terms.for_each_child(t, &mut |_, bound| out.push(bound.iter().map(|b| b.to_string()).collect()));
    out
}

#[test]
fn atom_classification() {
    let mut t = Terms::new();
    let one = t.add(Term::Int(1));
    let x = t.var("x");
    let y = t.var("y");
    let lam_y = lam(&mut t, "y", y);
    let f = t.var("f");
    let app = t.app(f, x);
    let fault = t.add(Term::Fault("boom".into()));
    assert!(t.is_atom(one));
    assert!(t.is_atom(x));
    assert!(t.is_atom(lam_y));
    assert!(!t.is_atom(app));
    assert!(!t.is_atom(fault));
}

#[test]
fn a_lambda_binds_its_parameter_in_its_body() {
    let mut t = Terms::new();
    let unit = t.add(Term::Unit);
    let l = lam(&mut t, "x", unit);
    assert_eq!(child_binders(&t, l), vec![vec!["x".to_string()]]);
}

#[test]
fn only_a_recursive_let_binds_its_name_in_its_value() {
    let mut t = Terms::new();
    let unit = t.add(Term::Unit);
    let plain = let_(&mut t, "x", false, unit, unit);
    assert_eq!(child_binders(&t, plain), vec![vec![], vec!["x".to_string()]]);
    let rec = let_(&mut t, "f", true, unit, unit);
    assert_eq!(child_binders(&t, rec), vec![vec!["f".to_string()], vec!["f".to_string()]]);
}

#[test]
fn a_case_arm_binds_its_pattern_left_to_right_but_not_in_its_embedded_terms() {
    let mut t = Terms::new();
    let zero = t.add(Term::Int(0));
    let pat = Pat::Tuple(vec![
        Pat::Var(id("a")),
        Pat::Range { lo: zero, hi: None },
        Pat::Struct { fields: vec![(id("f"), Pat::Var(id("b")))], rest: Some(id("r")) },
    ]);
    assert_eq!(pat.binders(), vec![id("a"), id("b"), id("r")]);
    let unit = t.add(Term::Unit);
    let yes = t.add(Term::Bool(true));
    let case = t.add(Term::Case {
        scrut: unit,
        arms: vec![CoreArm { pat, guard: Some(yes), body: unit }],
        default: Some(unit),
    });
    let ab = vec!["a".to_string(), "b".to_string(), "r".to_string()];
    assert_eq!(child_binders(&t, case), vec![vec![], vec![], ab.clone(), ab, vec![]]);
}

#[test]
fn a_handler_clause_binds_its_argument_then_the_continuation() {
    let mut t = Terms::new();
    let unit = t.add(Term::Unit);
    let h = t.add(Term::Handle {
        body: unit,
        handler: Box::new(CoreHandler {
            continuation: id("k"),
            clauses: vec![CoreClause { effect: None, op: id("ask"), arg: id("x"), body: unit }],
            default: Some((id("v"), unit)),
        }),
    });
    let expect: Vec<Vec<String>> = vec![vec![], vec!["x".into(), "k".into()], vec!["v".into()]];
    assert_eq!(child_binders(&t, h), expect);
}

#[test]
fn map_children_rebuilds_with_the_same_binders() {
    let mut t = Terms::new();
    let f = t.var("f");
    let inner = lam(&mut t, "x", f);
    let l = let_(&mut t, "f", true, inner, f);
    let mut seen = Vec::new();
    let out = t.map_children(l, &mut |_, c, bound| {
        seen.push(bound.len());
        c
    });
    assert_eq!(seen, vec![1, 1]);
    assert_eq!(out, l, "unchanged children leave the node as it was");
    let one = t.add(Term::Int(1));
    let out = t.map_children(l, &mut |_, _, _| one);
    assert_ne!(out, l);
    assert!(matches!(t.get(out), Term::Let { rec: true, val, body, .. } if *val == one && *body == one));
}

#[test]
fn mentions_finds_unqualified_variables_only() {
    let mut t = Terms::new();
    let f = t.var("f");
    let g = t.add(Term::Var { module: Some(id("M")), name: id("g") });
    let app = t.app(f, g);
    assert!(t.mentions(app, id("f")));
    assert!(!t.mentions(app, id("g")));
}

#[test]
fn import_copies_a_tree_between_stores() {
    let mut src = Terms::new();
    let x = src.var("x");
    let body = src.var("x");
    let l = lam(&mut src, "x", body);
    let app = src.app(l, x);
    let mut dst = Terms::new();
    let copied = dst.import(&src, app);
    assert_eq!(dst.render(copied), src.render(app));
    assert_eq!(dst.render(copied), "(App (Lam x (Var x)) (Var x))");
}

#[test]
fn fresh_names_count_from_their_first_number() {
    let mut names = FreshNames::new("%m", 0);
    assert_eq!(names.tagged("s"), "%ms0");
    assert_eq!(names.name(), "%m1");
    assert_eq!(FreshNames::new("%", 1).name(), "%1");
}
