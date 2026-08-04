//! Pattern-match compilation: rich `when`/`Case` arms (nested variant, struct,
//! tuple, string, and string-prefix patterns, with guards) are compiled into a
//! decision tree of SHALLOW cases, so the IR's flat `Alt` (a single constructor
//! or literal binding its payload positionally, exactly as `CR`) suffices. The
//! C++ front end does this before `CR`; the Rust rewrite kept rich patterns in
//! the Core and matched them at runtime, so this pass restores the C++ shape as
//! the front of the IR pipeline.
//!
//! Each arm is tried in order; on any mismatch (a wrong constructor/literal, a
//! failed sub-pattern, or a false guard) control falls through to the next arm
//! with the ORIGINAL scrutinee. Fallthrough is shared through a nullary thunk per
//! arm (`\_ = <rest>`, invoked as `k {}`) so a nested pattern's several mismatch
//! points do not duplicate the remaining arms. This mirrors the interpreter's
//! `eval_case_from`: match binds, an optional guard decides, a false guard
//! resumes at the next arm.
//!
//! The output is ordinary Core (shallow cases, `let` field extractions, and
//! `?=`/`array_slice` builtin tests for strings), so it still runs on the
//! tree-walker unchanged, which is how the pass is validated.

use std::sync::Arc;

use super::data::{Arm, Clause, Handler, Pat, Program, Term};

/// Compile rich patterns to shallow cases in every global.
pub fn compile_program(program: &mut Program) {
    let mut pm = Pm { n: 0 };
    for (_, term) in program.globals.iter_mut() {
        *term = pm.go(term);
    }
}

struct Pm {
    n: usize,
}

fn v(name: impl Into<String>) -> Term {
    Term::var(name)
}

/// `l <op> r` as a builtin application.
fn bin(op: &str, l: Term, r: Term) -> Term {
    Term::app(Term::app(Term::var(op), l), r)
}

/// A one-armed case: `when scrut is pat then body else default`.
fn case1(scrut: Term, pat: Pat, body: Term, default: Term) -> Term {
    Term::Case {
        scrut: Arc::new(scrut),
        arms: vec![Arm {
            pat,
            guard: None,
            body: Arc::new(body),
        }]
        .into(),
        default: Some(Arc::new(default)),
    }
}

impl Pm {
    fn fresh(&mut self, tag: &str) -> String {
        let n = self.n;
        self.n += 1;
        format!("%m{tag}{n}")
    }

    /// Flatten every `Case` under `t`, rebuilding the tree.
    fn go(&mut self, t: &Term) -> Term {
        match t {
            Term::Int(_)
            | Term::Real(_)
            | Term::Str(_)
            | Term::Bool(_)
            | Term::Unit
            | Term::Var { .. }
            | Term::Extern { .. }
            | Term::Fault(_) => t.clone(),

            Term::App(f, x) => Term::App(Arc::new(self.go(f)), Arc::new(self.go(x))),

            Term::Lam { param, body } => Term::Lam {
                param: param.clone(),
                body: Arc::new(self.go(body)),
            },

            Term::Let {
                name,
                rec,
                val,
                body,
            } => Term::Let {
                name: name.clone(),
                rec: *rec,
                val: Arc::new(self.go(val)),
                body: Arc::new(self.go(body)),
            },

            Term::Case {
                scrut,
                arms,
                default,
            } => {
                let scrut = self.go(scrut);
                let arms: Vec<Arm> = arms
                    .iter()
                    .map(|a| Arm {
                        pat: a.pat.clone(),
                        guard: a.guard.as_ref().map(|g| Arc::new(self.go(g))),
                        body: Arc::new(self.go(&a.body)),
                    })
                    .collect();
                let default = default.as_ref().map(|d| self.go(d));
                self.compile_case(scrut, &arms, default)
            }

            Term::Tuple(fields) => Term::Tuple(fields.iter().map(|f| self.go(f)).collect()),

            Term::Struct { name, base, fields } => Term::Struct {
                name: name.clone(),
                base: base.as_ref().map(|b| Arc::new(self.go(b))),
                fields: fields
                    .iter()
                    .map(|(n, val)| (n.clone(), self.go(val)))
                    .collect(),
            },

            Term::Variant { ty, tag, fields } => Term::Variant {
                ty: ty.clone(),
                tag: tag.clone(),
                fields: fields.iter().map(|f| self.go(f)).collect(),
            },

            Term::Field(rec, name) => Term::Field(Arc::new(self.go(rec)), name.clone()),

            Term::Handle { body, handler } => Term::Handle {
                body: Arc::new(self.go(body)),
                handler: Arc::new(Handler {
                    continuation: handler.continuation.clone(),
                    clauses: handler
                        .clauses
                        .iter()
                        .map(|c| Clause {
                            effect: c.effect.clone(),
                            op: c.op.clone(),
                            arg: c.arg.clone(),
                            body: self.go(&c.body),
                        })
                        .collect(),
                    default: handler
                        .default
                        .as_ref()
                        .map(|(x, b)| (x.clone(), self.go(b))),
                }),
            },

            Term::Defer { cleanup, body } => Term::Defer {
                cleanup: Arc::new(self.go(cleanup)),
                body: Arc::new(self.go(body)),
            },
        }
    }

    /// Compile one flattened `Case`: bind the scrutinee once, then chain the arms.
    fn compile_case(&mut self, scrut: Term, arms: &[Arm], default: Option<Term>) -> Term {
        let s = self.fresh("s");
        let ultimate = default
            .unwrap_or_else(|| Term::Fault("no pattern matched (non-exhaustive `when`)".into()));
        let chain = self.compile_arms(&s, arms, 0, ultimate);
        Term::Let {
            name: s,
            rec: false,
            val: Arc::new(scrut),
            body: Arc::new(chain),
        }
    }

    /// The code for arms `i..`: try arm `i`, falling through to arms `i+1..` (a
    /// shared thunk) on any mismatch, or to `fail` when the arms are exhausted.
    fn compile_arms(&mut self, s: &str, arms: &[Arm], i: usize, fail: Term) -> Term {
        if i == arms.len() {
            return fail;
        }
        let fail_rest = self.compile_arms(s, arms, i + 1, fail);
        let fk = self.fresh("k");
        let on_fail = Term::app(v(fk.clone()), Term::Unit);
        let arm_code = self.compile_arm(s, &arms[i], &on_fail);
        Term::Let {
            name: fk,
            rec: false,
            val: Arc::new(Term::Lam {
                param: "_".into(),
                body: Arc::new(fail_rest),
            }),
            body: Arc::new(arm_code),
        }
    }

    /// Compile a single arm: match its pattern, then run the guard (if any) and
    /// body; any failure goes to `on_fail`.
    fn compile_arm(&mut self, s: &str, arm: &Arm, on_fail: &Term) -> Term {
        let success = match &arm.guard {
            Some(g) => case1(
                (**g).clone(),
                Pat::Bool(true),
                (*arm.body).clone(),
                on_fail.clone(),
            ),
            None => (*arm.body).clone(),
        };
        self.compile_pat(s, &arm.pat, success, on_fail)
    }

    /// Match `pat` against the variable `sv`; on full match run `on_match`, else
    /// `on_fail`.
    fn compile_pat(&mut self, sv: &str, pat: &Pat, on_match: Term, on_fail: &Term) -> Term {
        match pat {
            Pat::Wild => on_match,
            Pat::Var(name) => Term::Let {
                name: name.clone(),
                rec: false,
                val: Arc::new(v(sv)),
                body: Arc::new(on_match),
            },
            Pat::Int(k) => case1(v(sv), Pat::Int(*k), on_match, on_fail.clone()),
            Pat::Real(r) => case1(v(sv), Pat::Real(*r), on_match, on_fail.clone()),
            Pat::Bool(b) => case1(v(sv), Pat::Bool(*b), on_match, on_fail.clone()),
            Pat::Str(bytes) => case1(
                bin("?=", v(sv), Term::Str(bytes.clone())),
                Pat::Bool(true),
                on_match,
                on_fail.clone(),
            ),
            Pat::Tuple(pats) => self.compile_fields(sv, &index_names(pats.len()), pats, on_match, on_fail),
            Pat::Struct { fields } => {
                let (names, pats): (Vec<String>, Vec<Pat>) =
                    fields.iter().map(|(n, p)| (n.clone(), p.clone())).unzip();
                self.compile_fields(sv, &names, &pats, on_match, on_fail)
            }
            Pat::Variant { tag, fields } => {
                let binders: Vec<String> = (0..fields.len()).map(|_| self.fresh("v")).collect();
                let inner = self.compile_binders(&binders, fields, on_match, on_fail);
                let shallow = Pat::Variant {
                    tag: tag.clone(),
                    fields: binders.iter().map(|b| Pat::Var(b.clone())).collect(),
                };
                case1(v(sv), shallow, inner, on_fail.clone())
            }
            Pat::StrPrefix { prefix, rest } => {
                let plen = prefix.len();
                let tv = self.fresh("t");
                let tail_match = Term::Let {
                    name: tv.clone(),
                    rec: false,
                    val: Arc::new(array_slice(sv, Term::Int(plen as i64), array_len(sv))),
                    body: Arc::new(self.compile_pat(&tv, rest, on_match, on_fail)),
                };
                let prefix_ok = case1(
                    bin(
                        "?=",
                        array_slice(sv, Term::Int(0), Term::Int(plen as i64)),
                        Term::Str(prefix.clone()),
                    ),
                    Pat::Bool(true),
                    tail_match,
                    on_fail.clone(),
                );
                case1(
                    bin(">=", array_len(sv), Term::Int(plen as i64)),
                    Pat::Bool(true),
                    prefix_ok,
                    on_fail.clone(),
                )
            }
        }
    }

    /// Match `pats` against fields of `sv` named `names` (a tuple index or struct
    /// field), each bound to a fresh variable, left-to-right.
    fn compile_fields(
        &mut self,
        sv: &str,
        names: &[String],
        pats: &[Pat],
        on_match: Term,
        on_fail: &Term,
    ) -> Term {
        self.compile_fields_from(sv, names, pats, 0, on_match, on_fail)
    }

    fn compile_fields_from(
        &mut self,
        sv: &str,
        names: &[String],
        pats: &[Pat],
        i: usize,
        on_match: Term,
        on_fail: &Term,
    ) -> Term {
        if i == pats.len() {
            return on_match;
        }
        let fv = self.fresh("f");
        let rest = self.compile_fields_from(sv, names, pats, i + 1, on_match, on_fail);
        let sub = self.compile_pat(&fv, &pats[i], rest, on_fail);
        Term::Let {
            name: fv,
            rec: false,
            val: Arc::new(Term::Field(Arc::new(v(sv)), names[i].clone())),
            body: Arc::new(sub),
        }
    }

    /// Match `pats` against already-bound variables `binders`, left-to-right (the
    /// payload of a shallow variant match).
    fn compile_binders(
        &mut self,
        binders: &[String],
        pats: &[Pat],
        on_match: Term,
        on_fail: &Term,
    ) -> Term {
        self.compile_binders_from(binders, pats, 0, on_match, on_fail)
    }

    fn compile_binders_from(
        &mut self,
        binders: &[String],
        pats: &[Pat],
        i: usize,
        on_match: Term,
        on_fail: &Term,
    ) -> Term {
        if i == pats.len() {
            return on_match;
        }
        let rest = self.compile_binders_from(binders, pats, i + 1, on_match, on_fail);
        self.compile_pat(&binders[i], &pats[i], rest, on_fail)
    }
}

/// `array_len sv`.
fn array_len(sv: &str) -> Term {
    Term::app(v("array_len"), v(sv))
}

/// `array_slice sv beg end`.
fn array_slice(sv: &str, beg: Term, end: Term) -> Term {
    Term::app(Term::app(Term::app(v("array_slice"), v(sv)), beg), end)
}

/// Tuple field names are their decimal indices.
fn index_names(n: usize) -> Vec<String> {
    (0..n).map(|i| i.to_string()).collect()
}

#[cfg(test)]
mod tests;
