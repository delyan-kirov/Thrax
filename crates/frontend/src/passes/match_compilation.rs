//! Match compilation: nested patterns to shallow, one-constructor tests.
//!
//! The IR's case alternative matches one constructor or literal and binds its
//! payload positionally. This pass compiles each rich `Case` (nested variant,
//! struct, tuple, string, prefix, range and hooked patterns, with guards) into
//! a tree of such shallow cases, in the backtracking style: arms are tried in
//! order, and any mismatch (a wrong constructor or literal, a failed
//! sub-pattern, a false guard) falls through to the next arm against the
//! original scrutinee. See Note [Fallthrough thunks].
//!
//! The output is ordinary Core: shallow cases, `let` field extractions, and
//! builtin tests (`==`, `@array_slice`, the hooks' equality) for what has no
//! constructor.
//!
//! Input: a desugared Core program. Output: the same program with every
//! pattern shallow.
//!
//! Entry point: [`compile_matches`].

use std::sync::Arc;

use crate::core::{CoreArm, CoreProgram, FreshNames, Pat, Term};

/// Compile rich patterns to shallow cases in every global.
pub fn compile_matches(program: &mut CoreProgram) {
    let mut pm = MatchCompiler::new();
    for (_, term) in program.globals.iter_mut() {
        *term = pm.go(term);
    }
}

struct MatchCompiler {
    names: FreshNames,
}

/// A one-armed case: `when scrut is pat then body else default`.
fn case1(scrut: Term, pat: Pat, body: Term, default: Term) -> Term {
    Term::Case {
        scrut: Arc::new(scrut),
        arms: vec![CoreArm {
            pat,
            guard: None,
            body: Arc::new(body),
        }]
        .into(),
        default: Some(Arc::new(default)),
    }
}

impl MatchCompiler {
    fn new() -> MatchCompiler {
        MatchCompiler { names: FreshNames::new("%m", 0) }
    }

    fn fresh(&mut self, tag: &str) -> String {
        self.names.tagged(tag)
    }

    /// Flatten every `Case` under `t`, rebuilding the tree.
    fn go(&mut self, t: &Term) -> Term {
        match t.map_children(&mut |child, _| self.go(child)) {
            Term::Case { scrut, arms, default } => {
                let default = default.map(Arc::unwrap_or_clone);
                self.compile_case(Arc::unwrap_or_clone(scrut), &arms, default)
            }
            t => t,
        }
    }

    /// Compile one flattened `Case`: bind the scrutinee once, then chain the arms.
    fn compile_case(&mut self, scrut: Term, arms: &[CoreArm], default: Option<Term>) -> Term {
        let s = self.fresh("s");
        let ultimate = default
            .unwrap_or_else(|| Term::Fault("no pattern matched (non-exhaustive match)".into()));
        let chain = self.compile_arms(&s, arms, 0, ultimate);
        Term::Let {
            name: s,
            rec: false,
            val: Arc::new(scrut),
            body: Arc::new(chain),
        }
    }

    // Note [Fallthrough thunks]
    //
    // A nested pattern can fail at several points: the outer constructor, then
    // each sub-pattern, then the guard. Every one of them must continue with
    // the next arm against the original scrutinee. Copying the code for the
    // remaining arms into each failure point would grow the output
    // exponentially in the nesting depth.
    //
    // So the remaining arms are compiled once, into a nullary function bound
    // just before the arm that may fail: `let %mk = \_ = <rest> in <arm>`.
    // Each failure point is the call `%mk {}`. Matching is then linear in the
    // patterns, a false guard falls through exactly like a mismatch, and the
    // rest runs only on failure, because a lambda's body does not run until it
    // is called. See Note [Case is never lowered to application].

    /// The code for arms `i..`: try arm `i`, falling through to arms `i+1..` on
    /// any mismatch, or to `fail` when the arms are exhausted.
    /// See Note [Fallthrough thunks].
    fn compile_arms(&mut self, s: &str, arms: &[CoreArm], i: usize, fail: Term) -> Term {
        if i == arms.len() {
            return fail;
        }
        let fail_rest = self.compile_arms(s, arms, i + 1, fail);
        let fk = self.fresh("k");
        let on_fail = Term::app(Term::var(fk.clone()), Term::Unit);
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
    fn compile_arm(&mut self, s: &str, arm: &CoreArm, on_fail: &Term) -> Term {
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
                val: Arc::new(Term::var(sv)),
                body: Arc::new(on_match),
            },
            Pat::Int(k) => case1(Term::var(sv), Pat::Int(*k), on_match, on_fail.clone()),
            Pat::Real(r) => case1(Term::var(sv), Pat::Real(*r), on_match, on_fail.clone()),
            Pat::Bool(b) => case1(Term::var(sv), Pat::Bool(*b), on_match, on_fail.clone()),
            Pat::Str(bytes) => case1(
                Term::bin("==", Term::var(sv), Term::Str(bytes.clone())),
                Pat::Bool(true),
                on_match,
                on_fail.clone(),
            ),
            // A literal pattern on a user type: succeed when its equality hook holds
            // between the scrutinee and the literal built into that type.
            Pat::HookEq { eq, value } => {
                let test = Term::app(Term::app(eq.clone(), Term::var(sv)), self.go(value));
                case1(test, Pat::Bool(true), on_match, on_fail.clone())
            }
            // A sequence pattern on a user type: unfold the `sequence_view` hook one
            // step at a time, matching each leading element and (`..rest`) binding or
            // (no rest) requiring `Empty` at the end.
            Pat::SeqView { view, back, elems, rest } => {
                self.compile_seqview(sv, view, *back, elems, rest.as_deref(), on_match, on_fail)
            }
            // Lengths were proven by the checker, so each part is read directly.
            Pat::Tensor { len, front, mid, back } => {
                let idx = |i: u64| Term::app(Term::app(Term::var("@tensor_index"), Term::var(sv)), Term::Int(i as i64));
                let back_start = len - back.len() as u64;
                let mut parts: Vec<(Term, &Pat)> = Vec::new();
                parts.extend(front.iter().enumerate().map(|(i, p)| (idx(i as u64), p)));
                if let Some(mid) = mid {
                    let slice = Term::app(
                        Term::app(Term::app(Term::var("@tensor_slice"), Term::var(sv)), Term::Int(front.len() as i64)),
                        Term::Int(back_start as i64),
                    );
                    parts.push((slice, mid));
                }
                parts.extend(back.iter().enumerate().map(|(j, p)| (idx(back_start + j as u64), p)));
                parts.into_iter().rev().fold(on_match, |inner, (val, p)| {
                    let fv = self.fresh("f");
                    let sub = self.compile_pat(&fv, p, inner, on_fail);
                    Term::Let {
                        name: fv,
                        rec: false,
                        val: Arc::new(val),
                        body: Arc::new(sub),
                    }
                })
            }
            // `lo ... hi`: match when `sv >= lo` and `sv <= hi` (inclusive). An open
            // range `lo ...` has no upper bound, so only the `sv >= lo` test.
            Pat::Range { lo, hi } => {
                let inner = match hi {
                    Some(hi) => case1(
                        Term::bin("<=", Term::var(sv), hi.clone()),
                        Pat::Bool(true),
                        on_match,
                        on_fail.clone(),
                    ),
                    None => on_match,
                };
                case1(
                    Term::bin(">=", Term::var(sv), lo.clone()),
                    Pat::Bool(true),
                    inner,
                    on_fail.clone(),
                )
            }
            Pat::Tuple(pats) => self.compile_fields(sv, &index_names(pats.len()), pats, on_match, on_fail),
            Pat::Struct { fields, rest } => {
                let (names, pats): (Vec<String>, Vec<Pat>) =
                    fields.iter().map(|(n, p)| (n.clone(), p.clone())).unzip();
                // `..name` binds the record minus the listed labels: drop each label
                // from `sv` in turn, then match the fields against the bound record.
                let on_match = match rest {
                    Some(name) => {
                        let restricted = names.iter().fold(Term::var(sv), |rec, label| {
                            record_without(rec, label)
                        });
                        Term::Let {
                            name: name.clone(),
                            rec: false,
                            val: Arc::new(restricted),
                            body: Arc::new(on_match),
                        }
                    }
                    None => on_match,
                };
                self.compile_fields(sv, &names, &pats, on_match, on_fail)
            }
            Pat::Variant { tag, fields } => {
                let binders: Vec<String> = (0..fields.len()).map(|_| self.fresh("v")).collect();
                let inner = self.compile_binders(&binders, fields, on_match, on_fail);
                let shallow = Pat::Variant {
                    tag: tag.clone(),
                    fields: binders.iter().map(|b| Pat::Var(b.clone())).collect(),
                };
                case1(Term::var(sv), shallow, inner, on_fail.clone())
            }
            // `back` matches a suffix: the literal is the last `plen` bytes and the
            // rest is everything before it.
            Pat::StrPrefix { prefix, back, rest } => {
                let plen = Term::Int(prefix.len() as i64);
                let cut = if *back { Term::bin("@isub", array_len(sv), plen.clone()) } else { plen.clone() };
                let (lit, left) = if *back {
                    (array_slice(sv, cut.clone(), array_len(sv)), array_slice(sv, Term::Int(0), cut))
                } else {
                    (array_slice(sv, Term::Int(0), cut.clone()), array_slice(sv, cut, array_len(sv)))
                };
                let plen = prefix.len();
                let tv = self.fresh("t");
                let tail_match = Term::Let {
                    name: tv.clone(),
                    rec: false,
                    val: Arc::new(left),
                    body: Arc::new(self.compile_pat(&tv, rest, on_match, on_fail)),
                };
                let prefix_ok = case1(
                    Term::bin("==", lit, Term::Str(prefix.clone())),
                    Pat::Bool(true),
                    tail_match,
                    on_fail.clone(),
                );
                case1(
                    Term::bin(">=", array_len(sv), Term::Int(plen as i64)),
                    Pat::Bool(true),
                    prefix_ok,
                    on_fail.clone(),
                )
            }
        }
    }

    /// Unfold a `sequence_view` sequence pattern: view the current sequence `sv`,
    /// then for each leading element take a `More head tail` step (matching the
    /// element pattern and recursing on the tail), and finally either bind `..rest`
    /// to the remaining sequence or require it to be `Empty`.
    fn compile_seqview(
        &mut self,
        sv: &str,
        view: &Term,
        back: bool,
        elems: &[Pat],
        rest: Option<&Pat>,
        on_match: Term,
        on_fail: &Term,
    ) -> Term {
        if elems.is_empty() {
            return match rest {
                // `..rest` binds the whole remaining sequence.
                Some(pr) => self.compile_pat(sv, pr, on_match, on_fail),
                // A fixed-length pattern: the tail must view as `Empty`.
                None => {
                    let vw = self.fresh("vw");
                    let body = case1(
                        Term::var(&vw),
                        Pat::Variant {
                            tag: "Empty".into(),
                            fields: Vec::new(),
                        },
                        on_match,
                        on_fail.clone(),
                    );
                    Term::Let {
                        name: vw,
                        rec: false,
                        val: Arc::new(view_call(view, sv)),
                        body: Arc::new(body),
                    }
                }
            };
        }
        let vw = self.fresh("vw");
        let h = self.fresh("h");
        let t = self.fresh("t");
        // From the back, each step peels the last remaining element.
        let (elem, others) = if back {
            let (last, init) = elems.split_last().expect("checked non-empty");
            (last, init)
        } else {
            let (first, tail) = elems.split_first().expect("checked non-empty");
            (first, tail)
        };
        let recur = self.compile_seqview(&t, view, back, others, rest, on_match, on_fail);
        let matched = self.compile_pat(&h, elem, recur, on_fail);
        let body = case1(
            Term::var(&vw),
            Pat::Variant {
                tag: "More".into(),
                fields: vec![Pat::Var(h.clone()), Pat::Var(t.clone())],
            },
            matched,
            on_fail.clone(),
        );
        Term::Let {
            name: vw,
            rec: false,
            val: Arc::new(view_call(view, sv)),
            body: Arc::new(body),
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
            val: Arc::new(Term::Field(Arc::new(Term::var(sv)), names[i].clone())),
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

/// `view sv`: apply the resolved `sequence_view` hook global to the scrutinee.
fn view_call(view: &Term, sv: &str) -> Term {
    Term::app(view.clone(), Term::var(sv))
}

/// `array_len sv`.
fn array_len(sv: &str) -> Term {
    Term::app(Term::var("@array_len"), Term::var(sv))
}

/// `array_slice sv beg end`.
fn array_slice(sv: &str, beg: Term, end: Term) -> Term {
    Term::app(Term::app(Term::app(Term::var("@array_slice"), Term::var(sv)), beg), end)
}

/// `record_without rec "label"`: the record with the head occurrence of `label`
/// dropped. Folded over the matched labels to build a record pattern's `..rest`.
fn record_without(rec: Term, label: &str) -> Term {
    Term::app(
        Term::app(Term::var("record_without"), rec),
        Term::Str(label.as_bytes().to_vec()),
    )
}

/// Tuple field names are their decimal indices.
fn index_names(n: usize) -> Vec<String> {
    (0..n).map(|i| i.to_string()).collect()
}

#[cfg(test)]
mod tests;
