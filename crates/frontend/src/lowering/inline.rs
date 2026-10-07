//! Fold a saturated interface projection whose method is an eta-expanded
//! primitive. Every `CORE` arithmetic and comparison instance has that shape
//! (`$ impl_IAdd_for_int : IAdd @int = .{ .add = \a b = @iadd a b }`), so `x + y` can cost
//! one builtin call again instead of a global lookup, a field read, and two
//! closure applications. Runs over the MERGED program, where a global's key is
//! already `MOD.name`, so a qualified projection matches by name.
//!
//! The rewrite is an eta-reduction, not an inliner: the method's body must apply
//! the primitive to exactly its own parameters, in order, and the use must be
//! saturated. Nothing is duplicated and no evaluation order changes.

use std::collections::HashMap;
use std::sync::Arc;

use super::data::{Arm, Clause, Handler, Pat, Program, Term};

/// `(global name, field) -> (primitive, arity)` for every eta-expanded primitive
/// method of a global struct literal.
type Prims = HashMap<(String, String), (String, usize)>;

pub fn fold_primitive_methods(program: &mut Program) {
    let prims = collect(program);
    if prims.is_empty() {
        return;
    }
    for (_, term) in program.globals.iter_mut() {
        *term = rewrite(term, &prims);
    }
}

fn collect(program: &Program) -> Prims {
    let mut out = Prims::new();
    for (name, term) in &program.globals {
        let Term::Struct { base: None, fields, .. } = term else {
            continue;
        };
        for (field, body) in fields.iter() {
            if let Some(found) = eta_primitive(body) {
                out.insert((name.clone(), field.clone()), found);
            }
        }
    }
    out
}

/// The primitive and arity of `\p1 .. pn = @prim p1 .. pn`, else `None`. The
/// parameters must be distinct and appear exactly once each, in order.
fn eta_primitive(term: &Term) -> Option<(String, usize)> {
    let mut params: Vec<&str> = Vec::new();
    let mut cur = term;
    while let Term::Lam { param, body } = cur {
        params.push(param);
        cur = body;
    }
    if params.is_empty() {
        return None;
    }
    let mut args: Vec<&Term> = Vec::new();
    while let Term::App(f, x) = cur {
        args.push(x);
        cur = f;
    }
    args.reverse();
    let Term::Var { module: None, name, .. } = cur else {
        return None;
    };
    // Only an `@`-sigil intrinsic: those are runtime builtins resolved by bare
    // name everywhere, so moving the reference to another module is sound.
    if !name.starts_with('@') || args.len() != params.len() {
        return None;
    }
    for (arg, want) in args.iter().zip(&params) {
        match arg {
            Term::Var { module: None, name, .. } if name == want => {}
            _ => return None,
        }
    }
    Some((name.clone(), params.len()))
}

/// The primitive a saturated application of `term` to `argc` arguments folds to.
fn folded_head(term: &Term, argc: usize, prims: &Prims) -> Option<String> {
    let Term::Field(record, field) = term else {
        return None;
    };
    let Term::Var { module: Some(m), name, .. } = &**record else {
        return None;
    };
    let (prim, arity) = prims.get(&(format!("{m}.{name}"), field.clone()))?;
    (*arity == argc).then(|| prim.clone())
}

fn rewrite(term: &Term, prims: &Prims) -> Term {
    // Peel the application spine first: the fold needs the argument count.
    if matches!(term, Term::App(..)) {
        let mut args: Vec<&Term> = Vec::new();
        let mut head = term;
        while let Term::App(f, x) = head {
            args.push(x);
            head = f;
        }
        args.reverse();
        let folded = folded_head(head, args.len(), prims);
        let mut out = match &folded {
            Some(prim) => Term::var(prim.clone()),
            None => rewrite(head, prims),
        };
        for a in args {
            out = Term::app(out, rewrite(a, prims));
        }
        return out;
    }
    match term {
        Term::Int(_)
        | Term::Real(_)
        | Term::Str(_)
        | Term::Bool(_)
        | Term::Unit
        | Term::Var { .. }
        | Term::Extern { .. }
        | Term::Fault(_) => term.clone(),
        Term::App(..) => unreachable!("handled above"),
        Term::Lam { param, body } => Term::Lam {
            param: param.clone(),
            body: Arc::new(rewrite(body, prims)),
        },
        Term::Let { name, rec, val, body } => Term::Let {
            name: name.clone(),
            rec: *rec,
            val: Arc::new(rewrite(val, prims)),
            body: Arc::new(rewrite(body, prims)),
        },
        Term::Case { scrut, arms, default } => Term::Case {
            scrut: Arc::new(rewrite(scrut, prims)),
            arms: arms
                .iter()
                .map(|a| Arm {
                    pat: rewrite_pat(&a.pat, prims),
                    guard: a.guard.as_ref().map(|g| Arc::new(rewrite(g, prims))),
                    body: Arc::new(rewrite(&a.body, prims)),
                })
                .collect(),
            default: default.as_ref().map(|d| Arc::new(rewrite(d, prims))),
        },
        Term::Tuple(items) => Term::Tuple(items.iter().map(|t| rewrite(t, prims)).collect()),
        Term::Struct { name, base, fields } => Term::Struct {
            name: name.clone(),
            base: base.as_ref().map(|b| Arc::new(rewrite(b, prims))),
            fields: fields
                .iter()
                .map(|(f, t)| (f.clone(), rewrite(t, prims)))
                .collect(),
        },
        Term::Variant { ty, tag, fields } => Term::Variant {
            ty: ty.clone(),
            tag: tag.clone(),
            fields: fields.iter().map(|t| rewrite(t, prims)).collect(),
        },
        Term::Field(record, field) => {
            Term::Field(Arc::new(rewrite(record, prims)), field.clone())
        }
        Term::Handle { body, handler } => Term::Handle {
            body: Arc::new(rewrite(body, prims)),
            handler: Arc::new(Handler {
                continuation: handler.continuation.clone(),
                clauses: handler
                    .clauses
                    .iter()
                    .map(|c| Clause {
                        effect: c.effect.clone(),
                        op: c.op.clone(),
                        arg: c.arg.clone(),
                        body: rewrite(&c.body, prims),
                    })
                    .collect(),
                default: handler
                    .default
                    .as_ref()
                    .map(|(n, t)| (n.clone(), rewrite(t, prims))),
                oneshot: handler.oneshot,
            }),
        },
        Term::Defer { cleanup, body } => Term::Defer {
            cleanup: Arc::new(rewrite(cleanup, prims)),
            body: Arc::new(rewrite(body, prims)),
        },
    }
}

fn rewrite_pat(pat: &Pat, prims: &Prims) -> Pat {
    match pat {
        Pat::Wild
        | Pat::Var(_)
        | Pat::Int(_)
        | Pat::Real(_)
        | Pat::Str(_)
        | Pat::Bool(_) => pat.clone(),
        Pat::Tuple(ps) => Pat::Tuple(ps.iter().map(|p| rewrite_pat(p, prims)).collect()),
        Pat::Variant { tag, fields } => Pat::Variant {
            tag: tag.clone(),
            fields: fields.iter().map(|p| rewrite_pat(p, prims)).collect(),
        },
        Pat::Struct { fields, rest } => Pat::Struct {
            fields: fields
                .iter()
                .map(|(f, p)| (f.clone(), rewrite_pat(p, prims)))
                .collect(),
            rest: rest.clone(),
        },
        Pat::StrPrefix { prefix, rest } => Pat::StrPrefix {
            prefix: prefix.clone(),
            rest: Box::new(rewrite_pat(rest, prims)),
        },
        Pat::Range { lo, hi } => Pat::Range {
            lo: rewrite(lo, prims),
            hi: hi.as_ref().map(|h| rewrite(h, prims)),
        },
        Pat::HookEq { eq, value } => Pat::HookEq {
            eq: rewrite(eq, prims),
            value: Box::new(rewrite(value, prims)),
        },
        Pat::SeqView { view, elems, rest } => Pat::SeqView {
            view: rewrite(view, prims),
            elems: elems.iter().map(|p| rewrite_pat(p, prims)).collect(),
            rest: rest.as_ref().map(|r| Box::new(rewrite_pat(r, prims))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog(globals: Vec<(&str, Term)>) -> Program {
        Program {
            module: "M".into(),
            effects: Vec::new(),
            globals: globals
                .into_iter()
                .map(|(n, t)| (n.to_string(), t))
                .collect(),
            crepr_layouts: Vec::new(),
            ct_runs: Vec::new(),
            ct_build: None,
            ct_evals: Vec::new(),
            ct_types: Vec::new(),
        }
    }

    /// `\a b = @iadd a b`, the shape every CORE arithmetic instance field has.
    fn eta_add() -> Term {
        Term::Lam {
            param: "a".into(),
            body: Arc::new(Term::Lam {
                param: "b".into(),
                body: Arc::new(Term::app(
                    Term::app(Term::var("@iadd"), Term::var("a")),
                    Term::var("b"),
                )),
            }),
        }
    }

    fn instance(field: Term) -> Term {
        Term::Struct {
            name: "IAdd".into(),
            base: None,
            fields: Arc::from(vec![("add".to_string(), field)]),
        }
    }

    fn projection() -> Term {
        Term::Field(
            Arc::new(Term::Var {
                module: Some("CORE".into()),
                name: "impl_IAdd_for_int".into(),
                idx: 0,
            }),
            "add".into(),
        )
    }

    #[test]
    fn a_saturated_projection_of_an_eta_primitive_folds() {
        let use_site = Term::app(Term::app(projection(), Term::Int(1)), Term::Int(2));
        let mut p = prog(vec![
            ("CORE.impl_IAdd_for_int", instance(eta_add())),
            ("M.sum", use_site),
        ]);
        fold_primitive_methods(&mut p);
        let folded = format!("{:?}", p.globals[1].1);
        assert!(folded.contains("@iadd"), "{folded}");
        assert!(!folded.contains("Field"), "{folded}");
    }

    /// An UNDER-applied projection keeps the field read: there is nothing to
    /// eta-reduce, and the primitive is not a first-class value.
    #[test]
    fn an_under_applied_projection_is_left_alone() {
        let use_site = Term::app(projection(), Term::Int(1));
        let mut p = prog(vec![
            ("CORE.impl_IAdd_for_int", instance(eta_add())),
            ("M.add1", use_site),
        ]);
        fold_primitive_methods(&mut p);
        assert!(format!("{:?}", p.globals[1].1).contains("Field"));
    }

    /// A method that is not an eta-expanded primitive (it reorders its arguments)
    /// must not be folded: the rewrite is only sound when the body applies the
    /// primitive to its own parameters in order.
    #[test]
    fn a_method_that_reorders_its_arguments_is_left_alone() {
        let flipped = Term::Lam {
            param: "a".into(),
            body: Arc::new(Term::Lam {
                param: "b".into(),
                body: Arc::new(Term::app(
                    Term::app(Term::var("@iadd"), Term::var("b")),
                    Term::var("a"),
                )),
            }),
        };
        let use_site = Term::app(Term::app(projection(), Term::Int(1)), Term::Int(2));
        let mut p = prog(vec![
            ("CORE.impl_IAdd_for_int", instance(flipped)),
            ("M.sum", use_site),
        ]);
        fold_primitive_methods(&mut p);
        assert!(format!("{:?}", p.globals[1].1).contains("Field"));
    }
}
