//! Fold a saturated interface projection whose method is an eta-expanded
//! primitive. Every `CORE` arithmetic and comparison instance has that shape
//! (`$ impl_IAdd_for_int : IAdd @Int = .{ .add = \a b = @iadd a b }`), so `x + y` can cost
//! one builtin call again instead of a global lookup, a field read, and two
//! closure applications. Runs over the MERGED program, where a global's key is
//! already `MOD.name`, so a qualified projection matches by name.
//!
//! A global that is itself an eta-expanded primitive folds the same way
//! (`$ at : @Str -> @Int -> @Int = \s i = @array_get s i`), so a byte loop over
//! `STR.at s i` reads the byte directly instead of building a closure per call.
//! That also covers the methods [`super::specialize`] lifts out of an instance
//! literal (`impl_IAdd_for_int#f_add`), which is what a projection from a known
//! instance becomes.
//!
//! The rewrite is an eta-reduction, not an inliner: the method's body must apply
//! the primitive to exactly its own parameters, in order, and the use must be
//! saturated. Nothing is duplicated and no evaluation order changes.
//!
//! Input: the merged Core program. Output: the same program, with each such
//! use calling the primitive directly, and each such method reduced to the
//! bare primitive.
//!
//! Entry point: [`fold_primitive_methods`].

use std::collections::HashMap;

use utilities::{Aol, StrId};

use crate::core::{CoreProgram, Term, Terms};

/// The eta-expanded primitives of a program's globals, as `(primitive, arity)`.
#[derive(Clone, Debug, Default)]
pub struct Prims {
    /// By `(global name, field)`: the methods of global struct literals.
    methods: HashMap<(StrId, StrId), (StrId, usize)>,
    /// By global name: globals that are themselves eta-expanded primitives.
    globals: HashMap<StrId, (StrId, usize)>,
}

impl Prims {
    fn is_empty(&self) -> bool {
        self.methods.is_empty() && self.globals.is_empty()
    }

    /// Add `other`'s entries, keeping this table's where both have one.
    pub fn inherit(&mut self, other: &Prims) {
        for (k, v) in &other.methods {
            self.methods.entry(*k).or_insert(*v);
        }
        for (k, v) in &other.globals {
            self.globals.entry(*k).or_insert(*v);
        }
    }
}

pub fn fold_primitive_methods(program: &mut CoreProgram) {
    let prims = primitive_methods(program);
    fold_primitive_methods_with(program, &prims);
}

/// [`fold_primitive_methods`] with the eta-expanded primitives `prims`, which
/// may include those of globals lowered separately.
pub fn fold_primitive_methods_with(program: &mut CoreProgram, prims: &Prims) {
    if prims.is_empty() {
        return;
    }
    program.rewrite_globals(|terms, term| rewrite(terms, term, prims));
    reduce_methods(program, prims);
}

/// Replace each eta-expanded primitive method by the primitive itself, so the
/// instance's field IS the builtin at runtime. A kernel handed the method as a
/// value (`@tensor_dot e.0.add ...`) can then recognize the arithmetic.
fn reduce_methods(program: &mut CoreProgram, prims: &Prims) {
    for i in 0..program.globals.len() {
        let (name, term) = program.globals[i];
        let Term::Struct { name: ty, base: None, fields } = program.terms.get(term) else {
            continue;
        };
        if !fields.iter().any(|(f, _)| prims.methods.contains_key(&(name, *f))) {
            continue;
        }
        let (ty, fields) = (*ty, fields.clone());
        let fields = fields
            .into_iter()
            .map(|(f, body)| match prims.methods.get(&(name, f)) {
                Some(&(prim, _)) => (f, program.terms.var(prim)),
                None => (f, body),
            })
            .collect();
        program.globals[i].1 = program.terms.add(Term::Struct { name: ty, base: None, fields });
    }
}

/// The eta-expanded primitives of `program`'s globals and of their struct
/// literals' methods. Read off the merged program, before any other pass.
pub fn primitive_methods(program: &CoreProgram) -> Prims {
    let terms = &program.terms;
    let mut out = Prims::default();
    for &(name, term) in &program.globals {
        if let Some(found) = eta_primitive(terms, term) {
            out.globals.entry(name).or_insert(found);
        }
    }
    for &(name, term) in &program.globals {
        let Term::Struct { base: None, fields, .. } = terms.get(term) else {
            continue;
        };
        for &(field, body) in fields {
            // A method `super::specialize` lifted out of the literal is a
            // reference to the global it became.
            let found = match terms.get(body) {
                Term::Var { module: Some(m), name } => {
                    out.globals.get(&StrId::intern(&format!("{m}.{name}"))).copied()
                }
                _ => eta_primitive(terms, body),
            };
            if let Some(found) = found {
                out.methods.insert((name, field), found);
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
/// A global folds under extra arguments too: they apply to the primitive's
/// result exactly as they applied to the global's.
fn folded_head(
    terms: &Terms,
    term: Aol<Term>,
    argc: usize,
    prims: &Prims,
) -> Option<StrId> {
    if let Term::Var { module: Some(m), name } = terms.get(term) {
        let global = StrId::intern(&format!("{m}.{name}"));
        let (prim, arity) = prims.globals.get(&global)?;
        return (argc >= *arity).then_some(*prim);
    }
    let Term::Field(record, field) = terms.get(term) else {
        return None;
    };
    let Term::Var { module: Some(m), name } = terms.get(*record) else {
        return None;
    };
    let global = StrId::intern(&format!("{m}.{name}"));
    let (prim, arity) = prims.methods.get(&(global, *field))?;
    (*arity == argc).then_some(*prim)
}

fn rewrite(
    terms: &mut Terms,
    term: Aol<Term>,
    prims: &Prims,
) -> Aol<Term> {
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
