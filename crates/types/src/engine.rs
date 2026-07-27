//! The Hindley-Milner inference engine: a union-find over type variables with
//! level-based generalization (Rémy's algorithm).
//!
//! Each variable is `Unbound` at some [`Level`], `Linked` to another type once
//! unified, or `Generic` after generalization (a quantified variable). Entering
//! a `let` right-hand side bumps the current level; on the way out, any variable
//! still `Unbound` at a deeper level is safe to generalize. Unification does an
//! occurs check that simultaneously lowers levels, which is what keeps
//! generalization sound.

use std::collections::{HashMap, HashSet};

use diag::{Code, Diagnostic, Result, Span};

use crate::ty::{display, Level, Type, VarId};

/// The state of one unification variable.
#[derive(Clone, Debug)]
enum Var {
    Unbound { level: Level },
    Linked(Type),
    Generic,
}

/// The mutable inference state shared across a whole program check.
pub struct Engine {
    vars: Vec<Var>,
    level: Level,
}

/// A checkpoint of the [`Engine`] state, taken by [`Engine::save`] and rewound by
/// [`Engine::restore`]. Opaque so callers cannot inspect the private var store.
pub struct Save {
    vars: Vec<Var>,
    level: Level,
}

impl Default for Engine {
    fn default() -> Engine {
        Engine::new()
    }
}

impl Engine {
    pub fn new() -> Engine {
        Engine {
            vars: Vec::new(),
            level: 0,
        }
    }

    // -- levels -------------------------------------------------------------

    /// Enter a deeper `let` scope; variables created here become generalizable
    /// once [`Engine::leave_level`] runs.
    pub fn enter_level(&mut self) {
        self.level += 1;
    }

    pub fn leave_level(&mut self) {
        debug_assert!(self.level > 0, "leave_level without a matching enter_level");
        self.level -= 1;
    }

    /// A fresh unbound variable at the current level.
    pub fn fresh(&mut self) -> Type {
        let id = self.vars.len() as VarId;
        self.vars.push(Var::Unbound { level: self.level });
        Type::Var(id)
    }

    /// A fresh already-`Generic` (quantified) variable. Used to construct the
    /// polymorphic types of built-ins, which are instantiated at each use.
    pub fn fresh_generic(&mut self) -> Type {
        let id = self.vars.len() as VarId;
        self.vars.push(Var::Generic);
        Type::Var(id)
    }

    // -- checkpointing ------------------------------------------------------

    /// Snapshot the whole variable store for trial unification. Restoring undoes
    /// every binding made since the snapshot, so a failed overload attempt leaves
    /// no trace. (A clone of the store is simplest and fine at these sizes; a
    /// change trail would be the optimization.)
    pub fn save(&self) -> Save {
        Save {
            vars: self.vars.clone(),
            level: self.level,
        }
    }

    pub fn restore(&mut self, save: Save) {
        self.vars = save.vars;
        self.level = save.level;
    }

    // -- resolution ---------------------------------------------------------

    /// Follow variable links until the head is a non-linked type (shallow).
    pub fn resolve(&self, ty: &Type) -> Type {
        match ty {
            Type::Var(id) => match &self.vars[*id as usize] {
                Var::Linked(inner) => self.resolve(inner),
                _ => ty.clone(),
            },
            _ => ty.clone(),
        }
    }

    /// Fully resolve a type, replacing every linked variable throughout ("zonk").
    pub fn zonk(&self, ty: &Type) -> Type {
        match self.resolve(ty) {
            Type::App(head, arg) => Type::app(self.zonk(&head), self.zonk(&arg)),
            Type::Arrow(from, to) => Type::arrow(self.zonk(&from), self.zonk(&to)),
            Type::Tuple(items) => Type::Tuple(items.iter().map(|t| self.zonk(t)).collect()),
            other => other, // Var (unbound/generic) or Con
        }
    }

    // -- unification --------------------------------------------------------

    /// Unify two types, mutating variables in place. `where_` names the context
    /// for diagnostics.
    pub fn unify(&mut self, a: &Type, b: &Type, where_: &str) -> Result<()> {
        let a = self.resolve(a);
        let b = self.resolve(b);
        match (&a, &b) {
            (Type::Var(i), Type::Var(j)) if i == j => Ok(()),
            (Type::Var(i), _) => self.bind(*i, &b),
            (_, Type::Var(j)) => self.bind(*j, &a),
            (Type::Con(x), Type::Con(y)) if x == y => Ok(()),
            (Type::Arrow(a1, a2), Type::Arrow(b1, b2)) => {
                self.unify(a1, b1, where_)?;
                self.unify(a2, b2, where_)
            }
            (Type::App(a1, a2), Type::App(b1, b2)) => {
                self.unify(a1, b1, where_)?;
                self.unify(a2, b2, where_)
            }
            (Type::Tuple(xs), Type::Tuple(ys)) if xs.len() == ys.len() => {
                for (x, y) in xs.iter().zip(ys) {
                    self.unify(x, y, where_)?;
                }
                Ok(())
            }
            _ => Err(self.mismatch(&a, &b, where_)),
        }
    }

    /// Point an unbound variable `id` at `ty` after the occurs/level check.
    fn bind(&mut self, id: VarId, ty: &Type) -> Result<()> {
        let level = match self.vars[id as usize] {
            Var::Unbound { level } => level,
            _ => {
                debug_assert!(false, "bind called on a non-unbound variable");
                return Ok(());
            }
        };
        self.occurs_and_adjust(id, level, ty)?;
        self.vars[id as usize] = Var::Linked(ty.clone());
        Ok(())
    }

    /// The occurs check, fused with the level-lowering that generalization
    /// relies on: fail if `id` appears in `ty`, and clamp every unbound variable
    /// inside `ty` to at most `level`.
    fn occurs_and_adjust(&mut self, id: VarId, level: Level, ty: &Type) -> Result<()> {
        match self.resolve(ty) {
            Type::Var(other) => {
                if other == id {
                    return Err(Diagnostic::error(
                        Code::TypeCycle,
                        Span::at(0),
                        0,
                        "cannot construct an infinite type (a variable occurs in its own binding)",
                    ));
                }
                if let Var::Unbound { level: other_level } = &mut self.vars[other as usize] {
                    if *other_level > level {
                        *other_level = level;
                    }
                }
                Ok(())
            }
            Type::Con(_) => Ok(()),
            Type::App(head, arg) | Type::Arrow(head, arg) => {
                self.occurs_and_adjust(id, level, &head)?;
                self.occurs_and_adjust(id, level, &arg)
            }
            Type::Tuple(items) => {
                for item in &items {
                    self.occurs_and_adjust(id, level, item)?;
                }
                Ok(())
            }
        }
    }

    fn mismatch(&self, a: &Type, b: &Type, where_: &str) -> Diagnostic {
        Diagnostic::error(
            Code::TypeMismatch,
            Span::at(0),
            0,
            format!(
                "type mismatch {where_}: expected {}, found {}",
                self.show(a),
                self.show(b)
            ),
        )
    }

    // -- generalization / instantiation ------------------------------------

    /// Generalize a type: every unbound variable deeper than the current level
    /// becomes `Generic` (quantified). Run after leaving a `let` binding's level.
    pub fn generalize(&mut self, ty: &Type) {
        self.generalize_except(ty, &HashSet::new());
    }

    /// Generalize as [`Engine::generalize`], but leave any variable in `mono`
    /// ungeneralized. This is the monomorphism restriction: a variable still
    /// constrained by an unresolved overload must stay a unification variable so
    /// a later use can pin it, rather than becoming spuriously polymorphic.
    pub fn generalize_except(&mut self, ty: &Type, mono: &HashSet<VarId>) {
        match self.resolve(ty) {
            Type::Var(id) => {
                if mono.contains(&id) {
                    return;
                }
                if let Var::Unbound { level } = self.vars[id as usize] {
                    if level > self.level {
                        self.vars[id as usize] = Var::Generic;
                    }
                }
            }
            Type::App(head, arg) | Type::Arrow(head, arg) => {
                self.generalize_except(&head, mono);
                self.generalize_except(&arg, mono);
            }
            Type::Tuple(items) => items.iter().for_each(|t| self.generalize_except(t, mono)),
            Type::Con(_) => {}
        }
    }

    /// Collect the unbound variables reachable from `ty` (after resolution) into
    /// `out`. Used to protect an overload's operands from generalization.
    pub fn collect_vars(&self, ty: &Type, out: &mut HashSet<VarId>) {
        match self.resolve(ty) {
            Type::Var(id) => {
                out.insert(id);
            }
            Type::App(head, arg) | Type::Arrow(head, arg) => {
                self.collect_vars(&head, out);
                self.collect_vars(&arg, out);
            }
            Type::Tuple(items) => items.iter().for_each(|t| self.collect_vars(t, out)),
            Type::Con(_) => {}
        }
    }

    /// Instantiate a (possibly polymorphic) type: replace each `Generic` variable
    /// with a fresh unbound variable, consistently within this one call.
    pub fn instantiate(&mut self, ty: &Type) -> Type {
        let mut mapping = HashMap::new();
        self.instantiate_with(ty, &mut mapping)
    }

    fn instantiate_with(&mut self, ty: &Type, mapping: &mut HashMap<VarId, Type>) -> Type {
        match self.resolve(ty) {
            Type::Var(id) => match self.vars[id as usize] {
                Var::Generic => mapping
                    .entry(id)
                    .or_insert_with(|| self.fresh_raw())
                    .clone(),
                _ => Type::Var(id),
            },
            Type::App(head, arg) => Type::app(
                self.instantiate_with(&head, mapping),
                self.instantiate_with(&arg, mapping),
            ),
            Type::Arrow(from, to) => Type::arrow(
                self.instantiate_with(&from, mapping),
                self.instantiate_with(&to, mapping),
            ),
            Type::Tuple(items) => Type::Tuple(
                items
                    .iter()
                    .map(|t| self.instantiate_with(t, mapping))
                    .collect(),
            ),
            other => other, // Con
        }
    }

    /// A fresh variable that does not borrow `self` twice (used inside closures).
    fn fresh_raw(&mut self) -> Type {
        let id = self.vars.len() as VarId;
        self.vars.push(Var::Unbound { level: self.level });
        Type::Var(id)
    }

    // -- display ------------------------------------------------------------

    /// Render a type with variables named `` `a ``, `` `b ``, ... in order.
    pub fn show(&self, ty: &Type) -> String {
        let zonked = self.zonk(ty);
        let mut names: HashMap<VarId, String> = HashMap::new();
        let mut next = 0u32;
        let mut namer = |id: VarId| {
            names
                .entry(id)
                .or_insert_with(|| {
                    let name = format!("`{}", (b'a' + (next % 26) as u8) as char);
                    next += 1;
                    name
                })
                .clone()
        };
        display(&zonked, &mut namer)
    }
}
