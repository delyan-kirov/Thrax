//! A-normalization: the port of `compiler/CRxANF.cpp` (Flanagan et al., "The
//! Essence of Compiling with Continuations"). Every operator and operand of an
//! application becomes an atom (a variable, literal, or lambda), and every
//! non-trivial intermediate computation is named by a `let`. A `tail` flag rides
//! through so an application in program tail position is marked as such (the CEK
//! machine reuses the activation for a tail call).
//!
//! The transform is continuation-passing: `norm(e, tail, k)` lowers `e` so its
//! value flows to the continuation `k`. `norm_name` additionally forces the
//! result to an atom, binding it to a fresh `%a` `let` when it is a computation.
//! Data-constructor fields stay INLINE (never hoisted into a `let`), matching the
//! interpreter's non-strict constructors; the closure converter later decides
//! their strictness.
//!
//! The Core is immutable (`Arc<Term>`), so the pass rebuilds the tree. Fresh-name
//! generation goes through a `Cell` counter so the CPS continuations can capture a
//! shared `&self`.

use std::cell::Cell;
use std::sync::Arc;

use super::data::{Arm, Clause, Handler, Program, Term};

/// A normalization continuation: given the (atomic-or-computation) value of the
/// current subexpression, produce the finished term. Called exactly once per
/// path. Borrows live as long as the input tree (`'a`).
type Cont<'a> = Box<dyn FnOnce(Term) -> Term + 'a>;

/// The A-normalizer. Holds only the fresh-name counter.
#[derive(Default)]
pub struct Anf {
    counter: Cell<usize>,
}

/// A-normalize every global in `program` (each definition body is in tail
/// position).
pub fn normalize_program(program: &mut Program) {
    let anf = Anf::default();
    for (_, term) in program.globals.iter_mut() {
        *term = anf.term(term);
    }
}

/// An atom needs no evaluation step to name: a literal, a variable, or a lambda.
/// `Fault` is a computation (it raises when forced), so it is hoisted into a
/// `let` in operand position and becomes an [`Expr`](super::data::Term) the IR
/// converter can render, never an atom.
fn is_atom(t: &Term) -> bool {
    matches!(
        t,
        Term::Int(_)
            | Term::Real(_)
            | Term::Str(_)
            | Term::Bool(_)
            | Term::Unit
            | Term::Var { .. }
            | Term::Lam { .. }
    )
}

impl Anf {
    fn fresh(&self) -> String {
        let n = self.counter.get();
        self.counter.set(n + 1);
        format!("%a{n}")
    }

    /// Normalize `e` in tail position, delivering its value directly.
    fn term(&self, e: &Term) -> Term {
        self.norm(e, true, Box::new(|v| v))
    }

    /// Normalize `e`, then hand `k` an ATOM: an atomic result passes straight
    /// through, a computation is bound to a fresh `let` whose variable is passed.
    fn norm_name<'a>(&'a self, e: &'a Term, k: Cont<'a>) -> Term {
        self.norm(
            e,
            false,
            Box::new(move |e2| {
                if is_atom(&e2) {
                    k(e2)
                } else {
                    let t = self.fresh();
                    Term::Let {
                        name: t.clone(),
                        rec: false,
                        val: Arc::new(e2),
                        body: Arc::new(k(Term::var(t))),
                    }
                }
            }),
        )
    }

    fn norm<'a>(&'a self, e: &'a Term, tail: bool, k: Cont<'a>) -> Term {
        match e {
            Term::Int(_)
            | Term::Real(_)
            | Term::Str(_)
            | Term::Bool(_)
            | Term::Unit
            | Term::Var { .. }
            | Term::Fault(_) => k(e.clone()),

            Term::Lam { param, body } => {
                // A lambda body is itself in tail position.
                let body = self.term(body);
                k(Term::Lam {
                    param: param.clone(),
                    body: Arc::new(body),
                })
            }

            Term::Variant { ty, tag, fields } => {
                // Constructor fields are non-strict: normalize each as its own
                // value producer but keep it inline (never hoist into a `let`).
                let fields: Arc<[Term]> = fields.iter().map(|f| self.term(f)).collect();
                k(Term::Variant {
                    ty: ty.clone(),
                    tag: tag.clone(),
                    fields,
                })
            }

            Term::App(f, x) => self.norm_name(
                f,
                Box::new(move |f2| {
                    self.norm_name(
                        x,
                        Box::new(move |x2| k(Term::App(Arc::new(f2), Arc::new(x2)))),
                    )
                }),
            ),

            Term::Case {
                scrut,
                arms,
                default,
            } => self.norm_name(
                scrut,
                Box::new(move |s2| {
                    // Each arm body (and the default) sits in the case's own
                    // position: tail arms stay tail, otherwise each produces the
                    // value the case is bound to (delivered through `k`).
                    let arms: Arc<[Arm]> = arms.iter().map(|a| self.norm_arm(a, tail)).collect();
                    let default = default
                        .as_ref()
                        .map(|d| Arc::new(self.norm_tail_or_value(d, tail)));
                    k(Term::Case {
                        scrut: Arc::new(s2),
                        arms,
                        default,
                    })
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
                let (name, rec) = (name.clone(), *rec);
                self.norm(
                    val,
                    false,
                    Box::new(move |v2| Term::Let {
                        name,
                        rec,
                        val: Arc::new(v2),
                        body: Arc::new(self.norm(body, tail, k)),
                    }),
                )
            }

            Term::Field(record, fname) => {
                let fname = fname.clone();
                self.norm_name(
                    record,
                    Box::new(move |r2| k(Term::Field(Arc::new(r2), fname))),
                )
            }

            Term::Tuple(fields) => self.norm_tuple(fields, 0, Vec::new(), k),

            Term::Struct { name, base, fields } => {
                let name = name.clone();
                match base {
                    Some(b) => self.norm_name(
                        b,
                        Box::new(move |b2| {
                            self.norm_struct(name, Some(Arc::new(b2)), fields, 0, Vec::new(), k)
                        }),
                    ),
                    None => self.norm_struct(name, None, fields, 0, Vec::new(), k),
                }
            }

            Term::Handle { body, handler } => {
                // The body sits in the prompt's tail position; the clause and
                // default bodies normalize like any other lambda body. The
                // handler is a computation, so a non-atom caller hoists it.
                let body = self.term(body);
                let clauses = handler
                    .clauses
                    .iter()
                    .map(|c| Clause {
                        effect: c.effect.clone(),
                        op: c.op.clone(),
                        arg: c.arg.clone(),
                        body: self.term(&c.body),
                    })
                    .collect();
                let default = handler
                    .default
                    .as_ref()
                    .map(|(x, b)| (x.clone(), self.term(b)));
                k(Term::Handle {
                    body: Arc::new(body),
                    handler: Arc::new(Handler {
                        continuation: handler.continuation.clone(),
                        clauses,
                        default,
                    }),
                })
            }

            Term::Defer { cleanup, body } => {
                // Both subterms normalize as their own tail computations; the
                // defer node is a computation delivered through `k`.
                let cleanup = self.term(cleanup);
                let body = self.term(body);
                k(Term::Defer {
                    cleanup: Arc::new(cleanup),
                    body: Arc::new(body),
                })
            }
        }
    }

    /// Normalize a case arm: its guard is a non-tail value, its body inherits the
    /// case's tail position.
    fn norm_arm(&self, a: &Arm, tail: bool) -> Arm {
        Arm {
            pat: a.pat.clone(),
            guard: a
                .guard
                .as_ref()
                .map(|g| Arc::new(self.norm(g, false, Box::new(|v| v)))),
            body: Arc::new(self.norm_tail_or_value(&a.body, tail)),
        }
    }

    /// Normalize a subexpression that is in tail position iff `tail`, otherwise as
    /// a self-contained value.
    fn norm_tail_or_value(&self, e: &Term, tail: bool) -> Term {
        if tail {
            self.term(e)
        } else {
            self.norm(e, false, Box::new(|v| v))
        }
    }

    fn norm_tuple<'a>(
        &'a self,
        fields: &'a [Term],
        i: usize,
        acc: Vec<Term>,
        k: Cont<'a>,
    ) -> Term {
        if i == fields.len() {
            return k(Term::Tuple(acc.into()));
        }
        self.norm_name(
            &fields[i],
            Box::new(move |a| {
                let mut acc = acc;
                acc.push(a);
                self.norm_tuple(fields, i + 1, acc, k)
            }),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn norm_struct<'a>(
        &'a self,
        name: String,
        base: Option<Arc<Term>>,
        fields: &'a [(String, Term)],
        i: usize,
        acc: Vec<(String, Term)>,
        k: Cont<'a>,
    ) -> Term {
        if i == fields.len() {
            return k(Term::Struct {
                name,
                base,
                fields: acc.into(),
            });
        }
        let fname = fields[i].0.clone();
        self.norm_name(
            &fields[i].1,
            Box::new(move |a| {
                let mut acc = acc;
                acc.push((fname, a));
                self.norm_struct(name, base, fields, i + 1, acc, k)
            }),
        )
    }
}

#[cfg(test)]
mod tests;
