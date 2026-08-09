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

use utilities::{Code, Diagnostic, Result, Span};

use crate::typing::data::{display, Level, Type, VarId};

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
    /// Record-row schemes of the declared structs, by name: `(parameter vars in
    /// declaration order, the row)`. Lets a nominal struct unify with a structural
    /// record row (the hybrid bridge): passing a struct where an open row
    /// `{ x | r }` is expected. A generic struct instance `App(Con("Box"), Int)`
    /// bridges by substituting its arguments for the parameter vars.
    struct_rows: std::collections::HashMap<String, (Vec<VarId>, Type)>,
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
            struct_rows: std::collections::HashMap::new(),
        }
    }

    /// Register the structs' record-row schemes for the nominal-struct / record-row
    /// unification bridge (see [`Engine::struct_rows`]).
    pub fn set_struct_rows(&mut self, rows: std::collections::HashMap<String, (Vec<VarId>, Type)>) {
        self.struct_rows = rows;
    }

    /// Look up field `label` in a record type, growing an open tail to include it.
    pub fn record_field(&mut self, record: &Type, label: &str, where_: &str) -> Result<Type> {
        match self.resolve(record) {
            Type::Record(row) => Ok(self.rewrite_field(&row, label, where_)?.0),
            _ => Ok(self.fresh()),
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
            Type::Arrow(from, to, eff) => {
                Type::arrow_eff(self.zonk(&from), self.zonk(&to), self.zonk(&eff))
            }
            Type::Tuple(items) => Type::Tuple(items.iter().map(|t| self.zonk(t)).collect()),
            Type::RowExtend(label, rest) => Type::RowExtend(label, Box::new(self.zonk(&rest))),
            Type::Record(row) => Type::record(self.zonk(&row)),
            Type::RowField(label, ty, rest) => {
                Type::RowField(label, Box::new(self.zonk(&ty)), Box::new(self.zonk(&rest)))
            }
            other => other, // Var (unbound/generic), Con, or RowEmpty
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
            (Type::Arrow(a1, a2, ae), Type::Arrow(b1, b2, be)) => {
                self.unify(a1, b1, where_)?;
                self.unify(a2, b2, where_)?;
                self.unify(ae, be, where_)
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
            (Type::RowEmpty, Type::RowEmpty) => Ok(()),
            (Type::RowExtend(..), _) | (_, Type::RowExtend(..)) => self.unify_row(&a, &b, where_),
            (Type::Record(ra), Type::Record(rb)) => self.unify_record(ra, rb, where_),
            (Type::RowField(..), _) | (_, Type::RowField(..)) => {
                self.unify_record_row(&a, &b, where_)
            }
            // The hybrid bridge: a nominal struct (bare `Con` or a generic instance
            // `App..(Con)`) satisfies a structural record row by expanding to its
            // row, with its type arguments substituted for the struct's parameters.
            _ => match self.struct_row_bridge(&a, &b, where_) {
                Some(r) => r,
                None => Err(self.mismatch(&a, &b, where_)),
            },
        }
    }

    /// If one side is a record row and the other a struct type (bare `Con` or an
    /// applied `App..(Con)`), unify the record against the struct's registered row
    /// with the struct's type arguments substituted for its parameters. Returns
    /// `None` when neither side bridges, so the caller reports a plain mismatch.
    fn struct_row_bridge(&mut self, a: &Type, b: &Type, where_: &str) -> Option<Result<()>> {
        let (spine, rec) = match (a, b) {
            (Type::Record(r), other) | (other, Type::Record(r)) => (other.clone(), (**r).clone()),
            _ => return None,
        };
        let mut args = Vec::new();
        let mut cur = spine;
        loop {
            match self.resolve(&cur) {
                Type::App(head, arg) => {
                    args.push((*arg).clone());
                    cur = *head;
                }
                Type::Con(name) => {
                    let (params, row) = self.struct_rows.get(&name)?.clone();
                    args.reverse();
                    let mut sub = std::collections::HashMap::new();
                    for (id, arg) in params.iter().zip(args.iter()) {
                        sub.insert(*id, arg.clone());
                    }
                    let row = self.subst_vars(&row, &sub);
                    return Some(self.unify_record_row(&row, &rec, where_));
                }
                _ => return None,
            }
        }
    }

    /// A copy of `ty` with each variable in `sub` replaced by its mapped type. Used
    /// to instantiate a struct-row scheme's parameters at a bridge site; the stored
    /// scheme is never mutated, so each use is independent.
    fn subst_vars(&self, ty: &Type, sub: &std::collections::HashMap<VarId, Type>) -> Type {
        match self.resolve(ty) {
            Type::Var(id) => sub.get(&id).cloned().unwrap_or(Type::Var(id)),
            Type::App(head, arg) => {
                Type::app(self.subst_vars(&head, sub), self.subst_vars(&arg, sub))
            }
            Type::Arrow(from, to, eff) => Type::arrow_eff(
                self.subst_vars(&from, sub),
                self.subst_vars(&to, sub),
                self.subst_vars(&eff, sub),
            ),
            Type::Tuple(items) => {
                Type::Tuple(items.iter().map(|t| self.subst_vars(t, sub)).collect())
            }
            Type::RowExtend(label, rest) => {
                Type::RowExtend(label, Box::new(self.subst_vars(&rest, sub)))
            }
            Type::Record(row) => Type::record(self.subst_vars(&row, sub)),
            Type::RowField(label, fty, rest) => Type::RowField(
                label,
                Box::new(self.subst_vars(&fty, sub)),
                Box::new(self.subst_vars(&rest, sub)),
            ),
            other => other, // Con or RowEmpty
        }
    }

    /// Unify two record types by unifying their rows.
    fn unify_record(&mut self, a: &Type, b: &Type, where_: &str) -> Result<()> {
        self.unify_record_row(a, b, where_)
    }

    /// Unify two record rows (Leijen scoped-label discipline, plus field types):
    /// bring the head field of one row to the head of the other, unify the field
    /// types, then unify the tails. An open tail (a row variable) grows to accept a
    /// missing field, which is where row polymorphism comes from.
    fn unify_record_row(&mut self, a: &Type, b: &Type, where_: &str) -> Result<()> {
        let a = self.resolve(a);
        let b = self.resolve(b);
        match (&a, &b) {
            (Type::RowEmpty, Type::RowEmpty) => Ok(()),
            (Type::Var(i), Type::Var(j)) if i == j => Ok(()),
            (Type::Var(i), _) => self.bind(*i, &b),
            (_, Type::Var(j)) => self.bind(*j, &a),
            (Type::RowField(label, fty, a_rest), _) => {
                let (b_fty, b_rest) = self.rewrite_field(&b, label, where_)?;
                self.unify(fty, &b_fty, where_)?;
                self.unify_record_row(a_rest, &b_rest, where_)
            }
            // The other side ran out of fields but this one still wants `label`.
            (Type::RowEmpty, Type::RowField(label, ..)) => Err(self.field_missing(label, where_)),
            _ => Err(self.mismatch(&a, &b, where_)),
        }
    }

    /// Bring an occurrence of field `label` to the head of record `row`, returning
    /// its field type and the remaining row. An open tail grows to include the
    /// field (fresh field type); a closed row lacking it is a type error.
    fn rewrite_field(&mut self, row: &Type, label: &str, where_: &str) -> Result<(Type, Type)> {
        match self.resolve(row) {
            Type::RowField(l, fty, rest) => {
                if l == label {
                    Ok(((*fty).clone(), (*rest).clone()))
                } else {
                    let (found, deeper) = self.rewrite_field(&rest, label, where_)?;
                    Ok((found, Type::RowField(l, fty, Box::new(deeper))))
                }
            }
            Type::Var(id) => {
                let fty = self.fresh();
                let tail = self.fresh();
                let ext = Type::RowField(label.to_string(), Box::new(fty.clone()), Box::new(tail.clone()));
                self.bind(id, &ext)?;
                Ok((fty, tail))
            }
            _ => Err(self.field_missing(label, where_)),
        }
    }

    fn field_missing(&self, label: &str, where_: &str) -> Diagnostic {
        Diagnostic::error(
            Code::TypeMismatch,
            Span::at(0),
            0,
            format!("record has no field `{label}` {where_}"),
        )
    }

    /// Unify two effect rows, at least one a `<label | rest>` extension (Leijen's
    /// scoped-label discipline): pull the head label of the extension out of the
    /// other row, then unify the remaining tails.
    fn unify_row(&mut self, a: &Type, b: &Type, where_: &str) -> Result<()> {
        // Normalize so `a` is the extension we decompose.
        let (a, b) = if matches!(a, Type::RowExtend(..)) {
            (a, b)
        } else {
            (b, a)
        };
        let Type::RowExtend(label, a_rest) = a else {
            unreachable!("unify_row without a row extension")
        };
        let b_rest = self.rewrite_row(b, label, where_)?;
        self.unify(a_rest, &b_rest, where_)
    }

    /// Bring an occurrence of effect `label` to the head of `row`, returning the
    /// row that remains once it is removed. An open tail (a row variable) grows to
    /// accept the label. A closed row lacking the label is the unhandled-effect
    /// error.
    fn rewrite_row(&mut self, row: &Type, label: &str, where_: &str) -> Result<Type> {
        match self.resolve(row) {
            Type::RowExtend(l, rest) => {
                if l == label {
                    Ok((*rest).clone())
                } else {
                    let deeper = self.rewrite_row(&rest, label, where_)?;
                    Ok(Type::RowExtend(l, Box::new(deeper)))
                }
            }
            Type::Var(id) => {
                let tail = self.fresh();
                let ext = Type::row_extend(label, tail.clone());
                self.bind(id, &ext)?;
                Ok(tail)
            }
            _ => Err(self.effect_not_handled(label, where_)),
        }
    }

    /// Effect subsumption: require `sub` to be a subrow of `super_`. Every effect
    /// the callee performs (`sub`) must be permitted by the ambient (`super_`).
    pub fn subrow(&mut self, sub: &Type, super_: &Type, where_: &str) -> Result<()> {
        match self.resolve(sub) {
            Type::RowEmpty => Ok(()),
            Type::Var(_) => self.unify(sub, super_, where_),
            Type::RowExtend(label, rest) => {
                let super_rest = self.rewrite_row(super_, &label, where_)?;
                self.subrow(&rest, &super_rest, where_)
            }
            other => self.unify(&other, super_, where_),
        }
    }

    fn effect_not_handled(&self, label: &str, where_: &str) -> Diagnostic {
        Diagnostic::error(
            Code::TypeMismatch,
            Span::at(0),
            0,
            format!("effect `{label}` is performed but not handled {where_}"),
        )
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
            Type::Con(_) | Type::RowEmpty => Ok(()),
            Type::App(head, arg) => {
                self.occurs_and_adjust(id, level, &head)?;
                self.occurs_and_adjust(id, level, &arg)
            }
            Type::Arrow(from, to, eff) => {
                self.occurs_and_adjust(id, level, &from)?;
                self.occurs_and_adjust(id, level, &to)?;
                self.occurs_and_adjust(id, level, &eff)
            }
            Type::Tuple(items) => {
                for item in &items {
                    self.occurs_and_adjust(id, level, item)?;
                }
                Ok(())
            }
            Type::RowExtend(_, rest) => self.occurs_and_adjust(id, level, &rest),
            Type::Record(row) => self.occurs_and_adjust(id, level, &row),
            Type::RowField(_, ty, rest) => {
                self.occurs_and_adjust(id, level, &ty)?;
                self.occurs_and_adjust(id, level, &rest)
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
            Type::App(head, arg) => {
                self.generalize_except(&head, mono);
                self.generalize_except(&arg, mono);
            }
            Type::Arrow(from, to, eff) => {
                self.generalize_except(&from, mono);
                self.generalize_except(&to, mono);
                self.generalize_except(&eff, mono);
            }
            Type::Tuple(items) => items.iter().for_each(|t| self.generalize_except(t, mono)),
            Type::RowExtend(_, rest) => self.generalize_except(&rest, mono),
            Type::Record(row) => self.generalize_except(&row, mono),
            Type::RowField(_, ty, rest) => {
                self.generalize_except(&ty, mono);
                self.generalize_except(&rest, mono);
            }
            Type::Con(_) | Type::RowEmpty => {}
        }
    }

    /// Collect the unbound variables reachable from `ty` (after resolution) into
    /// `out`. Used to protect an overload's operands from generalization.
    pub fn collect_vars(&self, ty: &Type, out: &mut HashSet<VarId>) {
        match self.resolve(ty) {
            Type::Var(id) => {
                out.insert(id);
            }
            Type::App(head, arg) => {
                self.collect_vars(&head, out);
                self.collect_vars(&arg, out);
            }
            Type::Arrow(from, to, eff) => {
                self.collect_vars(&from, out);
                self.collect_vars(&to, out);
                self.collect_vars(&eff, out);
            }
            Type::Tuple(items) => items.iter().for_each(|t| self.collect_vars(t, out)),
            Type::RowExtend(_, rest) => self.collect_vars(&rest, out),
            Type::Record(row) => self.collect_vars(&row, out),
            Type::RowField(_, ty, rest) => {
                self.collect_vars(&ty, out);
                self.collect_vars(&rest, out);
            }
            Type::Con(_) | Type::RowEmpty => {}
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
            Type::Arrow(from, to, eff) => Type::arrow_eff(
                self.instantiate_with(&from, mapping),
                self.instantiate_with(&to, mapping),
                self.instantiate_with(&eff, mapping),
            ),
            Type::Tuple(items) => Type::Tuple(
                items
                    .iter()
                    .map(|t| self.instantiate_with(t, mapping))
                    .collect(),
            ),
            Type::RowExtend(label, rest) => {
                Type::RowExtend(label, Box::new(self.instantiate_with(&rest, mapping)))
            }
            Type::Record(row) => Type::record(self.instantiate_with(&row, mapping)),
            Type::RowField(label, ty, rest) => Type::RowField(
                label,
                Box::new(self.instantiate_with(&ty, mapping)),
                Box::new(self.instantiate_with(&rest, mapping)),
            ),
            other => other, // Con or RowEmpty
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
                    // Type variables display lowercase (`a`), matching the source
                    // syntax where a lowercase name in type position is a variable.
                    let name = ((b'a' + (next % 26) as u8) as char).to_string();
                    next += 1;
                    name
                })
                .clone()
        };
        display(&zonked, &mut namer)
    }
}
