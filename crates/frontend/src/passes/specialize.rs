//! Specialize `@ctx` functions on the instances their call sites pass them.
//!
//! An interface method call costs a global lookup, a field read and a closure
//! application, and a generic function pays it on every method call because its
//! instance arrives as a runtime parameter. When the instance is known
//! statically, none of that is necessary:
//!
//! 1. A call `f K` of a `@ctx` global `f = \d = V` whose argument `K` is a KNOWN
//!    instance becomes a reference to the clone `f#sN = V[d := K]`, memoized per
//!    `(f, K)`. A known instance is a global whose body is a struct literal (an
//!    instance), a tuple of known instances (a context bundle, which is lifted to
//!    a global of its own), or another clone of that shape (an instance function
//!    applied to a known instance, such as `impl_IEq_for_vec impl_IEq_for_int`).
//! 2. A projection `K.method` from a known instance becomes the method itself: a
//!    global or literal field is copied, and a lambda field is lifted to a global
//!    `K#f_method`, which the instance's literal then references instead of
//!    holding a second copy. A projection out of a bundle is the component.
//!
//! The clone body is rewritten the same way, so the generic code inside calls
//! the methods directly, `eta_fold` then reduces the primitive ones, and a call
//! the clone makes to another `@ctx` function with the same instance chains into
//! that function's clone. A recursive call hits the memo and calls the clone
//! itself.
//!
//! Only a clone whose body is a value (a lambda, a struct or a tuple) is made: a
//! clone is a global, which the runtime evaluates once, where the original call
//! evaluated `V` on every call. The instance nesting depth and the total number
//! of clones are capped, so a polymorphic recursion that builds ever larger
//! instances terminates.
//!
//! Input: the merged Core program, with `ctx_globals` as `MOD.name` keys. Output:
//! the same program, with the clones and lifted methods appended as globals.
//!
//! Entry point: [`specialize_contexts`].

use std::collections::{HashMap, HashSet};

use utilities::{Aol, StrId};

use crate::core::{CoreProgram, Term, Terms};

/// How deeply instances may nest (`impl_IEq_for_vec (impl_IEq_for_vec ...)`).
const MAX_DEPTH: usize = 4;

/// The most globals the pass adds to one program.
const MAX_ADDED: usize = 4096;

/// A global, as the `(module, name)` of a qualified reference to it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Global {
    module: StrId,
    name: StrId,
}

/// A name the substitution replaces, and the known instance it becomes.
type Subst = Option<(StrId, Global)>;

pub fn specialize_contexts(program: &mut CoreProgram) {
    let mut terms = std::mem::take(&mut program.terms);
    let mut first: HashMap<StrId, usize> = HashMap::new();
    for (i, &(key, _)) in program.globals.iter().enumerate() {
        first.entry(key).or_insert(i);
    }
    let mut sp = Specializer {
        bodies: program
            .globals
            .iter()
            .enumerate()
            .filter(|(i, (key, _))| first[key] == *i)
            .map(|(_, &(key, body))| (key, body))
            .collect(),
        keys: HashMap::new(),
        crepr: program.crepr_layouts.iter().map(|(name, _)| *name).collect(),
        ctx: program.ctx_globals.iter().copied().collect(),
        clones: HashMap::new(),
        lifted: HashMap::new(),
        bundles: HashMap::new(),
        depth: HashMap::new(),
        pending: HashSet::new(),
        added: Vec::new(),
        next: 0,
    };
    let mut rewritten = Vec::with_capacity(program.globals.len());
    for (i, &(key, body)) in program.globals.iter().enumerate() {
        let body = sp.rw(&mut terms, body, None);
        if first[&key] == i {
            sp.bodies.insert(key, body);
        }
        rewritten.push(body);
    }
    sp.patch_lifted(&mut terms);
    for (i, global) in program.globals.iter_mut().enumerate() {
        global.1 = if first[&global.0] == i { sp.bodies[&global.0] } else { rewritten[i] };
    }
    for &key in &sp.added {
        program.globals.push((key, sp.bodies[&key]));
    }
    program.terms = terms;
}

struct Specializer {
    /// Every global's current body, by `MOD.name` key (the first definition of a
    /// key, which is the one a reference reaches).
    bodies: HashMap<StrId, Aol<Term>>,
    keys: HashMap<(StrId, StrId), StrId>,
    /// C-repr struct names. A field of one is marshalled through its C layout
    /// when the literal is built, so its literal value is not what a read sees.
    crepr: HashSet<StrId>,
    ctx: HashSet<StrId>,
    /// `(function key, instance key) -> clone`, `None` when it cannot be made.
    clones: HashMap<(StrId, StrId), Option<Global>>,
    /// `(instance key, field) -> lifted method`.
    lifted: HashMap<(StrId, StrId), Global>,
    /// The components' keys of a bundle -> its global.
    bundles: HashMap<Vec<StrId>, Global>,
    /// The nesting depth of each instance clone; an original global has none.
    depth: HashMap<StrId, usize>,
    /// Globals whose body is still being built, so not yet readable.
    pending: HashSet<StrId>,
    /// The keys of the added globals, in creation order.
    added: Vec<StrId>,
    next: usize,
}

impl Specializer {
    fn key(&mut self, g: Global) -> StrId {
        *self
            .keys
            .entry((g.module, g.name))
            .or_insert_with(|| StrId::intern(&format!("{}.{}", g.module, g.name)))
    }

    fn reference(&self, terms: &mut Terms, g: Global) -> Aol<Term> {
        terms.add(Term::Var { module: Some(g.module), name: g.name })
    }

    /// The global `t` references, with its key, when it has a readable body.
    fn global_of(&mut self, terms: &Terms, t: Aol<Term>) -> Option<(Global, StrId)> {
        let Term::Var { module: Some(module), name } = terms.get(t) else {
            return None;
        };
        let g = Global { module: *module, name: *name };
        let key = self.key(g);
        (self.bodies.contains_key(&key) && !self.pending.contains(&key)).then_some((g, key))
    }

    fn add_global(&mut self, key: StrId, body: Aol<Term>) {
        self.bodies.insert(key, body);
        self.added.push(key);
    }

    /// The `@ctx` global `t` references.
    fn ctx_head(&mut self, terms: &Terms, t: Aol<Term>) -> Option<Global> {
        let (g, key) = self.global_of(terms, t)?;
        self.ctx.contains(&key).then_some(g)
    }

    /// The known instance `t` is, lifting a bundle of known instances to a global.
    fn known(&mut self, terms: &mut Terms, t: Aol<Term>) -> Option<Global> {
        if let Term::Tuple(items) = terms.get(t) {
            let items = items.clone();
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                parts.push(self.known(terms, item)?);
            }
            return self.bundle(terms, parts);
        }
        let (g, key) = self.global_of(terms, t)?;
        match terms.get(self.bodies[&key]) {
            Term::Struct { base: None, .. } | Term::Tuple(_) => Some(g),
            _ => None,
        }
    }

    fn bundle(&mut self, terms: &mut Terms, parts: Vec<Global>) -> Option<Global> {
        let keys: Vec<StrId> = parts.iter().map(|&p| self.key(p)).collect();
        if let Some(&g) = self.bundles.get(&keys) {
            return Some(g);
        }
        if self.added.len() >= MAX_ADDED {
            return None;
        }
        let module = parts.first()?.module;
        let g = Global { module, name: StrId::intern(&format!("ctx#b{}", self.next)) };
        self.next += 1;
        let items = parts.iter().map(|&p| self.reference(terms, p)).collect();
        let body = terms.add(Term::Tuple(items));
        let key = self.key(g);
        let depth = keys.iter().filter_map(|k| self.depth.get(k)).copied().max().unwrap_or(0);
        self.depth.insert(key, depth);
        self.add_global(key, body);
        self.bundles.insert(keys, g);
        Some(g)
    }

    /// The clone of `f` with its context parameter bound to `k`.
    fn clone_for(&mut self, terms: &mut Terms, f: Global, k: Global) -> Option<Global> {
        let (fkey, kkey) = (self.key(f), self.key(k));
        if let Some(&c) = self.clones.get(&(fkey, kkey)) {
            return c;
        }
        let depth = self.depth.get(&kkey).copied().unwrap_or(0);
        if depth >= MAX_DEPTH || self.added.len() >= MAX_ADDED {
            return None;
        }
        let made = match terms.get(self.bodies[&fkey]) {
            Term::Lam { param, body } => match terms.get(*body) {
                Term::Lam { .. } | Term::Struct { base: None, .. } | Term::Tuple(_) => Some((*param, *body)),
                _ => None,
            },
            _ => None,
        };
        let Some((param, body)) = made else {
            self.clones.insert((fkey, kkey), None);
            return None;
        };
        let c = Global { module: f.module, name: StrId::intern(&format!("{}#s{}", f.name, self.next)) };
        self.next += 1;
        let ckey = self.key(c);
        self.clones.insert((fkey, kkey), Some(c));
        self.depth.insert(ckey, depth + 1);
        self.pending.insert(ckey);
        let body = self.rw(terms, body, Some((param, k)));
        self.pending.remove(&ckey);
        self.add_global(ckey, body);
        Some(c)
    }

    /// The method `k.field`, as a term that needs no projection.
    fn method(&mut self, terms: &mut Terms, k: Global, field: StrId) -> Option<Aol<Term>> {
        let kkey = self.key(k);
        match terms.get(self.bodies[&kkey]) {
            Term::Struct { name, .. } if self.crepr.contains(name) => None,
            Term::Struct { base: None, fields, .. } => {
                let &(_, value) = fields.iter().find(|(f, _)| *f == field)?;
                match terms.get(value) {
                    Term::Lam { .. } => {
                        let lifted = self.lift(terms, k, kkey, field, value)?;
                        Some(self.reference(terms, lifted))
                    }
                    node => copy_leaf(terms, node.clone()),
                }
            }
            Term::Tuple(items) => {
                let item = *items.get(field.as_str().parse::<usize>().ok()?)?;
                copy_leaf(terms, terms.get(item).clone())
            }
            _ => None,
        }
    }

    fn lift(&mut self, terms: &mut Terms, k: Global, kkey: StrId, field: StrId, value: Aol<Term>) -> Option<Global> {
        if let Some(&g) = self.lifted.get(&(kkey, field)) {
            return Some(g);
        }
        if self.added.len() >= MAX_ADDED {
            return None;
        }
        let g = Global { module: k.module, name: StrId::intern(&format!("{}#f_{field}", k.name)) };
        let key = self.key(g);
        self.lifted.insert((kkey, field), g);
        self.pending.insert(key);
        let body = self.rw(terms, value, None);
        self.pending.remove(&key);
        self.add_global(key, body);
        Some(g)
    }

    /// Point every lifted field of an instance literal at its lifted global, so
    /// the method's code exists once.
    fn patch_lifted(&mut self, terms: &mut Terms) {
        let mut by_instance: HashMap<StrId, Vec<(StrId, Global)>> = HashMap::new();
        for (&(kkey, field), &g) in &self.lifted {
            by_instance.entry(kkey).or_default().push((field, g));
        }
        for (kkey, lifted) in by_instance {
            let Term::Struct { name, base: None, fields } = terms.get(self.bodies[&kkey]).clone() else {
                continue;
            };
            let fields = fields
                .into_iter()
                .map(|(f, value)| match lifted.iter().find(|(l, _)| *l == f) {
                    Some(&(_, g)) if matches!(terms.get(value), Term::Lam { .. }) => (f, self.reference(terms, g)),
                    _ => (f, value),
                })
                .collect();
            let body = terms.add(Term::Struct { name, base: None, fields });
            self.bodies.insert(kkey, body);
        }
    }

    fn rw(&mut self, terms: &mut Terms, t: Aol<Term>, sub: Subst) -> Aol<Term> {
        match *terms.get(t) {
            Term::Var { module: None, name } => {
                return match sub {
                    Some((d, k)) if d == name => self.reference(terms, k),
                    _ => t,
                };
            }
            Term::App(f, x) => {
                let f2 = self.rw(terms, f, sub);
                let x2 = self.rw(terms, x, sub);
                if let Some(g) = self.ctx_head(terms, f2) {
                    if let Some(k) = self.known(terms, x2) {
                        if let Some(c) = self.clone_for(terms, g, k) {
                            return self.reference(terms, c);
                        }
                    }
                }
                return if f2 == f && x2 == x { t } else { terms.app(f2, x2) };
            }
            Term::Field(record, field) => {
                let r2 = self.rw(terms, record, sub);
                if let Some(m) = self.project(terms, r2, field) {
                    return m;
                }
                return if r2 == record { t } else { terms.field(r2, field) };
            }
            _ => {}
        }
        terms.map_children(t, &mut |terms, child, bound| {
            let sub = match sub {
                Some((d, _)) if bound.contains(&d) => None,
                s => s,
            };
            self.rw(terms, child, sub)
        })
    }

    /// `record.field` without the projection, when `record` is known.
    fn project(&mut self, terms: &mut Terms, record: Aol<Term>, field: StrId) -> Option<Aol<Term>> {
        if let Term::Tuple(items) = terms.get(record) {
            // Dropping the other components is only sound when none of them
            // does anything, which holds for a bundle of known instances.
            let items = items.clone();
            for &item in &items {
                self.global_of(terms, item)?;
            }
            let item = *items.get(field.as_str().parse::<usize>().ok()?)?;
            return copy_leaf(terms, terms.get(item).clone());
        }
        let (g, key) = self.global_of(terms, record)?;
        match terms.get(self.bodies[&key]) {
            Term::Struct { base: None, .. } | Term::Tuple(_) => self.method(terms, g, field),
            _ => None,
        }
    }
}

/// A fresh copy of a variable or literal node, else `None`. A field of a global
/// literal is closed, so an unqualified variable there is a global or a builtin
/// and means the same anywhere.
fn copy_leaf(terms: &mut Terms, node: Term) -> Option<Aol<Term>> {
    match node {
        Term::Var { .. } | Term::Int(_) | Term::Real(_) | Term::Str(_) | Term::Bool(_) | Term::Unit => {
            Some(terms.add(node))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
