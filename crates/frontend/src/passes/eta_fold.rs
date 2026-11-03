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
//! A global that applies a primitive to its parameters REORDERED, or negates
//! the result with `not`, folds too: that is the shape of the derived
//! comparisons (`$ (>=) ... = \d a b = not (d.lt a b)`, once specialized to a
//! known instance). A use becomes `not (@ilt x y)` in place of a call. Reordered
//! arguments fold only when at most one of them takes an evaluation step, so
//! the order of effects is still the source order.
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
    /// By global name: globals that apply a primitive to their parameters
    /// reordered, or negate it.
    wrappers: HashMap<StrId, Wrapper>,
}

/// `\p0 .. pn = [not] (@prim p_order[0] .. p_order[n])`.
#[derive(Clone, Debug, PartialEq)]
struct Wrapper {
    prim: StrId,
    /// For each operand of the primitive, the parameter it is.
    order: Vec<usize>,
    negate: bool,
}

impl Wrapper {
    fn reorders(&self) -> bool {
        self.order.iter().enumerate().any(|(k, p)| k != *p)
    }
}

impl Prims {
    fn is_empty(&self) -> bool {
        self.methods.is_empty() && self.globals.is_empty() && self.wrappers.is_empty()
    }

    /// Add `other`'s entries, keeping this table's where both have one.
    pub fn inherit(&mut self, other: &Prims) {
        for (k, v) in &other.methods {
            self.methods.entry(*k).or_insert(*v);
        }
        for (k, v) in &other.globals {
            self.globals.entry(*k).or_insert(*v);
        }
        for (k, v) in &other.wrappers {
            self.wrappers.entry(*k).or_insert_with(|| v.clone());
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
    // Second, the wrappers: their head may be a projection the first table folds.
    for &(name, term) in &program.globals {
        if out.globals.contains_key(&name) {
            continue;
        }
        if let Some(w) = wrapped_primitive(terms, term, &out) {
            out.wrappers.insert(name, w);
        }
    }
    out
}

/// The [`Wrapper`] `term` is, if it is one: a lambda over distinct parameters
/// whose body is, under an optional `not`, a primitive (or a head `prims` folds
/// to one) applied to each parameter exactly once.
fn wrapped_primitive(terms: &Terms, term: Aol<Term>, prims: &Prims) -> Option<Wrapper> {
    let not = StrId::intern("not");
    let mut params: Vec<StrId> = Vec::new();
    let mut cur = term;
    while let Term::Lam { param, body } = terms.get(cur) {
        if params.contains(param) || *param == not {
            return None;
        }
        params.push(*param);
        cur = *body;
    }
    if params.is_empty() {
        return None;
    }
    let mut negate = false;
    if let Term::App(f, x) = terms.get(cur) {
        if matches!(terms.get(*f), Term::Var { module: None, name } if *name == not) {
            negate = true;
            cur = *x;
        }
    }
    let mut args: Vec<Aol<Term>> = Vec::new();
    while let Term::App(f, x) = terms.get(cur) {
        args.push(*x);
        cur = *f;
    }
    args.reverse();
    if args.len() != params.len() {
        return None;
    }
    let prim = match terms.get(cur) {
        Term::Var { module: None, name } if name.as_str().starts_with('@') => *name,
        _ => folded_head(terms, cur, args.len(), prims)?,
    };
    if crate::ir::data::builtin_arity(prim.as_str()) != Some(args.len()) {
        return None;
    }
    let mut order = Vec::with_capacity(args.len());
    for arg in &args {
        let Term::Var { module: None, name } = terms.get(*arg) else {
            return None;
        };
        let p = params.iter().position(|q| q == name)?;
        if order.contains(&p) {
            return None;
        }
        order.push(p);
    }
    let w = Wrapper { prim, order, negate };
    (w.negate || w.reorders()).then_some(w)
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
        if let Some(out) = unwrap(terms, head, &args, prims) {
            return out;
        }
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

/// A saturated use of a [`Wrapper`] global, as the primitive applied to the
/// (rewritten) arguments in the wrapper's order, under `not` if it negates.
/// `None` when `head` is no wrapper, the use is under-applied, or reordering
/// would move one argument's evaluation step past another's.
fn unwrap(terms: &mut Terms, head: Aol<Term>, args: &[Aol<Term>], prims: &Prims) -> Option<Aol<Term>> {
    let Term::Var { module: Some(m), name } = terms.get(head) else {
        return None;
    };
    let w = prims.wrappers.get(&StrId::intern(&format!("{m}.{name}")))?;
    let n = w.order.len();
    if args.len() < n {
        return None;
    }
    let args: Vec<Aol<Term>> = args.iter().map(|a| rewrite(terms, *a, prims)).collect();
    if w.reorders() && args[..n].iter().filter(|a| !terms.is_atom(**a)).count() > 1 {
        return None;
    }
    let mut out = terms.var(w.prim);
    for &p in &w.order {
        out = terms.app(out, args[p]);
    }
    if w.negate {
        let not = terms.var("not");
        out = terms.app(not, out);
    }
    for a in &args[n..] {
        out = terms.app(out, *a);
    }
    Some(out)
}

#[cfg(test)]
mod tests;
