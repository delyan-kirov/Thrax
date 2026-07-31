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
pub struct Program {
    pub module: String,
    pub effects: Vec<Effect>,
    pub globals: Vec<(String, Term)>,
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
    /// A form the interpreter does not yet support (FFI). It raises a runtime
    /// fault only if actually forced, so a module may still run the globals that
    /// avoid it.
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
    /// not listed are ignored.
    Struct {
        fields: Vec<(String, Pat)>,
    },
    /// A literal byte-string prefix `"GET " ++ rest`.
    StrPrefix {
        prefix: Vec<u8>,
        rest: Box<Pat>,
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
