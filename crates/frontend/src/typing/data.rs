//! Monomorphic type representation.
//!
//! Types are owned (not arena) because inference mutates type variables in place
//! via the union-find in [`crate::engine`]. A [`Type::Var`] is an index into
//! that store; everything else is structural.

use std::fmt;

/// Identity of a unification variable: an index into the engine's var store.
pub type VarId = u32;

/// The rank ("level") used for efficient generalization (Rémy's algorithm): a
/// variable introduced at a deeper `let` gets a higher level, and only variables
/// whose level is deeper than the current one may be generalized.
pub type Level = u32;

/// A monomorphic type. Polymorphism is represented by `Generic` variables inside
/// a type (see [`crate::engine`]); there is no separate scheme constructor.
#[derive(Clone, PartialEq, Debug)]
pub enum Type {
    /// A unification variable, resolved through the engine's store.
    Var(VarId),
    /// A nullary type constructor: `Int`, `Real`, `Str`, `Bool`, unit `{}`, or a
    /// user-declared type name.
    Con(String),
    /// A type-level natural (the size in a sized tensor `[n]T`). A distinct KIND
    /// from ordinary types: a `Nat` unifies only with another `Nat` or a
    /// Nat-kinded variable, never with a `Type`. Modular (Z/2^64).
    Nat(u64),
    /// A type-level size sum `a + b`, modular (Z/2^64). Both operands are sizes.
    /// Equality is decided by normalizing to a canonical polynomial.
    NatAdd(Box<Type>, Box<Type>),
    /// A type-level size product `a * b`, modular (Z/2^64). Both operands are sizes.
    NatMul(Box<Type>, Box<Type>),
    /// Type application `Head Arg`, e.g. `List Int` is `App(Con("List"), Int)`.
    App(Box<Type>, Box<Type>),
    /// A function type `From -[eff]-> To`. `eff` is the arrow's latent effect
    /// row: the effects a call may perform. A pure arrow's `eff` is
    /// [`Type::RowEmpty`].
    Arrow(Box<Type>, Box<Type>, Box<Type>),
    /// A tuple `{ A, B, ... }`; the empty tuple is [`Type::Con`]`("{}")` (unit).
    Tuple(Vec<Type>),
    /// The empty, closed effect row `<>`: a pure computation. Also the empty
    /// record row (the tail of a closed record).
    RowEmpty,
    /// An effect-row extension `<label | rest>`. Rows are unordered up to
    /// reordering (Leijen scoped labels); a row variable in tail position is an
    /// ordinary [`Type::Var`].
    RowExtend(String, Box<Type>),
    /// A record type `{ label: ty, ... | rest }`, wrapping a record row built from
    /// [`Type::RowField`] / [`Type::RowEmpty`] / a tail [`Type::Var`]. A declared
    /// struct is nominal ([`Type::Con`]); this is the structural, row-polymorphic
    /// form, and a `Con` struct unifies with an open record row structurally.
    Record(Box<Type>),
    /// A record-row field `label: ty | rest`. Like [`Type::RowExtend`] but carries
    /// the field's type; scoped like effect rows (duplicate labels stack, first
    /// wins). Only appears inside a [`Type::Record`].
    RowField(String, Box<Type>, Box<Type>),
}

impl Type {
    pub fn con(name: &str) -> Type {
        Type::Con(name.to_string())
    }
    pub fn nat(n: u64) -> Type {
        Type::Nat(n)
    }
    /// A pure arrow (empty latent effect). The default for built-ins and for a
    /// written arrow with no `<...>` annotation.
    pub fn arrow(from: Type, to: Type) -> Type {
        Type::arrow_eff(from, to, Type::RowEmpty)
    }
    /// An arrow carrying an explicit latent effect row.
    pub fn arrow_eff(from: Type, to: Type, eff: Type) -> Type {
        Type::Arrow(Box::new(from), Box::new(to), Box::new(eff))
    }
    pub fn row_extend(label: &str, rest: Type) -> Type {
        Type::RowExtend(label.to_string(), Box::new(rest))
    }
    pub fn record(row: Type) -> Type {
        Type::Record(Box::new(row))
    }
    pub fn row_field(label: &str, ty: Type, rest: Type) -> Type {
        Type::RowField(label.to_string(), Box::new(ty), Box::new(rest))
    }
    /// A closed record row from `(label, ty)` pairs in order.
    pub fn record_of(fields: impl DoubleEndedIterator<Item = (String, Type)>) -> Type {
        Type::record(fields.rev().fold(Type::RowEmpty, |rest, (l, t)| {
            Type::RowField(l, Box::new(t), Box::new(rest))
        }))
    }
    pub fn app(head: Type, arg: Type) -> Type {
        Type::App(Box::new(head), Box::new(arg))
    }

    /// Build a curried arrow `a -> b -> ... -> result`.
    pub fn arrows(params: impl DoubleEndedIterator<Item = Type>, result: Type) -> Type {
        params.rev().fold(result, |acc, p| Type::arrow(p, acc))
    }
}

// Built-in constructor names, kept as constants so use sites don't stringly-type.
pub const INT: &str = "Int";
pub const REAL: &str = "Real";
pub const STR: &str = "Str";
pub const BOOL: &str = "Bool";
pub const UNIT: &str = "{}";
pub const LIST: &str = "List";
pub const ARRAY: &str = "Array";
pub const VEC: &str = "Vec";
/// The canonical infinite codata stream (defined in `CORE`), the target an
/// open range `[lo ...]` builds.
pub const STREAM: &str = "Stream";

/// Format a fully resolved type (no `Var` links left) for display. Variables are
/// named `t0`, `t1`, ... by first appearance via `namer`.
pub fn display(ty: &Type, namer: &mut dyn FnMut(VarId) -> String) -> String {
    fn go(ty: &Type, namer: &mut dyn FnMut(VarId) -> String, out: &mut String, prec: u8) {
        match ty {
            Type::Var(id) => out.push_str(&namer(*id)),
            // The axis-variance markers (`@co`/`@contra`/`@neutral`) read back in
            // their source spelling when they surface on their own (a variance
            // mismatch); a whole tensor renders via the `[..]` path below.
            Type::Con(name) => out.push_str(name),
            Type::Nat(n) => out.push_str(&n.to_string()),
            Type::NatAdd(a, b) => paren(out, prec > 2, |out| {
                go(a, namer, out, 2);
                out.push_str(" + ");
                go(b, namer, out, 2);
            }),
            Type::NatMul(a, b) => paren(out, prec > 3, |out| {
                go(a, namer, out, 3);
                out.push_str(" * ");
                go(b, namer, out, 3);
            }),
            Type::App(head, arg) => {
                if let Some((variance, size, elem)) = tensor_spine_raw(ty) {
                    out.push('[');
                    if let Type::Con(n) = variance {
                        if n == "@contra" {
                            out.push_str("@contra ");
                        } else if n == "@co" {
                            out.push_str("@co ");
                        }
                    }
                    go(size, namer, out, 0);
                    out.push(']');
                    go(elem, namer, out, 2);
                } else {
                    let wrap = prec > 1;
                    paren(out, wrap, |out| {
                        go(head, namer, out, 1);
                        out.push(' ');
                        go(arg, namer, out, 2);
                    });
                }
            }
            Type::Arrow(from, to, eff) => {
                let wrap = prec > 0;
                paren(out, wrap, |out| {
                    go(from, namer, out, 1);
                    out.push_str(" -> ");
                    // Only a row with concrete labels is shown; a pure or
                    // fully-polymorphic effect (empty row / bare row variable) is
                    // elided, so ordinary functions read as `A -> B`.
                    let (labels, tail) = row_parts(eff);
                    if !labels.is_empty() {
                        out.push('<');
                        out.push_str(&labels.join(", "));
                        if let Some(t) = tail {
                            out.push_str(" | ");
                            out.push_str(&namer(t));
                        }
                        out.push_str("> ");
                    }
                    go(to, namer, out, 0);
                });
            }
            Type::Tuple(items) => {
                out.push('{');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    go(item, namer, out, 0);
                }
                out.push('}');
            }
            // A bare row outside an arrow (only shown in raw dumps / diagnostics).
            Type::RowEmpty => out.push_str("<>"),
            Type::RowExtend(..) => {
                let (labels, tail) = row_parts(ty);
                out.push('<');
                out.push_str(&labels.join(", "));
                if let Some(t) = tail {
                    out.push_str(" | ");
                    out.push_str(&namer(t));
                }
                out.push('>');
            }
            Type::Record(row) => {
                out.push('{');
                let mut cur = row.as_ref();
                let mut first = true;
                loop {
                    match cur {
                        Type::RowField(label, fty, rest) => {
                            out.push_str(if first { " " } else { ", " });
                            first = false;
                            out.push_str(label);
                            out.push_str(": ");
                            go(fty, namer, out, 0);
                            cur = rest;
                        }
                        Type::Var(id) => {
                            out.push_str(" | ");
                            out.push_str(&namer(*id));
                            break;
                        }
                        _ => break, // RowEmpty (closed) or malformed
                    }
                }
                out.push_str(" }");
            }
            // A record row seen outside a `Record` wrapper (raw dumps only).
            Type::RowField(label, fty, rest) => {
                out.push_str(label);
                out.push_str(": ");
                go(fty, namer, out, 0);
                out.push_str(" | ");
                go(rest, namer, out, 0);
            }
        }
    }
    /// Flatten a row into its concrete labels and its tail variable (if the row is
    /// open). An empty or bare-variable row yields no labels.
    fn row_parts(row: &Type) -> (Vec<String>, Option<VarId>) {
        let mut labels = Vec::new();
        let mut cur = row;
        loop {
            match cur {
                Type::RowExtend(label, rest) => {
                    labels.push(label.clone());
                    cur = rest;
                }
                Type::Var(id) => return (labels, Some(*id)),
                _ => return (labels, None), // RowEmpty or a malformed tail
            }
        }
    }
    /// Peel a literal (already-zonked) sized-tensor spine `@tensor variance size
    /// elem` for pretty-printing as `[..]`.
    fn tensor_spine_raw(ty: &Type) -> Option<(&Type, &Type, &Type)> {
        if let Type::App(head, elem) = ty {
            if let Type::App(head2, size) = head.as_ref() {
                if let Type::App(con, variance) = head2.as_ref() {
                    if matches!(con.as_ref(), Type::Con(n) if n == "@tensor") {
                        return Some((variance, size, elem));
                    }
                }
            }
        }
        None
    }
    fn paren(out: &mut String, wrap: bool, f: impl FnOnce(&mut String)) {
        if wrap {
            out.push('(');
        }
        f(out);
        if wrap {
            out.push(')');
        }
    }
    let mut out = String::new();
    go(ty, namer, &mut out, 0);
    out
}

impl fmt::Display for Type {
    /// A raw dump (variables shown as `?id`); [`display`] gives nicer output.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut namer = |id: VarId| format!("?{id}");
        f.write_str(&display(self, &mut namer))
    }
}

/// How a program entry point is invoked, derived from its declared type. A `main`
/// like C's: a function that may perform any effect and returns an `Int` exit
/// code, taking either no arguments (`{} -> Int`) or the argument vector
/// (`[n]Str -> Int`). A plain value (e.g. the test harness's `test : Int`) is
/// just forced.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryKind {
    /// Not a function: force the value (the result / exit code).
    Value,
    /// `{} -> R`: apply to unit.
    UnitFn,
    /// `[n]Str -> R`: apply to the argument vector.
    ArgvFn,
    /// A function whose parameter is neither `{}` nor `[n]Str`.
    BadFn,
}

/// Classify an entry point from its (zonked) type.
pub fn classify_entry(ty: &Type) -> EntryKind {
    let Type::Arrow(from, _, _) = ty else {
        return EntryKind::Value;
    };
    match from.as_ref() {
        Type::Con(n) if n == "{}" => EntryKind::UnitFn,
        Type::Tuple(items) if items.is_empty() => EntryKind::UnitFn,
        // `[n]Str`: a `@tensor variance size elem` spine whose element is `Str`.
        Type::App(head, elem) => {
            let is_tensor = matches!(head.as_ref(),
                Type::App(h2, _) if matches!(h2.as_ref(),
                    Type::App(con, _) if matches!(con.as_ref(),
                        Type::Con(n) if n == "@tensor")));
            if is_tensor && matches!(elem.as_ref(), Type::Con(n) if n == "Str") {
                EntryKind::ArgvFn
            } else {
                EntryKind::BadFn
            }
        }
        _ => EntryKind::BadFn,
    }
}
