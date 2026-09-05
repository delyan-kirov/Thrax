//! The abstract syntax tree, in handle form.
//!
//! Nodes carry no lifetime. A child is an [`Aol`] handle into one of the [`Ast`]
//! stores (`Aol<Expr>`, `Aol<Ty>`, `Aol<Pattern>`) rather than a `&'a` reference,
//! and every name/string is a [`StrId`] into the [`Ast`]'s interner rather than a
//! slice of the source. So the whole tree is self-owned: it outlives the source
//! it was parsed from, it can be annotated by node identity (see
//! [`utilities::SecondaryMap`]), and it can be rewritten in place ([`utilities::Store::commit`]).
//!
//! The tree stays close to the surface syntax: sequencing/pipes/cons and other
//! sugar are kept as explicit nodes ([`Expr::BinOp`], [`Expr::List`], ...) and a
//! later `core` pass desugars them.

use utilities::{Aol, Interner, SecondaryMap, Slice, Span, Store, StrId};

/// The stores that back a parsed tree. Every handle in the tree indexes into one
/// of these; reads go through [`Store::lookup`] / [`Interner::resolve`].
///
/// Variable-length child lists are NOT boxed onto the node; they are runs in a
/// per-element-type [`Store`], addressed by a [`Slice`] handle. Read one with
/// [`Ast::slice`] and build one with [`Ast::make_slice`] (both generic over the
/// element type via the [`Sliced`] trait), so the whole tree stays arena-owned and
/// frees all at once.
#[derive(Default)]
pub struct Ast {
    pub exprs: Store<Expr>,
    pub tys: Store<Ty>,
    pub pats: Store<Pattern>,
    pub strings: Interner,
    // Backing stores for the tree's variable-length runs, one per element type.
    expr_seqs: Store<Aol<Expr>>,
    ty_seqs: Store<Aol<Ty>>,
    pat_seqs: Store<Aol<Pattern>>,
    str_seqs: Store<StrId>,
    field_decls: Store<FieldDecl>,
    rec_fields: Store<RecField>,
    field_inits: Store<FieldInit>,
    field_pats: Store<FieldPat>,
    payload_fields: Store<PayloadField>,
    variant_decls: Store<VariantDecl>,
    clauses: Store<Clause>,
    arms: Store<Arm>,
    bindings: Store<Binding>,
    items: Store<Item>,
    slice_slots: Store<SliceSlot>,
    /// Source span of each `Expr` node, for diagnostics. Populated by the parser;
    /// a node absent here (e.g. one synthesized by a later pass) has no span.
    pub expr_spans: SecondaryMap<Expr, Span>,
    /// Source span of each `Ty` node, for diagnostics on type annotations.
    pub ty_spans: SecondaryMap<Ty, Span>,
}

/// An element type stored in a run: names the [`Store`] on the [`Ast`] that holds
/// its runs, so [`Ast::slice`] / [`Ast::make_slice`] work uniformly.
pub trait Sliced: Sized {
    fn store(ast: &Ast) -> &Store<Self>;
    fn store_mut(ast: &mut Ast) -> &mut Store<Self>;
}

macro_rules! sliced {
    ($($ty:ty => $field:ident),+ $(,)?) => {$(
        impl Sliced for $ty {
            fn store(ast: &Ast) -> &Store<Self> { &ast.$field }
            fn store_mut(ast: &mut Ast) -> &mut Store<Self> { &mut ast.$field }
        }
    )+};
}

sliced! {
    Aol<Expr> => expr_seqs,
    Aol<Ty> => ty_seqs,
    Aol<Pattern> => pat_seqs,
    StrId => str_seqs,
    FieldDecl => field_decls,
    RecField => rec_fields,
    FieldInit => field_inits,
    FieldPat => field_pats,
    PayloadField => payload_fields,
    VariantDecl => variant_decls,
    Clause => clauses,
    Arm => arms,
    Binding => bindings,
    Item => items,
    SliceSlot => slice_slots,
}

impl Ast {
    pub fn new() -> Ast {
        Ast::default()
    }

    /// Read a run's elements.
    pub fn slice<T: Sliced>(&self, s: Slice<T>) -> &[T] {
        T::store(self).lookup_slice(s)
    }

    /// Store a run contiguously and return its handle.
    pub fn make_slice<T: Sliced>(&mut self, xs: impl IntoIterator<Item = T>) -> Slice<T> {
        T::store_mut(self).create_slice(xs)
    }

    /// Resolve an interned name to text.
    pub fn text(&self, id: StrId) -> &str {
        self.strings.resolve(id)
    }
    /// Resolve an interned byte string (literals are byte vectors).
    pub fn bytes(&self, id: StrId) -> &[u8] {
        self.strings.bytes(id)
    }
    pub fn expr(&self, id: Aol<Expr>) -> &Expr {
        self.exprs.lookup(id)
    }
    /// The source span recorded for an `Expr`, if the parser stamped one.
    pub fn expr_span(&self, id: Aol<Expr>) -> Option<Span> {
        self.expr_spans.get(id).copied()
    }
    /// The source span recorded for a `Ty`, if the parser stamped one.
    pub fn ty_span(&self, id: Aol<Ty>) -> Option<Span> {
        self.ty_spans.get(id).copied()
    }
    pub fn ty(&self, id: Aol<Ty>) -> &Ty {
        self.tys.lookup(id)
    }
    pub fn pat(&self, id: Aol<Pattern>) -> &Pattern {
        self.pats.lookup(id)
    }
}

/// A whole compilation unit: `@mod NAME` followed by top-level items.
#[derive(Debug)]
pub struct Program {
    pub module: StrId,
    pub items: Slice<Item>,
}

// -- top-level items ---------------------------------------------------------

/// A top-level `$ ...` declaration.
#[derive(Debug)]
pub enum Item {
    /// `$ name [: ty] [@ctx c : T ...] = body`. `implicits` are the `@ctx`
    /// declarations: implicit parameters resolved by name at each call site and
    /// passed as leading arguments (dictionary passing). Empty for a normal def.
    Def {
        name: StrId,
        sig: Option<Aol<Ty>>,
        implicits: Slice<FieldDecl>,
        body: Aol<Expr>,
    },
    /// `$ Name : @struct [@extern "abi"] [a b ...] = [with Other, ...] field, ...`.
    /// `params` are the declared type parameters, in order; when omitted they are
    /// inferred from the free type variables in the fields. `includes` are struct
    /// types whose fields are copied in (before the declared ones), in order. This is
    /// a declaration-time splice for convenience; it creates NO type relationship (no
    /// subtyping), just a fresh struct that repeats those fields. `abi` is set when
    /// the struct is a C-layout foreign type (`@struct @extern "C"`): its runtime
    /// representation is a flat, unboxed C struct, so it can cross the `@extern`
    /// boundary by value. The string names the ABI, matching the function form.
    Struct {
        name: StrId,
        params: Slice<StrId>,
        includes: Slice<StrId>,
        fields: Slice<FieldDecl>,
        abi: Option<StrId>,
        /// A C `union` (`@union @extern "abi"`): the members share offset 0 and the
        /// size is the largest. Parsed with the struct-field syntax and otherwise
        /// handled as a C-repr struct, so it reuses construction and access.
        c_union: bool,
    },
    /// `$ Name : @union [a b ...] = [with Other, ...] Tag : payload, ...`. `params`
    /// are the declared type parameters (inferred from the variants when omitted).
    /// `includes` are union types whose variants are copied in (before the declared
    /// ones). As for structs this is a splice, not a subtype relationship.
    Union {
        name: StrId,
        params: Slice<StrId>,
        includes: Slice<StrId>,
        variants: Slice<VariantDecl>,
    },
    /// `$ Name : @alias [a b ...] = ty`. `params` are the declared type parameters
    /// (mandatory: every type variable used in `ty` must be listed). An alias may
    /// partially instantiate another generic type, e.g. `MapInt : @alias v = Map Int v`.
    Alias {
        name: StrId,
        params: Slice<StrId>,
        ty: Aol<Ty>,
    },
    /// `$ Name : @effect = op : ty, ...`
    Effect { name: StrId, ops: Slice<FieldDecl> },
    /// `$ Name : @codata [a b ...] = obs : ty, ...` -- a coinductive type defined
    /// by its observations (destructors), dual to a struct. `params` are the
    /// declared type parameters (inferred from the observations when omitted).
    /// Observing is non-memoized: each observation is a thunk, run afresh on every
    /// look.
    Codata {
        name: StrId,
        params: Slice<StrId>,
        observations: Slice<FieldDecl>,
    },
    /// `$ with module [= rename]`
    Import {
        module: Slice<StrId>,
        rename: Option<Slice<StrId>>,
    },
    /// `$ @private` / `$ @public`
    Visibility(Visibility),
    /// `$ @assert expr`
    Assert(Aol<Expr>),
    /// `$ @run expr`
    Run(Aol<Expr>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Visibility {
    Private,
    Public,
}

/// A `name : Type` field (struct fields and effect operations share this shape).
#[derive(Clone, Copy, Debug)]
pub struct FieldDecl {
    pub name: StrId,
    pub ty: Aol<Ty>,
}

/// One `Tag : payload` arm of a union declaration.
#[derive(Debug)]
pub struct VariantDecl {
    pub tag: StrId,
    pub payload: Payload,
}

/// A union variant's payload.
#[derive(Debug)]
pub enum Payload {
    /// `Tag` with no payload.
    None,
    /// `Tag : { a, b }` / `Tag : { x: A, y: B }` positional or named fields.
    Fields(Slice<PayloadField>),
    /// `Tag : Type` a single bare (non-brace) type.
    Bare(Aol<Ty>),
}

/// A payload field: positional (`type`) or named (`name : type`).
#[derive(Clone, Copy, Debug)]
pub struct PayloadField {
    pub name: Option<StrId>,
    pub ty: Aol<Ty>,
}

// -- types -------------------------------------------------------------------

/// The variance of a tensor axis. `Contra` is an upper (contravariant) index, a
/// vector/column component living in `V`; `Co` is a lower (covariant) index, a
/// covector/row living in the dual `V*`. `Neutral` is an unmarked axis, compatible
/// with either (so plain `[n]T` code interoperates with variance-typed tensors).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variance {
    Neutral,
    Co,
    Contra,
}

#[derive(Debug)]
pub enum Ty {
    /// A type constructor: `Int`, `@int64`, or module-qualified `A.B`.
    Con { module: Option<StrId>, name: StrId },
    /// A type variable `` `a ``.
    Var(StrId),
    /// A type-level natural literal (a size inside `[n]T`).
    Nat(u64),
    /// A size sum `a + b` inside a tensor size (`[n+m]T`).
    SizeAdd(Aol<Ty>, Aol<Ty>),
    /// A size product `a * b` inside a tensor size (`[n*m]T`).
    SizeMul(Aol<Ty>, Aol<Ty>),
    /// A sized tensor type `[size]elem`, e.g. `[5]Int` or `[n]a`. `size` is a `Nat`
    /// literal or a (Nat-kinded) `Var`; `elem` is the element type. `variance` tags
    /// the axis: `[@Contra n]`/`[@Co n]` are the two standard tensor-index kinds, a
    /// bare `[n]` is `Neutral`.
    Sized {
        variance: Variance,
        size: Aol<Ty>,
        elem: Aol<Ty>,
    },
    /// Type application `Head Arg` (left-associative at use sites).
    App(Aol<Ty>, Aol<Ty>),
    /// A function type `From -> To`, optionally carrying an effect row.
    Arrow {
        from: Aol<Ty>,
        effect: Option<EffectRow>,
        to: Aol<Ty>,
    },
    /// The unit type `{}`.
    Unit,
    /// A tuple type `{ A, B, ... }` (n >= 1).
    Tuple(Slice<Aol<Ty>>),
    /// A record type `{ x: A, y: B }` (closed) or `{ x: A | r }` (open, `tail` is
    /// the row variable). A closed record with no `with` fields in parameter
    /// position is also the named-record parameter sugar; an open one (`tail`
    /// present) is always a row-polymorphic record type.
    Record {
        fields: Slice<RecField>,
        tail: Option<StrId>,
    },
}

/// A field of the named-record parameter sugar; `with` scopes its fields in.
#[derive(Clone, Copy, Debug)]
pub struct RecField {
    pub with: bool,
    pub name: StrId,
    pub ty: Aol<Ty>,
}

/// An effect row `< A, B | `e >` on a function arrow.
#[derive(Debug)]
pub struct EffectRow {
    pub names: Slice<StrId>,
    pub tail: Option<StrId>,
}

// -- patterns ----------------------------------------------------------------

#[derive(Debug)]
pub enum Pattern {
    /// `_`
    Wild,
    /// A lowercase name that binds the scrutinee.
    Var(StrId),
    Int(i64),
    Real(f64),
    Str(StrId),
    Bool(bool),
    /// An inclusive numeric range `lo ... hi`: matches when `lo <= x <= hi`. Both
    /// bounds are numeric literal patterns (`Int`/`Real`). An open range `lo ...`
    /// omits the upper bound and matches when `lo <= x`. Refutable; binds nothing.
    Range {
        lo: Aol<Pattern>,
        hi: Option<Aol<Pattern>>,
    },
    /// A literal string prefix match: `"GET " ++ rest`.
    StrPrefix {
        prefix: StrId,
        rest: Aol<Pattern>,
    },
    /// `head :: tail`
    Cons {
        head: Aol<Pattern>,
        tail: Aol<Pattern>,
    },
    /// `[ a, b, ..rest ]` / `[]`
    List {
        elems: Slice<Aol<Pattern>>,
        rest: Option<Aol<Pattern>>,
    },
    /// `{ a, b }`
    Tuple(Slice<Aol<Pattern>>),
    /// `Type.{ field-patterns }`
    Struct {
        ty: StrId,
        fields: Slice<FieldPat>,
    },
    /// An anonymous record pattern `{ .x = p, .y = q [, ..rest] }`: match a record
    /// (open-row value or struct) by field name. `rest` binds the remaining fields
    /// (`..name`) or discards them (`.._`); absent, the unlisted fields are ignored.
    Record {
        fields: Slice<FieldPat>,
        rest: Option<Aol<Pattern>>,
    },
    /// `.Tag`, `Type.Tag`, `Module.Type.Tag`, each with an optional payload.
    Variant {
        module: Option<StrId>,
        ty: Option<StrId>,
        tag: StrId,
        fields: Slice<FieldPat>,
    },
}

/// A field pattern inside a struct pattern or variant payload.
#[derive(Clone, Copy, Debug)]
pub enum FieldPat {
    /// `.field = pat`
    Named { name: StrId, pat: Aol<Pattern> },
    /// `.field` shorthand, binding the field to its own name.
    Shorthand(StrId),
    /// A positional pattern.
    Positional(Aol<Pattern>),
}

// -- expressions -------------------------------------------------------------

/// One axis position in a multi-axis tensor slice `t.[s0, s1, ...]`.
#[derive(Debug)]
pub enum SliceSlot {
    /// `i`: index (reduce) this axis.
    Index(Aol<Expr>),
    /// `lo ... hi`: keep this axis, narrowed to the inclusive range `[lo, hi]`.
    Range(Aol<Expr>, Aol<Expr>),
    /// `..`: keep this axis whole.
    Full,
}

#[derive(Debug)]
pub enum Expr {
    Int(i64),
    Real(f64),
    Str(StrId),
    Bool(bool),
    /// The unit value `{}`.
    Unit,
    /// A variable, optionally module-qualified (`Module.name`).
    Var {
        module: Option<StrId>,
        name: StrId,
    },
    /// Application by juxtaposition `f x`.
    App(Aol<Expr>, Aol<Expr>),
    /// A binary operator, keyed by lexeme (`+`, `?=`, `::`, `;`, `|>`, ...).
    BinOp {
        op: StrId,
        lhs: Aol<Expr>,
        rhs: Aol<Expr>,
    },
    /// A prefix operator; `op` is the canonical name (`neg`, `not`).
    UnOp {
        op: StrId,
        operand: Aol<Expr>,
    },
    /// `{ a, b, ... }` (n >= 1).
    Tuple(Slice<Aol<Expr>>),
    /// `[ a, b, ... ]` / `[]`.
    List(Slice<Aol<Expr>>),
    /// The inclusive range builder `[lo ... hi]`. A TYPE-DIRECTED literal: it
    /// materializes into a sized tensor `[n]T` (when a tensor is expected and the
    /// bounds are literals, so the length is a compile-time constant), otherwise a
    /// `List Int` (the default). Both bounds are `Int`. An open range `[lo ...]`
    /// omits the upper bound and builds an infinite codata `Stream Int`.
    Range {
        lo: Aol<Expr>,
        hi: Option<Aol<Expr>>,
    },
    /// Multi-axis tensor slice `recv.[s0, s1, ...]` where at least one slot is a
    /// range or a full `..` (an all-index access desugars to `index` instead). An
    /// `Index` slot reduces its axis; a `Range`/`Full` slot keeps it (a view).
    Slice {
        recv: Aol<Expr>,
        slots: Slice<SliceSlot>,
    },
    /// `@array.{ n }` (size form) or `@array.{ .field = n }`.
    Array {
        size: Aol<Expr>,
    },
    /// Field access / tuple index `record.field` (numeric fields for `.0`).
    Field {
        record: Aol<Expr>,
        name: StrId,
    },
    /// `Type.{ ... }` / bare `.{ ... }` (type inferred), with an optional trailing
    /// `| base` update: `spread` is `base`, its unlisted fields filling the result.
    StructLit {
        ty: Option<StrId>,
        fields: Slice<FieldInit>,
        spread: Option<Aol<Expr>>,
    },
    /// An anonymous, structural record value: `{ .foo = 1, .bar = 2 }` (plain),
    /// `{ .foo = v | base }` (update: the rest come from `base`), or
    /// `{ .foo = 1, with base }` (stack: this record's fields on top of `base`'s).
    /// Its type is a [`Type::Record`] row, not a nominal struct.
    Record {
        fields: Slice<FieldInit>,
        with: Option<Aol<Expr>>,
        update: Option<Aol<Expr>>,
    },
    /// `Type.Tag.{ ... }` / `.Tag` variant construction.
    Variant {
        module: Option<StrId>,
        ty: Option<StrId>,
        tag: StrId,
        fields: Slice<FieldInit>,
    },
    /// `let b1, b2 in body`.
    Let {
        bindings: Slice<Binding>,
        body: Aol<Expr>,
    },
    /// `if cond then a else b`.
    If {
        cond: Aol<Expr>,
        then: Aol<Expr>,
        alt: Aol<Expr>,
    },
    /// `when scrut is p then e ... [else d]`.
    Match {
        scrut: Aol<Expr>,
        arms: Slice<Arm>,
        default: Option<Aol<Expr>>,
    },
    /// `\p1 p2 = body`.
    Lambda {
        params: Slice<Aol<Pattern>>,
        body: Aol<Expr>,
    },
    /// `with subject in body` field-scoping.
    With {
        subject: Aol<Expr>,
        body: Aol<Expr>,
    },
    /// `do body [ctl k clauses ...]`.
    Handle {
        body: Aol<Expr>,
        handler: Option<Box<Handler>>,
    },
    /// `defer cleanup do body`.
    Defer {
        cleanup: Aol<Expr>,
        body: Aol<Expr>,
    },
    /// `@extern "abi" "symbol" "lib"`.
    Extern {
        abi: StrId,
        symbol: StrId,
        lib: StrId,
    },
    /// `(e : T)`: an ascription. Checks `e` against `T` and has type `T`. Purely a
    /// checking hint; lowering emits `e` unchanged.
    Ascribe {
        expr: Aol<Expr>,
        ty: Aol<Ty>,
    },
    /// `callee @ctx e` / `callee @ctx { .name = e, .. }`: override the implicit
    /// `@ctx` arguments of `callee`. `overrides` are the given ones (a single
    /// `Positional` for the one-implicit form, or `.name = e` for the record
    /// form); `rest` (`..`) fills any unspecified implicits by name from scope.
    Ctx {
        callee: Aol<Expr>,
        overrides: Slice<FieldInit>,
        rest: bool,
    },
}

/// A field initializer in a struct literal or variant payload.
#[derive(Clone, Copy, Debug)]
pub enum FieldInit {
    /// `.field = value`
    Named { name: StrId, value: Aol<Expr> },
    /// A positional value.
    Positional(Aol<Expr>),
}

/// One binding of a (possibly comma-chained) `let`.
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    pub pat: Aol<Pattern>,
    pub sig: Option<Aol<Ty>>,
    pub value: Aol<Expr>,
}

/// One arm of a `when`. Or-patterns (`is p1 is p2`) share a body and guard.
#[derive(Debug)]
pub struct Arm {
    pub patterns: Slice<Aol<Pattern>>,
    pub guard: Option<Aol<Expr>>,
    pub body: Aol<Expr>,
}

/// A `ctl k ...` handler attached to a `do` block.
#[derive(Debug)]
pub struct Handler {
    pub continuation: StrId,
    pub clauses: Slice<Clause>,
    pub default: Option<(StrId, Aol<Expr>)>,
}

/// One `is Effect.op arg = body` handler clause.
#[derive(Clone, Copy, Debug)]
pub struct Clause {
    pub effect: Option<StrId>,
    pub op: StrId,
    pub arg: StrId,
    pub body: Aol<Expr>,
}
