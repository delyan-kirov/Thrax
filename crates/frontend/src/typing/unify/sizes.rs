//! Sizes: the `Nat` kind of tensor axes, and their equality.
//!
//! A tensor type `[n]T` carries its size `n` as a type of kind `Nat`: a literal,
//! a `Nat` variable, or a `+`/`*` of sizes. Two sizes are equal when their
//! normal forms are: `normalize_size` turns a size into a polynomial
//! over Z/2^64 (a sum of coefficient-times-monomial terms, like terms combined,
//! sorted), and structural equality of polynomials decides the rest. A
//! polynomial that is a lone variable binds that variable, which is how a size
//! is inferred.
//!
//! This module also keeps the kind discipline (a `Nat` variable binds only a
//! size) and unifies the per-axis variance markers of the tensor spine.
//!
//! Entry points: [`Unifier::fresh_nat`], [`Unifier::nat_value`]; plain
//! [`Unifier::unify`] dispatches here for sizes and tensors.

use utilities::{Code, Diagnostic, Result, Span};

use super::{is_float_name, Undo, Unifier};
use crate::typing::types::{Type, TypeNode, VarId};


// ## Nat variables

impl Unifier {
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

    pub(super) fn is_nat_var(&self, id: VarId) -> bool {
        self.nat_vars.contains(&id)
    }

    /// Mark a variable `Nat`-kinded, recording it if this is the first time.
    pub(super) fn mark_nat(&mut self, id: VarId) {
        if self.nat_vars.insert(id) {
            self.trail.push(Undo::NatVar(id));
        }
    }

    /// Mark every variable in a size expression (a `Nat` position) as `Nat`-kinded.
    pub(super) fn note_nat(&mut self, ty: Type) {
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

    /// A `Nat`-kinded variable may bind only to a `Nat` literal or another
    /// `Nat`-kinded variable; a `Type` variable may not bind to a `Nat`. Binding a
    /// `Nat` variable to a plain variable makes that variable `Nat` too.
    pub(super) fn check_kind(&mut self, id: VarId, ty: Type) -> Result<()> {
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
                // The integer-literal restriction travels with the chain, so
                // `1 + x` keeps `x` from being a float too.
                if self.is_int_var(id) {
                    let ty = self.types.add(TypeNode::Var(other));
                    self.mark_int_literal(ty);
                }
            }
            TypeNode::Con(name) if self.is_int_var(id) && is_float_name(self.types.name(name)) => {
                return Err(Diagnostic::error(
                    Code::TypeMismatch,
                    Span::at(0),
                    0,
                    format!("an integer literal cannot be a `{}`", self.types.name(name)),
                )
                .with_note("an integer literal never becomes a float")
                .with_help("write a float literal instead (`1.0`, not `1`)"));
            }
            _ => {
                if want_nat {
                    return Err(self.kind_mismatch(false));
                }
            }
        }
        Ok(())
    }

    pub(super) fn kind_mismatch(&self, found_nat: bool) -> Diagnostic {
        let msg = if found_nat {
            "kind mismatch: a size (`Nat`) where a type was expected"
        } else {
            "kind mismatch: a type where a size (`Nat`) was expected"
        };
        Diagnostic::error(Code::TypeMismatch, Span::at(0), 0, msg)
    }
}

// ## Tensors

impl Unifier {
    /// Peel a sized-tensor type `@tensor variance size elem` into its three parts.
    pub fn tensor_spine(&self, ty: Type) -> Option<(Type, Type, Type)> {
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
    pub(super) fn unify_variance(&mut self, a: Type, b: Type, where_: &str) -> Result<()> {
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
}

// ## Size equality

impl Unifier {
    /// Is `ty` a type-level size (a `Nat` literal or a `+`/`*` expression)? A bare
    /// size *variable* is handled by unify's `Var` arms, so it is not needed here.
    pub(super) fn is_size(&self, ty: Type) -> bool {
        matches!(
            self.head(ty),
            TypeNode::Nat(_) | TypeNode::NatAdd(..) | TypeNode::NatMul(..)
        )
    }

    /// Unify two sizes by their canonical polynomial over Z/2^64. Equal polynomials
    /// unify; otherwise, if one whole side is a lone unbound variable not occurring
    /// in the other, bind it (the forward-eval rule). No back-solving of embedded
    /// variables (e.g. `n + 1 == 5` is not solved), which keeps this decidable.
    pub(super) fn unify_size(&mut self, a: Type, b: Type, where_: &str) -> Result<()> {
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

    /// A size's value when it has no variables left (`2 + 3` is 5).
    pub fn nat_value(&self, ty: Type) -> Option<u64> {
        if !matches!(self.head(ty), TypeNode::Nat(_) | TypeNode::NatAdd(..) | TypeNode::NatMul(..)) {
            return None;
        }
        match self.normalize_size(ty).as_slice() {
            [] => Some(0),
            [(k, vars)] if vars.is_empty() => Some(*k),
            _ => None,
        }
    }

    /// Normalize a size to a canonical polynomial over Z/2^64: a sorted list of
    /// `(coefficient, sorted variable monomial)` terms, like terms combined, zero
    /// terms dropped. Bound variables are followed, so equality is decidable and
    /// `n + m` and `m + n` share a normal form.
    pub(super) fn normalize_size(&self, ty: Type) -> Poly {
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
