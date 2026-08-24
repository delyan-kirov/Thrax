//! Closure conversion: the port of `compiler/IR.cpp`. Every lambda is lifted to a
//! top-level closed [`Code`], capturing exactly its free variables, and each
//! variable reference is resolved through its De-Bruijn index (assigned by
//! [`crate::lowering::debruijn`]) to a [`Local`](data::Atom::Local) slot, an
//! [`Env`](data::Atom::Env) capture, or a [`Glob`](data::Atom::Glob).
//!
//! A per-function [`Ctx`] maps each in-scope Core binder (innermost last,
//! parallel to the De-Bruijn stack) to the IR atom that reads it in THIS function,
//! and hands out activation slots stack-disciplined so independent scopes reuse
//! them (`nlocals` is the high-water mark). `collect_free` walks a lifted body
//! with the SAME binder discipline the indexer used, so a De-Bruijn index maps
//! back to an enclosing-scope position.

use std::collections::BTreeSet;
use std::collections::HashMap;

use super::data::{self, Alt, AltKind, Code, Expr, HandleClause};
use crate::lowering::data::{Clause, Effect, Handler, Pat, Program as Core, Term};

/// Lower a whole program (root module first) to the IR, the way the C++ pipeline
/// does: give every global the canonical key `Module.name` (the role of MR's
/// name mangling), concatenate the modules into one Core, run the middle-end
/// passes, then closure-convert. A qualified reference resolves to its canonical
/// key; a bare reference resolves through the runtime's bare fallback (see
/// `machine::glob`). Root first, so a bare name collision resolves to the root.
pub fn lower_modules(modules: &[Core]) -> data::Program {
    let mut merged = Core {
        module: modules.first().map(|m| m.module.clone()).unwrap_or_default(),
        effects: Vec::new(),
        globals: Vec::new(),
        crepr_layouts: Vec::new(),
    };
    for m in modules {
        for e in &m.effects {
            merged.effects.push(Effect {
                effect: e.effect.clone(),
                op: e.op.clone(),
            });
        }
        for (name, term) in &m.globals {
            merged
                .globals
                .push((format!("{}.{}", m.module, name), term.clone()));
        }
        for (name, layout) in &m.crepr_layouts {
            if !merged.crepr_layouts.iter().any(|(n, _)| n == name) {
                merged.crepr_layouts.push((name.clone(), layout.clone()));
            }
        }
    }
    crate::lowering::patmat::compile_program(&mut merged);
    crate::lowering::anf::normalize_program(&mut merged);
    crate::lowering::debruijn::assign_program(&mut merged);
    lower(&merged)
}

/// Closure-convert a lowered Core program (already pattern-compiled, A-normal,
/// and De-Bruijn indexed) into the IR.
pub fn lower(core: &Core) -> data::Program {
    let mut conv = Conv { codes: Vec::new() };
    let mut globals = Vec::new();
    for (name, term) in &core.globals {
        let mut ctx = Ctx::new();
        let body = conv.expr(term, &mut ctx, true);
        let id = conv.codes.len();
        conv.codes.push(Code {
            nparams: 0,
            nlocals: ctx.nlocals,
            body,
            name: name.clone(),
        });
        globals.push((name.clone(), id));
    }
    data::Program {
        codes: conv.codes,
        globals,
        effects: core
            .effects
            .iter()
            .map(|e| data::Effect {
                effect: e.effect.clone(),
                op: e.op.clone(),
            })
            .collect(),
        crepr_layouts: core.crepr_layouts.clone(),
    }
}

/// Per-function conversion state: `scope[i]` is the atom that reads the binder at
/// De-Bruijn position `i` (innermost last), or `None` for an enclosing binder
/// this function neither captured nor owns.
struct Ctx {
    scope: Vec<Option<data::Atom>>,
    next_local: usize,
    nlocals: usize,
}

impl Ctx {
    fn new() -> Ctx {
        Ctx {
            scope: Vec::new(),
            next_local: 0,
            nlocals: 0,
        }
    }

    fn alloc_local(&mut self) -> usize {
        let s = self.next_local;
        self.next_local += 1;
        if self.next_local > self.nlocals {
            self.nlocals = self.next_local;
        }
        s
    }

    fn free_local(&mut self) {
        self.next_local -= 1;
    }
}

/// A Core term that closure-converts directly to an atom (rest are computations,
/// hoisted into a `let` to appear in atom position).
fn is_atom(t: &Term) -> bool {
    matches!(
        t,
        Term::Int(_)
            | Term::Real(_)
            | Term::Str(_)
            | Term::Bool(_)
            | Term::Unit
            | Term::Var { .. }
            | Term::Extern { .. }
            | Term::Lam { .. }
    )
}

/// The number of binders a shallow (post-`patmat`) pattern introduces.
fn pat_binders(p: &Pat) -> usize {
    match p {
        Pat::Wild | Pat::Int(_) | Pat::Real(_) | Pat::Str(_) | Pat::Bool(_) => 0,
        Pat::Var(_) => 1,
        Pat::Variant { fields, .. } => fields.len(),
        Pat::Tuple(ps) => ps.iter().map(pat_binders).sum(),
        Pat::Struct { fields, rest } => {
            fields.iter().map(|(_, p)| pat_binders(p)).sum::<usize>() + rest.is_some() as usize
        }
        Pat::StrPrefix { rest, .. } => pat_binders(rest),
        Pat::Range { .. } => 0,
    }
}

/// Collect the enclosing-scope positions (`< m`) that a lifted body references,
/// so the lifted function captures exactly its free variables. `depth` counts the
/// binders introduced INSIDE the lifted construct; a Var at De-Bruijn index `i`
/// resolves to outer position `(m + depth) - i`, free iff `< m`. The binder
/// discipline must match [`crate::lowering::debruijn`].
fn collect_free(t: &Term, m: usize, depth: usize, out: &mut BTreeSet<usize>) {
    match t {
        Term::Var {
            module: None, idx, ..
        } if *idx >= 1 => {
            let pos = (m + depth) as isize - *idx as isize;
            if pos >= 0 && (pos as usize) < m {
                out.insert(pos as usize);
            }
        }
        Term::Var { .. }
        | Term::Int(_)
        | Term::Real(_)
        | Term::Str(_)
        | Term::Bool(_)
        | Term::Unit
        | Term::Extern { .. }
        | Term::Fault(_) => {}

        Term::Lam { body, .. } => collect_free(body, m, depth + 1, out),
        Term::Let { rec, val, body, .. } => {
            collect_free(val, m, if *rec { depth + 1 } else { depth }, out);
            collect_free(body, m, depth + 1, out);
        }
        Term::App(f, x) => {
            collect_free(f, m, depth, out);
            collect_free(x, m, depth, out);
        }
        Term::Case {
            scrut,
            arms,
            default,
        } => {
            collect_free(scrut, m, depth, out);
            for arm in arms.iter() {
                let d = depth + pat_binders(&arm.pat);
                if let Some(g) = &arm.guard {
                    collect_free(g, m, d, out);
                }
                collect_free(&arm.body, m, d, out);
            }
            if let Some(d) = default {
                collect_free(d, m, depth, out);
            }
        }
        Term::Tuple(fields) => {
            for f in fields.iter() {
                collect_free(f, m, depth, out);
            }
        }
        Term::Struct { base, fields, .. } => {
            if let Some(b) = base {
                collect_free(b, m, depth, out);
            }
            for (_, v) in fields.iter() {
                collect_free(v, m, depth, out);
            }
        }
        Term::Variant { fields, .. } => {
            for f in fields.iter() {
                collect_free(f, m, depth, out);
            }
        }
        Term::Field(rec, _) => collect_free(rec, m, depth, out),
        Term::Handle { body, handler } => {
            collect_free(body, m, depth, out);
            for c in &handler.clauses {
                collect_free(&c.body, m, depth + 2, out);
            }
            if let Some((_, b)) = &handler.default {
                collect_free(b, m, depth + 1, out);
            }
        }
        Term::Defer { cleanup, body } => {
            collect_free(cleanup, m, depth, out);
            collect_free(body, m, depth, out);
        }
    }
}

struct Conv {
    codes: Vec<Code>,
}

impl Conv {
    /// Convert a Core atom (Var / literal / lambda).
    fn atom(&mut self, t: &Term, ctx: &mut Ctx) -> data::Atom {
        match t {
            Term::Int(n) => data::Atom::LitI(*n),
            Term::Real(r) => data::Atom::LitR(*r),
            Term::Str(s) => data::Atom::LitS(s.clone()),
            Term::Bool(b) => data::Atom::LitB(*b),
            Term::Unit => data::Atom::Unit,
            Term::Var { module, name, idx } => {
                if module.is_some() || *idx == 0 {
                    // Canonicalize to a single name, as the C++ MR does: a
                    // qualified reference becomes `Module.name`; an unqualified one
                    // stays bare (a builtin, an ambient operation, or an
                    // own-module global resolved by the runtime's bare fallback).
                    let name = match module {
                        Some(m) => format!("{m}.{name}"),
                        None => name.clone(),
                    };
                    data::Atom::Glob { name }
                } else {
                    ctx.scope[ctx.scope.len() - idx]
                        .clone()
                        .expect("a non-captured enclosing binder was referenced")
                }
            }
            Term::Lam { param, body } => self.lift(body, 1, &[param.clone()], ctx),
            Term::Extern {
                abi,
                symbol,
                lib,
                arg_types,
                ret_type,
            } => data::Atom::Extern {
                abi: abi.clone(),
                symbol: symbol.clone(),
                lib: lib.clone(),
                arg_types: arg_types.to_vec(),
                ret_type: ret_type.clone(),
            },
            _ => unreachable!("non-atomic Core term in atom position (ANF violation)"),
        }
    }

    /// Lift a body introducing `nparams` binders (named `params`, innermost last)
    /// into a fresh `Code`, capturing its free enclosing binders, and return the
    /// closure that builds it. Shared by lambdas (1 param), handler clauses (2:
    /// argument, continuation), the value clause (1), and `defer` thunks (0).
    fn lift(&mut self, body: &Term, nparams: usize, params: &[String], outer: &mut Ctx) -> data::Atom {
        let m = outer.scope.len();
        let mut free = BTreeSet::new();
        collect_free(body, m, nparams, &mut free);

        let mut captures = Vec::new();
        let mut env_of: HashMap<usize, usize> = HashMap::new();
        for pos in &free {
            env_of.insert(*pos, captures.len());
            captures.push(
                outer.scope[*pos]
                    .clone()
                    .expect("free position without an atom"),
            );
        }

        let mut inner = Ctx::new();
        inner.scope = vec![None; m + nparams];
        for (pos, slot) in inner.scope.iter_mut().enumerate().take(m) {
            if let Some(&e) = env_of.get(&pos) {
                *slot = Some(data::Atom::Env(e));
            }
        }
        for (i, _) in params.iter().enumerate() {
            inner.scope[m + i] = Some(data::Atom::Local(i));
        }
        inner.next_local = nparams;
        inner.nlocals = nparams;

        let cbody = self.expr(body, &mut inner, true);
        let id = self.codes.len();
        self.codes.push(Code {
            nparams,
            nlocals: inner.nlocals,
            body: cbody,
            name: params.last().cloned().unwrap_or_else(|| "%thunk".into()),
        });
        data::Atom::Clos { code: id, captures }
    }

    fn expr(&mut self, t: &Term, ctx: &mut Ctx, tail: bool) -> Expr {
        match t {
            Term::Int(_)
            | Term::Real(_)
            | Term::Str(_)
            | Term::Bool(_)
            | Term::Unit
            | Term::Var { .. }
            | Term::Extern { .. }
            | Term::Lam { .. } => Expr::Ret(self.atom(t, ctx)),

            Term::Fault(s) => Expr::Fault(s.clone()),

            Term::App(f, x) => Expr::App {
                fun: self.atom(f, ctx),
                arg: self.atom(x, ctx),
                tail,
            },

            Term::Let { rec, val, body, .. } => {
                let slot = ctx.alloc_local();
                if *rec {
                    ctx.scope.push(Some(data::Atom::Local(slot)));
                    let rhs = self.expr(val, ctx, false);
                    let b = self.expr(body, ctx, tail);
                    ctx.scope.pop();
                    ctx.free_local();
                    Expr::Let {
                        slot,
                        rhs: Box::new(rhs),
                        body: Box::new(b),
                    }
                } else {
                    let rhs = self.expr(val, ctx, false);
                    ctx.scope.push(Some(data::Atom::Local(slot)));
                    let b = self.expr(body, ctx, tail);
                    ctx.scope.pop();
                    ctx.free_local();
                    Expr::Let {
                        slot,
                        rhs: Box::new(rhs),
                        body: Box::new(b),
                    }
                }
            }

            Term::Case {
                scrut,
                arms,
                default,
            } => {
                let scrut_atom = self.atom(scrut, ctx);
                let alts = arms.iter().map(|a| self.alt(a, ctx)).collect();
                let default = match default {
                    Some(d) => self.expr(d, ctx, tail),
                    None => Expr::Fault("no pattern matched (non-exhaustive match)".into()),
                };
                Expr::Case {
                    scrut: scrut_atom,
                    alts,
                    default: Box::new(default),
                }
            }

            Term::Tuple(fields) => {
                Expr::MkTuple(fields.iter().map(|f| self.atom(f, ctx)).collect())
            }

            Term::Struct { name, base, fields } => Expr::MkStruct {
                name: name.clone(),
                base: base.as_ref().map(|b| self.atom(b, ctx)),
                fields: fields
                    .iter()
                    .map(|(n, v)| (n.clone(), self.atom(v, ctx)))
                    .collect(),
            },

            Term::Variant { ty, tag, fields } => self.variant(ty, tag, fields, ctx),

            Term::Field(rec, name) => Expr::Field {
                rec: self.atom(rec, ctx),
                name: name.clone(),
            },

            Term::Handle { body, handler } => self.handle(body, handler, ctx),

            Term::Defer { cleanup, body } => {
                let cl = self.lift(cleanup, 0, &[], ctx);
                let b = self.expr(body, ctx, tail);
                Expr::Defer {
                    cleanup: cl,
                    body: Box::new(b),
                }
            }
        }
    }

    /// Convert a shallow case alternative, binding its payload to fresh local
    /// slots `[binder_base ..)`.
    fn alt(&mut self, arm: &crate::lowering::data::Arm, ctx: &mut Ctx) -> Alt {
        let (kind, binder_names): (AltKind, Vec<Option<String>>) = match &arm.pat {
            Pat::Int(k) => (AltKind::Int(*k), Vec::new()),
            Pat::Real(r) => (AltKind::Real(*r), Vec::new()),
            Pat::Bool(b) => (AltKind::Bool(*b), Vec::new()),
            Pat::Variant { tag, fields } => (
                AltKind::Con(tag.clone()),
                fields
                    .iter()
                    .map(|p| match p {
                        Pat::Var(n) => Some(n.clone()),
                        Pat::Wild => None,
                        _ => unreachable!("non-shallow variant field after patmat"),
                    })
                    .collect(),
            ),
            _ => unreachable!("non-shallow pattern in a case alternative after patmat"),
        };

        let binder_base = ctx.next_local;
        for _ in &binder_names {
            let slot = ctx.alloc_local();
            ctx.scope.push(Some(data::Atom::Local(slot)));
        }
        let body = self.expr(&arm.body, ctx, true);
        for _ in &binder_names {
            ctx.scope.pop();
            ctx.free_local();
        }
        Alt {
            kind,
            binder_base,
            binders: binder_names,
            body,
        }
    }

    /// Convert a variant construction. Constructor fields are non-strict in the
    /// Core (kept inline by ANF); under the strict machine a computed field is
    /// hoisted into a `let`, its slot reserved BEFORE the field converts so a
    /// nested hoist gets a distinct slot.
    fn variant(&mut self, ty: &str, tag: &str, fields: &[Term], ctx: &mut Ctx) -> Expr {
        let mut atoms = Vec::new();
        let mut lets: Vec<(usize, Expr)> = Vec::new();
        for f in fields {
            if is_atom(f) {
                atoms.push(self.atom(f, ctx));
            } else {
                let slot = ctx.alloc_local();
                let rhs = self.expr(f, ctx, false);
                lets.push((slot, rhs));
                atoms.push(data::Atom::Local(slot));
            }
        }
        let mut res = Expr::MkVariant {
            ty: ty.to_string(),
            tag: tag.to_string(),
            fields: atoms,
        };
        for (slot, rhs) in lets.iter().rev() {
            res = Expr::Let {
                slot: *slot,
                rhs: Box::new(rhs.clone()),
                body: Box::new(res),
            };
        }
        for _ in &lets {
            ctx.free_local();
        }
        res
    }

    fn handle(&mut self, body: &Term, handler: &Handler, ctx: &mut Ctx) -> Expr {
        let body_e = self.expr(body, ctx, true);
        let clauses = handler
            .clauses
            .iter()
            .map(|c| self.clause(c, &handler.continuation, ctx))
            .collect();
        let els = match &handler.default {
            Some((x, b)) => self.lift(b, 1, &[x.clone()], ctx),
            None => {
                // The identity value clause `\x = x`: its body is a reference to
                // the sole parameter (De-Bruijn index 1).
                let x = Term::Var {
                    module: None,
                    name: "%x".into(),
                    idx: 1,
                };
                self.lift(&x, 1, &["%x".into()], ctx)
            }
        };
        Expr::Handle {
            body: Box::new(body_e),
            clauses,
            els,
        }
    }

    /// A handler clause `\arg = \k = body` becomes a single 2-parameter code: the
    /// operation argument is `Local 0`, the continuation `Local 1`.
    fn clause(&mut self, c: &Clause, continuation: &str, ctx: &mut Ctx) -> HandleClause {
        let fun = self.lift(&c.body, 2, &[c.arg.clone(), continuation.to_string()], ctx);
        HandleClause {
            effect: c.effect.clone(),
            op: c.op.clone(),
            fun,
        }
    }
}

#[cfg(test)]
mod tests;
