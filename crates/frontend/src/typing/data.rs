//! Type representation: nodes in a store, addressed by `Copy` handles.
//!
//! A [`Type`] is an `Aol<TypeNode>` into the [`Types`] store the engine owns, the
//! same handle-and-store shape the AST uses (see [`crate::parser::data`]). Nodes
//! are immutable once built and a handle is one word, so inference passes types
//! around without ever copying a tree, and a node can be read out by value
//! (`TypeNode` is `Copy`) without holding a borrow on the store.
//!
//! A [`TypeNode::Var`] is an index into the engine's *var* store, which is the
//! one place inference mutates.

use std::cell::RefCell;

use utilities::{Aol, Interner, Slice, Store, StrId};

/// Identity of a unification variable: an index into the engine's var store.
pub type VarId = u32;

/// The rank ("level") used for efficient generalization (Rémy's algorithm): a
/// variable introduced at a deeper `let` gets a higher level, and only variables
/// whose level is deeper than the current one may be generalized.
pub type Level = u32;

/// A type: a handle into the [`Types`] store.
pub type Type = Aol<TypeNode>;

/// A monomorphic type node. Polymorphism is represented by `Generic` variables
/// inside a type (see [`crate::typing::engine`]); there is no separate scheme
/// constructor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeNode {
    /// A unification variable, resolved through the engine's var store.
    Var(VarId),
    /// A nullary type constructor: `@int`, `@str`, unit `{}`, or a user-declared
    /// type name.
    Con(StrId),
    /// A type-level natural (the size in a sized tensor `[n]T`). A distinct KIND
    /// from ordinary types: a `Nat` unifies only with another `Nat` or a
    /// Nat-kinded variable, never with a type. Modular (Z/2^64).
    Nat(u64),
    /// A type-level size sum `a + b`, modular (Z/2^64). Both operands are sizes.
    /// Equality is decided by normalizing to a canonical polynomial.
    NatAdd(Type, Type),
    /// A type-level size product `a * b`, modular (Z/2^64).
    NatMul(Type, Type),
    /// Type application `Head Arg`, e.g. `@vec @int` is `App(Con("@vec"), @int)`.
    App(Type, Type),
    /// A function type `From -[eff]-> To`. `eff` is the arrow's latent effect row:
    /// the effects a call may perform. A pure arrow's `eff` is
    /// [`TypeNode::RowEmpty`].
    Arrow(Type, Type, Type),
    /// A tuple `{ A, B, ... }`; the empty tuple is [`TypeNode::Con`]`("{}")`.
    Tuple(Slice<Type>),
    /// The empty, closed effect row `<>`: a pure computation. Also the empty
    /// record row (the tail of a closed record).
    RowEmpty,
    /// An effect-row extension `<label | rest>`. Rows are unordered up to
    /// reordering (Leijen scoped labels); a row variable in tail position is an
    /// ordinary [`TypeNode::Var`].
    RowExtend(StrId, Type),
    /// A record type `{ label: ty, ... | rest }`, wrapping a record row built from
    /// [`TypeNode::RowField`] / [`TypeNode::RowEmpty`] / a tail [`TypeNode::Var`].
    /// A declared struct is nominal ([`TypeNode::Con`]); this is the structural,
    /// row-polymorphic form, and a `Con` struct unifies with an open record row.
    Record(Type),
    /// A record-row field `label: ty | rest`. Like [`TypeNode::RowExtend`] but
    /// carries the field's type; scoped like effect rows (duplicate labels stack,
    /// first wins). Only appears inside a [`TypeNode::Record`].
    RowField(StrId, Type, Type),
}

/// Every type node, plus the interner for constructor and label names.
///
/// `Con` nodes and the empty row are shared rather than rebuilt, because they are
/// the leaves inference creates over and over. The rest are appended: the store
/// lives for one check and is dropped whole, so nothing is ever freed piecemeal.
pub struct Types {
    nodes: RefCell<Store<TypeNode>>,
    tuples: RefCell<Store<Type>>,
    names: RefCell<Interner>,
    cons: RefCell<std::collections::HashMap<StrId, Type>>,
    row_empty: Type,
}

impl Default for Types {
    fn default() -> Types {
        Types::new()
    }
}

impl Types {
    pub fn new() -> Types {
        let mut nodes = Store::new();
        let row_empty = nodes.create(TypeNode::RowEmpty);
        Types {
            nodes: RefCell::new(nodes),
            tuples: RefCell::new(Store::new()),
            names: RefCell::new(Interner::new()),
            cons: RefCell::new(std::collections::HashMap::new()),
            row_empty,
        }
    }

    /// The node a handle addresses, by value: `TypeNode` is `Copy`, so a caller
    /// can match on it without holding a borrow that would block building a type.
    pub fn node(&self, t: Type) -> TypeNode {
        *self.nodes.borrow().lookup(t)
    }

    /// The text of an interned name.
    /// The text of an interned name, copied out: the interner sits behind a
    /// `RefCell`, so a borrow of it cannot escape.
    pub fn name(&self, s: StrId) -> String {
        self.names.borrow().resolve(s).to_string()
    }

    pub fn intern(&self, s: &str) -> StrId {
        self.names.borrow_mut().intern(s)
    }

    /// The elements of a tuple node.
    pub fn items(&self, s: Slice<Type>) -> Vec<Type> {
        self.tuples.borrow().lookup_slice(s).to_vec()
    }

    pub fn add(&self, n: TypeNode) -> Type {
        self.nodes.borrow_mut().create(n)
    }

    pub fn var(&self, id: VarId) -> Type {
        self.add(TypeNode::Var(id))
    }

    pub fn nat(&self, n: u64) -> Type {
        self.add(TypeNode::Nat(n))
    }

    pub fn row_empty(&self) -> Type {
        self.row_empty
    }

    pub fn con_id(&self, name: StrId) -> Type {
        if let Some(&t) = self.cons.borrow().get(&name) {
            return t;
        }
        let t = self.add(TypeNode::Con(name));
        self.cons.borrow_mut().insert(name, t);
        t
    }

    pub fn con(&self, name: &str) -> Type {
        let id = self.intern(name);
        self.con_id(id)
    }

    pub fn app(&self, head: Type, arg: Type) -> Type {
        self.add(TypeNode::App(head, arg))
    }

    /// A pure arrow (empty latent effect). The default for built-ins and for a
    /// written arrow with no `<...>` annotation.
    pub fn arrow(&self, from: Type, to: Type) -> Type {
        let eff = self.row_empty;
        self.arrow_eff(from, to, eff)
    }

    /// An arrow carrying an explicit latent effect row.
    pub fn arrow_eff(&self, from: Type, to: Type, eff: Type) -> Type {
        self.add(TypeNode::Arrow(from, to, eff))
    }

    pub fn row_extend(&self, label: &str, rest: Type) -> Type {
        let l = self.intern(label);
        self.add(TypeNode::RowExtend(l, rest))
    }

    pub fn record(&self, row: Type) -> Type {
        self.add(TypeNode::Record(row))
    }

    pub fn row_field(&self, label: &str, ty: Type, rest: Type) -> Type {
        let l = self.intern(label);
        self.add(TypeNode::RowField(l, ty, rest))
    }

    pub fn tuple(&self, items: impl IntoIterator<Item = Type>) -> Type {
        let s = self.tuples.borrow_mut().create_slice(items);
        self.add(TypeNode::Tuple(s))
    }

    /// A closed record row from `(label, ty)` pairs in order.
    pub fn record_of(&self, fields: impl DoubleEndedIterator<Item = (String, Type)>) -> Type {
        let mut row = self.row_empty;
        for (l, t) in fields.rev() {
            row = self.row_field(&l, t, row);
        }
        self.record(row)
    }

    /// Build a curried arrow `a -> b -> ... -> result`.
    pub fn arrows(&self, params: impl DoubleEndedIterator<Item = Type>, result: Type) -> Type {
        let mut acc = result;
        for p in params.rev() {
            acc = self.arrow(p, acc);
        }
        acc
    }
}

// Built-in constructor names, kept as constants so use sites don't stringly-type.
// The `@`-sigil builtins' internal name IS their source spelling (no translation
// layer); only `Real`/`Str` keep a friendly spelling.
pub const INT: &str = "@int";
pub const REAL: &str = "@float64";
pub const STR: &str = "@str";
pub const BOOL: &str = "@bool";
pub const UNIT: &str = "{}";
pub const PTR: &str = "@ptr";
pub const ARRAY: &str = "@array";
pub const VEC: &str = "@vec";

/// Format a fully resolved type (no `Var` links left) for display. Variables are
/// named `t0`, `t1`, ... by first appearance via `namer`.
pub fn display(types: &Types, ty: Type, namer: &mut dyn FnMut(VarId) -> String) -> String {
    fn go(
        types: &Types,
        ty: Type,
        namer: &mut dyn FnMut(VarId) -> String,
        out: &mut String,
        prec: u8,
    ) {
        match types.node(ty) {
            TypeNode::Var(id) => out.push_str(&namer(id)),
            // The axis-variance markers (`@co`/`@contra`/`@neutral`) read back in
            // their source spelling when they surface on their own (a variance
            // mismatch); a whole tensor renders via the `[..]` path below.
            TypeNode::Con(name) => out.push_str(&types.name(name)),
            TypeNode::Nat(n) => out.push_str(&n.to_string()),
            TypeNode::NatAdd(a, b) => paren(out, prec > 2, |out| {
                go(types, a, namer, out, 2);
                out.push_str(" + ");
                go(types, b, namer, out, 2);
            }),
            TypeNode::NatMul(a, b) => paren(out, prec > 3, |out| {
                go(types, a, namer, out, 3);
                out.push_str(" * ");
                go(types, b, namer, out, 3);
            }),
            TypeNode::App(head, arg) => {
                if let Some((variance, size, elem)) = tensor_spine_raw(types, ty) {
                    out.push('[');
                    if let TypeNode::Con(n) = types.node(variance) {
                        let n = types.name(n);
                        if n == "@contra" {
                            out.push_str("@contra ");
                        } else if n == "@co" {
                            out.push_str("@co ");
                        }
                    }
                    go(types, size, namer, out, 0);
                    out.push(']');
                    go(types, elem, namer, out, 2);
                } else {
                    let wrap = prec > 1;
                    paren(out, wrap, |out| {
                        go(types, head, namer, out, 1);
                        out.push(' ');
                        go(types, arg, namer, out, 2);
                    });
                }
            }
            TypeNode::Arrow(from, to, eff) => {
                let wrap = prec > 0;
                paren(out, wrap, |out| {
                    go(types, from, namer, out, 1);
                    out.push_str(" -> ");
                    // Only a row with concrete labels is shown; a pure or
                    // fully-polymorphic effect (empty row / bare row variable) is
                    // elided, so ordinary functions read as `A -> B`.
                    let (labels, tail) = row_parts(types, eff);
                    if !labels.is_empty() {
                        out.push('<');
                        out.push_str(&labels.join(", "));
                        if let Some(t) = tail {
                            out.push_str(" | ");
                            out.push_str(&namer(t));
                        }
                        out.push_str("> ");
                    }
                    go(types, to, namer, out, 0);
                });
            }
            TypeNode::Tuple(items) => {
                out.push('{');
                for (i, item) in types.items(items).to_vec().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    go(types, *item, namer, out, 0);
                }
                out.push('}');
            }
            // A bare row outside an arrow (only shown in raw dumps / diagnostics).
            TypeNode::RowEmpty => out.push_str("<>"),
            TypeNode::RowExtend(..) => {
                let (labels, tail) = row_parts(types, ty);
                out.push('<');
                out.push_str(&labels.join(", "));
                if let Some(t) = tail {
                    out.push_str(" | ");
                    out.push_str(&namer(t));
                }
                out.push('>');
            }
            TypeNode::Record(row) => {
                out.push('{');
                let mut cur = row;
                let mut first = true;
                loop {
                    match types.node(cur) {
                        TypeNode::RowField(label, fty, rest) => {
                            out.push_str(if first { " " } else { ", " });
                            first = false;
                            out.push_str(&types.name(label));
                            out.push_str(": ");
                            go(types, fty, namer, out, 0);
                            cur = rest;
                        }
                        TypeNode::Var(id) => {
                            out.push_str(" | ");
                            out.push_str(&namer(id));
                            break;
                        }
                        _ => break, // RowEmpty (closed) or malformed
                    }
                }
                out.push_str(" }");
            }
            // A record row seen outside a `Record` wrapper (raw dumps only).
            TypeNode::RowField(label, fty, rest) => {
                out.push_str(&types.name(label));
                out.push_str(": ");
                go(types, fty, namer, out, 0);
                out.push_str(" | ");
                go(types, rest, namer, out, 0);
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
    go(types, ty, namer, &mut out, 0);
    out
}

/// Flatten a row into its concrete labels and its tail variable (if the row is
/// open). An empty or bare-variable row yields no labels.
fn row_parts(types: &Types, row: Type) -> (Vec<String>, Option<VarId>) {
    let mut labels = Vec::new();
    let mut cur = row;
    loop {
        match types.node(cur) {
            TypeNode::RowExtend(label, rest) => {
                labels.push(types.name(label).to_string());
                cur = rest;
            }
            TypeNode::Var(id) => return (labels, Some(id)),
            _ => return (labels, None), // RowEmpty or a malformed tail
        }
    }
}

/// Peel a literal (already-zonked) sized-tensor spine `@tensor variance size elem`
/// for pretty-printing as `[..]`.
fn tensor_spine_raw(types: &Types, ty: Type) -> Option<(Type, Type, Type)> {
    if let TypeNode::App(head, elem) = types.node(ty) {
        if let TypeNode::App(head2, size) = types.node(head) {
            if let TypeNode::App(con, variance) = types.node(head2) {
                if matches!(types.node(con), TypeNode::Con(n) if types.name(n) == "@tensor") {
                    return Some((variance, size, elem));
                }
            }
        }
    }
    None
}

/// A raw dump of a type (variables shown as `?id`); [`display`] gives nicer
/// output. Not a `Display` impl, because rendering needs the store.
pub fn dump(types: &Types, ty: Type) -> String {
    let mut namer = |id: VarId| format!("?{id}");
    display(types, ty, &mut namer)
}

/// The one name the compiler knows a program by: `$ @main`, a C-style entry that
/// takes the argument vector and returns an exit code. Nothing else is special;
/// a test harness is ordinary code (a compile-time check under `$ @run`, or a
/// global some tool evaluates by name).
pub const ENTRY: &str = "@main";

/// The entry's mandated signature, in source spelling, for diagnostics.
pub const ENTRY_SIG: &str = "@vec @str -> <@io> @int";

/// Whether a (zonked) type is [`ENTRY_SIG`]: `@vec @str -> <@io> @int`. The one
/// accepted entry shape, effect row included, so an entry that performs anything
/// beyond `@io` is a type error at its own signature rather than a surprise at
/// run time.
pub fn is_entry_type(types: &Types, ty: Type) -> bool {
    let TypeNode::Arrow(from, to, eff) = types.node(ty) else {
        return false;
    };
    let is_con = |t: Type, want: &str| {
        matches!(types.node(t), TypeNode::Con(n) if types.name(n) == want)
    };
    let argv = match types.node(from) {
        TypeNode::App(head, elem) => is_con(head, VEC) && is_con(elem, STR),
        _ => false,
    };
    let code = is_con(to, INT);
    let io = match types.node(eff) {
        TypeNode::RowExtend(label, rest) => {
            types.name(label) == "@io" && matches!(types.node(rest), TypeNode::RowEmpty)
        }
        _ => false,
    };
    argv && code && io
}

/// The compile-time analogue of [`ENTRY`]: `$ @build`, the function the compiler
/// runs *during* compilation. Its `@code` result is injected in place of its own
/// definition, so a module generates part of itself from whatever the build can
/// see. It is the only context whose effect row discharges `<@io>` at compile
/// time, which is what separates it from `$ @run`: building is IO.
pub const BUILD: &str = "@build";

/// The build function's mandated signature, in source spelling, for diagnostics.
pub const BUILD_SIG: &str = "{} -> <@meta, @io> @code";

/// Whether a (zonked) type is [`BUILD_SIG`]: `{} -> <@meta, @io> @code`. As with
/// the entry, one name means one signature, so a `@build` that performs anything
/// beyond `@meta`/`@io` is a type error at its own signature.
pub fn is_build_type(types: &Types, ty: Type) -> bool {
    let TypeNode::Arrow(from, to, eff) = types.node(ty) else {
        return false;
    };
    let is_con = |t: Type, want: &str| {
        matches!(types.node(t), TypeNode::Con(n) if types.name(n) == want)
    };
    // The row is order-insensitive, so collect its labels rather than matching a
    // fixed spelling: `<@meta, @io>` and `<@io, @meta>` are the same row.
    let mut labels: Vec<String> = Vec::new();
    let mut row = eff;
    loop {
        match types.node(row) {
            TypeNode::RowExtend(label, rest) => {
                labels.push(types.name(label));
                row = rest;
            }
            TypeNode::RowEmpty => break,
            _ => return false,
        }
    }
    labels.sort_unstable();
    labels.dedup();
    is_con(from, UNIT) && is_con(to, "@code") && labels == ["@io", "@meta"]
}
