//! Row unification: records and effect rows as scoped-label rows.
//!
//! The algorithm is Leijen's, from "Extensible records with scoped labels" (TFP
//! 2005). A row is a chain of labelled entries ending in the empty row or a row
//! variable. To unify `l : t | r` with a row `s`, `rewrite_row` digs
//! `l` out of `s` (binding `s`'s tail to a fresh row with `l` added when the
//! label is absent and the tail is open), unifies the two `t`s, and unifies what
//! remains. Duplicate labels are kept, in order, which is what makes the labels
//! "scoped".
//!
//! Records use the same machinery over [`TypeNode::RowField`]; a nominal struct
//! meets a record row through its declared row. Effect rows add subsumption:
//! [`Unifier::subrow`] checks that one effect row's labels all appear in
//! another. See Note [Nominal struct / record row bridge].
//!
//! Entry points: [`Unifier::subrow`], [`Unifier::record_field`]; plain
//! [`Unifier::unify`] dispatches here for rows and records.


use utilities::{Code, Diagnostic, Result, Span, StrId};

use super::Unifier;
use crate::typing::types::{Type, TypeNode, VarId};


// ## Records

impl Unifier {
    /// Register the structs' record-row schemes for the nominal-struct / record-row
    /// unification bridge (see the `struct_rows` map).
    pub fn set_struct_rows(&mut self, rows: std::collections::HashMap<StrId, (Vec<VarId>, Type)>) {
        self.struct_rows = rows;
    }

    /// Look up field `label` in a record type, growing an open tail to include it.
    pub fn record_field(&mut self, record: Type, label: impl Into<StrId>, where_: &str) -> Result<Type> {
        match self.head(record) {
            TypeNode::Record(row) => Ok(self.rewrite_field(row, label.into(), where_)?.0),
            _ => Ok(self.fresh()),
        }
    }

    // Note [Nominal struct / record row bridge]
    //
    // Thrax has both nominal structs (`$ P : @struct = x: @Int, y: @Int`) and
    // structural records (`{ x: @Int | r }`, a row). A function written against
    // a row should accept any struct with those fields, so a struct has to meet
    // a row somewhere, without the two kinds of type becoming one.
    //
    // They meet in unification and only there. Every struct registers its
    // fields as a closed row, quantified over the struct's parameters
    // (`Checker::register_struct_rows` into `struct_rows`). When unification
    // finds a record on one side and a struct type (a bare constructor or an
    // applied one) on the other, it instantiates that row with the struct's
    // arguments and unifies it with the record. Structs never unify with each
    // other this way: `P` and `Q` with the same fields stay different types. The
    // bridge is one-way per use, so nothing records that `P` "is" a record.

    /// If one side is a record row and the other a struct type (bare `Con` or an
    /// applied `App..(Con)`), unify the record against the struct's registered row
    /// with the struct's type arguments substituted for its parameters. Returns
    /// `None` when neither side bridges, so the caller reports a plain mismatch.
    /// See Note [Nominal struct / record row bridge].
    pub(super) fn struct_row_bridge(&mut self, a: Type, b: Type, where_: &str) -> Option<Result<()>> {
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
    pub(super) fn subst_vars(&self, ty: Type, sub: &std::collections::HashMap<VarId, Type>) -> Type {
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
            TypeNode::RowExtend(label, args, rest) => {
                let (a, r) = (self.subst_vars(args, sub), self.subst_vars(rest, sub));
                self.types.add(TypeNode::RowExtend(label, a, r))
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
    pub(super) fn unify_record(&mut self, a: Type, b: Type, where_: &str) -> Result<()> {
        self.unify_record_row(a, b, where_)
    }

    /// Unify two record rows (Leijen scoped-label discipline, plus field types):
    /// bring the head field of one row to the head of the other, unify the field
    /// types, then unify the tails. An open tail (a row variable) grows to accept a
    /// missing field, which is where row polymorphism comes from.
    pub(super) fn unify_record_row(&mut self, a: Type, b: Type, where_: &str) -> Result<()> {
        let a = self.resolve(a);
        let b = self.resolve(b);
        match (self.types.node(a), self.types.node(b)) {
            (TypeNode::RowEmpty, TypeNode::RowEmpty) => Ok(()),
            (TypeNode::Var(i), TypeNode::Var(j)) if i == j => Ok(()),
            (TypeNode::Var(i), _) => self.bind(i, b),
            (_, TypeNode::Var(j)) => self.bind(j, a),
            (TypeNode::RowField(label, fty, a_rest), _) => {
                let (b_fty, b_rest) = self.rewrite_field(b, label, where_)?;
                self.unify(fty, b_fty, where_)?;
                self.unify_record_row(a_rest, b_rest, where_)
            }
            // The other side ran out of fields but this one still wants `label`.
            (TypeNode::RowEmpty, TypeNode::RowField(label, ..)) => Err(self.field_missing(label, where_)),
            _ => Err(self.mismatch(a, b, where_)),
        }
    }

    /// Bring an occurrence of field `label` to the head of record `row`, returning
    /// its field type and the remaining row. An open tail grows to include the
    /// field (fresh field type); a closed row lacking it is a type error.
    pub(super) fn rewrite_field(&mut self, row: Type, label: StrId, where_: &str) -> Result<(Type, Type)> {
        match self.head(row) {
            TypeNode::RowField(l, fty, rest) => {
                if l == label {
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

    pub(super) fn field_missing(&self, label: StrId, where_: &str) -> Diagnostic {
        Diagnostic::error(
            Code::TypeMismatch,
            Span::at(0),
            0,
            format!("record has no field `{label}` {where_}"),
        )
    }
}

// ## Effect rows

impl Unifier {
    /// Unify two effect rows, at least one a `<label | rest>` extension (Leijen's
    /// scoped-label discipline): pull the head label of the extension out of the
    /// other row, then unify the remaining tails.
    pub(super) fn unify_row(&mut self, a: Type, b: Type, where_: &str) -> Result<()> {
        // Normalize so `a` is the extension we decompose.
        let (a, b) = if matches!(self.types.node(a), TypeNode::RowExtend(..)) {
            (a, b)
        } else {
            (b, a)
        };
        let TypeNode::RowExtend(label, a_args, a_rest) = self.types.node(a) else {
            unreachable!("unify_row without a row extension")
        };
        let outermost = self.performed.is_none();
        if outermost {
            self.performed = Some(self.row_labels(a));
        }
        let r = self
            .rewrite_row(b, label, a_args, where_)
            .and_then(|b_rest| self.unify(a_rest, b_rest, where_));
        if outermost {
            self.performed = None;
        }
        r
    }

    /// Bring an occurrence of effect `label` to the head of `row`, unify its
    /// arguments with `args`, and return the row that remains once it is
    /// removed. An open tail (a row variable) grows to accept the label. A closed
    /// row lacking the label is the unhandled-effect error.
    pub(super) fn rewrite_row(&mut self, row: Type, label: StrId, args: Type, where_: &str) -> Result<Type> {
        match self.head(row) {
            TypeNode::RowExtend(l, l_args, rest) => {
                if l == label {
                    self.unify_effect_args(label, args, l_args, where_)?;
                    Ok(rest)
                } else {
                    let deeper = self.rewrite_row(rest, label, args, where_)?;
                    Ok(self.types.add(TypeNode::RowExtend(l, l_args, deeper)))
                }
            }
            TypeNode::Var(id) => {
                let tail = self.fresh();
                let ext = self.types.row_extend_args(label, args, tail);
                self.bind(id, ext)?;
                Ok(tail)
            }
            _ => Err(self.effect_not_handled(label, where_)),
        }
    }

    /// Effect subsumption: require `sub` to be a subrow of `super_`. Every effect
    /// the callee performs (`sub`) must be permitted by the ambient (`super_`).
    pub fn subrow(&mut self, sub: Type, super_: Type, where_: &str) -> Result<()> {
        let saved = self.performed.replace(self.row_labels(sub));
        let r = self.subrow_go(sub, super_, where_);
        self.performed = saved;
        r
    }

    pub(super) fn subrow_go(&mut self, sub: Type, super_: Type, where_: &str) -> Result<()> {
        match self.head(sub) {
            TypeNode::RowEmpty => Ok(()),
            TypeNode::Var(_) => self.unify(sub, super_, where_),
            TypeNode::RowExtend(label, args, rest) => {
                let super_rest = self.rewrite_row(super_, label, args, where_)?;
                self.subrow_go(rest, super_rest, where_)
            }
            _ => self.unify(sub, super_, where_),
        }
    }

    /// The labels of a row, in order, up to its tail.
    pub(super) fn row_labels(&self, row: Type) -> Vec<StrId> {
        let mut labels = Vec::new();
        let mut cur = row;
        while let TypeNode::RowExtend(label, _, rest) = self.head(cur) {
            labels.push(label);
            cur = rest;
        }
        labels
    }

    /// Unify the type arguments of two occurrences of one effect label. Unknown
    /// arguments (`RowEmpty`, a label written where its effect is not visible)
    /// match any.
    pub(super) fn unify_effect_args(&mut self, label: StrId, a: Type, b: Type, where_: &str) -> Result<()> {
        let (a, b) = (self.resolve(a), self.resolve(b));
        if matches!(self.types.node(a), TypeNode::RowEmpty)
            || matches!(self.types.node(b), TypeNode::RowEmpty)
        {
            return Ok(());
        }
        let saved = self.performed.take();
        let r = self.unify(a, b, where_);
        self.performed = saved;
        r.map_err(|d| {
            let note = format!(
                "effect `{label}` is used here at `{}` and at `{}`; one handler serves one instance",
                self.show_args(a),
                self.show_args(b)
            );
            d.with_note(note)
        })
    }

    /// An effect's type arguments, space-separated as written in a row.
    pub(super) fn show_args(&self, args: Type) -> String {
        match self.types.node(args) {
            TypeNode::Tuple(items) => self
                .types
                .items(items)
                .into_iter()
                .map(|t| self.show(t))
                .collect::<Vec<_>>()
                .join(" "),
            _ => self.show(args),
        }
    }

    pub(super) fn effect_not_handled(&self, label: StrId, where_: &str) -> Diagnostic {
        let part_of = self.performed.as_ref().and_then(|performed| {
            self.row_folds.iter().find(|(_, members)| {
                members.contains(&label) && members.iter().all(|m| performed.contains(m))
            })
        });
        let effect = match part_of {
            Some((alias, _)) => format!("effect `{label}` (part of `{alias}`)"),
            None => format!("effect `{label}`"),
        };
        Diagnostic::error(
            Code::TypeMismatch,
            Span::at(0),
            0,
            format!("{effect} is performed but not handled {where_}"),
        )
    }
}
