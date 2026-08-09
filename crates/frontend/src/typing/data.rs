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

/// Format a fully resolved type (no `Var` links left) for display. Variables are
/// named `t0`, `t1`, ... by first appearance via `namer`.
pub fn display(ty: &Type, namer: &mut dyn FnMut(VarId) -> String) -> String {
    fn go(ty: &Type, namer: &mut dyn FnMut(VarId) -> String, out: &mut String, prec: u8) {
        match ty {
            Type::Var(id) => out.push_str(&namer(*id)),
            Type::Con(name) => out.push_str(name),
            Type::App(head, arg) => {
                let wrap = prec > 1;
                paren(out, wrap, |out| {
                    go(head, namer, out, 1);
                    out.push(' ');
                    go(arg, namer, out, 2);
                });
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
