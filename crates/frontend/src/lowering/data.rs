//! The Core: a desugared lambda calculus the front-end AST lowers to.
//!
//! Every surface convenience (operators, `if`, `when`, list/cons sugar, pipes,
//! sequencing, record parameters) is gone. What remains is variables,
//! application, single-parameter lambdas, `let`, one branching form ([`Term::Case`]),
//! literals, and the aggregate constructors/accessors. This is the tree the
//! interpreter ([`crate::eval`]) walks; a future backend would consume the same
//! shape.
//!
//! Unlike the front-end AST (handle-addressed nodes in a [`syntax::Ast`] store),
//! the Core owns its data. It is immutable after lowering, and its child pointers
//! are `Arc<Term>` (the collections the interpreter walks lazily are `Arc<[T]>`).
//! So it carries no lifetime, a shared subterm is a pointer bump rather than a
//! deep clone, and the whole program is `Send`: a compiled module can be handed
//! to another thread. The interpreter's own per-thread value graph
//! ([`crate::eval`]) uses `Rc` instead, since it is mutable and never crosses a
//! thread.

use std::sync::Arc;

/// A whole lowered module: its `@mod` name and its globals in source order.
#[derive(Clone)]
pub struct Program {
    pub module: String,
    pub effects: Vec<Effect>,
    pub globals: Vec<(String, Term)>,
    /// C memory layouts of C-repr structs, keyed by type name, for marshalling
    /// struct values across the `@extern` boundary. Same across every module (the
    /// resolver aggregates them); the IR lowering merges duplicates by name.
    pub crepr_layouts: Vec<(String, utilities::CLayout)>,
}

/// One effect operation declared by `$ Effect : @effect = op : ...`.
#[derive(Clone, Debug)]
pub struct Effect {
    pub effect: String,
    pub op: String,
}

/// A Core term.
#[derive(Clone, Debug)]
pub enum Term {
    Int(i64),
    Real(f64),
    Str(Vec<u8>),
    Bool(bool),
    Unit,
    /// A variable. `module` is set for a qualified reference (`MOD.name`); an
    /// unqualified name resolves through the local environment, then the globals,
    /// then the built-in operators.
    ///
    /// `idx` is a De-Bruijn index filled by [`super::debruijn::assign_id`] after
    /// lowering: `0` marks a global (resolved by `module`/`name`), and a positive
    /// index counts outward through the binder stack to a local. The name-based
    /// tree-walker ignores it; the closure converter reads it to split a variable
    /// into a local slot vs an environment capture.
    Var {
        module: Option<String>,
        name: String,
        idx: usize,
    },
    /// Application of one argument (curried).
    App(Arc<Term>, Arc<Term>),
    /// A single-parameter lambda; multi-parameter surface lambdas curry into a
    /// nest of these, and a pattern parameter becomes a lambda over a fresh name
    /// whose body is a [`Term::Case`] that destructures it.
    Lam {
        param: String,
        body: Arc<Term>,
    },
    /// `let name = val in body`. `rec` marks a self-recursive binding (the name is
    /// in scope inside `val`), used for a plain-variable binder.
    Let {
        name: String,
        rec: bool,
        val: Arc<Term>,
        body: Arc<Term>,
    },
    /// The one branching form: force `scrut`, take the first arm whose pattern
    /// matches (and whose guard, if any, holds), else `default`. `if` and `when`
    /// both lower here.
    Case {
        scrut: Arc<Term>,
        arms: Arc<[Arm]>,
        default: Option<Arc<Term>>,
    },
    Tuple(Arc<[Term]>),
    /// A struct literal, fields in declaration order (named after lowering). When
    /// `base` is set the literal is a record update: the base struct's fields
    /// seed the value and the listed fields override (or extend) them.
    Struct {
        name: String,
        base: Option<Arc<Term>>,
        fields: Arc<[(String, Term)]>,
    },
    /// A union construction: the union type name, the tag, and the payload in the
    /// variant's declared order.
    Variant {
        ty: String,
        tag: String,
        fields: Arc<[Term]>,
    },
    /// Field access `record.field`; a numeric `field` indexes a tuple.
    Field(Arc<Term>, String),
    /// `do body ctl k ...`: install an algebraic-effect handler around `body`.
    Handle {
        body: Arc<Term>,
        handler: Arc<Handler>,
    },
    /// `defer cleanup do body`: run `cleanup` when `body`'s dynamic scope exits.
    /// That is on normal completion of `body`, when a continuation capturing it is
    /// resumed to completion, or when a handler abandons such a continuation
    /// (`cleanup` still runs, under the enclosing handlers). Nested defers run
    /// innermost-first.
    Defer {
        cleanup: Arc<Term>,
        body: Arc<Term>,
    },
    /// A foreign function bound by `@extern "abi" "symbol" "lib"`. A curried,
    /// first-class value (like a builtin): applying it accumulates arguments and,
    /// once saturated to `arg_types.len()` (a nullary C function still takes one
    /// unit argument), marshals them across the seam and calls `symbol`. `abi`
    /// selects the seam: `"C"` is a C library symbol, `"wasm"` is a host import
    /// supplied by the embedder (the browser playground). `arg_types`/`ret_type`
    /// are the marshalling type names the checker recovered from the declared
    /// signature (`Str`/`Ptr`/`Int`/`Real`/`{}`/sized), driving how each slot
    /// crosses the seam. The record that groups several C parameters is flattened
    /// into positional arguments at the call site (see `Lowering::expr`), so this
    /// node never sees or materializes it.
    Extern {
        abi: String,
        symbol: String,
        lib: String,
        arg_types: Arc<[String]>,
        ret_type: String,
    },
    /// A form the interpreter does not yet support. It raises a runtime fault
    /// only if actually forced, so a module may still run the globals that avoid
    /// it.
    Fault(String),
}

/// One arm of a [`Term::Case`]: a pattern, an optional guard, and a body. Surface
/// or-patterns (`is p1 is p2 then e`) expand into one arm per alternative sharing
/// the guard and body.
#[derive(Clone, Debug)]
pub struct Arm {
    pub pat: Pat,
    pub guard: Option<Arc<Term>>,
    pub body: Arc<Term>,
}

/// A Core pattern: the surface patterns with sugar resolved (lists and `::` are
/// `List` variants, struct/variant field patterns are positional-or-named against
/// the declaration).
#[derive(Clone, Debug)]
pub enum Pat {
    Wild,
    Var(String),
    Int(i64),
    Real(f64),
    Str(Vec<u8>),
    Bool(bool),
    Tuple(Vec<Pat>),
    /// Match a union value by `tag`, binding the payload positionally.
    Variant {
        tag: String,
        fields: Vec<Pat>,
    },
    /// Match a struct: each `(field, subpattern)` is checked by field name; fields
    /// not listed are ignored. `rest`, when set, binds the leftover fields (the
    /// record minus the listed labels) to that name (`..name` in a record pattern).
    Struct {
        fields: Vec<(String, Pat)>,
        rest: Option<String>,
    },
    /// A literal byte-string prefix `"GET " ++ rest`.
    StrPrefix {
        prefix: Vec<u8>,
        rest: Box<Pat>,
    },
    /// An inclusive numeric range `lo ... hi`: matches when `lo <= x <= hi`. `lo`
    /// and `hi` are numeric literal terms; compiled to two comparison tests. An open
    /// range `lo ...` has no `hi` and matches when `lo <= x` (one test).
    Range {
        lo: Term,
        hi: Option<Term>,
    },
    /// A literal pattern on a USER type (Stage 2): matches when the equality hook
    /// `eq scrut value` is `@true`, where `value` is the literal built into the user
    /// type (its construction hook applied to the raw payload). Binds nothing.
    /// Compiled to a boolean test by `patmat`, so it never reaches the later passes.
    HookEq {
        eq: (Option<String>, String),
        value: Box<Term>,
    },
    /// A sequence pattern on a USER type (Stage 2): matches by unfolding the
    /// `@compiler_interface_sequence_view` hook (`view`) one step at a time. `elems`
    /// are the leading element patterns; `rest`, when present, binds the remaining
    /// sequence, else the tail must be `Empty`. Compiled to nested `SeqView`
    /// (`More`/`Empty`) matches by `patmat`, so it never reaches the later passes.
    SeqView {
        view: (Option<String>, String),
        elems: Vec<Pat>,
        rest: Option<Box<Pat>>,
    },
}

/// A Core effect handler.
#[derive(Clone, Debug)]
pub struct Handler {
    pub continuation: String,
    pub clauses: Vec<Clause>,
    pub default: Option<(String, Term)>,
}

/// One `is Effect.op arg = body` handler clause.
#[derive(Clone, Debug)]
pub struct Clause {
    pub effect: Option<String>,
    pub op: String,
    pub arg: String,
    pub body: Term,
}

impl Term {
    pub fn app(f: Term, x: Term) -> Term {
        Term::App(Arc::new(f), Arc::new(x))
    }
    pub fn var(name: impl Into<String>) -> Term {
        Term::Var {
            module: None,
            name: name.into(),
            idx: 0,
        }
    }
}
