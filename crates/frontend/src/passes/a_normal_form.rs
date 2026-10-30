//! A-normal form: every intermediate result gets a name.
//!
//! The transform is from Flanagan, Sabry, Duba and Felleisen, "The Essence of
//! Compiling with Continuations" (PLDI 1993). Every operator and operand of an
//! application becomes an atom (a variable, literal, or lambda), and every
//! non-trivial intermediate computation is named by a `let`. A `tail` flag
//! rides through so an application in tail position is marked as such (the
//! engines reuse the activation for a tail call).
//!
//! It is written in continuation-passing style: `norm(e, tail, k)` lowers `e`
//! so its value flows to the continuation `k`. `norm_name` additionally forces
//! the result to an atom, binding it to a fresh `%a` `let` when it is a
//! computation. Variant fields are the exception.
//! See Note [Constructor fields stay inline in ANF].
//!
//! Core nodes are immutable, so the pass adds the rebuilt tree beside the old
//! one. The store is threaded through the continuations as an argument, and
//! fresh names come from a `RefCell` supply, so the continuations need only a
//! shared `&self`.
//!
//! Input: a match-compiled Core program. Output: the same program in A-normal
//! form.
//!
//! Entry point: [`to_a_normal_form`].

use std::cell::RefCell;

use utilities::{Aol, StrId};

use crate::core::{CoreArm, CoreClause, CoreHandler, CoreProgram, FreshNames, Term, Terms};

/// A normalization continuation: given the (atomic-or-computation) value of the
/// current subexpression, produce the finished term. Called exactly once per
/// path.
type Cont<'a> = Box<dyn FnOnce(&mut Terms, Aol<Term>) -> Aol<Term> + 'a>;

/// The A-normalizer. Holds only the fresh-name supply.
pub struct ANormalizer {
    names: RefCell<FreshNames>,
}

impl Default for ANormalizer {
    fn default() -> ANormalizer {
        ANormalizer { names: RefCell::new(FreshNames::new("%a", 0)) }
    }
}

/// A-normalize every global in `program` (each definition body is in tail
/// position).
pub fn to_a_normal_form(program: &mut CoreProgram) {
    let anf = ANormalizer::default();
    program.rewrite_globals(|terms, term| anf.term(terms, term));
}

impl ANormalizer {
    fn fresh(&self) -> StrId {
        self.names.borrow_mut().name()
    }

    /// Normalize `e` in tail position, delivering its value directly.
    fn term(&self, t: &mut Terms, e: Aol<Term>) -> Aol<Term> {
        self.norm(t, e, true, Box::new(|_, v| v))
    }

    /// Normalize `e` as a value, not in tail position.
    fn value(&self, t: &mut Terms, e: Aol<Term>) -> Aol<Term> {
        self.norm(t, e, false, Box::new(|_, v| v))
    }

    /// Normalize `e`, then hand `k` an ATOM: an atomic result passes straight
    /// through, a computation is bound to a fresh `let` whose variable is passed.
    fn norm_name<'a>(&'a self, t: &mut Terms, e: Aol<Term>, k: Cont<'a>) -> Aol<Term> {
        self.norm(
            t,
            e,
            false,
            Box::new(move |t, e2| {
                if t.is_atom(e2) {
                    k(t, e2)
                } else {
                    let name = self.fresh();
                    let var = t.var(name);
                    let body = k(t, var);
                    t.let_(name, e2, body)
                }
            }),
        )
    }

    fn norm<'a>(&'a self, t: &mut Terms, e: Aol<Term>, tail: bool, k: Cont<'a>) -> Aol<Term> {
        match t.get(e).clone() {
            Term::Int(_)
            | Term::Real(_)
            | Term::Str(_)
            | Term::Bool(_)
            | Term::Unit
            | Term::Var { .. }
            | Term::Extern { .. }
            | Term::Fault(_) => k(t, e),

            Term::Lam { param, body } => {
                // A lambda body is itself in tail position.
                let body = self.term(t, body);
                let lam = t.lam(param, body);
                k(t, lam)
            }

            // Note [Constructor fields stay inline in ANF]
            //
            // A variant's fields are the one place this pass leaves a
            // computation in operand position. Each field is normalized as a
            // term of its own, but not named by a `let` around the
            // construction; tuple and struct fields are named like any other
            // operand. The shape is a contract with closure conversion:
            // `ClosureConverter::variant` names each computed field itself,
            // one `let` per field just around the `MkVariant`, reserving the
            // field's slot before converting the field so the field's own
            // hoisted `let`s get distinct slots. Changing either side alone
            // changes the slots every engine and the generated C use.
            Term::Variant { ty, tag, fields } => {
                let fields = fields.iter().map(|f| self.term(t, *f)).collect();
                let variant = t.add(Term::Variant { ty, tag, fields });
                k(t, variant)
            }

            Term::App(f, x) => self.norm_name(
                t,
                f,
                Box::new(move |t, f2| {
                    self.norm_name(t, x, Box::new(move |t, x2| {
                        let app = t.app(f2, x2);
                        k(t, app)
                    }))
                }),
            ),

            Term::Case {
                scrut,
                arms,
                default,
            } => self.norm_name(
                t,
                scrut,
                Box::new(move |t, s2| {
                    // Each arm body (and the default) sits in the case's own
                    // position: tail arms stay tail, otherwise each produces the
                    // value the case is bound to (delivered through `k`).
                    let arms = arms.iter().map(|a| self.norm_arm(t, a, tail)).collect();
                    let default = default.map(|d| self.norm_tail_or_value(t, d, tail));
                    let case = t.add(Term::Case {
                        scrut: s2,
                        arms,
                        default,
                    });
                    k(t, case)
                }),
            ),

            Term::Let {
                name,
                rec,
                val,
                body,
            } => {
                // The bound value is never in tail position; the body inherits
                // this let's position and continuation.
                if rec && t.mentions(val, name) {
                    // Normalizing the value with the let as its continuation
                    // would float the value's own lets above this one, out of
                    // the scope of the name they refer to (#250).
                    let v2 = self.value(t, val);
                    let body = self.norm(t, body, tail, k);
                    return t.add(Term::Let { name, rec, val: v2, body });
                }
                self.norm(
                    t,
                    val,
                    false,
                    Box::new(move |t, v2| {
                        let body = self.norm(t, body, tail, k);
                        t.add(Term::Let { name, rec, val: v2, body })
                    }),
                )
            }

            Term::Field(record, fname) => self.norm_name(
                t,
                record,
                Box::new(move |t, r2| {
                    let field = t.field(r2, fname);
                    k(t, field)
                }),
            ),

            Term::Tuple(fields) => self.norm_tuple(t, fields, 0, Vec::new(), k),

            Term::Struct { name, base, fields } => match base {
                Some(b) => self.norm_name(
                    t,
                    b,
                    Box::new(move |t, b2| self.norm_struct(t, name, Some(b2), fields, 0, Vec::new(), k)),
                ),
                None => self.norm_struct(t, name, None, fields, 0, Vec::new(), k),
            },

            Term::Handle { body, handler } => {
                // The body sits in the prompt's tail position; the clause and
                // default bodies normalize like any other lambda body. The
                // handler is a computation, so a non-atom caller hoists it.
                let body = self.term(t, body);
                let clauses = handler
                    .clauses
                    .iter()
                    .map(|c| CoreClause {
                        effect: c.effect,
                        op: c.op,
                        arg: c.arg,
                        body: self.term(t, c.body),
                    })
                    .collect();
                let default = handler.default.map(|(x, b)| (x, self.term(t, b)));
                let handle = t.add(Term::Handle {
                    body,
                    handler: Box::new(CoreHandler {
                        continuation: handler.continuation,
                        clauses,
                        default,
                    }),
                });
                k(t, handle)
            }

            Term::Defer { cleanup, body } => {
                // Both subterms normalize as their own tail computations; the
                // defer node is a computation delivered through `k`.
                let cleanup = self.term(t, cleanup);
                let body = self.term(t, body);
                let defer = t.add(Term::Defer { cleanup, body });
                k(t, defer)
            }
        }
    }

    /// Normalize a case arm: its guard is a non-tail value, its body inherits the
    /// case's tail position.
    fn norm_arm(&self, t: &mut Terms, a: &CoreArm, tail: bool) -> CoreArm {
        CoreArm {
            pat: a.pat.clone(),
            guard: a.guard.map(|g| self.value(t, g)),
            body: self.norm_tail_or_value(t, a.body, tail),
        }
    }

    /// Normalize a subexpression that is in tail position iff `tail`, otherwise as
    /// a self-contained value.
    fn norm_tail_or_value(&self, t: &mut Terms, e: Aol<Term>, tail: bool) -> Aol<Term> {
        if tail {
            self.term(t, e)
        } else {
            self.value(t, e)
        }
    }

    fn norm_tuple<'a>(
        &'a self,
        t: &mut Terms,
        fields: Vec<Aol<Term>>,
        i: usize,
        acc: Vec<Aol<Term>>,
        k: Cont<'a>,
    ) -> Aol<Term> {
        if i == fields.len() {
            let tuple = t.add(Term::Tuple(acc));
            return k(t, tuple);
        }
        let field = fields[i];
        self.norm_name(
            t,
            field,
            Box::new(move |t, a| {
                let mut acc = acc;
                acc.push(a);
                self.norm_tuple(t, fields, i + 1, acc, k)
            }),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn norm_struct<'a>(
        &'a self,
        t: &mut Terms,
        name: StrId,
        base: Option<Aol<Term>>,
        fields: Vec<(StrId, Aol<Term>)>,
        i: usize,
        acc: Vec<(StrId, Aol<Term>)>,
        k: Cont<'a>,
    ) -> Aol<Term> {
        if i == fields.len() {
            let record = t.add(Term::Struct {
                name,
                base,
                fields: acc,
            });
            return k(t, record);
        }
        let (fname, field) = fields[i];
        self.norm_name(
            t,
            field,
            Box::new(move |t, a| {
                let mut acc = acc;
                acc.push((fname, a));
                self.norm_struct(t, name, base, fields, i + 1, acc, k)
            }),
        )
    }
}

#[cfg(test)]
mod tests;
