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
    /// A function type `From -> To`.
    Arrow(Box<Type>, Box<Type>),
    /// A tuple `{ A, B, ... }`; the empty tuple is [`Type::Con`]`("{}")` (unit).
    Tuple(Vec<Type>),
}

impl Type {
    pub fn con(name: &str) -> Type {
        Type::Con(name.to_string())
    }
    pub fn arrow(from: Type, to: Type) -> Type {
        Type::Arrow(Box::new(from), Box::new(to))
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
            Type::Arrow(from, to) => {
                let wrap = prec > 0;
                paren(out, wrap, |out| {
                    go(from, namer, out, 1);
                    out.push_str(" -> ");
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
