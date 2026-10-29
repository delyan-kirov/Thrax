//! Type representation: nodes in a store, addressed by `Copy` handles.
//!
//! A [`Type`] is an `Aol<TypeNode>` into the [`Types`] store the unifier owns, the
//! same handle-and-store shape the AST uses (see [`crate::parser::data`]). Nodes
//! are immutable once built and a handle is one word, so inference passes types
//! around without ever copying a tree, and a node can be read out by value
//! (`TypeNode` is `Copy`) without holding a borrow on the store.
//!
//! A [`TypeNode::Var`] is an index into the unifier's *var* store, which is the
//! one place inference mutates.

use std::cell::RefCell;

use utilities::{Aol, Slice, Store, StrId};

use crate::parser::data::Variance;

/// Identity of a unification variable: an index into the unifier's var store.
pub type VarId = u32;

/// The rank ("level") used for efficient generalization (Rémy's algorithm): a
/// variable introduced at a deeper `let` gets a higher level, and only variables
/// whose level is deeper than the current one may be generalized.
pub type Level = u32;

/// A type: a handle into the [`Types`] store.
pub type Type = Aol<TypeNode>;

/// A monomorphic type node. Polymorphism is represented by `Generic` variables
/// inside a type (see [`crate::typing::unify`]); there is no separate scheme
/// constructor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeNode {
    /// A unification variable, resolved through the unifier's var store.
    Var(VarId),
    /// A nullary type constructor: `@Int`, `@Str`, unit `{}`, or a user-declared
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
    /// Type application `Head Arg`, e.g. `@Vec @Int` is `App(Con("@Vec"), @Int)`.
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
    /// An effect-row extension `<label args | rest>`. Rows are unordered up to
    /// reordering (Leijen scoped labels); a row variable in tail position is an
    /// ordinary [`TypeNode::Var`]. `args` is the effect's type arguments as a
    /// [`TypeNode::Tuple`] (`<Chan @Int e>`), or [`TypeNode::RowEmpty`] for a
    /// label whose arguments are unknown, which matches any (an effect this
    /// module cannot see). Two occurrences of one label unify their arguments,
    /// so every operation performed under one handler shares its instance.
    RowExtend(StrId, Type, Type),
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

/// Every type node. Constructor and label names are [`StrId`]s.
///
/// `Con` nodes and the empty row are shared rather than rebuilt, because they are
/// the leaves inference creates over and over. The rest are appended: the store
/// lives for one check and is dropped whole, so nothing is ever freed piecemeal.
/// See [`Types::mark`].
#[derive(Clone, Debug)]
pub struct TypesMark {
    nodes: usize,
    tuples: usize,
    rigids: std::collections::HashMap<String, u32>,
}

/// How [`Types::error`] prints. Not lexable as a type, so no source can name it.
pub const ERROR_CON: &str = "{error}";

pub struct Types {
    nodes: RefCell<Store<TypeNode>>,
    tuples: RefCell<Store<Type>>,
    cons: RefCell<std::collections::HashMap<StrId, Type>>,
    row_empty: Type,
    /// The type of a definition or expression that failed to check. It unifies
    /// with everything, so one error does not cascade into its uses.
    error: Type,
    /// How many rigid types each name has been given (see [`Types::rigid`]).
    rigids: RefCell<std::collections::HashMap<String, u32>>,
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
        let error_name = StrId::intern(ERROR_CON);
        let error = nodes.create(TypeNode::Con(error_name));
        Types {
            nodes: RefCell::new(nodes),
            tuples: RefCell::new(Store::new()),
            cons: RefCell::new(std::collections::HashMap::from([(error_name, error)])),
            row_empty,
            error,
            rigids: RefCell::new(std::collections::HashMap::new()),
        }
    }

    /// The node a handle addresses, by value: `TypeNode` is `Copy`, so a caller
    /// can match on it without holding a borrow that would block building a type.
    pub fn node(&self, t: Type) -> TypeNode {
        *self.nodes.borrow().lookup(t)
    }

    /// The text of an interned name.
    pub fn name(&self, s: StrId) -> &'static str {
        s.as_str()
    }

    pub fn intern(&self, s: &str) -> StrId {
        StrId::intern(s)
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

    pub fn error(&self) -> Type {
        self.error
    }

    /// A watermark of everything built so far, for [`Types::truncate`].
    pub fn mark(&self) -> TypesMark {
        TypesMark {
            nodes: self.nodes.borrow().len(),
            tuples: self.tuples.borrow().len(),
            rigids: self.rigids.borrow().clone(),
        }
    }

    /// Drop every type built since `mark`. Sound only when nothing that holds
    /// one of them is used again: an editor re-checking one module over checked
    /// dependencies rewinds to the dependencies' mark before each new check.
    pub fn truncate(&self, mark: &TypesMark) {
        self.nodes.borrow_mut().truncate(mark.nodes);
        self.tuples.borrow_mut().truncate(mark.tuples);
        self.cons.borrow_mut().retain(|_, t| t.index() < mark.nodes);
        *self.rigids.borrow_mut() = mark.rigids.clone();
    }

    pub fn con_id(&self, name: StrId) -> Type {
        if let Some(&t) = self.cons.borrow().get(&name) {
            return t;
        }
        let t = self.add(TypeNode::Con(name));
        self.cons.borrow_mut().insert(name, t);
        t
    }

    pub fn con(&self, name: impl Into<StrId>) -> Type {
        self.con_id(name.into())
    }

    /// A rigid type: an opaque constructor equal only to itself, shown as
    /// `name`. Each call makes a new one; a later one with the same name is
    /// told apart by a `#n` suffix, since the store is shared by every module.
    pub fn rigid(&self, name: &str) -> Type {
        let n = {
            let mut rigids = self.rigids.borrow_mut();
            let n = rigids.entry(name.to_string()).or_insert(0);
            *n += 1;
            *n
        };
        if n == 1 {
            self.add(TypeNode::Con(self.intern(name)))
        } else {
            self.add(TypeNode::Con(self.intern(&format!("{name} #{n}"))))
        }
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

    /// Extend a row with an effect label that takes no arguments.
    pub fn row_extend(&self, label: impl Into<StrId>, rest: Type) -> Type {
        let args = self.row_empty;
        self.row_extend_args(label, args, rest)
    }

    /// Extend a row with an effect label and its type arguments (a tuple).
    pub fn row_extend_args(&self, label: impl Into<StrId>, args: Type, rest: Type) -> Type {
        self.add(TypeNode::RowExtend(label.into(), args, rest))
    }

    pub fn record(&self, row: Type) -> Type {
        self.add(TypeNode::Record(row))
    }

    pub fn row_field(&self, label: impl Into<StrId>, ty: Type, rest: Type) -> Type {
        self.add(TypeNode::RowField(label.into(), ty, rest))
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
pub const INT: &str = "@Int";
pub const REAL: &str = "@Float64";
pub const STR: &str = "@Str";
pub const BOOL: &str = "@Bool";
pub const UNIT: &str = "{}";
pub const PTR: &str = "@Ptr";
pub const ARRAY: &str = "@Array";
pub const VEC: &str = "@Vec";

/// Format a fully resolved type (no `Var` links left) for display. Variables are
/// named `t0`, `t1`, ... by first appearance via `namer`.
pub fn display(types: &Types, ty: Type, namer: &mut dyn FnMut(VarId) -> String) -> String {
    display_folding(types, ty, namer, &[])
}

/// A row alias that display may print in place of its labels: the alias name
/// and the labels it stands for.
pub type RowFold = (String, Vec<String>);

/// [`display`], printing any row that holds every label of a fold in `folds` with
/// the fold's name instead of those labels. Earlier folds are tried first.
pub fn display_folding(
    types: &Types,
    ty: Type,
    namer: &mut dyn FnMut(VarId) -> String,
    folds: &[RowFold],
) -> String {
    fn go(
        types: &Types,
        folds: &[RowFold],
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
            TypeNode::Con(name) => out.push_str(types.name(name)),
            TypeNode::Nat(n) => out.push_str(&n.to_string()),
            TypeNode::NatAdd(a, b) => paren(out, prec > 2, |out| {
                go(types, folds, a, namer, out, 2);
                out.push_str(" + ");
                go(types, folds, b, namer, out, 2);
            }),
            TypeNode::NatMul(a, b) => paren(out, prec > 3, |out| {
                go(types, folds, a, namer, out, 3);
                out.push_str(" * ");
                go(types, folds, b, namer, out, 3);
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
                    go(types, folds, size, namer, out, 0);
                    out.push(']');
                    go(types, folds, elem, namer, out, 2);
                } else {
                    let wrap = prec > 1;
                    paren(out, wrap, |out| {
                        go(types, folds, head, namer, out, 1);
                        out.push(' ');
                        go(types, folds, arg, namer, out, 2);
                    });
                }
            }
            TypeNode::Arrow(from, to, eff) => {
                let wrap = prec > 0;
                paren(out, wrap, |out| {
                    go(types, folds, from, namer, out, 1);
                    out.push_str(" -> ");
                    // Only a row with concrete labels is shown; a pure or
                    // fully-polymorphic effect (empty row / bare row variable) is
                    // elided, so ordinary functions read as `A -> B`.
                    let (labels, tail) = row_parts(types, folds, eff, namer);
                    let labels = fold_labels(labels, folds);
                    if !labels.is_empty() {
                        out.push('<');
                        out.push_str(&labels.join(", "));
                        if let Some(t) = tail {
                            out.push_str(" | ");
                            out.push_str(&namer(t));
                        }
                        out.push_str("> ");
                    }
                    go(types, folds, to, namer, out, 0);
                });
            }
            TypeNode::Tuple(items) => {
                out.push('{');
                for (i, item) in types.items(items).to_vec().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    go(types, folds, *item, namer, out, 0);
                }
                out.push('}');
            }
            // A bare row outside an arrow (only shown in raw dumps / diagnostics).
            TypeNode::RowEmpty => out.push_str("<>"),
            TypeNode::RowExtend(..) => {
                let (labels, tail) = row_parts(types, folds, ty, namer);
                let labels = fold_labels(labels, folds);
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
                            out.push_str(types.name(label));
                            out.push_str(": ");
                            go(types, folds, fty, namer, out, 0);
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
                out.push_str(types.name(label));
                out.push_str(": ");
                go(types, folds, fty, namer, out, 0);
                out.push_str(" | ");
                go(types, folds, rest, namer, out, 0);
            }
        }
    }
    /// Flatten a row into its concrete labels, each with its type arguments,
    /// and its tail variable (if the row is open).
    fn row_parts(
        types: &Types,
        folds: &[RowFold],
        row: Type,
        namer: &mut dyn FnMut(VarId) -> String,
    ) -> (Vec<String>, Option<VarId>) {
        let mut labels = Vec::new();
        let mut cur = row;
        let tail = loop {
            match types.node(cur) {
                TypeNode::RowExtend(label, args, rest) => {
                    let mut text = types.name(label).to_string();
                    if let TypeNode::Tuple(items) = types.node(args) {
                        for arg in types.items(items) {
                            text.push(' ');
                            go(types, folds, arg, namer, &mut text, 2);
                        }
                    }
                    labels.push(text);
                    cur = rest;
                }
                TypeNode::Var(id) => break Some(id),
                _ => break None, // RowEmpty or a malformed tail
            }
        };
        (labels, tail)
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
    go(types, folds, ty, namer, &mut out, 0);
    out
}

/// Replace each fold whose labels all occur in `labels` by the fold's name, at
/// the position of its first label. One occurrence of each label is consumed, so
/// a label the row repeats outside the alias still shows.
pub fn fold_labels(mut labels: Vec<String>, folds: &[RowFold]) -> Vec<String> {
    for (name, members) in folds {
        if members.is_empty() || !members.iter().all(|m| labels.contains(m)) {
            continue;
        }
        let at = labels.iter().position(|l| members.contains(l)).expect("a member is present");
        for m in members {
            let i = labels.iter().position(|l| l == m).expect("checked above");
            labels.remove(i);
        }
        labels.insert(at, name.clone());
    }
    labels
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

/// The one builtin effect: the outside world, handled by the runtime around
/// [`ENTRY`] and around each `@spawn` task. Anything finer is an ordinary effect
/// whose handler discharges into `@IO`.
pub const IO: &str = "@IO";

/// The labels of a closed row as a sorted set, or `None` for an open row.
fn closed_row_labels(types: &Types, row: Type) -> Option<Vec<String>> {
    let mut labels: Vec<String> = Vec::new();
    let mut cur = row;
    loop {
        match types.node(cur) {
            TypeNode::RowExtend(label, _, rest) => {
                labels.push(types.name(label).to_string());
                cur = rest;
            }
            TypeNode::RowEmpty => break,
            _ => return None,
        }
    }
    labels.sort_unstable();
    labels.dedup();
    Some(labels)
}

/// `labels` as the sorted set [`closed_row_labels`] yields.
fn label_set(labels: &[&str]) -> Vec<String> {
    let mut want: Vec<String> = labels.iter().map(|s| s.to_string()).collect();
    want.sort_unstable();
    want
}

/// The one name the compiler knows a program by: `$ @main`, a C-style entry that
/// takes the argument vector and returns an exit code. Nothing else is special;
/// a test harness is ordinary code (a compile-time check under `$ @e`, or a
/// global some tool evaluates by name).
pub const ENTRY: &str = "@main";

/// The entry's mandated signature, in source spelling, for diagnostics.
pub const ENTRY_SIG: &str = "@Vec @Str -> <@IO> @Int";

/// Whether a (zonked) type is [`ENTRY_SIG`]: `@Vec @Str -> <@IO> @Int`. The one
/// accepted entry shape, effect row included, so an entry that performs anything
/// beyond `@IO` is a type error at its own signature rather than a surprise at
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
    let io = closed_row_labels(types, eff) == Some(label_set(&[IO]));
    argv && code && io
}

/// The compile-time analogue of [`ENTRY`]: `$ @build`, the function the compiler
/// runs *during* compilation. Its `@Code` result is injected in place of its own
/// definition, so a module generates part of itself from whatever the build can
/// see. It is the only context whose effect row discharges `<@IO>` at compile
/// time, which is what separates it from `$ @e`: building is IO.
pub const BUILD: &str = "@build";

/// The build function's mandated signature, in source spelling, for diagnostics.
pub const BUILD_SIG: &str = "{} -> <@Meta, @IO> @Code";

/// Whether a (zonked) type is [`BUILD_SIG`]: `{} -> <@Meta, @IO> @Code`. As with
/// the entry, one name means one signature, so a `@build` that performs anything
/// beyond `@Meta`/`@IO` is a type error at its own signature.
pub fn is_build_type(types: &Types, ty: Type) -> bool {
    let TypeNode::Arrow(from, to, eff) = types.node(ty) else {
        return false;
    };
    let is_con = |t: Type, want: &str| {
        matches!(types.node(t), TypeNode::Con(n) if types.name(n) == want)
    };
    // The row is order-insensitive, so compare label sets rather than a fixed
    // spelling: `<@Meta, @IO>` and `<@IO, @Meta>` are the same row.
    let io_meta = closed_row_labels(types, eff) == Some(label_set(&["@Meta", IO]));
    is_con(from, UNIT) && is_con(to, "@Code") && io_meta
}

// ## Base types

/// The built-in types, by their `@` names.
pub const BASE_TYPES: &[&str] = &[
    "@Int", "@Nat", "@Int8", "@Int16", "@Int32", "@Int64", "@Nat8", "@Nat16", "@Nat32", "@Nat64",
    "@Float32", "@Float64", "@Str", "@Ptr", "@Bool", "@Array", "@Vec", "@Token", "@Code", "@Future",
    "@Chan", "@TVar",
];

pub(crate) fn is_base_type(name: &str) -> bool {
    BASE_TYPES.contains(&name)
}

/// The internal constructor name of a sized tensor. Its type is the application
/// spine `@tensor variance size elem`, one layer per axis. `@`-prefixed so it
/// cannot collide with a user type; the whole thing is erased before runtime.
pub(crate) const TENSOR: &str = "@tensor";

/// The three axis-variance markers, carried as nullary constructors in the
/// `variance` position of the `@tensor` spine (see [`Variance`]).
pub(crate) const VAR_NEUTRAL: &str = "@neutral";

pub(crate) const VAR_CO: &str = "@co";

pub(crate) const VAR_CONTRA: &str = "@contra";

/// The `Type` constructor for a source-level axis variance.
pub(crate) fn variance_con(types: &Types, v: Variance) -> Type {
    types.con(match v {
        Variance::Neutral => VAR_NEUTRAL,
        Variance::Co => VAR_CO,
        Variance::Contra => VAR_CONTRA,
    })
}

/// Build a sized-tensor type from its axis variance, size, and element type.
pub(crate) fn tensor_type(types: &Types, variance: Type, size: Type, elem: Type) -> Type {
    let con = types.con(TENSOR);
    let a = types.app(con, variance);
    let b = types.app(a, size);
    types.app(b, elem)
}

/// The blessed interface each desugar site resolves. CORE declares them as
/// one-field `@struct`s (see [`BLESSED_INTERFACES`]); the compiler
/// resolves a value of the applied type and projects that field.
pub(crate) const IF_STR_LIT: &str = "@IStrLit";

pub(crate) const IF_INT_LIT: &str = "@IIntLit";

pub(crate) const IF_REAL_LIT: &str = "@IRealLit";

pub(crate) const IF_SEQ_LIT: &str = "@ISeqLit";

pub(crate) const IF_RANGE: &str = "@IRange";

pub(crate) const IF_RANGE_FROM: &str = "@IRangeFrom";

pub(crate) const IF_SLICE: &str = "@ISlice";

pub(crate) const IF_SEQ_VIEW: &str = "@ISeqView";

pub(crate) const IF_SEQ_VIEW_BACK: &str = "@ISeqViewBack";

/// The equality a literal PATTERN on a user type compares with: CORE's ordinary
/// `IEq`, the same interface `==` wraps, so a type needs no second instance.
pub(crate) const IF_EQ: &str = "IEq";

// ## Blessed interfaces

/// The interface types the COMPILER resolves. Each is declared in CORE as a
/// one-field `@struct` under its `@`-sigilled name (`$ @IRange : @struct b t =
/// range: b -> b -> t,`); a desugar site resolves a value of that type by type and
/// projects the single field. The names are fixed here because the desugars name
/// them; the method name stays CORE's choice.
pub const BLESSED_INTERFACES: &[&str] = &[
    "@IIntLit",
    "@IRealLit",
    "@IImagLit",
    "@IStrLit",
    "@ISeqLit",
    "@IRange",
    "@IRangeFrom",
    "@IIndex",
    "@ISlice",
    "@ISeqView",
    "@ISeqViewBack",
];

/// Whether `name` (with its `@` sigil) is one of [`BLESSED_INTERFACES`].
pub fn is_blessed_interface(name: &str) -> bool {
    BLESSED_INTERFACES.contains(&name)
}
