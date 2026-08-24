//! The intermediate representation: the Core after CLOSURE CONVERSION, a port of
//! `compiler/IR.hpp`. Every lambda is lifted to a top-level closed [`Code`] block,
//! and a lambda expression becomes an explicit [`Atom::Clos`]. Variable access is
//! split three ways: [`Atom::Local`] (a slot in the current activation),
//! [`Atom::Env`] (a field of the current closure's captured record), and
//! [`Atom::Glob`] (a top-level binding) which is the closure-record vs
//! stack-frame distinction a stack machine and a C backend need, and which
//! replaces the Core's De-Bruijn search with O(1) array indexing.
//!
//! The IR is in A-normal form (from the [`crate::lowering::anf`] pass): every
//! operator/operand is an [`Atom`], every non-trivial computation is named by a
//! [`Expr::Let`]. Patterns are already shallow (from
//! [`crate::lowering::patmat`]), so a [`Case`] alternative matches one
//! constructor or literal and binds its payload positionally.
//!
//! Differences from the C++ IR, mirroring the Rust Core: `Bool`/`Unit` are their
//! own atoms (not erased to Int / `Unk`), tuples are a first-class
//! [`Expr::MkTuple`], and `defer` is [`Expr::Defer`]. `Glob` is a single canonical
//! name (like the C++ `IR::Glob`), resolved by the runtime.

/// A trivial value-expression: no evaluation step to name it.
#[derive(Clone, Debug)]
pub enum Atom {
    /// A slot in the current activation's local array (params + let/case binders).
    Local(usize),
    /// A field of the current closure's captured record.
    Env(usize),
    /// A top-level binding, by its single canonical name: a user global is
    /// `Module.name`; a built-in operator is bare (`+`); the auto-injected
    /// namespaces keep their prefix (`C.fopen`, `TARGET.os`); an effect operation
    /// is `Effect.op` when qualified, else its bare name. This mirrors the C++
    /// `IR::Glob` (one mangled string), so the runtime resolves everything by
    /// name (see `machine::glob`), rather than carrying a separate module.
    Glob {
        name: String,
    },
    LitI(i64),
    LitR(f64),
    LitS(Vec<u8>),
    LitB(bool),
    Unit,
    /// Allocate a closure: the lifted `code` plus the captured atoms (read from
    /// the enclosing activation) that become the new closure's environment.
    Clos {
        code: usize,
        captures: Vec<Atom>,
    },
    /// A foreign function (`@extern`). Produces a value that accumulates its
    /// positional C arguments (`arg_types`, a nullary C function still taking one
    /// unit argument) and, once saturated, marshals them across the seam selected
    /// by `abi` (`"C"` = a C library symbol, `"WASM"` = a host import from the
    /// embedder) and calls `symbol`. The record grouping several C parameters is
    /// flattened into positional arguments at the call site during lowering, so it
    /// never reaches here.
    Extern {
        abi: String,
        symbol: String,
        lib: String,
        arg_types: Vec<String>,
        ret_type: String,
    },
}

/// A Case alternative: matched when the scrutinee's head agrees with `kind`. A
/// `Con` alternative binds its payload positionally into local slots
/// `[binder_base .. binder_base + binders.len())`, where a `None` binder ignores
/// its slot.
#[derive(Clone, Debug)]
pub struct Alt {
    pub kind: AltKind,
    pub binder_base: usize,
    pub binders: Vec<Option<String>>,
    pub body: Expr,
}

/// The head a Case alternative matches on.
#[derive(Clone, Debug)]
pub enum AltKind {
    Int(i64),
    Real(f64),
    Bool(bool),
    Con(String),
}

/// One handler clause: its operation (optionally effect-qualified) and the
/// closure of its `\arg = \k = body` (a 2-parameter [`Code`]: the operation
/// argument is `Local 0`, the continuation is `Local 1`).
#[derive(Clone, Debug)]
pub struct HandleClause {
    pub effect: Option<String>,
    pub op: String,
    pub fun: Atom,
}

/// A computation: each takes an evaluation step.
#[derive(Clone, Debug)]
pub enum Expr {
    /// Return an atom (the ANF tail position / answer).
    Ret(Atom),
    /// `let slot = rhs in body`.
    Let {
        slot: usize,
        rhs: Box<Expr>,
        body: Box<Expr>,
    },
    /// Apply a closure/builtin/operation to one atom; `tail` marks tail position.
    App {
        fun: Atom,
        arg: Atom,
        tail: bool,
    },
    /// Match `scrut` against `alts`, falling to `default`.
    Case {
        scrut: Atom,
        alts: Vec<Alt>,
        default: Box<Expr>,
    },
    MkStruct {
        name: String,
        /// A record-update base whose fields seed the value before the listed
        /// overrides (the Rust Core's `Base.{ ... }`).
        base: Option<Atom>,
        fields: Vec<(String, Atom)>,
    },
    Field {
        rec: Atom,
        name: String,
    },
    MkVariant {
        ty: String,
        tag: String,
        fields: Vec<Atom>,
    },
    MkTuple(Vec<Atom>),
    /// Install a handler around `body`. `els` is the closure of the value clause
    /// `\x = e` (identity when no `else` was written), run on the body's normal
    /// result (a deep handler).
    Handle {
        body: Box<Expr>,
        clauses: Vec<HandleClause>,
        els: Atom,
    },
    /// `defer cleanup do body`: `cleanup` is the closure of a nullary thunk run
    /// when `body`'s dynamic scope exits.
    Defer {
        cleanup: Atom,
        body: Box<Expr>,
    },
    /// A form unsupported at runtime (raised only if forced).
    Fault(String),
}

/// A lifted, closed function. `nparams` is its arity at this block (curried
/// lambdas nest, so a multi-argument source function is several `Code`s);
/// `nlocals` is the activation's slot count (params occupy `[0..nparams)`);
/// globals are nullary `Code`s (CAFs) whose body computes the global's value.
#[derive(Clone, Debug)]
pub struct Code {
    pub nparams: usize,
    pub nlocals: usize,
    pub body: Expr,
    pub name: String,
}

/// One declared effect operation (carried through for the machine's operation
/// resolution).
#[derive(Clone, Debug)]
pub struct Effect {
    pub effect: String,
    pub op: String,
}

/// The whole program: a pool of code blocks, the global table (name -> the
/// nullary code that computes it), and the declared effect operations.
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub codes: Vec<Code>,
    pub globals: Vec<(String, usize)>,
    pub effects: Vec<Effect>,
    /// C memory layouts of C-repr structs, keyed by type name. The machine uses
    /// them to marshal a struct value across the `@extern` boundary by value.
    pub crepr_layouts: Vec<(String, utilities::CLayout)>,
}
