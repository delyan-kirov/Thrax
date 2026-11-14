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
//! sugar are kept as explicit nodes ([`Expr::BinOp`], [`Expr::List`], ...), which
//! [`crate::desugar`] removes.

use utilities::{Aol, SecondaryMap, Slice, Span, Store, StrId};

/// The `@extern` ABI that names a runtime builtin rather than a foreign symbol.
pub const BUILTIN_ABI: &str = "ares";

/// The `@extern` ABI of a load from C memory: `{ p: @Ptr, i: @Int } -> T` reads
/// element `i` of a C array of `T` at `p`. It names no symbol or library.
pub const LOAD_ABI: &str = "load";

/// The stores that back a parsed tree. Every handle in the tree indexes into one
/// of these; reads go through [`Store::lookup`]. Names are [`StrId`]s into the
/// process-wide interner.
///
/// Variable-length child lists are NOT boxed onto the node; they are runs in a
/// per-element-type [`Store`], addressed by a [`Slice`] handle. Read one with
/// [`Ast::slice`] and build one with [`Ast::make_slice`] (both generic over the
/// element type via the [`Sliced`] trait), so the whole tree stays arena-owned and
/// frees all at once.
#[derive(Clone, Default)]
pub struct Ast {
    pub exprs: Store<Expr>,
    pub tys: Store<Ty>,
    pub pats: Store<Pattern>,
    // Backing stores for the tree's variable-length runs, one per element type.
    expr_seqs: Store<Aol<Expr>>,
    ty_seqs: Store<Aol<Ty>>,
    pat_seqs: Store<Aol<Pattern>>,
    str_seqs: Store<StrId>,
    field_decls: Store<FieldDecl>,
    entry_splices: Store<EntrySplice>,
    rec_fields: Store<RecField>,
    field_inits: Store<FieldInit>,
    field_pats: Store<FieldPat>,
    payload_fields: Store<PayloadField>,
    variant_decls: Store<VariantDecl>,
    clauses: Store<Clause>,
    arms: Store<Arm>,
    lam_params: Store<LamParam>,
    row_entries: Store<RowEntry>,
    items: Store<Item>,
    item_spans: Store<ItemSpan>,
    slice_slots: Store<SliceSlot>,
    use_fields: Store<UseField>,
    /// The `@use` fields of a definition's signature, keyed by the signature (the
    /// handle `Item::Def::sig` and the checker's context table carry), in the
    /// order written. Present only when the signature declares `@use`.
    pub use_params: SecondaryMap<Ty, Slice<UseField>>,
    /// Source span of each `Expr` node, for diagnostics. Populated by the parser;
    /// a node absent here (e.g. one synthesized by a later pass) has no span.
    pub expr_spans: SecondaryMap<Expr, Span>,
    /// Source span of each `Ty` node, for diagnostics on type annotations.
    pub ty_spans: SecondaryMap<Ty, Span>,
    /// Source span of each `Pattern` node, for diagnostics on match arms.
    pub pat_spans: SecondaryMap<Pattern, Span>,
    /// Source span of the name of each named-parameter arrow `(x: A) -> B`.
    pub binder_spans: SecondaryMap<Ty, Span>,
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
    EntrySplice => entry_splices,
    RecField => rec_fields,
    FieldInit => field_inits,
    FieldPat => field_pats,
    PayloadField => payload_fields,
    VariantDecl => variant_decls,
    Clause => clauses,
    Arm => arms,
    LamParam => lam_params,
    RowEntry => row_entries,
    Item => items,
    ItemSpan => item_spans,
    SliceSlot => slice_slots,
    UseField => use_fields,
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
    pub fn text(&self, id: StrId) -> &'static str {
        id.as_str()
    }
    /// Resolve an interned byte string (literals are byte vectors).
    pub fn bytes(&self, id: StrId) -> &'static [u8] {
        id.bytes()
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
    /// The source span recorded for a `Pattern`, if the parser stamped one.
    pub fn pat_span(&self, id: Aol<Pattern>) -> Option<Span> {
        self.pat_spans.get(id).copied()
    }
    pub fn pat(&self, id: Aol<Pattern>) -> &Pattern {
        self.pats.lookup(id)
    }

    /// A declared type rendered back to source that parses as the same type, or
    /// None while it still holds a `&` splice. Reflection reports field and
    /// payload types this way, for a generator to copy them into a declaration.
    pub fn ty_source(&self, ty: Aol<Ty>) -> Option<String> {
        self.ty_src(ty, 0)
    }

    /// `prec` is what the context binds: 0 takes any type, 1 an application (an
    /// arrow's domain), 2 an atom (an application's argument).
    fn ty_src(&self, ty: Aol<Ty>, prec: u8) -> Option<String> {
        let wrap = |s: String, at: u8| if prec > at { format!("({s})") } else { s };
        Some(match self.ty(ty) {
            Ty::Con { module: Some(m), name } => format!("{m}.{name}"),
            Ty::Con { module: None, name } => name.to_string(),
            Ty::Var(name) => name.to_string(),
            Ty::Nat(_) | Ty::SizeAdd(..) | Ty::SizeMul(..) => self.size_src(ty, 0)?,
            Ty::Sized { size, elem } => {
                wrap(format!("@Tensor {} {}", self.size_src(*size, 2)?, self.ty_src(*elem, 2)?), 1)
            }
            Ty::App(head, arg) => wrap(format!("{} {}", self.ty_src(*head, 1)?, self.ty_src(*arg, 2)?), 1),
            Ty::MetaE(_) => return None,
            Ty::Arrow { binder, from, effect, to } => {
                let row = match effect {
                    Some(row) => format!("{} ", self.row_src(row)?),
                    None => String::new(),
                };
                let from = match binder {
                    Some(x) => format!("({x}: {})", self.ty_src(*from, 0)?),
                    None => self.ty_src(*from, 1)?,
                };
                wrap(format!("{from} -> {row}{}", self.ty_src(*to, 0)?), 0)
            }
            Ty::Unit => "{}".to_string(),
            Ty::Tuple(items) => {
                let items: Option<Vec<String>> = self.slice(*items).iter().map(|t| self.ty_src(*t, 0)).collect();
                format!("{{{}}}", items?.join(", "))
            }
            Ty::Record { fields, tail } => {
                let fields: Option<Vec<String>> = self
                    .slice(*fields)
                    .iter()
                    .map(|f| Some(format!("{}: {}", f.name, self.ty_src(f.ty, 0)?)))
                    .collect();
                let tail = tail.map_or(String::new(), |t| format!(" | {t}"));
                format!("{{{}{tail}}}", fields?.join(", "))
            }
        })
    }

    /// A tensor size: `*` binds tighter than `+`. `prec` is what the context
    /// binds: 0 a sum, 1 a product, 2 an atom (a tensor's size argument).
    fn size_src(&self, ty: Aol<Ty>, prec: u8) -> Option<String> {
        Some(match self.ty(ty) {
            Ty::Nat(n) => n.to_string(),
            Ty::SizeAdd(a, b) => {
                let s = format!("{} + {}", self.size_src(*a, 0)?, self.size_src(*b, 1)?);
                if prec > 0 { format!("({s})") } else { s }
            }
            Ty::SizeMul(a, b) => {
                let s = format!("{} * {}", self.size_src(*a, 1)?, self.size_src(*b, 2)?);
                if prec > 1 { format!("({s})") } else { s }
            }
            _ => self.ty_src(ty, 2)?,
        })
    }

    fn row_src(&self, row: &EffectRow) -> Option<String> {
        let mut entries = Vec::new();
        for e in self.slice(row.entries) {
            if e.meta.is_some() {
                return None;
            }
            let mut parts = vec![e.name.to_string()];
            for arg in self.slice(e.args) {
                parts.push(self.ty_src(*arg, 2)?);
            }
            entries.push(parts.join(" "));
        }
        Some(match (entries.is_empty(), row.tail) {
            (true, None) => "<>".to_string(),
            (true, Some(t)) => format!("<{t}>"),
            (false, None) => format!("<{}>", entries.join(", ")),
            (false, Some(t)) => format!("<{} | {t}>", entries.join(", ")),
        })
    }
}

/// A whole compilation unit: `$ @mod NAME` followed by top-level items.
#[derive(Clone, Debug)]
pub struct Program {
    pub module: StrId,
    /// The `$ @mod NAME` header.
    pub header: Span,
    pub items: Slice<Item>,
    /// Where each item sits in the source, parallel to `items`.
    pub spans: Slice<ItemSpan>,
    /// Names of items that failed to parse and were skipped. The checker gives
    /// their uses the error type instead of reporting them unbound.
    pub broken: Slice<StrId>,
}

/// An item's whole extent (from its `$` to its last token) and the span of the
/// name it defines, when it defines one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemSpan {
    pub full: Span,
    pub name: Option<Span>,
}

// -- top-level items ---------------------------------------------------------

/// A top-level `$ ...` declaration.
#[derive(Clone, Debug)]
pub enum Item {
    /// `$ name [: ty] = body`. `ctx` is the context parameter's type
    /// when the signature declares one: an ordinary first parameter that call
    /// sites do not write, resolved by type and passed as a leading argument. It
    /// is the same handle as the signature arrow's first `from`, so `sig` stays
    /// exactly what the user wrote.
    Def {
        name: StrId,
        sig: Option<Aol<Ty>>,
        ctx: Option<Aol<Ty>>,
        body: Aol<Expr>,
    },
    /// `$ Name : @struct [@extern "abi"] [a b ...] = field, ...`. `params` are the
    /// declared type parameters, in order; when omitted they are inferred from the
    /// free type variables in the fields. `splices` are the `&X` entries whose
    /// generated fields the expand loop splices in before the final compile. `abi` is set when
    /// the struct is a C-layout foreign type (`@struct @extern "C"`): its runtime
    /// representation is a flat, unboxed C struct, so it can cross the `@extern`
    /// boundary by value. The string names the ABI, matching the function form.
    Struct {
        name: StrId,
        params: Slice<StrId>,
        splices: Slice<EntrySplice>,
        fields: Slice<FieldDecl>,
        abi: Option<StrId>,
        /// A C `union` (`@union @extern "abi"`): the members share offset 0 and the
        /// size is the largest. Parsed with the struct-field syntax and otherwise
        /// handled as a C-repr struct, so it reuses construction and access.
        c_union: bool,
    },
    /// `$ Name : @union [a b ...] = Tag : payload, ...`. `params` are the declared
    /// type parameters (inferred from the variants when omitted). `splices` are
    /// `&X` entries generating variants, as for structs.
    Union {
        name: StrId,
        params: Slice<StrId>,
        splices: Slice<EntrySplice>,
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
    /// `$ Name : @effect params = op : ty, ...`. An operation's type may mention
    /// only the effect's parameters: a handler fixes them for every operation it
    /// serves.
    Effect {
        name: StrId,
        params: Slice<StrId>,
        ops: Slice<FieldDecl>,
    },
    /// `$ @import module`, or `$ @import ALIAS = module`: a qualified-only import whose
    /// names are reachable only as `ALIAS.name`. `span` covers the module path.
    Import {
        module: Slice<StrId>,
        alias: Option<StrId>,
        span: Span,
    },
    /// `$ @private`: marks every following declaration in the file as
    /// module-private (not exported). Symbols are public by default; there is no
    /// `@public` counterpart, so a module's public interface sits above the marker.
    Private,
    /// `$ &expr`: run the `@Code`-valued expression at compile time and insert
    /// the code it returns in place of the directive (see the driver). Also an
    /// expression/type intrinsic; this is the item form. The `Span` is the whole
    /// directive, which the driver replaces.
    Run(Aol<Expr>, Span),
}

/// A `name : Type` field (struct fields and effect operations share this shape).
#[derive(Clone, Copy, Debug)]
pub struct FieldDecl {
    pub name: StrId,
    pub ty: Aol<Ty>,
}

/// One `&X` entry of a struct or union declaration: `X` builds the `@Code` of
/// a list of fields or variants, which the driver checks and splices in place
/// of the entry. `span` covers the whole entry.
#[derive(Clone, Copy, Debug)]
pub struct EntrySplice {
    pub expr: Aol<Expr>,
    pub span: Span,
}

/// One `Tag : payload` arm of a union declaration.
#[derive(Clone, Debug)]
pub struct VariantDecl {
    pub tag: StrId,
    pub payload: Payload,
}

/// A union variant's payload.
#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
pub enum Ty {
    /// A type constructor: `Int`, `@Int64`, or module-qualified `A.B`.
    Con { module: Option<StrId>, name: StrId },
    /// A type variable `` `a ``.
    Var(StrId),
    /// A type-level natural literal (a tensor size).
    Nat(u64),
    /// A size sum `a + b` (`@Tensor (n + m) T`).
    SizeAdd(Aol<Ty>, Aol<Ty>),
    /// A size product `a * b` (`@Tensor (n * m) T`).
    SizeMul(Aol<Ty>, Aol<Ty>),
    /// A sized tensor type `@Tensor size elem`: `@Tensor 5 @Int`, `@Tensor n a`.
    /// `size` is a `Nat` literal, a (Nat-kinded) `Var` or a `+`/`*` of them; a
    /// matrix nests, `@Tensor m (@Tensor n a)`.
    Sized { size: Aol<Ty>, elem: Aol<Ty> },
    /// Type application `Head Arg` (left-associative at use sites).
    App(Aol<Ty>, Aol<Ty>),
    /// A compile-time type splice `&X`: `X` is a compile-time expression that
    /// produces an `@Code` denoting a type, spliced into this annotation. Erased
    /// before the final (strict) compile: the driver's expand loop runs `X`,
    /// substitutes the resulting type source at this node's span, and re-parses.
    MetaE(Aol<Expr>),
    /// A function type `From -> To`, optionally carrying an effect row.
    /// `binder` names the parameter of `(x: From) -> To`; a definition's body
    /// sees it bound, like a record parameter's fields.
    Arrow {
        binder: Option<StrId>,
        from: Aol<Ty>,
        effect: Option<EffectRow>,
        to: Aol<Ty>,
    },
    /// The unit type `{}`.
    Unit,
    /// A tuple type `{ A, B, ... }` (n >= 1).
    Tuple(Slice<Aol<Ty>>),
    /// A record type `{ x: A, y: B }` (closed) or `{ x: A | r }` (open, `tail` is
    /// the row variable). A closed record in parameter
    /// position is also the named-record parameter sugar; an open one (`tail`
    /// present) is always a row-polymorphic record type.
    Record {
        fields: Slice<RecField>,
        tail: Option<StrId>,
    },
}

/// A field of the named-record parameter sugar.
#[derive(Clone, Copy, Debug)]
pub struct RecField {
    pub name: StrId,
    pub ty: Aol<Ty>,
}

/// An effect row `< A, B | `e >` on a function arrow.
#[derive(Clone, Debug)]
pub struct EffectRow {
    pub entries: Slice<RowEntry>,
    pub tail: Option<StrId>,
}

/// One comma-separated entry of an effect row: an effect label with its type
/// arguments (`<Chan @Int e>`). `args` are type atoms, empty for a bare label. `meta`
/// holds `X` for a compile-time splice `<&X>`, whose code supplies the
/// entries; its `name` is `&` and it has no `args`.
#[derive(Clone, Copy, Debug)]
pub struct RowEntry {
    pub name: StrId,
    pub args: Slice<Aol<Ty>>,
    pub span: Span,
    pub meta: Option<Aol<Expr>>,
}

// -- patterns ----------------------------------------------------------------

#[derive(Clone, Debug)]
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
    /// A `++` chain: fixed pieces before and after one variable-length part,
    /// `pre ++ mid ++ suf`. Each piece is a `Str` or a closed `List`; `h :: t` is
    /// `[h] ++ t`. `mid` is absent only when the pieces mix kinds and leave no
    /// hole (`"a" ++ [98]`), in which case `suf` is empty and the remainder after
    /// `pre` must be empty. Adjacent pieces of one kind are merged by the parser.
    Concat {
        pre: Slice<Aol<Pattern>>,
        mid: Option<Aol<Pattern>>,
        suf: Slice<Aol<Pattern>>,
    },
    /// `[a, b]` / `[]`: exactly these elements.
    List(Slice<Aol<Pattern>>),
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
    /// `&X`: a pattern a generator produces. `X` builds the `@Code`, and the
    /// driver splices its source here before the final compile.
    MetaE(Aol<Expr>),
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
#[derive(Clone, Debug)]
pub enum SliceSlot {
    /// `i`: index (reduce) this axis.
    Index(Aol<Expr>),
    /// `lo ... hi`: keep this axis, narrowed to the inclusive range `[lo, hi]`.
    Range(Aol<Expr>, Aol<Expr>),
    /// `..`: keep this axis whole.
    Full,
}

#[derive(Clone, Debug)]
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
    /// A binary operator, keyed by lexeme (`+`, `==`, `::`, `;`, `|>`, ...).
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
    /// An interpolated string `"a ?(x) b"`, as two readings the checker chooses
    /// between by the expected type. `default` is the `@Str` concatenation
    /// `"a " ++ to_string x ++ " b"`. `target` binds the holes in order, then
    /// builds through the blessed interpolation interfaces:
    /// `let #h0 = x in @IInterp (\#b => @IInterpText (@IInterpHole (@IInterpText
    /// #b "a ") #h0) " b")`. Both share the hole expressions. `quoted` is the
    /// reading as a CORE `Template`, whose holes are not evaluated but quoted:
    /// `.{ .pieces = ["a ", " b"], .holes = [@code "x"] }`. Only the chosen one is
    /// checked and lowered.
    Interp {
        default: Aol<Expr>,
        target: Aol<Expr>,
        quoted: Aol<Expr>,
    },
    /// The build-time splice `&e`: `e` runs at compile time and its result
    /// replaces the site, spliced in when it is `@Code` and inserted as a literal
    /// when it is first-order data.
    Splice(Aol<Expr>),
    /// `{ a, b, ... }` (n >= 1).
    Tuple(Slice<Aol<Expr>>),
    /// `[ a, b, ... ]` / `[]`.
    List(Slice<Aol<Expr>>),
    /// The inclusive range builder `[lo ... hi]`. A TYPE-DIRECTED literal: it
    /// materializes into a sized tensor `@Tensor n T` (when a tensor is expected and the
    /// bounds are literals, so the length is a compile-time constant), otherwise a
    /// `List Int` (the default). Both bounds are `Int`. An open range `[lo ...]`
    /// omits the upper bound and builds an unbounded `Stream Int`.
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
    /// `@Array.{ n }` (size form) or `@Array.{ .field = n }`.
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
    /// `{ .foo = 1, ..base }` (extend: this record's fields on top of `base`'s).
    /// Its type is a [`crate::typing::types::TypeNode::Record`] row, not a nominal struct.
    Record {
        fields: Slice<FieldInit>,
        extend: Option<Aol<Expr>>,
        update: Option<Aol<Expr>>,
    },
    /// `Type.Tag.{ ... }` / `.Tag` variant construction.
    Variant {
        module: Option<StrId>,
        ty: Option<StrId>,
        tag: StrId,
        fields: Slice<FieldInit>,
    },
    /// `let p = e in body`.
    Let {
        binding: Binding,
        body: Aol<Expr>,
    },
    /// `if cond then a else b`.
    If {
        cond: Aol<Expr>,
        then: Aol<Expr>,
        alt: Aol<Expr>,
    },
    /// A match on `scrut`: the body of a multi-arm lambda `\ | p => e ...`, or of
    /// `@absurd` (no arms). There is no default arm; a catch-all is written as an
    /// ordinary `| _ => d` or `| x => d` arm.
    ///
    /// `arity` is how many arguments of a multi-arm lambda the arms match. It is
    /// 1 unless the arms list several comma-separated patterns, `| p, q => e`:
    /// then `scrut` is the tuple of the lambda's parameters and each pattern
    /// list is a tuple pattern over it. See Note [Multi-argument arms].
    Match {
        scrut: Aol<Expr>,
        arms: Slice<Arm>,
        arity: u32,
    },
    /// `\p1 p2 => body`.
    Lambda {
        params: Slice<LamParam>,
        body: Aol<Expr>,
    },
    /// `do body [ctl k clauses ...]`.
    Handle {
        body: Aol<Expr>,
        handler: Option<Box<Handler>>,
    },
    /// `@extern "abi" "symbol" "lib"`, or `@extern "load"` (no symbol or
    /// library: both are empty).
    Extern {
        abi: StrId,
        symbol: StrId,
        lib: StrId,
    },
    /// `@extern "ares"`: the definition it is the body of IS the runtime builtin
    /// of the same name. Only CORE declares these, so a builtin has a signature and
    /// a doc comment in source; every reference lowers to the bare builtin.
    Builtin,
    /// `(e : T)`: an ascription. Checks `e` against `T` and has type `T`. Purely a
    /// checking hint; lowering emits `e` unchanged.
    Ascribe {
        expr: Aol<Expr>,
        ty: Aol<Ty>,
    },
    /// `@use {.f = e, ...}` in argument position: supply the named `@use` fields
    /// of the callee; the ones it omits are still searched.
    UseArg(Slice<FieldInit>),
}

/// A field initializer in a struct literal or variant payload.
#[derive(Clone, Copy, Debug)]
pub enum FieldInit {
    /// `.field = value`
    Named { name: StrId, value: Aol<Expr> },
    /// A positional value.
    Positional(Aol<Expr>),
}

/// One field of a `@use {name: Type, ...}` parameter. `before` counts the
/// explicit parameters the signature declares ahead of it, which is where an
/// explicit `@use {.name = e}` argument goes.
#[derive(Clone, Copy, Debug)]
pub struct UseField {
    pub name: StrId,
    pub ty: Aol<Ty>,
    pub before: u32,
    pub span: Span,
}

/// The binding of a `let`: `p: T = value`.
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    pub pat: Aol<Pattern>,
    pub sig: Option<Aol<Ty>>,
    pub value: Aol<Expr>,
}

/// One parameter of a lambda: its pattern and an optional `: T` annotation
/// (`\(x: Int) => x`, `\(x: Int) y => x`).
#[derive(Clone, Copy, Debug)]
pub struct LamParam {
    pub pat: Aol<Pattern>,
    pub sig: Option<Aol<Ty>>,
}

/// One arm of a match, or a value arm of a handler. Or-patterns (`| p1 | p2 => e`)
/// share a body and guard.
#[derive(Clone, Debug)]
pub struct Arm {
    pub patterns: Slice<Aol<Pattern>>,
    pub guard: Option<Aol<Expr>>,
    pub body: Aol<Expr>,
}

/// A `ctl k ...` handler attached to a `do` block.
#[derive(Clone, Debug)]
pub struct Handler {
    pub continuation: StrId,
    pub clauses: Slice<Clause>,
    /// The return clause, run when the body finishes normally: a binder for the
    /// body's value and the result expression. The parser builds it from the
    /// handler's value arms (`| p => e`); `None` means the identity.
    pub value: Option<(StrId, Aol<Expr>)>,
}

/// One `| Effect.op arg => body` handler clause.
#[derive(Clone, Copy, Debug)]
pub struct Clause {
    pub effect: Option<StrId>,
    pub op: StrId,
    pub arg: StrId,
    pub body: Aol<Expr>,
}
