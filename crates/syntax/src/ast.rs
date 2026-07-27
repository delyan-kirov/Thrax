//! The abstract syntax tree.
//!
//! Every node borrows the source (`Name` is a `&str` slice) and is arena
//! allocated, so the whole tree is `Copy` and children are `&'a` references.
//! The tree stays close to the surface syntax: sequencing/pipes/cons and other
//! sugar are kept as explicit nodes ([`Expr::BinOp`], [`Expr::List`], ...) and a
//! later `core` pass desugars them. That keeps the parser legible and the tree
//! faithful to what the programmer wrote.

/// An identifier: a borrowed slice of the source.
pub type Name<'a> = &'a str;

/// A whole compilation unit: `@mod NAME` followed by top-level items.
#[derive(Clone, Copy, Debug)]
pub struct Program<'a> {
    pub module: Name<'a>,
    pub items: &'a [Item<'a>],
}

// -- top-level items ---------------------------------------------------------

/// A top-level `$ ...` declaration.
#[derive(Clone, Copy, Debug)]
pub enum Item<'a> {
    /// `$ name [: ty] = body`
    Def {
        name: Name<'a>,
        sig: Option<&'a Ty<'a>>,
        body: &'a Expr<'a>,
    },
    /// `$ Name : @struct = field, ...`
    Struct {
        name: Name<'a>,
        fields: &'a [FieldDecl<'a>],
    },
    /// `$ Name : @union = Tag : payload, ...`
    Union {
        name: Name<'a>,
        variants: &'a [VariantDecl<'a>],
    },
    /// `$ Name : @alias = ty`
    Alias { name: Name<'a>, ty: &'a Ty<'a> },
    /// `$ Name : @effect = op : ty, ...`
    Effect {
        name: Name<'a>,
        ops: &'a [FieldDecl<'a>],
    },
    /// `$ with module [= rename]`
    Import {
        module: &'a [Name<'a>],
        rename: Option<&'a [Name<'a>]>,
    },
    /// `$ @private` / `$ @public`
    Visibility(Visibility),
    /// `$ @assert expr`
    Assert(&'a Expr<'a>),
    /// `$ @run expr`
    Run(&'a Expr<'a>),
    /// `$ @operator.{ op } : ty = expr`
    OperatorDef {
        op: Name<'a>,
        sig: &'a Ty<'a>,
        body: &'a Expr<'a>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Visibility {
    Private,
    Public,
}

/// A `name : Type` field (struct fields and effect operations share this shape).
#[derive(Clone, Copy, Debug)]
pub struct FieldDecl<'a> {
    pub name: Name<'a>,
    pub ty: &'a Ty<'a>,
}

/// One `Tag : payload` arm of a union declaration.
#[derive(Clone, Copy, Debug)]
pub struct VariantDecl<'a> {
    pub tag: Name<'a>,
    pub payload: Payload<'a>,
}

/// A union variant's payload.
#[derive(Clone, Copy, Debug)]
pub enum Payload<'a> {
    /// `Tag` with no payload.
    None,
    /// `Tag : { a, b }` / `Tag : { x: A, y: B }` positional or named fields.
    Fields(&'a [PayloadField<'a>]),
    /// `Tag : Type` a single bare (non-brace) type.
    Bare(&'a Ty<'a>),
}

/// A payload field: positional (`type`) or named (`name : type`).
#[derive(Clone, Copy, Debug)]
pub struct PayloadField<'a> {
    pub name: Option<Name<'a>>,
    pub ty: &'a Ty<'a>,
}

// -- types -------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub enum Ty<'a> {
    /// A type constructor: `Int`, `@int64`, or module-qualified `A.B`.
    Con {
        module: Option<Name<'a>>,
        name: Name<'a>,
    },
    /// A type variable `` `a ``.
    Var(Name<'a>),
    /// Type application `Head Arg` (left-associative at use sites).
    App(&'a Ty<'a>, &'a Ty<'a>),
    /// A function type `From -> To`, optionally carrying an effect row.
    Arrow {
        from: &'a Ty<'a>,
        effect: Option<EffectRow<'a>>,
        to: &'a Ty<'a>,
    },
    /// The unit type `{}`.
    Unit,
    /// A tuple type `{ A, B, ... }` (n >= 1).
    Tuple(&'a [Ty<'a>]),
    /// Named-record parameter sugar `{ x: A, y: B }`.
    Record(&'a [RecField<'a>]),
}

/// A field of the named-record parameter sugar; `with` scopes its fields in.
#[derive(Clone, Copy, Debug)]
pub struct RecField<'a> {
    pub with: bool,
    pub name: Name<'a>,
    pub ty: &'a Ty<'a>,
}

/// An effect row `< A, B | `e >` on a function arrow. An empty `names` with a
/// `tail` is the bare open row (`<`e>` / `<| `e>`); no `tail` is a closed row.
#[derive(Clone, Copy, Debug)]
pub struct EffectRow<'a> {
    pub names: &'a [Name<'a>],
    pub tail: Option<Name<'a>>,
}

// -- patterns ----------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub enum Pattern<'a> {
    /// `_`
    Wild,
    /// A lowercase name that binds the scrutinee.
    Var(Name<'a>),
    Int(i64),
    Real(f64),
    Str(&'a [u8]),
    Bool(bool),
    /// A literal string prefix match: `"GET " ++ rest`.
    StrPrefix {
        prefix: &'a [u8],
        rest: &'a Pattern<'a>,
    },
    /// `head :: tail`
    Cons {
        head: &'a Pattern<'a>,
        tail: &'a Pattern<'a>,
    },
    /// `[ a, b, ..rest ]` / `[]`
    List {
        elems: &'a [Pattern<'a>],
        rest: Option<&'a Pattern<'a>>,
    },
    /// `{ a, b }`
    Tuple(&'a [Pattern<'a>]),
    /// `Type.{ field-patterns }`
    Struct {
        ty: Name<'a>,
        fields: &'a [FieldPat<'a>],
    },
    /// `.Tag`, `Type.Tag`, `Module.Type.Tag`, each with an optional payload.
    Variant {
        module: Option<Name<'a>>,
        ty: Option<Name<'a>>,
        tag: Name<'a>,
        fields: &'a [FieldPat<'a>],
    },
}

/// A field pattern inside a struct pattern or variant payload.
#[derive(Clone, Copy, Debug)]
pub enum FieldPat<'a> {
    /// `.field = pat`
    Named {
        name: Name<'a>,
        pat: &'a Pattern<'a>,
    },
    /// `.field` shorthand, binding the field to its own name.
    Shorthand(Name<'a>),
    /// A positional pattern.
    Positional(&'a Pattern<'a>),
}

// -- expressions -------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub enum Expr<'a> {
    Int(i64),
    Real(f64),
    Str(&'a [u8]),
    Bool(bool),
    /// The unit value `{}`.
    Unit,
    /// A variable, optionally module-qualified (`Module.name`).
    Var {
        module: Option<Name<'a>>,
        name: Name<'a>,
    },
    /// Application by juxtaposition `f x`.
    App(&'a Expr<'a>, &'a Expr<'a>),
    /// A binary operator, keyed by lexeme (`+`, `?=`, `::`, `;`, `|>`, ...).
    BinOp {
        op: Name<'a>,
        lhs: &'a Expr<'a>,
        rhs: &'a Expr<'a>,
    },
    /// A prefix operator; `op` is the canonical name (`neg`, `not`).
    UnOp {
        op: Name<'a>,
        operand: &'a Expr<'a>,
    },
    /// `{ a, b, ... }` (n >= 1).
    Tuple(&'a [Expr<'a>]),
    /// `[ a, b, ... ]` / `[]`.
    List(&'a [Expr<'a>]),
    /// `@array.{ n }` (size form) or `@array.{ .field = n }`; the payload
    /// expression is captured in `size`.
    Array {
        size: &'a Expr<'a>,
    },
    /// Field access / tuple index `record.field` (numeric fields for `.0`).
    Field {
        record: &'a Expr<'a>,
        name: Name<'a>,
    },
    /// `Type.{ ... }` / bare `.{ ... }` (type inferred), with optional `..spread`.
    StructLit {
        ty: Option<Name<'a>>,
        fields: &'a [FieldInit<'a>],
        spread: Option<&'a Expr<'a>>,
    },
    /// `Type.Tag.{ ... }` / `.Tag` variant construction.
    Variant {
        module: Option<Name<'a>>,
        ty: Option<Name<'a>>,
        tag: Name<'a>,
        fields: &'a [FieldInit<'a>],
    },
    /// `let b1, b2 in body`.
    Let {
        bindings: &'a [Binding<'a>],
        body: &'a Expr<'a>,
    },
    /// `if cond then a else b`.
    If {
        cond: &'a Expr<'a>,
        then: &'a Expr<'a>,
        alt: &'a Expr<'a>,
    },
    /// `when scrut is p then e ... [else d]`.
    Match {
        scrut: &'a Expr<'a>,
        arms: &'a [Arm<'a>],
        default: Option<&'a Expr<'a>>,
    },
    /// `\p1 p2 = body`.
    Lambda {
        params: &'a [Pattern<'a>],
        body: &'a Expr<'a>,
    },
    /// `with subject in body` field-scoping.
    With {
        subject: &'a Expr<'a>,
        body: &'a Expr<'a>,
    },
    /// `do body [ctl k clauses ...]`.
    Handle {
        body: &'a Expr<'a>,
        handler: Option<&'a Handler<'a>>,
    },
    /// `defer cleanup do body`.
    Defer {
        cleanup: &'a Expr<'a>,
        body: &'a Expr<'a>,
    },
    /// `@extern "abi" "symbol" "lib"`.
    Extern {
        abi: &'a [u8],
        symbol: &'a [u8],
        lib: &'a [u8],
    },
}

/// A field initializer in a struct literal or variant payload.
#[derive(Clone, Copy, Debug)]
pub enum FieldInit<'a> {
    /// `.field = value`
    Named { name: Name<'a>, value: &'a Expr<'a> },
    /// A positional value.
    Positional(&'a Expr<'a>),
}

/// One binding of a (possibly comma-chained) `let`.
#[derive(Clone, Copy, Debug)]
pub struct Binding<'a> {
    pub pat: &'a Pattern<'a>,
    pub sig: Option<&'a Ty<'a>>,
    pub value: &'a Expr<'a>,
}

/// One arm of a `when`. Or-patterns (`is p1 is p2`) share a body and guard.
#[derive(Clone, Copy, Debug)]
pub struct Arm<'a> {
    pub patterns: &'a [Pattern<'a>],
    pub guard: Option<&'a Expr<'a>>,
    pub body: &'a Expr<'a>,
}

/// A `ctl k ...` handler attached to a `do` block.
#[derive(Clone, Copy, Debug)]
pub struct Handler<'a> {
    pub continuation: Name<'a>,
    pub clauses: &'a [Clause<'a>],
    pub default: Option<(Name<'a>, &'a Expr<'a>)>,
}

/// One `is Effect.op arg = body` handler clause. `arg` binds the operation's
/// argument; the resumable continuation is bound once by [`Handler`].
#[derive(Clone, Copy, Debug)]
pub struct Clause<'a> {
    pub effect: Option<Name<'a>>,
    pub op: Name<'a>,
    pub arg: Name<'a>,
    pub body: &'a Expr<'a>,
}
