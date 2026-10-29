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

use crate::core::{CoreProgram, Term};

/// `(global name, field) -> (primitive, arity)` for every eta-expanded primitive
/// method of a global struct literal.
type Prims = HashMap<(String, String), (String, usize)>;

pub fn fold_primitive_methods(program: &mut CoreProgram) {
    let prims = collect(program);
    if prims.is_empty() {
        return;
    }
    for (_, term) in program.globals.iter_mut() {
        *term = rewrite(term, &prims);
    }
}

fn collect(program: &CoreProgram) -> Prims {
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
    term.map_children(&mut |child, _| rewrite(child, prims))
}

#[cfg(test)]
mod tests;
