//! De-Bruijn indexing: the port of `CR.cpp`'s `assign_id`. After lowering (and
//! ANF), every [`Term::Var`] gets its De-Bruijn index against the binder stack.
//! `0` marks a global, resolved by module/name; a positive index counts outward
//! through the runtime binders to a local. The closure converter reads this to
//! split a variable into a local activation slot vs an environment capture,
//! replacing a name search with O(1) array indexing.
//!
//! The Core is immutable (`Arc<Term>`), so this rebuilds the tree rather than
//! mutating in place. Binder discipline mirrors `CR`: a lambda pushes its
//! parameter; a recursive `let` pushes its name for both the value and the body,
//! a non-recursive one only for the body; a `case` arm pushes the arm's pattern
//! binders (left-to-right, depth-first) for its guard and body; a handler clause
//! pushes the operation argument then the continuation.

use std::sync::Arc;

use super::data::{Arm, Clause, Handler, Pat, Program, Term};

/// Assign De-Bruijn indices to every variable in each of `program`'s globals. A
/// global's own name is resolved through the global table (index `0`), so each
/// definition starts from an empty binder stack.
pub fn assign_program(program: &mut Program) {
    for (_, term) in program.globals.iter_mut() {
        let mut names = Vec::new();
        *term = assign(term, &mut names);
    }
}

fn assign(t: &Term, names: &mut Vec<String>) -> Term {
    match t {
        Term::Int(_)
        | Term::Real(_)
        | Term::Str(_)
        | Term::Bool(_)
        | Term::Unit
        | Term::Fault(_) => t.clone(),

        Term::Var { module, name, .. } => Term::Var {
            module: module.clone(),
            name: name.clone(),
            idx: if module.is_some() {
                0
            } else {
                lookup(names, name)
            },
        },

        Term::App(f, x) => Term::App(Arc::new(assign(f, names)), Arc::new(assign(x, names))),

        Term::Lam { param, body } => {
            names.push(param.clone());
            let body = assign(body, names);
            names.pop();
            Term::Lam {
                param: param.clone(),
                body: Arc::new(body),
            }
        }

        Term::Let {
            name,
            rec,
            val,
            body,
        } => {
            if *rec {
                names.push(name.clone());
                let val = assign(val, names);
                let body = assign(body, names);
                names.pop();
                Term::Let {
                    name: name.clone(),
                    rec: true,
                    val: Arc::new(val),
                    body: Arc::new(body),
                }
            } else {
                let val = assign(val, names);
                names.push(name.clone());
                let body = assign(body, names);
                names.pop();
                Term::Let {
                    name: name.clone(),
                    rec: false,
                    val: Arc::new(val),
                    body: Arc::new(body),
                }
            }
        }

        Term::Case {
            scrut,
            arms,
            default,
        } => {
            let scrut = assign(scrut, names);
            let arms: Vec<Arm> = arms.iter().map(|a| assign_arm(a, names)).collect();
            let default = default.as_ref().map(|d| Arc::new(assign(d, names)));
            Term::Case {
                scrut: Arc::new(scrut),
                arms: arms.into(),
                default,
            }
        }

        Term::Tuple(fields) => {
            Term::Tuple(fields.iter().map(|f| assign(f, names)).collect())
        }

        Term::Struct { name, base, fields } => Term::Struct {
            name: name.clone(),
            base: base.as_ref().map(|b| Arc::new(assign(b, names))),
            fields: fields
                .iter()
                .map(|(n, v)| (n.clone(), assign(v, names)))
                .collect(),
        },

        Term::Variant { ty, tag, fields } => Term::Variant {
            ty: ty.clone(),
            tag: tag.clone(),
            fields: fields.iter().map(|f| assign(f, names)).collect(),
        },

        Term::Field(rec, name) => Term::Field(Arc::new(assign(rec, names)), name.clone()),

        Term::Handle { body, handler } => Term::Handle {
            body: Arc::new(assign(body, names)),
            handler: Arc::new(assign_handler(handler, names)),
        },

        Term::Defer { cleanup, body } => Term::Defer {
            cleanup: Arc::new(assign(cleanup, names)),
            body: Arc::new(assign(body, names)),
        },
    }
}

/// The De-Bruijn index of `name` in the binder stack (innermost last): the
/// 1-based distance from the top, or `0` if unbound (a global).
fn lookup(names: &[String], name: &str) -> usize {
    for (i, n) in names.iter().enumerate().rev() {
        if n == name {
            return names.len() - i;
        }
    }
    0
}

fn assign_arm(a: &Arm, names: &mut Vec<String>) -> Arm {
    let mut bound = Vec::new();
    collect_pat_binders(&a.pat, &mut bound);
    let depth = bound.len();
    names.append(&mut bound.clone());
    let guard = a.guard.as_ref().map(|g| Arc::new(assign(g, names)));
    let body = assign(&a.body, names);
    names.truncate(names.len() - depth);
    Arm {
        pat: a.pat.clone(),
        guard,
        body: Arc::new(body),
    }
}

/// Collect a pattern's variable binders, left-to-right depth-first. This order
/// defines the local-slot order the closure converter and machine must reuse.
fn collect_pat_binders(p: &Pat, out: &mut Vec<String>) {
    match p {
        Pat::Wild | Pat::Int(_) | Pat::Real(_) | Pat::Str(_) | Pat::Bool(_) => {}
        Pat::Var(n) => out.push(n.clone()),
        Pat::Tuple(ps) => {
            for p in ps {
                collect_pat_binders(p, out);
            }
        }
        Pat::Variant { fields, .. } => {
            for p in fields {
                collect_pat_binders(p, out);
            }
        }
        Pat::Struct { fields } => {
            for (_, p) in fields {
                collect_pat_binders(p, out);
            }
        }
        Pat::StrPrefix { rest, .. } => collect_pat_binders(rest, out),
    }
}

fn assign_handler(h: &Handler, names: &mut Vec<String>) -> Handler {
    let clauses = h
        .clauses
        .iter()
        .map(|c| {
            // A clause body sees the operation argument, then the continuation `k`
            // innermost (CR models it as `\arg = \k = body`).
            names.push(c.arg.clone());
            names.push(h.continuation.clone());
            let body = assign(&c.body, names);
            names.pop();
            names.pop();
            Clause {
                effect: c.effect.clone(),
                op: c.op.clone(),
                arg: c.arg.clone(),
                body,
            }
        })
        .collect();
    let default = h.default.as_ref().map(|(x, body)| {
        names.push(x.clone());
        let b = assign(body, names);
        names.pop();
        (x.clone(), b)
    });
    Handler {
        continuation: h.continuation.clone(),
        clauses,
        default,
    }
}

#[cfg(test)]
mod tests;
