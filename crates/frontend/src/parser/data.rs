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

use utilities::{Aol, Interner, SecondaryMap, Span, Store, StrId};

/// The stores that back a parsed tree. Every handle in the tree indexes into one
/// of these; reads go through [`Store::lookup`] / [`Interner::resolve`].
#[derive(Default)]
pub struct Ast {
    pub exprs: Store<Expr>,
    pub tys: Store<Ty>,
    pub pats: Store<Pattern>,
    pub strings: Interner,
    /// Source span of each `Expr` node, for diagnostics. Populated by the parser;
    /// a node absent here (e.g. one synthesized by a later pass) has no span.
    pub expr_spans: SecondaryMap<Expr, Span>,
    /// Source span of each `Ty` node, for diagnostics on type annotations.
    pub ty_spans: SecondaryMap<Ty, Span>,
}

impl Ast {
    pub fn new() -> Ast {
        Ast::default()
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
    pub items: Box<[Item]>,
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
        implicits: Box<[FieldDecl]>,
        body: Aol<Expr>,
    },
    /// `$ Name : @struct [a b ...] = [with Other, ...] field, ...`. `params` are
    /// the declared type parameters, in order; when omitted they are inferred from
    /// the free type variables in the fields. `includes` are struct types whose
    /// fields are copied in (before the declared ones), in order. This is a
    /// declaration-time splice for convenience; it creates NO type relationship (no
    /// subtyping), just a fresh struct that repeats those fields.
    Struct {
        name: StrId,
        params: Box<[StrId]>,
        includes: Box<[StrId]>,
        fields: Box<[FieldDecl]>,
    },
    /// `$ Name : @union [a b ...] = [with Other, ...] Tag : payload, ...`. `params`
    /// are the declared type parameters (inferred from the variants when omitted).
    /// `includes` are union types whose variants are copied in (before the declared
    /// ones). As for structs this is a splice, not a subtype relationship.
    Union {
        name: StrId,
        params: Box<[StrId]>,
        includes: Box<[StrId]>,
        variants: Box<[VariantDecl]>,
    },
    /// `$ Name : @alias = ty`
    Alias { name: StrId, ty: Aol<Ty> },
    /// `$ Name : @effect = op : ty, ...`
    Effect { name: StrId, ops: Box<[FieldDecl]> },
    /// `$ Name : @codata [a b ...] = obs : ty, ...` -- a coinductive type defined
    /// by its observations (destructors), dual to a struct. `params` are the
    /// declared type parameters (inferred from the observations when omitted).
    /// Observing is non-memoized: each observation is a thunk, run afresh on every
    /// look.
    Codata {
        name: StrId,
        params: Box<[StrId]>,
        observations: Box<[FieldDecl]>,
    },
    /// `$ with module [= rename]`
    Import {
        module: Box<[StrId]>,
        rename: Option<Box<[StrId]>>,
    },
    /// `$ @private` / `$ @public`
    Visibility(Visibility),
    /// `$ @assert expr`
    Assert(Aol<Expr>),
    /// `$ @run expr`
    Run(Aol<Expr>),
    /// `$ @operator.{ op } : ty = expr`
    OperatorDef {
        op: StrId,
        sig: Aol<Ty>,
        body: Aol<Expr>,
    },
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
    Fields(Box<[PayloadField]>),
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

#[derive(Debug)]
pub enum Ty {
    /// A type constructor: `Int`, `@int64`, or module-qualified `A.B`.
    Con { module: Option<StrId>, name: StrId },
    /// A type variable `` `a ``.
    Var(StrId),
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
    Tuple(Box<[Aol<Ty>]>),
    /// A record type `{ x: A, y: B }` (closed) or `{ x: A | r }` (open, `tail` is
    /// the row variable). A closed record with no `with` fields in parameter
    /// position is also the named-record parameter sugar; an open one (`tail`
    /// present) is always a row-polymorphic record type.
    Record {
        fields: Box<[RecField]>,
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
    pub names: Box<[StrId]>,
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
        elems: Box<[Aol<Pattern>]>,
        rest: Option<Aol<Pattern>>,
    },
    /// `{ a, b }`
    Tuple(Box<[Aol<Pattern>]>),
    /// `Type.{ field-patterns }`
    Struct {
        ty: StrId,
        fields: Box<[FieldPat]>,
    },
    /// An anonymous record pattern `{ .x = p, .y = q [, ..rest] }`: match a record
    /// (open-row value or struct) by field name. `rest` binds the remaining fields
    /// (`..name`) or discards them (`.._`); absent, the unlisted fields are ignored.
    Record {
        fields: Box<[FieldPat]>,
        rest: Option<Aol<Pattern>>,
    },
    /// `.Tag`, `Type.Tag`, `Module.Type.Tag`, each with an optional payload.
    Variant {
        module: Option<StrId>,
        ty: Option<StrId>,
        tag: StrId,
        fields: Box<[FieldPat]>,
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
    Tuple(Box<[Aol<Expr>]>),
    /// `[ a, b, ... ]` / `[]`.
    List(Box<[Aol<Expr>]>),
    /// `@array.{ n }` (size form) or `@array.{ .field = n }`.
    Array {
        size: Aol<Expr>,
    },
    /// Field access / tuple index `record.field` (numeric fields for `.0`).
    Field {
        record: Aol<Expr>,
        name: StrId,
    },
    /// `Type.{ ... }` / bare `.{ ... }` (type inferred), with optional `..spread`.
    StructLit {
        ty: Option<StrId>,
        fields: Box<[FieldInit]>,
        spread: Option<Aol<Expr>>,
    },
    /// An anonymous, structural record value: `{ .foo = 1, .bar = 2 }` (plain),
    /// `{ .foo = v | base }` (update: the rest come from `base`), or
    /// `{ .foo = 1, with base }` (stack: this record's fields on top of `base`'s).
    /// Its type is a [`Type::Record`] row, not a nominal struct.
    Record {
        fields: Box<[FieldInit]>,
        with: Option<Aol<Expr>>,
        update: Option<Aol<Expr>>,
    },
    /// `Type.Tag.{ ... }` / `.Tag` variant construction.
    Variant {
        module: Option<StrId>,
        ty: Option<StrId>,
        tag: StrId,
        fields: Box<[FieldInit]>,
    },
    /// `let b1, b2 in body`.
    Let {
        bindings: Box<[Binding]>,
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
        arms: Box<[Arm]>,
        default: Option<Aol<Expr>>,
    },
    /// `\p1 p2 = body`.
    Lambda {
        params: Box<[Aol<Pattern>]>,
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
    /// `callee @ctx e` / `callee @ctx { .name = e, .. }`: override the implicit
    /// `@ctx` arguments of `callee`. `overrides` are the given ones (a single
    /// `Positional` for the one-implicit form, or `.name = e` for the record
    /// form); `rest` (`..`) fills any unspecified implicits by name from scope.
    Ctx {
        callee: Aol<Expr>,
        overrides: Box<[FieldInit]>,
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
    pub patterns: Box<[Aol<Pattern>]>,
    pub guard: Option<Aol<Expr>>,
    pub body: Aol<Expr>,
}

/// A `ctl k ...` handler attached to a `do` block.
#[derive(Debug)]
pub struct Handler {
    pub continuation: StrId,
    pub clauses: Box<[Clause]>,
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
