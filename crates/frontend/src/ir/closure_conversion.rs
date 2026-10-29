//! Closure conversion: Core to the IR.
//!
//! Every lambda is lifted to a
//! top-level closed [`Code`], capturing exactly its free variables, and each
//! variable reference is resolved through its De-Bruijn index (assigned by
//! [`crate::passes::de_bruijn`]) to a [`Local`](data::Atom::Local) slot, an
//! [`Env`](data::Atom::Env) capture, or a [`Glob`](data::Atom::Glob).
//!
//! A per-function `FunctionScope` maps each in-scope Core binder (innermost last,
//! parallel to the De-Bruijn stack) to the IR atom that reads it in THIS function,
//! and hands out activation slots stack-disciplined so independent scopes reuse
//! them (`nlocals` is the high-water mark). `collect_free` walks a lifted body
//! with the SAME binder discipline the indexer used, so a De-Bruijn index maps
//! back to an enclosing-scope position.
//!
//! Input: a Core program after the [`crate::passes`]. Output: the IR's codes
//! and globals, before [`super::ownership`] marks moves and drops.
//!
//! Entry point: [`convert_closures`].

use std::collections::BTreeSet;
use std::collections::{HashMap, HashSet};

use super::data::{self, builtin_arity, Alt, AltKind, Atom, Code, IrExpr, HandleClause, ResumeUse};
use crate::core::{CoreClause, CoreHandler, Pat, CoreProgram, Term};

/// Closure-convert a Core program (already match-compiled, A-normal, and De
/// Bruijn indexed) into the IR.
pub fn convert_closures(core: &CoreProgram) -> data::IrProgram {
    let mut conv = ClosureConverter { codes: Vec::new() };
    let mut globals = Vec::new();
    for (name, term) in &core.globals {
        let mut ctx = FunctionScope::new();
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
    let mut codes = conv.codes;
    let names: HashSet<&str> = globals.iter().map(|(n, _)| n.as_str()).collect();
    for code in &mut codes {
        let body = std::mem::replace(&mut code.body, IrExpr::Fault(String::new()));
        code.body = sink_partials(body, &names);
    }
    data::IrProgram {
        codes,
        globals,
        effects: core
            .effects
            .iter()
            .map(|e| data::IrEffect {
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
struct FunctionScope {
    scope: Vec<Option<data::Atom>>,
    next_local: usize,
    nlocals: usize,
    /// Slots below this are never handed out again: a handler body that ran in
    /// them can be resumed after the code following the handler has started.
    floor: usize,
}

impl FunctionScope {
    fn new() -> FunctionScope {
        FunctionScope {
            scope: Vec::new(),
            next_local: 0,
            nlocals: 0,
            floor: 0,
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
        if self.next_local > self.floor {
            self.next_local -= 1;
        }
    }
}

/// Collect the enclosing-scope positions (`< m`) that a lifted body references,
/// so the lifted function captures exactly its free variables. `depth` counts the
/// binders introduced INSIDE the lifted construct; a Var at De-Bruijn index `i`
/// resolves to outer position `(m + depth) - i`, free iff `< m`. The binder
/// discipline must match [`crate::passes::de_bruijn`].
fn collect_free(t: &Term, m: usize, depth: usize, out: &mut BTreeSet<usize>) {
    if let Term::Var { module, idx, .. } = t {
        if module.is_none() && *idx >= 1 {
            let pos = (m + depth) as isize - *idx as isize;
            if pos >= 0 && (pos as usize) < m {
                out.insert(pos as usize);
            }
        }
        return;
    }
    t.for_each_child(&mut |child, bound| collect_free(child, m, depth + bound.len(), out));
}

struct ClosureConverter {
    codes: Vec<Code>,
}

impl ClosureConverter {
    /// Convert a Core atom (Var / literal / lambda).
    fn atom(&mut self, t: &Term, ctx: &mut FunctionScope) -> data::Atom {
        match t {
            Term::Int(n) => data::Atom::LitI(*n),
            Term::Real(r) => data::Atom::LitR(*r),
            Term::Str(s) => data::Atom::LitS(s.clone()),
            Term::Bool(b) => data::Atom::LitB(*b),
            Term::Unit => data::Atom::Unit,
            Term::Var { module, name, idx } => {
                if module.is_some() || *idx == 0 {
                    // Canonicalize to a single name: a
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
            _ => unreachable!("non-atomic CoreProgram term in atom position (ANF violation)"),
        }
    }

    /// Lift a body introducing `nparams` binders (named `params`, innermost last)
    /// into a fresh `Code`, capturing its free enclosing binders, and return the
    /// closure that builds it. Shared by lambdas (1 param), handler clauses (2:
    /// argument, continuation), the value clause (1), and `defer` thunks (0).
    fn lift(&mut self, body: &Term, nparams: usize, params: &[String], outer: &mut FunctionScope) -> data::Atom {
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

        let mut inner = FunctionScope::new();
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

    fn expr(&mut self, t: &Term, ctx: &mut FunctionScope, tail: bool) -> IrExpr {
        match t {
            Term::Int(_)
            | Term::Real(_)
            | Term::Str(_)
            | Term::Bool(_)
            | Term::Unit
            | Term::Var { .. }
            | Term::Extern { .. }
            | Term::Lam { .. } => IrExpr::Ret(self.atom(t, ctx)),

            Term::Fault(s) => IrExpr::Fault(s.clone()),

            Term::App(f, x) => IrExpr::App {
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
                    IrExpr::Let {
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
                    IrExpr::Let {
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
                    None => IrExpr::Fault("no pattern matched (non-exhaustive match)".into()),
                };
                IrExpr::Case {
                    scrut: scrut_atom,
                    alts,
                    default: Box::new(default),
                }
            }

            Term::Tuple(fields) => {
                IrExpr::MkTuple(fields.iter().map(|f| self.atom(f, ctx)).collect())
            }

            Term::Struct { name, base, fields } => IrExpr::MkStruct {
                name: name.clone(),
                base: base.as_ref().map(|b| self.atom(b, ctx)),
                fields: fields
                    .iter()
                    .map(|(n, v)| (n.clone(), self.atom(v, ctx)))
                    .collect(),
            },

            Term::Variant { ty, tag, fields } => self.variant(ty, tag, fields, ctx),

            Term::Field(rec, name) => IrExpr::Field {
                rec: self.atom(rec, ctx),
                name: name.clone(),
            },

            Term::Handle { body, handler } => self.handle(body, handler, ctx),

            Term::Defer { cleanup, body } => {
                let cl = self.lift(cleanup, 0, &[], ctx);
                let b = self.expr(body, ctx, tail);
                IrExpr::Defer {
                    cleanup: cl,
                    body: Box::new(b),
                }
            }
        }
    }

    /// Convert a shallow case alternative, binding its payload to fresh local
    /// slots `[binder_base ..)`.
    fn alt(&mut self, arm: &crate::core::CoreArm, ctx: &mut FunctionScope) -> Alt {
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
                        _ => unreachable!("non-shallow variant field after match compilation"),
                    })
                    .collect(),
            ),
            _ => unreachable!("non-shallow pattern in a case alternative after match compilation"),
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

    /// Convert a variant construction. A computed field is hoisted into a `let`,
    /// its slot reserved before the field converts so a nested hoist gets a
    /// distinct slot. See Note [Constructor fields stay inline in ANF].
    fn variant(&mut self, ty: &str, tag: &str, fields: &[Term], ctx: &mut FunctionScope) -> IrExpr {
        let mut atoms = Vec::new();
        let mut lets: Vec<(usize, IrExpr)> = Vec::new();
        for f in fields {
            if f.is_atom() {
                atoms.push(self.atom(f, ctx));
            } else {
                let slot = ctx.alloc_local();
                let rhs = self.expr(f, ctx, false);
                lets.push((slot, rhs));
                atoms.push(data::Atom::Local(slot));
            }
        }
        let mut res = IrExpr::MkVariant {
            ty: ty.to_string(),
            tag: tag.to_string(),
            fields: atoms,
        };
        for (slot, rhs) in lets.iter().rev() {
            res = IrExpr::Let {
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

    fn handle(&mut self, body: &Term, handler: &CoreHandler, ctx: &mut FunctionScope) -> IrExpr {
        let high_water = ctx.nlocals;
        ctx.nlocals = ctx.next_local;
        let body_e = self.expr(body, ctx, true);
        ctx.floor = ctx.floor.max(ctx.nlocals);
        ctx.next_local = ctx.next_local.max(ctx.floor);
        ctx.nlocals = ctx.nlocals.max(high_water);
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
        IrExpr::Handle {
            body: Box::new(body_e),
            clauses,
            els,
        }
    }

    /// A handler clause `\arg = \k = body` becomes a single 2-parameter code: the
    /// operation argument is `Local 0`, the continuation `Local 1`.
    fn clause(&mut self, c: &CoreClause, continuation: &str, ctx: &mut FunctionScope) -> HandleClause {
        let fun = self.lift(&c.body, 2, &[c.arg.clone(), continuation.to_string()], ctx);
        let resume = match &fun {
            Atom::Clos { code, .. } => resume_use(&self.codes[*code].body),
            _ => ResumeUse::Captured,
        };
        HandleClause {
            effect: c.effect.clone(),
            op: c.op.clone(),
            fun,
            resume,
        }
    }
}

/// Move each partial application of a built-in down to the application that
/// saturates it. A-normal form evaluates `@iadd x (f y)` as `let p = @iadd x in
/// let a = f y in p a`; this gives `let a = f y in let p = @iadd x in p a`, which
/// a backend can run as one direct call. Applying a built-in to fewer operands
/// than its arity does nothing but collect them, so the move changes no
/// behaviour, and `x`'s slots stay bound across the body it moves through.
fn sink_partials(e: IrExpr, globals: &HashSet<&str>) -> IrExpr {
    match e {
        IrExpr::Let { slot, rhs, body } => {
            let rhs = sink_partials(*rhs, globals);
            let body = sink_partials(*body, globals);
            let partial = match &rhs {
                IrExpr::App {
                    fun: Atom::Glob { name },
                    ..
                } => !globals.contains(name.as_str()) && builtin_arity(name).is_some_and(|n| n >= 2),
                _ => false,
            };
            if partial {
                match place_partial(slot, &rhs, body) {
                    Ok(placed) => placed,
                    Err(body) => IrExpr::Let {
                        slot,
                        rhs: Box::new(rhs),
                        body: Box::new(body),
                    },
                }
            } else {
                IrExpr::Let {
                    slot,
                    rhs: Box::new(rhs),
                    body: Box::new(body),
                }
            }
        }
        IrExpr::Case {
            scrut,
            alts,
            default,
        } => IrExpr::Case {
            scrut,
            alts: alts
                .into_iter()
                .map(|a| Alt {
                    body: sink_partials(a.body, globals),
                    ..a
                })
                .collect(),
            default: Box::new(sink_partials(*default, globals)),
        },
        IrExpr::Handle { body, clauses, els } => IrExpr::Handle {
            body: Box::new(sink_partials(*body, globals)),
            clauses,
            els,
        },
        IrExpr::Defer { cleanup, body } => IrExpr::Defer {
            cleanup,
            body: Box::new(sink_partials(*body, globals)),
        },
        e => e,
    }
}

/// Bind `slot` to `rhs` just before the application of it on `body`'s spine of
/// lets, when that is its only use; otherwise hand `body` back unchanged.
fn place_partial(slot: usize, rhs: &IrExpr, body: IrExpr) -> Result<IrExpr, IrExpr> {
    let bind = |at: IrExpr| IrExpr::Let {
        slot,
        rhs: Box::new(rhs.clone()),
        body: Box::new(at),
    };
    let applies = |e: &IrExpr| matches!(e, IrExpr::App { fun: Atom::Local(s), arg, .. } if *s == slot && !arg.mentions(slot));
    match body {
        e if applies(&e) => Ok(bind(e)),
        IrExpr::Let {
            slot: next,
            rhs: next_rhs,
            body: rest,
        } => {
            if applies(&next_rhs) {
                if rest.mentions(slot) {
                    return Err(IrExpr::Let {
                        slot: next,
                        rhs: next_rhs,
                        body: rest,
                    });
                }
                return Ok(bind(IrExpr::Let {
                    slot: next,
                    rhs: next_rhs,
                    body: rest,
                }));
            }
            if next_rhs.mentions(slot) {
                return Err(IrExpr::Let {
                    slot: next,
                    rhs: next_rhs,
                    body: rest,
                });
            }
            match place_partial(slot, rhs, *rest) {
                Ok(placed) => Ok(IrExpr::Let {
                    slot: next,
                    rhs: next_rhs,
                    body: Box::new(placed),
                }),
                Err(rest) => Err(IrExpr::Let {
                    slot: next,
                    rhs: next_rhs,
                    body: Box::new(rest),
                }),
            }
        }
        e => Err(e),
    }
}

/// See Note [Resume uniqueness].
/// Classify a lifted clause body by whether it mentions `k`, which is `Local(1)`
/// there (slot 0 is the operation argument, and the slot allocator hands out 2
/// and up to the body's own binders, so no other binder can alias it). Any
/// mention counts, including a capture or a store: only a clause that cannot
/// reach `k` at all may skip capturing the slice.
fn resume_use(body: &IrExpr) -> ResumeUse {
    if body.mentions(1) {
        ResumeUse::Captured
    } else {
        ResumeUse::Never
    }
}

#[cfg(test)]
mod tests;
