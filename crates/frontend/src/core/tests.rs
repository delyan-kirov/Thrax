use std::sync::Arc;

use super::*;

fn lam(param: &str, body: Term) -> Term {
    Term::Lam { param: param.into(), body: Arc::new(body) }
}

fn let_(name: &str, rec: bool, val: Term, body: Term) -> Term {
    Term::Let { name: name.into(), rec, val: Arc::new(val), body: Arc::new(body) }
}

/// Each child's binders, as owned strings, in visiting order.
fn child_binders(t: &Term) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    t.for_each_child(&mut |_, bound| out.push(bound.iter().map(|b| b.to_string()).collect()));
    out
}

#[test]
fn atom_classification() {
    assert!(Term::Int(1).is_atom());
    assert!(Term::var("x").is_atom());
    assert!(lam("y", Term::var("y")).is_atom());
    assert!(!Term::app(Term::var("f"), Term::var("x")).is_atom());
    assert!(!Term::Fault("boom".into()).is_atom());
}

#[test]
fn a_lambda_binds_its_parameter_in_its_body() {
    assert_eq!(child_binders(&lam("x", Term::Unit)), vec![vec!["x".to_string()]]);
}

#[test]
fn only_a_recursive_let_binds_its_name_in_its_value() {
    let plain = let_("x", false, Term::Unit, Term::Unit);
    assert_eq!(child_binders(&plain), vec![vec![], vec!["x".to_string()]]);
    let rec = let_("f", true, Term::Unit, Term::Unit);
    assert_eq!(child_binders(&rec), vec![vec!["f".to_string()], vec!["f".to_string()]]);
}

#[test]
fn a_case_arm_binds_its_pattern_left_to_right_but_not_in_its_embedded_terms() {
    let pat = Pat::Tuple(vec![
        Pat::Var("a".into()),
        Pat::Range { lo: Term::Int(0), hi: None },
        Pat::Struct { fields: vec![("f".into(), Pat::Var("b".into()))], rest: Some("r".into()) },
    ]);
    assert_eq!(pat.binders(), vec!["a", "b", "r"]);
    let case = Term::Case {
        scrut: Arc::new(Term::Unit),
        arms: vec![CoreArm { pat, guard: Some(Arc::new(Term::Bool(true))), body: Arc::new(Term::Unit) }].into(),
        default: Some(Arc::new(Term::Unit)),
    };
    let ab = vec!["a".to_string(), "b".to_string(), "r".to_string()];
    assert_eq!(child_binders(&case), vec![vec![], vec![], ab.clone(), ab, vec![]]);
}

#[test]
fn a_handler_clause_binds_its_argument_then_the_continuation() {
    let t = Term::Handle {
        body: Arc::new(Term::Unit),
        handler: Arc::new(CoreHandler {
            continuation: "k".into(),
            clauses: vec![CoreClause { effect: None, op: "ask".into(), arg: "x".into(), body: Term::Unit }],
            default: Some(("v".into(), Term::Unit)),
        }),
    };
    let expect: Vec<Vec<String>> = vec![vec![], vec!["x".into(), "k".into()], vec!["v".into()]];
    assert_eq!(child_binders(&t), expect);
}

#[test]
fn map_children_rebuilds_with_the_same_binders() {
    let t = let_("f", true, lam("x", Term::var("f")), Term::var("f"));
    let mut seen = Vec::new();
    let out = t.map_children(&mut |c, bound| {
        seen.push(bound.len());
        c.clone()
    });
    assert_eq!(seen, vec![1, 1]);
    assert!(matches!(out, Term::Let { rec: true, .. }));
}

#[test]
fn mentions_finds_unqualified_variables_only() {
    let t = Term::app(Term::var("f"), Term::Var { module: Some("M".into()), name: "g".into(), idx: 0 });
    assert!(t.mentions("f"));
    assert!(!t.mentions("g"));
}

#[test]
fn fresh_names_count_from_their_first_number() {
    let mut names = FreshNames::new("%m", 0);
    assert_eq!(names.tagged("s"), "%ms0");
    assert_eq!(names.name(), "%m1");
    assert_eq!(FreshNames::new("%", 1).name(), "%1");
}
