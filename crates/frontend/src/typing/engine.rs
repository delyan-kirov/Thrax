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

use crate::typing::data::{display, Level, Type, TypeNode, Types, VarId};
use std::rc::Rc;

/// The state of one unification variable.
#[derive(Clone, Debug)]
enum Var {
    Unbound { level: Level },
    Linked(Type),
    Generic,
}

/// The mutable inference state shared across a whole program check.
pub struct Engine {
    /// Every type node, and the interner for constructor and label names. Shared
    /// by every module's checker in one compilation: a `Type` is a handle into
    /// this store, so a type crossing a module boundary must address the same one.
    pub types: Rc<Types>,
    vars: Vec<Var>,
    level: Level,
    /// Record-row schemes of the declared structs, by name: `(parameter vars in
    /// declaration order, the row)`. Lets a nominal struct unify with a structural
    /// record row (the hybrid bridge): passing a struct where an open row
    /// `{ x | r }` is expected. A generic struct instance `App(Con("Box"), Int)`
    /// bridges by substituting its arguments for the parameter vars.
    struct_rows: std::collections::HashMap<String, (Vec<VarId>, Type)>,
    /// Variables of the `Nat` kind (a size in `[n]T`). A `Nat` variable unifies
    /// only with another `Nat` variable or a `TypeNode::Nat`; kind mismatches are
    /// rejected in [`Engine::bind`]. Snapshotted by save/restore so a rolled-back
    /// trial cannot leave a stale id that a reused var slot would inherit.
    nat_vars: HashSet<VarId>,
    /// The undo log behind [`Engine::save`] / [`Engine::restore`].
    trail: Vec<Undo>,
}

/// One reversible mutation, recorded so [`Engine::restore`] can rewind a trial.
enum Undo {
    /// Put this var slot back to the value it held.
    Var(VarId, Var),
    /// Drop this id from the `Nat`-kinded set again.
    NatVar(VarId),
}

/// A checkpoint of the [`Engine`] state, taken by [`Engine::save`] and rewound by
/// [`Engine::restore`]. A mark into the undo trail, not a copy of the var store:
/// overload resolution takes one per candidate, and copying the store made that
/// cost grow with every variable the program had introduced so far.
pub struct Save {
    trail: usize,
    vars: usize,
    level: Level,
}

impl Engine {
    pub fn new(types: Rc<Types>) -> Engine {
        Engine {
            types,
            vars: Vec::new(),
            level: 0,
            struct_rows: std::collections::HashMap::new(),
            nat_vars: HashSet::new(),
            trail: Vec::new(),
        }
    }

    /// A fresh unbound variable of the `Nat` kind (a tensor size).
    pub fn fresh_nat(&mut self) -> Type {
        let ty = self.fresh();
        if let TypeNode::Var(id) = self.types.node(ty) {
            self.mark_nat(id);
        }
        ty
    }

    /// A fresh `Generic` `Nat` variable, for the scheme of a size-polymorphic
    /// built-in (instantiated, staying Nat-kinded, at each use).
    pub fn fresh_generic_nat(&mut self) -> Type {
        let ty = self.fresh_generic();
        if let TypeNode::Var(id) = self.types.node(ty) {
            self.mark_nat(id);
        }
        ty
    }

    fn is_nat_var(&self, id: VarId) -> bool {
        self.nat_vars.contains(&id)
    }

    /// Mark every variable in a size expression (a `Nat` position) as `Nat`-kinded.
    fn note_nat(&mut self, ty: Type) {
        match self.head(ty) {
            TypeNode::Var(id) => {
                self.mark_nat(id);
            }
            TypeNode::NatAdd(a, b) | TypeNode::NatMul(a, b) => {
                self.note_nat(a);
                self.note_nat(b);
            }
            _ => {}
        }
    }

    /// Walk a type and mark every tensor-size variable as `Nat`. Kind is not carried
    /// in a `TypeNode::Var`, so a scheme imported across modules (fresh plain generics)
    /// must be re-annotated from its structure, else a size var unifies as a type.
    pub fn note_tensor_sizes(&mut self, ty: Type) {
        match self.head(ty) {
            TypeNode::App(head, elem) => {
                if let Some((_variance, size, _elem)) = self.tensor_spine(ty) {
                    self.note_nat(size);
                }
                self.note_tensor_sizes(head);
                self.note_tensor_sizes(elem);
            }
            TypeNode::Arrow(from, to, eff) => {
                self.note_tensor_sizes(from);
                self.note_tensor_sizes(to);
                self.note_tensor_sizes(eff);
            }
            TypeNode::Tuple(items) => {
                for t in self.types.items(items).to_vec() {
                    self.note_tensor_sizes(t);
                }
            }
            TypeNode::Record(row) => self.note_tensor_sizes(row),
            TypeNode::RowField(_, t, rest) => {
                self.note_tensor_sizes(t);
                self.note_tensor_sizes(rest);
            }
            _ => {}
        }
    }

    /// Register the structs' record-row schemes for the nominal-struct / record-row
    /// unification bridge (see the `struct_rows` map).
    pub fn set_struct_rows(&mut self, rows: std::collections::HashMap<String, (Vec<VarId>, Type)>) {
        self.struct_rows = rows;
    }

    /// Look up field `label` in a record type, growing an open tail to include it.
    pub fn record_field(&mut self, record: Type, label: &str, where_: &str) -> Result<Type> {
        match self.head(record) {
            TypeNode::Record(row) => Ok(self.rewrite_field(row, label, where_)?.0),
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

    /// Overwrite a var slot, recording the old value so a trial can be rewound.
    fn set_var(&mut self, id: VarId, v: Var) {
        let old = std::mem::replace(&mut self.vars[id as usize], v);
        self.trail.push(Undo::Var(id, old));
    }

    /// Mark a variable `Nat`-kinded, recording it if this is the first time.
    fn mark_nat(&mut self, id: VarId) {
        if self.nat_vars.insert(id) {
            self.trail.push(Undo::NatVar(id));
        }
    }

    /// A fresh unbound variable at the current level.
    pub fn fresh(&mut self) -> Type {
        let id = self.vars.len() as VarId;
        self.vars.push(Var::Unbound { level: self.level });
        self.types.add(TypeNode::Var(id))
    }

    /// A fresh already-`Generic` (quantified) variable. Used to construct the
    /// polymorphic types of built-ins, which are instantiated at each use.
    pub fn fresh_generic(&mut self) -> Type {
        let id = self.vars.len() as VarId;
        self.vars.push(Var::Generic);
        self.types.add(TypeNode::Var(id))
    }

    // -- checkpointing ------------------------------------------------------

    /// Snapshot the whole variable store for trial unification. Restoring undoes
    /// every binding made since the snapshot, so a failed overload attempt leaves
    /// no trace. (A clone of the store is simplest and fine at these sizes; a
    /// change trail would be the optimization.)
    pub fn save(&self) -> Save {
        Save {
            trail: self.trail.len(),
            vars: self.vars.len(),
            level: self.level,
        }
    }

    /// Rewind every mutation made since `save`. Undoing in reverse order matters:
    /// one slot may have been written more than once.
    pub fn restore(&mut self, save: Save) {
        while self.trail.len() > save.trail {
            match self.trail.pop().expect("length checked") {
                Undo::Var(id, old) => self.vars[id as usize] = old,
                Undo::NatVar(id) => {
                    self.nat_vars.remove(&id);
                }
            }
        }
        self.vars.truncate(save.vars);
        self.level = save.level;
    }

    // -- resolution ---------------------------------------------------------

    /// Follow variable links until the head is a non-linked type (shallow).
    pub fn resolve(&self, ty: Type) -> Type {
        match self.types.node(ty) {
            TypeNode::Var(id) => match self.vars[id as usize] {
                Var::Linked(inner) => self.resolve(inner),
                _ => ty,
            },
            _ => ty,
        }
    }

    /// The node a type resolves to: [`Engine::resolve`] then a store read. What
    /// almost every caller wants, since they match on the head constructor.
    pub fn head(&self, ty: Type) -> TypeNode {
        self.types.node(self.resolve(ty))
    }

    /// Fully resolve a type, replacing every linked variable throughout ("zonk").
    pub fn zonk(&self, ty: Type) -> Type {
        match self.head(ty) {
            TypeNode::App(head, arg) => {
                let (h, a) = (self.zonk(head), self.zonk(arg));
                self.types.app(h, a)
            }
            TypeNode::NatAdd(a, b) => {
                let (x, y) = (self.zonk(a), self.zonk(b));
                self.types.add(TypeNode::NatAdd(x, y))
            }
            TypeNode::NatMul(a, b) => {
                let (x, y) = (self.zonk(a), self.zonk(b));
                self.types.add(TypeNode::NatMul(x, y))
            }
            TypeNode::Arrow(from, to, eff) => {
                let (f, t, e) = (self.zonk(from), self.zonk(to), self.zonk(eff));
                self.types.arrow_eff(f, t, e)
            }
            TypeNode::Tuple(items) => {
                let items = self.types.items(items).to_vec();
                let zonked: Vec<Type> = items.into_iter().map(|t| self.zonk(t)).collect();
                self.types.tuple(zonked)
            }
            TypeNode::RowExtend(label, rest) => {
                let r = self.zonk(rest);
                self.types.add(TypeNode::RowExtend(label, r))
            }
            TypeNode::Record(row) => {
                let r = self.zonk(row);
                self.types.record(r)
            }
            TypeNode::RowField(label, ty, rest) => {
                let (f, r) = (self.zonk(ty), self.zonk(rest));
                self.types.add(TypeNode::RowField(label, f, r))
            }
            // Var (unbound/generic), Con, or RowEmpty: already its own resolution.
            _ => self.resolve(ty),
        }
    }

    // -- unification --------------------------------------------------------

    /// Unify two types, mutating variables in place. `where_` names the context
    /// for diagnostics.
    pub fn unify(&mut self, a: Type, b: Type, where_: &str) -> Result<()> {
        let a = self.resolve(a);
        let b = self.resolve(b);
        match (self.types.node(a), self.types.node(b)) {
            (TypeNode::Var(i), TypeNode::Var(j)) if i == j => Ok(()),
            (TypeNode::Var(i), _) => self.bind(i, b),
            (_, TypeNode::Var(j)) => self.bind(j, a),
            (TypeNode::Con(x), TypeNode::Con(y)) if x == y => Ok(()),
            // Sizes (Nat literals and `+`/`*` expressions) unify by their canonical
            // polynomial (a bare Nat variable was already handled by the Var arms).
            _ if self.is_size(a) || self.is_size(b) => self.unify_size(a, b, where_),
            (TypeNode::Arrow(a1, a2, ae), TypeNode::Arrow(b1, b2, be)) => {
                self.unify(a1, b1, where_)?;
                self.unify(a2, b2, where_)?;
                self.unify(ae, be, where_)
            }
            // Sized tensors carry a per-axis variance in their spine, unified by the
            // compatibility rule (Neutral is a wildcard, Co and Contra clash) rather
            // than plain structural App equality.
            _ if self.tensor_spine(a).is_some() && self.tensor_spine(b).is_some() => {
                let (va, sa, ea) = self.tensor_spine(a).expect("checked");
                let (vb, sb, eb) = self.tensor_spine(b).expect("checked");
                self.unify_variance(va, vb, where_)?;
                self.unify(sa, sb, where_)?;
                self.unify(ea, eb, where_)
            }
            (TypeNode::App(a1, a2), TypeNode::App(b1, b2)) => {
                self.unify(a1, b1, where_)?;
                self.unify(a2, b2, where_)
            }
            (TypeNode::Tuple(xs), TypeNode::Tuple(ys)) if xs.len() == ys.len() => {
                let pairs: Vec<(Type, Type)> = self
                    .types
                    .items(xs)
                    .iter()
                    .copied()
                    .zip(self.types.items(ys).iter().copied())
                    .collect();
                for (x, y) in pairs {
                    self.unify(x, y, where_)?;
                }
                Ok(())
            }
            (TypeNode::RowEmpty, TypeNode::RowEmpty) => Ok(()),
            (TypeNode::RowExtend(..), _) | (_, TypeNode::RowExtend(..)) => {
                self.unify_row(a, b, where_)
            }
            (TypeNode::Record(ra), TypeNode::Record(rb)) => self.unify_record(ra, rb, where_),
            (TypeNode::RowField(..), _) | (_, TypeNode::RowField(..)) => {
                self.unify_record_row(a, b, where_)
            }
            // The hybrid bridge: a nominal struct (bare `Con` or a generic instance
            // `App..(Con)`) satisfies a structural record row by expanding to its
            // row, with its type arguments substituted for the struct's parameters.
            _ => match self.struct_row_bridge(a, b, where_) {
                Some(r) => r,
                None => Err(self.mismatch(a, b, where_)),
            },
        }
    }

    /// Peel a sized-tensor type `@tensor variance size elem` into its three parts.
    fn tensor_spine(&self, ty: Type) -> Option<(Type, Type, Type)> {
        if let TypeNode::App(head, elem) = self.head(ty) {
            if let TypeNode::App(head2, size) = self.head(head) {
                if let TypeNode::App(con, variance) = self.head(head2) {
                    if matches!(self.head(con), TypeNode::Con(n) if self.types.name(n) == "@tensor")
                    {
                        return Some((variance, size, elem));
                    }
                }
            }
        }
        None
    }

    /// Unify two axis variances. A variance variable binds like any unification
    /// variable; `@neutral` is compatible with any concrete variance (so unmarked
    /// `[n]T` axes interoperate with variance-typed tensors); `@co` and `@contra`
    /// unify only with themselves, so mixing them is the type error variance exists
    /// to catch.
    fn unify_variance(&mut self, a: Type, b: Type, where_: &str) -> Result<()> {
        let a = self.resolve(a);
        let b = self.resolve(b);
        match (self.types.node(a), self.types.node(b)) {
            (TypeNode::Var(i), TypeNode::Var(j)) if i == j => Ok(()),
            (TypeNode::Var(i), _) => self.bind(i, b),
            (_, TypeNode::Var(j)) => self.bind(j, a),
            (TypeNode::Con(n), _) | (_, TypeNode::Con(n))
                if self.types.name(n) == "@neutral" =>
            {
                Ok(())
            }
            (TypeNode::Con(x), TypeNode::Con(y)) if x == y => Ok(()),
            _ => Err(self.mismatch(a, b, where_)),
        }
    }

    /// Is `ty` a type-level size (a `Nat` literal or a `+`/`*` expression)? A bare
    /// size *variable* is handled by unify's `Var` arms, so it is not needed here.
    fn is_size(&self, ty: Type) -> bool {
        matches!(
            self.head(ty),
            TypeNode::Nat(_) | TypeNode::NatAdd(..) | TypeNode::NatMul(..)
        )
    }

    /// Unify two sizes by their canonical polynomial over Z/2^64. Equal polynomials
    /// unify; otherwise, if one whole side is a lone unbound variable not occurring
    /// in the other, bind it (the forward-eval rule). No back-solving of embedded
    /// variables (e.g. `n + 1 == 5` is not solved), which keeps this decidable.
    fn unify_size(&mut self, a: Type, b: Type, where_: &str) -> Result<()> {
        let pa = self.normalize_size(a);
        let pb = self.normalize_size(b);
        if pa == pb {
            return Ok(());
        }
        if let Some(v) = lone_var(&pa) {
            if !poly_has_var(&pb, v) {
                return self.bind(v, b);
            }
        }
        if let Some(v) = lone_var(&pb) {
            if !poly_has_var(&pa, v) {
                return self.bind(v, a);
            }
        }
        Err(Diagnostic::error(
            Code::TypeMismatch,
            Span::at(0),
            0,
            format!(
                "size mismatch {where_}: cannot unify `{}` with `{}`",
                show_poly(&pa),
                show_poly(&pb)
            ),
        ))
    }

    /// Normalize a size to a canonical polynomial over Z/2^64: a sorted list of
    /// `(coefficient, sorted variable monomial)` terms, like terms combined, zero
    /// terms dropped. Bound variables are followed, so equality is decidable and
    /// `n + m` and `m + n` share a normal form.
    fn normalize_size(&self, ty: Type) -> Poly {
        match self.head(ty) {
            TypeNode::Nat(k) => canon(vec![(k, vec![])]),
            TypeNode::Var(id) => vec![(1, vec![id])],
            TypeNode::NatAdd(a, b) => {
                let mut t = self.normalize_size(a);
                t.extend(self.normalize_size(b));
                canon(t)
            }
            TypeNode::NatMul(a, b) => poly_mul(&self.normalize_size(a), &self.normalize_size(b)),
            _ => Vec::new(),
        }
    }

    /// If one side is a record row and the other a struct type (bare `Con` or an
    /// applied `App..(Con)`), unify the record against the struct's registered row
    /// with the struct's type arguments substituted for its parameters. Returns
    /// `None` when neither side bridges, so the caller reports a plain mismatch.
    fn struct_row_bridge(&mut self, a: Type, b: Type, where_: &str) -> Option<Result<()>> {
        let (spine, rec) = match (self.types.node(a), self.types.node(b)) {
            (TypeNode::Record(r), _) => (b, r),
            (_, TypeNode::Record(r)) => (a, r),
            _ => return None,
        };
        let mut args = Vec::new();
        let mut cur = spine;
        loop {
            match self.head(cur) {
                TypeNode::App(head, arg) => {
                    args.push(arg);
                    cur = head;
                }
                TypeNode::Con(name) => {
                    let name = self.types.name(name).to_string();
                    let (params, row) = self.struct_rows.get(&name)?.clone();
                    args.reverse();
                    let mut sub = std::collections::HashMap::new();
                    for (id, arg) in params.iter().zip(args.iter()) {
                        sub.insert(*id, *arg);
                    }
                    let row = self.subst_vars(row, &sub);
                    return Some(self.unify_record_row(row, rec, where_));
                }
                _ => return None,
            }
        }
    }

    /// A copy of `ty` with each variable in `sub` replaced by its mapped type. Used
    /// to instantiate a struct-row scheme's parameters at a bridge site; the stored
    /// scheme is never mutated, so each use is independent.
    fn subst_vars(&self, ty: Type, sub: &std::collections::HashMap<VarId, Type>) -> Type {
        match self.head(ty) {
            TypeNode::Var(id) => match sub.get(&id) {
                Some(t) => *t,
                None => self.types.add(TypeNode::Var(id)),
            },
            TypeNode::App(head, arg) => {
                let (h, a) = (self.subst_vars(head, sub), self.subst_vars(arg, sub));
                self.types.app(h, a)
            }
            TypeNode::NatAdd(a, b) => {
                let (x, y) = (self.subst_vars(a, sub), self.subst_vars(b, sub));
                self.types.add(TypeNode::NatAdd(x, y))
            }
            TypeNode::NatMul(a, b) => {
                let (x, y) = (self.subst_vars(a, sub), self.subst_vars(b, sub));
                self.types.add(TypeNode::NatMul(x, y))
            }
            TypeNode::Arrow(from, to, eff) => {
                let (f, t, e) = (
                    self.subst_vars(from, sub),
                    self.subst_vars(to, sub),
                    self.subst_vars(eff, sub),
                );
                self.types.arrow_eff(f, t, e)
            }
            TypeNode::Tuple(items) => {
                let items = self.types.items(items).to_vec();
                let mapped: Vec<Type> = items.into_iter().map(|t| self.subst_vars(t, sub)).collect();
                self.types.tuple(mapped)
            }
            TypeNode::RowExtend(label, rest) => {
                let r = self.subst_vars(rest, sub);
                self.types.add(TypeNode::RowExtend(label, r))
            }
            TypeNode::Record(row) => {
                let r = self.subst_vars(row, sub);
                self.types.record(r)
            }
            TypeNode::RowField(label, fty, rest) => {
                let (f, r) = (self.subst_vars(fty, sub), self.subst_vars(rest, sub));
                self.types.add(TypeNode::RowField(label, f, r))
            }
            _ => self.resolve(ty), // Con or RowEmpty
        }
    }

    /// Unify two record types by unifying their rows.
    fn unify_record(&mut self, a: Type, b: Type, where_: &str) -> Result<()> {
        self.unify_record_row(a, b, where_)
    }

    /// Unify two record rows (Leijen scoped-label discipline, plus field types):
    /// bring the head field of one row to the head of the other, unify the field
    /// types, then unify the tails. An open tail (a row variable) grows to accept a
    /// missing field, which is where row polymorphism comes from.
    fn unify_record_row(&mut self, a: Type, b: Type, where_: &str) -> Result<()> {
        let a = self.resolve(a);
        let b = self.resolve(b);
        match (self.types.node(a), self.types.node(b)) {
            (TypeNode::RowEmpty, TypeNode::RowEmpty) => Ok(()),
            (TypeNode::Var(i), TypeNode::Var(j)) if i == j => Ok(()),
            (TypeNode::Var(i), _) => self.bind(i, b),
            (_, TypeNode::Var(j)) => self.bind(j, a),
            (TypeNode::RowField(label, fty, a_rest), _) => {
                let label = self.types.name(label).to_string();
                let (b_fty, b_rest) = self.rewrite_field(b, &label, where_)?;
                self.unify(fty, b_fty, where_)?;
                self.unify_record_row(a_rest, b_rest, where_)
            }
            // The other side ran out of fields but this one still wants `label`.
            (TypeNode::RowEmpty, TypeNode::RowField(label, ..)) => {
                let label = self.types.name(label).to_string();
                Err(self.field_missing(&label, where_))
            }
            _ => Err(self.mismatch(a, b, where_)),
        }
    }

    /// Bring an occurrence of field `label` to the head of record `row`, returning
    /// its field type and the remaining row. An open tail grows to include the
    /// field (fresh field type); a closed row lacking it is a type error.
    fn rewrite_field(&mut self, row: Type, label: &str, where_: &str) -> Result<(Type, Type)> {
        match self.head(row) {
            TypeNode::RowField(l, fty, rest) => {
                if self.types.name(l) == label {
                    Ok((fty, rest))
                } else {
                    let (found, deeper) = self.rewrite_field(rest, label, where_)?;
                    let rebuilt = self.types.add(TypeNode::RowField(l, fty, deeper));
                    Ok((found, rebuilt))
                }
            }
            TypeNode::Var(id) => {
                let fty = self.fresh();
                let tail = self.fresh();
                let ext = self.types.row_field(label, fty, tail);
                self.bind(id, ext)?;
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
    fn unify_row(&mut self, a: Type, b: Type, where_: &str) -> Result<()> {
        // Normalize so `a` is the extension we decompose.
        let (a, b) = if matches!(self.types.node(a), TypeNode::RowExtend(..)) {
            (a, b)
        } else {
            (b, a)
        };
        let TypeNode::RowExtend(label, a_rest) = self.types.node(a) else {
            unreachable!("unify_row without a row extension")
        };
        let label = self.types.name(label).to_string();
        let b_rest = self.rewrite_row(b, &label, where_)?;
        self.unify(a_rest, b_rest, where_)
    }

    /// Bring an occurrence of effect `label` to the head of `row`, returning the
    /// row that remains once it is removed. An open tail (a row variable) grows to
    /// accept the label. A closed row lacking the label is the unhandled-effect
    /// error.
    fn rewrite_row(&mut self, row: Type, label: &str, where_: &str) -> Result<Type> {
        match self.head(row) {
            TypeNode::RowExtend(l, rest) => {
                if self.types.name(l) == label {
                    Ok(rest)
                } else {
                    let deeper = self.rewrite_row(rest, label, where_)?;
                    Ok(self.types.add(TypeNode::RowExtend(l, deeper)))
                }
            }
            TypeNode::Var(id) => {
                let tail = self.fresh();
                let ext = self.types.row_extend(label, tail);
                self.bind(id, ext)?;
                Ok(tail)
            }
            _ => Err(self.effect_not_handled(label, where_)),
        }
    }

    /// Effect subsumption: require `sub` to be a subrow of `super_`. Every effect
    /// the callee performs (`sub`) must be permitted by the ambient (`super_`).
    pub fn subrow(&mut self, sub: Type, super_: Type, where_: &str) -> Result<()> {
        match self.head(sub) {
            TypeNode::RowEmpty => Ok(()),
            TypeNode::Var(_) => self.unify(sub, super_, where_),
            TypeNode::RowExtend(label, rest) => {
                let label = self.types.name(label).to_string();
                let super_rest = self.rewrite_row(super_, &label, where_)?;
                self.subrow(rest, super_rest, where_)
            }
            _ => self.unify(sub, super_, where_),
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
    fn bind(&mut self, id: VarId, ty: Type) -> Result<()> {
        let level = match self.vars[id as usize] {
            Var::Unbound { level } => level,
            _ => {
                debug_assert!(false, "bind called on a non-unbound variable");
                return Ok(());
            }
        };
        self.check_kind(id, ty)?;
        self.occurs_and_adjust(id, level, ty)?;
        self.set_var(id, Var::Linked(ty));
        Ok(())
    }

    /// A `Nat`-kinded variable may bind only to a `Nat` literal or another
    /// `Nat`-kinded variable; a `Type` variable may not bind to a `Nat`. Binding a
    /// `Nat` variable to a plain variable makes that variable `Nat` too.
    fn check_kind(&mut self, id: VarId, ty: Type) -> Result<()> {
        let want_nat = self.is_nat_var(id);
        match self.head(ty) {
            TypeNode::Nat(_) | TypeNode::NatAdd(..) | TypeNode::NatMul(..) => {
                if !want_nat {
                    return Err(self.kind_mismatch(true));
                }
            }
            TypeNode::Var(other) => {
                let other_nat = self.is_nat_var(other);
                if want_nat && !other_nat {
                    self.mark_nat(other);
                } else if !want_nat && other_nat {
                    self.mark_nat(id);
                }
            }
            _ => {
                if want_nat {
                    return Err(self.kind_mismatch(false));
                }
            }
        }
        Ok(())
    }

    fn kind_mismatch(&self, found_nat: bool) -> Diagnostic {
        let msg = if found_nat {
            "kind mismatch: a size (`Nat`) where a type was expected"
        } else {
            "kind mismatch: a type where a size (`Nat`) was expected"
        };
        Diagnostic::error(Code::TypeMismatch, Span::at(0), 0, msg)
    }

    /// The occurs check, fused with the level-lowering that generalization
    /// relies on: fail if `id` appears in `ty`, and clamp every unbound variable
    /// inside `ty` to at most `level`.
    fn occurs_and_adjust(&mut self, id: VarId, level: Level, ty: Type) -> Result<()> {
        match self.head(ty) {
            TypeNode::Var(other) => {
                if other == id {
                    return Err(Diagnostic::error(
                        Code::TypeCycle,
                        Span::at(0),
                        0,
                        "cannot construct an infinite type (a variable occurs in its own binding)",
                    ));
                }
                if let Var::Unbound { level: other_level } = self.vars[other as usize] {
                    if other_level > level {
                        self.set_var(other, Var::Unbound { level });
                    }
                }
                Ok(())
            }
            TypeNode::Con(_) | TypeNode::Nat(_) | TypeNode::RowEmpty => Ok(()),
            TypeNode::App(head, arg) => {
                self.occurs_and_adjust(id, level, head)?;
                self.occurs_and_adjust(id, level, arg)
            }
            TypeNode::NatAdd(a, b) | TypeNode::NatMul(a, b) => {
                self.occurs_and_adjust(id, level, a)?;
                self.occurs_and_adjust(id, level, b)
            }
            TypeNode::Arrow(from, to, eff) => {
                self.occurs_and_adjust(id, level, from)?;
                self.occurs_and_adjust(id, level, to)?;
                self.occurs_and_adjust(id, level, eff)
            }
            TypeNode::Tuple(items) => {
                for item in self.types.items(items).to_vec() {
                    self.occurs_and_adjust(id, level, item)?;
                }
                Ok(())
            }
            TypeNode::RowExtend(_, rest) => self.occurs_and_adjust(id, level, rest),
            TypeNode::Record(row) => self.occurs_and_adjust(id, level, row),
            TypeNode::RowField(_, ty, rest) => {
                self.occurs_and_adjust(id, level, ty)?;
                self.occurs_and_adjust(id, level, rest)
            }
        }
    }

    fn mismatch(&self, a: Type, b: Type, where_: &str) -> Diagnostic {
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
    pub fn generalize(&mut self, ty: Type) {
        self.generalize_except(ty, &HashSet::new());
    }

    /// Generalize as [`Engine::generalize`], but leave any variable in `mono`
    /// ungeneralized. This is the monomorphism restriction: a variable still
    /// constrained by an unresolved overload must stay a unification variable so
    /// a later use can pin it, rather than becoming spuriously polymorphic.
    pub fn generalize_except(&mut self, ty: Type, mono: &HashSet<VarId>) {
        match self.head(ty) {
            TypeNode::Var(id) => {
                if mono.contains(&id) {
                    return;
                }
                if let Var::Unbound { level } = self.vars[id as usize] {
                    if level > self.level {
                        self.set_var(id, Var::Generic);
                    }
                }
            }
            TypeNode::App(head, arg) => {
                self.generalize_except(head, mono);
                self.generalize_except(arg, mono);
            }
            TypeNode::NatAdd(a, b) | TypeNode::NatMul(a, b) => {
                self.generalize_except(a, mono);
                self.generalize_except(b, mono);
            }
            TypeNode::Arrow(from, to, eff) => {
                self.generalize_except(from, mono);
                self.generalize_except(to, mono);
                self.generalize_except(eff, mono);
            }
            TypeNode::Tuple(items) => {
                for t in self.types.items(items).to_vec() {
                    self.generalize_except(t, mono);
                }
            }
            TypeNode::RowExtend(_, rest) => self.generalize_except(rest, mono),
            TypeNode::Record(row) => self.generalize_except(row, mono),
            TypeNode::RowField(_, ty, rest) => {
                self.generalize_except(ty, mono);
                self.generalize_except(rest, mono);
            }
            TypeNode::Con(_) | TypeNode::Nat(_) | TypeNode::RowEmpty => {}
        }
    }

    /// Collect the unbound variables reachable from `ty` (after resolution) into
    /// `out`. Used to protect an overload's operands from generalization.
    pub fn collect_vars(&self, ty: Type, out: &mut HashSet<VarId>) {
        match self.head(ty) {
            TypeNode::Var(id) => {
                out.insert(id);
            }
            TypeNode::App(head, arg) => {
                self.collect_vars(head, out);
                self.collect_vars(arg, out);
            }
            TypeNode::NatAdd(a, b) | TypeNode::NatMul(a, b) => {
                self.collect_vars(a, out);
                self.collect_vars(b, out);
            }
            TypeNode::Arrow(from, to, eff) => {
                self.collect_vars(from, out);
                self.collect_vars(to, out);
                self.collect_vars(eff, out);
            }
            TypeNode::Tuple(items) => {
                for t in self.types.items(items) {
                    self.collect_vars(t, out);
                }
            }
            TypeNode::RowExtend(_, rest) => self.collect_vars(rest, out),
            TypeNode::Record(row) => self.collect_vars(row, out),
            TypeNode::RowField(_, ty, rest) => {
                self.collect_vars(ty, out);
                self.collect_vars(rest, out);
            }
            TypeNode::Con(_) | TypeNode::Nat(_) | TypeNode::RowEmpty => {}
        }
    }

    /// Instantiate a (possibly polymorphic) type: replace each `Generic` variable
    /// with a fresh unbound variable, consistently within this one call.
    pub fn instantiate(&mut self, ty: Type) -> Type {
        let mut mapping = HashMap::new();
        self.instantiate_with(ty, &mut mapping)
    }

    /// Instantiate several types that share generalized variables with ONE fresh
    /// mapping, so a `Generic` common to two of them maps to the same fresh var.
    /// Used to instantiate an overload candidate together with its `@ctx` implicit
    /// requirements (a `Box t` candidate and its `t -> @str` dictionary share `t`).
    pub fn instantiate_bundle(&mut self, tys: &[Type]) -> Vec<Type> {
        let mut mapping = HashMap::new();
        tys.to_vec()
            .into_iter()
            .map(|t| self.instantiate_with(t, &mut mapping))
            .collect()
    }

    fn instantiate_with(&mut self, ty: Type, mapping: &mut HashMap<VarId, Type>) -> Type {
        match self.head(ty) {
            TypeNode::Var(id) => match self.vars[id as usize] {
                Var::Generic => {
                    if let Some(t) = mapping.get(&id) {
                        return *t;
                    }
                    let fresh = self.fresh_raw();
                    // A refreshed size variable stays `Nat`-kinded.
                    if self.is_nat_var(id) {
                        if let TypeNode::Var(fid) = self.types.node(fresh) {
                            self.mark_nat(fid);
                        }
                    }
                    mapping.insert(id, fresh);
                    fresh
                }
                _ => self.types.add(TypeNode::Var(id)),
            },
            TypeNode::App(head, arg) => {
                let (h, a) = (
                    self.instantiate_with(head, mapping),
                    self.instantiate_with(arg, mapping),
                );
                self.types.app(h, a)
            }
            TypeNode::NatAdd(a, b) => {
                let (x, y) = (
                    self.instantiate_with(a, mapping),
                    self.instantiate_with(b, mapping),
                );
                self.types.add(TypeNode::NatAdd(x, y))
            }
            TypeNode::NatMul(a, b) => {
                let (x, y) = (
                    self.instantiate_with(a, mapping),
                    self.instantiate_with(b, mapping),
                );
                self.types.add(TypeNode::NatMul(x, y))
            }
            TypeNode::Arrow(from, to, eff) => {
                let (f, t, e) = (
                    self.instantiate_with(from, mapping),
                    self.instantiate_with(to, mapping),
                    self.instantiate_with(eff, mapping),
                );
                self.types.arrow_eff(f, t, e)
            }
            TypeNode::Tuple(items) => {
                let items = self.types.items(items).to_vec();
                let mapped: Vec<Type> = items
                    .into_iter()
                    .map(|t| self.instantiate_with(t, mapping))
                    .collect();
                self.types.tuple(mapped)
            }
            TypeNode::RowExtend(label, rest) => {
                let r = self.instantiate_with(rest, mapping);
                self.types.add(TypeNode::RowExtend(label, r))
            }
            TypeNode::Record(row) => {
                let r = self.instantiate_with(row, mapping);
                self.types.record(r)
            }
            TypeNode::RowField(label, ty, rest) => {
                let (f, r) = (
                    self.instantiate_with(ty, mapping),
                    self.instantiate_with(rest, mapping),
                );
                self.types.add(TypeNode::RowField(label, f, r))
            }
            // Con or RowEmpty: already its own instance.
            _ => self.resolve(ty),
        }
    }

    /// A fresh variable that does not borrow `self` twice (used inside closures).
    fn fresh_raw(&mut self) -> Type {
        let id = self.vars.len() as VarId;
        self.vars.push(Var::Unbound { level: self.level });
        self.types.add(TypeNode::Var(id))
    }

    // -- display ------------------------------------------------------------

    /// Render a type with variables named `` `a ``, `` `b ``, ... in order.
    pub fn show(&self, ty: Type) -> String {
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
        display(&self.types, zonked, &mut namer)
    }
}

/// A size normalized to a polynomial over Z/2^64: `(coefficient, sorted variable
/// monomial)` terms, canonical (like terms combined, zeros dropped, sorted), so
/// structural equality decides size equality (`n + m` and `m + n` share a form).
type Poly = Vec<(u64, Vec<VarId>)>;

/// Canonicalize: sort each monomial, combine like terms (wrapping add), drop zeros.
fn canon(mut terms: Poly) -> Poly {
    for (_, m) in terms.iter_mut() {
        m.sort_unstable();
    }
    terms.sort_by(|a, b| a.1.cmp(&b.1));
    let mut out: Poly = Vec::new();
    for (c, m) in terms {
        match out.last_mut() {
            Some(last) if last.1 == m => last.0 = last.0.wrapping_add(c),
            _ => out.push((c, m)),
        }
    }
    out.retain(|(c, _)| *c != 0);
    out
}

fn poly_mul(a: &Poly, b: &Poly) -> Poly {
    let mut terms: Poly = Vec::new();
    for (ca, ma) in a {
        for (cb, mb) in b {
            let mut m = ma.clone();
            m.extend(mb.iter().copied());
            terms.push((ca.wrapping_mul(*cb), m));
        }
    }
    canon(terms)
}

/// The single variable of a bare-variable polynomial `1*v`, else None.
fn lone_var(p: &Poly) -> Option<VarId> {
    match p.as_slice() {
        [(1, m)] if m.len() == 1 => Some(m[0]),
        _ => None,
    }
}

fn poly_has_var(p: &Poly, v: VarId) -> bool {
    p.iter().any(|(_, m)| m.contains(&v))
}

fn show_poly(p: &Poly) -> String {
    if p.is_empty() {
        return "0".to_string();
    }
    let mut s = String::new();
    for (i, (c, m)) in p.iter().enumerate() {
        if i > 0 {
            s.push_str(" + ");
        }
        if m.is_empty() {
            s.push_str(&c.to_string());
        } else {
            if *c != 1 {
                s.push_str(&format!("{c}*"));
            }
            let vs: Vec<String> = m.iter().map(|v| format!("t{v}")).collect();
            s.push_str(&vs.join("*"));
        }
    }
    s
}
