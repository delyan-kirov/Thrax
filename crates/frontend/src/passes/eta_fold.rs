//! Fold a saturated interface projection whose method is an eta-expanded
//! primitive. Every `CORE` arithmetic and comparison instance has that shape
//! (`$ impl_IAdd_for_int : IAdd @Int = .{ .add = \a b = @iadd a b }`), so `x + y` can cost
//! one builtin call again instead of a global lookup, a field read, and two
//! closure applications. Runs over the MERGED program, where a global's key is
//! already `MOD.name`, so a qualified projection matches by name.
//!
//! The rewrite is an eta-reduction, not an inliner: the method's body must apply
//! the primitive to exactly its own parameters, in order, and the use must be
//! saturated. Nothing is duplicated and no evaluation order changes.
//!
//! Input: the merged Core program. Output: the same program, with each such
//! use calling the primitive directly.
//!
//! Entry point: [`fold_primitive_methods`].

use std::collections::HashMap;

use utilities::{Aol, StrId};

use crate::core::{CoreProgram, Term, Terms};

/// `(global name, field) -> (primitive, arity)` for every eta-expanded primitive
/// method of a global struct literal.
type Prims = HashMap<(StrId, StrId), (StrId, usize)>;

pub fn fold_primitive_methods(program: &mut CoreProgram) {
    let prims = collect(program);
    if prims.is_empty() {
        return;
    }
    program.rewrite_globals(|terms, term| rewrite(terms, term, &prims));
}

fn collect(program: &CoreProgram) -> Prims {
    let terms = &program.terms;
    let mut out = Prims::new();
    for &(name, term) in &program.globals {
        let Term::Struct { base: None, fields, .. } = terms.get(term) else {
            continue;
        };
        for &(field, body) in fields {
            if let Some(found) = eta_primitive(terms, body) {
                out.insert((name, field), found);
            }
        }
    }
    out
}

/// The primitive and arity of `\p1 .. pn = @prim p1 .. pn`, else `None`. The
/// parameters must be distinct and appear exactly once each, in order.
fn eta_primitive(terms: &Terms, term: Aol<Term>) -> Option<(StrId, usize)> {
    let mut params: Vec<StrId> = Vec::new();
    let mut cur = term;
    while let Term::Lam { param, body } = terms.get(cur) {
        params.push(*param);
        cur = *body;
    }
    if params.is_empty() {
        return None;
    }
    let mut args: Vec<Aol<Term>> = Vec::new();
    while let Term::App(f, x) = terms.get(cur) {
        args.push(*x);
        cur = *f;
    }
    args.reverse();
    let Term::Var { module: None, name } = terms.get(cur) else {
        return None;
    };
    // Only an `@`-sigil intrinsic: those are runtime builtins resolved by bare
    // name everywhere, so moving the reference to another module is sound.
    if !name.as_str().starts_with('@') || args.len() != params.len() {
        return None;
    }
    for (arg, want) in args.iter().zip(&params) {
        match terms.get(*arg) {
            Term::Var { module: None, name } if name == want => {}
            _ => return None,
        }
    }
    Some((*name, params.len()))
}

/// The primitive a saturated application of `term` to `argc` arguments folds to.
fn folded_head(terms: &Terms, term: Aol<Term>, argc: usize, prims: &Prims) -> Option<StrId> {
    let Term::Field(record, field) = terms.get(term) else {
        return None;
    };
    let Term::Var { module: Some(m), name } = terms.get(*record) else {
        return None;
    };
    let global = StrId::intern(&format!("{m}.{name}"));
    let (prim, arity) = prims.get(&(global, *field))?;
    (*arity == argc).then_some(*prim)
}

fn rewrite(terms: &mut Terms, term: Aol<Term>, prims: &Prims) -> Aol<Term> {
    // Peel the application spine first: the fold needs the argument count.
    if matches!(terms.get(term), Term::App(..)) {
        let mut args: Vec<Aol<Term>> = Vec::new();
        let mut head = term;
        while let Term::App(f, x) = terms.get(head) {
            args.push(*x);
            head = *f;
        }
        args.reverse();
        let mut out = match folded_head(terms, head, args.len(), prims) {
            Some(prim) => terms.var(prim),
            None => rewrite(terms, head, prims),
        };
        for a in args {
            let a = rewrite(terms, a, prims);
            out = terms.app(out, a);
        }
        return out;
    }
    terms.map_children(term, &mut |terms, child, _| rewrite(terms, child, prims))
}

#[cfg(test)]
mod tests;
