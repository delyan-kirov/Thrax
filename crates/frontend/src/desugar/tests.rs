use crate::core::{Pat, CoreProgram, Term};
use super::desugar_program;
use crate::typing::collect_resolved;
use crate::{Ast, Checker, Types};
use utilities::Aol;

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

fn global(program: &CoreProgram, name: &str) -> Aol<Term> {
    program
        .globals
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no global `{name}`"))
        .1
}

#[test]
fn if_becomes_one_case_on_the_condition() {
    let p = lower("$ @mod M\n$ f : @Bool -> @Int = \\b = if b => 1 else 2\n");
    let t = &p.terms;
    let Term::Lam { param, body } = t.get(global(&p, "f")) else {
        panic!("expected a lambda");
    };
    let Term::Case { scrut, arms, default } = t.get(*body) else {
        panic!("expected a case, got {}", t.render(*body));
    };
    assert!(matches!(t.get(*scrut), Term::Var { name, .. } if name == param));
    assert_eq!(arms.len(), 1);
    assert!(matches!(arms[0].pat, Pat::Bool(true)));
    assert!(matches!(t.get(arms[0].body), Term::Int(1)));
    assert!(matches!(default.map(|d| t.get(d)), Some(Term::Int(2))));
}

#[test]
fn a_multi_parameter_lambda_curries() {
    let p = lower("$ @mod M\n$ k : @Int -> @Int -> @Int = \\a b = a\n");
    assert_eq!(p.terms.render(global(&p, "k")), "(Lam a (Lam b (Var a)))");
}

#[test]
fn a_positional_struct_literal_is_labelled_in_declaration_order() {
    let p = lower("$ @mod M\n$ P : @struct = x: @Int, y: @Int\n$ p : P = P.{ 1, 2 }\n");
    let Term::Struct { name, base, fields } = p.terms.get(global(&p, "p")) else {
        panic!("expected a struct");
    };
    assert_eq!(*name, "P");
    assert!(base.is_none());
    let labels: Vec<&str> = fields.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["x", "y"]);
}

#[test]
fn a_variable_let_is_always_recursive() {
    let p = lower("$ @mod M\n$ y : @Int = let a = 3 in a\n");
    assert_eq!(p.terms.render(global(&p, "y")), "(LetRec a (Int 3) (Var a))");
}
