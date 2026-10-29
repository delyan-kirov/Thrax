use crate::core::{Pat, CoreProgram, Term};
use super::desugar_program;
use crate::typing::collect_resolved;
use crate::{Ast, Checker, Types};

/// Parse, check and desugar one module that uses no CORE names.
fn lower(src: &str) -> CoreProgram {
    let (ast, program) = crate::parse_into(Ast::new(), src).expect("parse");
    let mut checker = Checker::new(&ast, std::rc::Rc::new(Types::new()));
    checker
        .check_program(&program)
        .unwrap_or_else(|e| panic!("{}", e.render(src, "test.thx")));
    let resolved = collect_resolved(std::slice::from_ref(&checker));
    desugar_program(&ast, &program, &resolved)
}

fn global<'p>(program: &'p CoreProgram, name: &str) -> &'p Term {
    &program
        .globals
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no global `{name}`"))
        .1
}

#[test]
fn if_becomes_one_case_on_the_condition() {
    let p = lower("$ @mod M\n$ f : @Bool -> @Int = \\b = if b => 1 else 2\n");
    let Term::Lam { param, body } = global(&p, "f") else {
        panic!("expected a lambda");
    };
    let Term::Case { scrut, arms, default } = body.as_ref() else {
        panic!("expected a case, got {body:?}");
    };
    assert!(matches!(scrut.as_ref(), Term::Var { name, .. } if name == param));
    assert_eq!(arms.len(), 1);
    assert!(matches!(arms[0].pat, Pat::Bool(true)));
    assert!(matches!(arms[0].body.as_ref(), Term::Int(1)));
    assert!(matches!(default.as_deref(), Some(Term::Int(2))));
}

#[test]
fn a_multi_parameter_lambda_curries() {
    let p = lower("$ @mod M\n$ k : @Int -> @Int -> @Int = \\a b = a\n");
    let Term::Lam { param: a, body } = global(&p, "k") else {
        panic!("expected a lambda");
    };
    let Term::Lam { param: b, body } = body.as_ref() else {
        panic!("expected a nested lambda");
    };
    assert_eq!((a.as_str(), b.as_str()), ("a", "b"));
    assert!(matches!(body.as_ref(), Term::Var { name, .. } if name == "a"));
}

#[test]
fn a_positional_struct_literal_is_labelled_in_declaration_order() {
    let p = lower("$ @mod M\n$ P : @struct = x: @Int, y: @Int\n$ p : P = P.{ 1, 2 }\n");
    let Term::Struct { name, base, fields } = global(&p, "p") else {
        panic!("expected a struct");
    };
    assert_eq!(name, "P");
    assert!(base.is_none());
    let labels: Vec<&str> = fields.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["x", "y"]);
}

#[test]
fn a_variable_let_is_always_recursive() {
    let p = lower("$ @mod M\n$ y : @Int = let a = 3 in a\n");
    let Term::Let { name, rec, val, body } = global(&p, "y") else {
        panic!("expected a let");
    };
    assert_eq!(name, "a");
    assert!(rec);
    assert!(matches!(val.as_ref(), Term::Int(3)));
    assert!(matches!(body.as_ref(), Term::Var { name, .. } if name == "a"));
}
