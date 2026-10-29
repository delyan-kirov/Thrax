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

use utilities::{Aol, StrId};

use crate::core::{CoreArm, CoreProgram, FreshNames, Pat, Term, Terms};

/// Compile rich patterns to shallow cases in every global.
pub fn compile_matches(program: &mut CoreProgram) {
    let mut pm = MatchCompiler::new();
    program.rewrite_globals(|terms, term| pm.go(terms, term));
}

struct MatchCompiler {
    names: FreshNames,
}

/// A one-armed case: `when scrut is pat then body else default`.
fn case1(t: &mut Terms, scrut: Aol<Term>, pat: Pat, body: Aol<Term>, default: Aol<Term>) -> Aol<Term> {
    t.add(Term::Case {
        scrut,
        arms: vec![CoreArm {
            pat,
            guard: None,
            body,
        }],
        default: Some(default),
    })
}

/// A one-armed case that succeeds when the boolean `test` holds.
fn when(t: &mut Terms, test: Aol<Term>, body: Aol<Term>, default: Aol<Term>) -> Aol<Term> {
    case1(t, test, Pat::Bool(true), body, default)
}

impl MatchCompiler {
    fn new() -> MatchCompiler {
        MatchCompiler { names: FreshNames::new("%m", 0) }
    }

    fn fresh(&mut self, tag: &str) -> StrId {
        self.names.tagged(tag)
    }

    /// Flatten every `Case` under `term`, rebuilding the tree.
    fn go(&mut self, t: &mut Terms, term: Aol<Term>) -> Aol<Term> {
        let term = t.map_children(term, &mut |t, child, _| self.go(t, child));
        match t.get(term) {
            Term::Case { scrut, arms, default } => {
                let (scrut, arms, default) = (*scrut, arms.clone(), *default);
                self.compile_case(t, scrut, &arms, default)
            }
            _ => term,
        }
    }

    /// Compile one flattened `Case`: bind the scrutinee once, then chain the arms.
    fn compile_case(&mut self, t: &mut Terms, scrut: Aol<Term>, arms: &[CoreArm], default: Option<Aol<Term>>) -> Aol<Term> {
        let s = self.fresh("s");
        let ultimate = match default {
            Some(d) => d,
            None => t.add(Term::Fault("no pattern matched (non-exhaustive match)".into())),
        };
        let chain = self.compile_arms(t, s, arms, 0, ultimate);
        t.let_(s, scrut, chain)
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
    fn compile_arms(&mut self, t: &mut Terms, s: StrId, arms: &[CoreArm], i: usize, fail: Aol<Term>) -> Aol<Term> {
        if i == arms.len() {
            return fail;
        }
        let fail_rest = self.compile_arms(t, s, arms, i + 1, fail);
        let fk = self.fresh("k");
        let fkv = t.var(fk);
        let unit = t.add(Term::Unit);
        let on_fail = t.app(fkv, unit);
        let arm_code = self.compile_arm(t, s, &arms[i], on_fail);
        let thunk = t.lam(StrId::intern("_"), fail_rest);
        t.let_(fk, thunk, arm_code)
    }

    /// Compile a single arm: match its pattern, then run the guard (if any) and
    /// body; any failure goes to `on_fail`.
    fn compile_arm(&mut self, t: &mut Terms, s: StrId, arm: &CoreArm, on_fail: Aol<Term>) -> Aol<Term> {
        let success = match arm.guard {
            Some(g) => when(t, g, arm.body, on_fail),
            None => arm.body,
        };
        self.compile_pat(t, s, &arm.pat, success, on_fail)
    }

    /// Match `pat` against the variable `sv`; on full match run `on_match`, else
    /// `on_fail`.
    fn compile_pat(&mut self, t: &mut Terms, sv: StrId, pat: &Pat, on_match: Aol<Term>, on_fail: Aol<Term>) -> Aol<Term> {
        match pat {
            Pat::Wild => on_match,
            Pat::Var(name) => {
                let val = t.var(sv);
                t.let_(*name, val, on_match)
            }
            Pat::Int(k) => {
                let scrut = t.var(sv);
                case1(t, scrut, Pat::Int(*k), on_match, on_fail)
            }
            Pat::Real(r) => {
                let scrut = t.var(sv);
                case1(t, scrut, Pat::Real(*r), on_match, on_fail)
            }
            Pat::Bool(b) => {
                let scrut = t.var(sv);
                case1(t, scrut, Pat::Bool(*b), on_match, on_fail)
            }
            Pat::Str(bytes) => {
                let scrut = t.var(sv);
                let lit = t.add(Term::Str(*bytes));
                let test = t.bin("==", scrut, lit);
                when(t, test, on_match, on_fail)
            }
            // A literal pattern on a user type: succeed when its equality hook holds
            // between the scrutinee and the literal built into that type.
            Pat::HookEq { eq, value } => {
                let scrut = t.var(sv);
                let partial = t.app(*eq, scrut);
                let value = self.go(t, *value);
                let test = t.app(partial, value);
                when(t, test, on_match, on_fail)
            }
            // A sequence pattern on a user type: unfold the `sequence_view` hook one
            // step at a time, matching each leading element and (`..rest`) binding or
            // (no rest) requiring `Empty` at the end.
            Pat::SeqView { view, back, elems, rest } => {
                self.compile_seqview(t, sv, *view, *back, elems, rest.as_deref(), on_match, on_fail)
            }
            // Lengths were proven by the checker, so each part is read directly.
            Pat::Tensor { len, front, mid, back } => {
                let idx = |t: &mut Terms, i: u64| {
                    let op = t.var("@tensor_index");
                    let scrut = t.var(sv);
                    let partial = t.app(op, scrut);
                    let i = t.add(Term::Int(i as i64));
                    t.app(partial, i)
                };
                let back_start = len - back.len() as u64;
                let mut parts: Vec<(Aol<Term>, &Pat)> = Vec::new();
                for (i, p) in front.iter().enumerate() {
                    parts.push((idx(t, i as u64), p));
                }
                if let Some(mid) = mid {
                    let op = t.var("@tensor_slice");
                    let scrut = t.var(sv);
                    let partial = t.app(op, scrut);
                    let lo = t.add(Term::Int(front.len() as i64));
                    let partial = t.app(partial, lo);
                    let hi = t.add(Term::Int(back_start as i64));
                    parts.push((t.app(partial, hi), mid));
                }
                for (j, p) in back.iter().enumerate() {
                    parts.push((idx(t, back_start + j as u64), p));
                }
                parts.into_iter().rev().fold(on_match, |inner, (val, p)| {
                    let fv = self.fresh("f");
                    let sub = self.compile_pat(t, fv, p, inner, on_fail);
                    t.let_(fv, val, sub)
                })
            }
            // `lo ... hi`: match when `sv >= lo` and `sv <= hi` (inclusive). An open
            // range `lo ...` has no upper bound, so only the `sv >= lo` test.
            Pat::Range { lo, hi } => {
                let inner = match hi {
                    Some(hi) => {
                        let scrut = t.var(sv);
                        let test = t.bin("<=", scrut, *hi);
                        when(t, test, on_match, on_fail)
                    }
                    None => on_match,
                };
                let scrut = t.var(sv);
                let test = t.bin(">=", scrut, *lo);
                when(t, test, inner, on_fail)
            }
            Pat::Tuple(pats) => self.compile_fields(t, sv, &index_names(pats.len()), pats, on_match, on_fail),
            Pat::Struct { fields, rest } => {
                let (names, pats): (Vec<StrId>, Vec<Pat>) =
                    fields.iter().map(|(n, p)| (*n, p.clone())).unzip();
                // `..name` binds the record minus the listed labels: drop each label
                // from `sv` in turn, then match the fields against the bound record.
                let on_match = match rest {
                    Some(name) => {
                        let mut restricted = t.var(sv);
                        for label in &names {
                            restricted = record_without(t, restricted, *label);
                        }
                        t.let_(*name, restricted, on_match)
                    }
                    None => on_match,
                };
                self.compile_fields(t, sv, &names, &pats, on_match, on_fail)
            }
            Pat::Variant { tag, fields } => {
                let binders: Vec<StrId> = (0..fields.len()).map(|_| self.fresh("v")).collect();
                let inner = self.compile_binders(t, &binders, fields, on_match, on_fail);
                let shallow = Pat::Variant {
                    tag: *tag,
                    fields: binders.iter().map(|b| Pat::Var(*b)).collect(),
                };
                let scrut = t.var(sv);
                case1(t, scrut, shallow, inner, on_fail)
            }
            // `back` matches a suffix: the literal is the last `plen` bytes and the
            // rest is everything before it.
            Pat::StrPrefix { prefix, back, rest } => {
                let plen = prefix.bytes().len() as i64;
                let cut = if *back {
                    let len = array_len(t, sv);
                    let p = t.add(Term::Int(plen));
                    t.bin("@isub", len, p)
                } else {
                    t.add(Term::Int(plen))
                };
                let (lit, left) = if *back {
                    let len = array_len(t, sv);
                    let lit = array_slice(t, sv, cut, len);
                    let zero = t.add(Term::Int(0));
                    (lit, array_slice(t, sv, zero, cut))
                } else {
                    let zero = t.add(Term::Int(0));
                    let lit = array_slice(t, sv, zero, cut);
                    let len = array_len(t, sv);
                    (lit, array_slice(t, sv, cut, len))
                };
                let tv = self.fresh("t");
                let tail = self.compile_pat(t, tv, rest, on_match, on_fail);
                let tail_match = t.let_(tv, left, tail);
                let expect = t.add(Term::Str(*prefix));
                let test = t.bin("==", lit, expect);
                let prefix_ok = when(t, test, tail_match, on_fail);
                let len = array_len(t, sv);
                let p = t.add(Term::Int(plen));
                let test = t.bin(">=", len, p);
                when(t, test, prefix_ok, on_fail)
            }
        }
    }

    /// Unfold a `sequence_view` sequence pattern: view the current sequence `sv`,
    /// then for each leading element take a `More head tail` step (matching the
    /// element pattern and recursing on the tail), and finally either bind `..rest`
    /// to the remaining sequence or require it to be `Empty`.
    #[allow(clippy::too_many_arguments)]
    fn compile_seqview(
        &mut self,
        t: &mut Terms,
        sv: StrId,
        view: Aol<Term>,
        back: bool,
        elems: &[Pat],
        rest: Option<&Pat>,
        on_match: Aol<Term>,
        on_fail: Aol<Term>,
    ) -> Aol<Term> {
        if elems.is_empty() {
            return match rest {
                // `..rest` binds the whole remaining sequence.
                Some(pr) => self.compile_pat(t, sv, pr, on_match, on_fail),
                // A fixed-length pattern: the tail must view as `Empty`.
                None => {
                    let vw = self.fresh("vw");
                    let scrut = t.var(vw);
                    let empty = Pat::Variant {
                        tag: StrId::intern("Empty"),
                        fields: Vec::new(),
                    };
                    let body = case1(t, scrut, empty, on_match, on_fail);
                    let call = view_call(t, view, sv);
                    t.let_(vw, call, body)
                }
            };
        }
        let vw = self.fresh("vw");
        let h = self.fresh("h");
        let tl = self.fresh("t");
        // From the back, each step peels the last remaining element.
        let (elem, others) = if back {
            let (last, init) = elems.split_last().expect("checked non-empty");
            (last, init)
        } else {
            let (first, tail) = elems.split_first().expect("checked non-empty");
            (first, tail)
        };
        let recur = self.compile_seqview(t, tl, view, back, others, rest, on_match, on_fail);
        let matched = self.compile_pat(t, h, elem, recur, on_fail);
        let scrut = t.var(vw);
        let more = Pat::Variant {
            tag: StrId::intern("More"),
            fields: vec![Pat::Var(h), Pat::Var(tl)],
        };
        let body = case1(t, scrut, more, matched, on_fail);
        let call = view_call(t, view, sv);
        t.let_(vw, call, body)
    }

    /// Match `pats` against fields of `sv` named `names` (a tuple index or struct
    /// field), each bound to a fresh variable, left-to-right.
    fn compile_fields(
        &mut self,
        t: &mut Terms,
        sv: StrId,
        names: &[StrId],
        pats: &[Pat],
        on_match: Aol<Term>,
        on_fail: Aol<Term>,
    ) -> Aol<Term> {
        self.compile_fields_from(t, sv, names, pats, 0, on_match, on_fail)
    }

    #[allow(clippy::too_many_arguments)]
    fn compile_fields_from(
        &mut self,
        t: &mut Terms,
        sv: StrId,
        names: &[StrId],
        pats: &[Pat],
        i: usize,
        on_match: Aol<Term>,
        on_fail: Aol<Term>,
    ) -> Aol<Term> {
        if i == pats.len() {
            return on_match;
        }
        let fv = self.fresh("f");
        let rest = self.compile_fields_from(t, sv, names, pats, i + 1, on_match, on_fail);
        let sub = self.compile_pat(t, fv, &pats[i], rest, on_fail);
        let record = t.var(sv);
        let val = t.field(record, names[i]);
        t.let_(fv, val, sub)
    }

    /// Match `pats` against already-bound variables `binders`, left-to-right (the
    /// payload of a shallow variant match).
    fn compile_binders(
        &mut self,
        t: &mut Terms,
        binders: &[StrId],
        pats: &[Pat],
        on_match: Aol<Term>,
        on_fail: Aol<Term>,
    ) -> Aol<Term> {
        self.compile_binders_from(t, binders, pats, 0, on_match, on_fail)
    }

    fn compile_binders_from(
        &mut self,
        t: &mut Terms,
        binders: &[StrId],
        pats: &[Pat],
        i: usize,
        on_match: Aol<Term>,
        on_fail: Aol<Term>,
    ) -> Aol<Term> {
        if i == pats.len() {
            return on_match;
        }
        let rest = self.compile_binders_from(t, binders, pats, i + 1, on_match, on_fail);
        self.compile_pat(t, binders[i], &pats[i], rest, on_fail)
    }
}

/// `view sv`: apply the resolved `sequence_view` hook global to the scrutinee.
fn view_call(t: &mut Terms, view: Aol<Term>, sv: StrId) -> Aol<Term> {
    let scrut = t.var(sv);
    t.app(view, scrut)
}

/// `array_len sv`.
fn array_len(t: &mut Terms, sv: StrId) -> Aol<Term> {
    let op = t.var("@array_len");
    let scrut = t.var(sv);
    t.app(op, scrut)
}

/// `array_slice sv beg end`.
fn array_slice(t: &mut Terms, sv: StrId, beg: Aol<Term>, end: Aol<Term>) -> Aol<Term> {
    let op = t.var("@array_slice");
    let scrut = t.var(sv);
    let partial = t.app(op, scrut);
    let partial = t.app(partial, beg);
    t.app(partial, end)
}

/// `record_without rec "label"`: the record with the head occurrence of `label`
/// dropped. Folded over the matched labels to build a record pattern's `..rest`.
fn record_without(t: &mut Terms, rec: Aol<Term>, label: StrId) -> Aol<Term> {
    let op = t.var("record_without");
    let partial = t.app(op, rec);
    let label = t.add(Term::Str(label));
    t.app(partial, label)
}

/// Tuple field names are their decimal indices.
fn index_names(n: usize) -> Vec<StrId> {
    (0..n).map(|i| StrId::intern(&i.to_string())).collect()
}

#[cfg(test)]
mod tests;
