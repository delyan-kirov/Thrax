//! The intermediate representation: the Core after closure conversion.
//!
//! Every lambda is lifted to a top-level closed [`Code`] block, and a lambda
//! expression becomes an explicit [`Atom::Clos`]. Variable access is split
//! three ways: [`Atom::Local`] (a slot in the current activation),
//! [`Atom::Env`] (a field of the current closure's captured record), and
//! [`Atom::Glob`] (a top-level binding, by its canonical name). That is the
//! closure-record versus stack-frame distinction a stack machine and a C
//! backend need, and it replaces the Core's De Bruijn search with array
//! indexing.
//!
//! The IR is in A-normal form: every operator and operand is an [`Atom`], and
//! every non-trivial computation is named by an [`IrExpr::Let`]. Patterns are
//! shallow, so an [`IrExpr::Case`] alternative matches one constructor or
//! literal and binds its payload positionally.
//!
//! Every name in the IR (a global, a constructor, a field, an effect
//! operation) is a [`StrId`].
//!
//! This module also holds the one table of built-in operations ([`BUILTINS`])
//! that both engines read, and a small traversal over [`IrExpr`].

use utilities::StrId;

/// A trivial value-expression: no evaluation step to name it.
#[derive(Clone, Debug)]
pub enum Atom {
    /// A slot in the current activation's local array (params + let/case binders).
    /// Reading it borrows: the slot keeps its reference.
    Local(usize),
    /// The last use of a local slot on this path (see [`super::ownership`]): the
    /// reference moves out and leaves the slot empty, so whatever consumes the
    /// value can be its only owner.
    Move(usize),
    /// A field of the current closure's captured record.
    Env(usize),
    /// The last use of a captured field. It moves out only when the activation is
    /// the closure's sole owner (nothing else can enter it again); otherwise the
    /// engines read it as a borrow, like [`Atom::Env`].
    MoveEnv(usize),
    /// A top-level binding, by its single canonical name: a user global is
    /// `Module.name`; a built-in operator is bare (`+`); the auto-injected
    /// namespaces keep their prefix (`C.fopen`, `TARGET.os`); an effect operation
    /// is `Effect.op` when qualified, else its bare name. This mirrors the
    /// `IR::Glob` (one mangled string), so the runtime resolves everything by
    /// name (see `machine::glob`), rather than carrying a separate module.
    Glob {
        name: StrId,
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
        abi: StrId,
        symbol: StrId,
        lib: StrId,
        arg_types: Vec<StrId>,
        ret_type: StrId,
    },
}

/// A Case alternative: matched when the scrutinee's head agrees with `kind`. A
/// `Con` alternative binds its payload positionally into local slots
/// `[binder_base .. binder_base + binders.len())`, where a `None` binder (a
/// wildcard, or a binder the body never reads) leaves its slot untouched.
#[derive(Clone, Debug)]
pub struct Alt {
    pub kind: AltKind,
    pub binder_base: usize,
    pub binders: Vec<Option<StrId>>,
    pub body: IrExpr,
}

/// The head a Case alternative matches on.
#[derive(Clone, Debug)]
pub enum AltKind {
    Int(i64),
    Real(f64),
    Bool(bool),
    Con(StrId),
}

/// One handler clause: its operation (optionally effect-qualified) and the
/// closure of its `\arg = \k = body` (a 2-parameter [`Code`]: the operation
/// argument is `Local 0`, the continuation is `Local 1`).
#[derive(Clone, Debug)]
pub struct HandleClause {
    pub effect: Option<StrId>,
    pub op: StrId,
    pub fun: Atom,
    pub resume: ResumeUse,
}

// Note [Resume uniqueness]
//
// Effects are multi-shot: a handler may resume its continuation `k` once,
// many times, or never. Capturing a continuation copies the stack slice
// between the perform and the handler, and resuming it must not disturb a
// copy that may be resumed again. Copying at every resume would make the
// common one-shot handler pay for the rare multi-shot one.
//
// So the cost is decided at runtime, by ownership. A resume moves the slice
// when the resumption value is uniquely owned at that moment, and copies it
// otherwise. That check is only as good as the reference counts behind it,
// which is why the ownership pass makes them exact: the last use of `k` in a
// clause moves it, so a one-shot handler holds the only reference when it
// resumes. See Note [Last use moves].
//
// One case is decided statically. A clause whose body never mentions `k` (an
// exception, `break`) cannot resume, so the engines skip capturing the slice
// at all and keep only its `defer` cleanups ([`ResumeUse::Never`]).

/// Whether a handler clause needs its continuation at all, read by the engines at
/// the moment they capture one (the perform point already holds the clause that
/// matched). Derived from the clause body by closure conversion.
/// See Note [Resume uniqueness].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResumeUse {
    /// The clause never mentions `k` (an exception, `break`/`continue`): there is
    /// nothing to resume, so the slice is never captured. Only its `defer`
    /// cleanups are kept, to run after the clause body.
    Never,
    /// The clause mentions `k`, so a perform captures the slice.
    Captured,
}

/// A computation: each takes an evaluation step.
#[derive(Clone, Debug)]
pub enum IrExpr {
    /// Return an atom (the ANF tail position / answer).
    Ret(Atom),
    /// `let slot = rhs in body`.
    Let {
        slot: usize,
        rhs: Box<IrExpr>,
        body: Box<IrExpr>,
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
        default: Box<IrExpr>,
    },
    MkStruct {
        name: StrId,
        /// A record-update base whose fields seed the value before the listed
        /// overrides (the Rust Core's `Base.{ ... }`).
        base: Option<Atom>,
        fields: Vec<(StrId, Atom)>,
    },
    Field {
        rec: Atom,
        name: StrId,
    },
    MkVariant {
        ty: StrId,
        tag: StrId,
        fields: Vec<Atom>,
    },
    MkTuple(Vec<Atom>),
    /// Install a handler around `body`. `els` is the closure of the value clause
    /// `\x = e` (identity when no `else` was written), run on the body's normal
    /// result (a deep handler).
    Handle {
        body: Box<IrExpr>,
        clauses: Vec<HandleClause>,
        els: Atom,
    },
    /// `defer cleanup in body`: `cleanup` is the closure of a nullary thunk run
    /// when `body`'s dynamic scope exits.
    Defer {
        cleanup: Atom,
        body: Box<IrExpr>,
    },
    /// Release the listed local slots, then continue with `body`. Inserted by the
    /// ownership pass where a slot's value dies without a last use to move it: a
    /// binder that is never read, or a value one branch of a `Case` no longer
    /// needs.
    Drop {
        slots: Vec<usize>,
        body: Box<IrExpr>,
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
    pub body: IrExpr,
    pub name: StrId,
}

/// One declared effect operation (carried through for the machine's operation
/// resolution).
#[derive(Clone, Debug)]
pub struct IrEffect {
    pub effect: StrId,
    pub op: StrId,
}

/// The whole program: a pool of code blocks, the global table (name -> the
/// nullary code that computes it), and the declared effect operations.
#[derive(Clone, Debug, Default)]
pub struct IrProgram {
    pub codes: Vec<Code>,
    pub globals: Vec<(StrId, usize)>,
    pub effects: Vec<IrEffect>,
    /// C memory layouts of C-repr structs, keyed by type name. The machine uses
    /// them to marshal a struct value across the `@extern` boundary by value.
    pub crepr_layouts: Vec<(StrId, utilities::CLayout)>,
}

// ## Traversal

impl Atom {
    /// Whether the atom reads local `slot`, directly or as a closure capture.
    pub fn mentions(&self, slot: usize) -> bool {
        match self {
            Atom::Local(s) | Atom::Move(s) => *s == slot,
            Atom::Clos { captures, .. } => captures.iter().any(|c| c.mentions(slot)),
            _ => false,
        }
    }
}

impl IrExpr {
    /// Visit each expression nested directly in this one: a `let`'s right-hand
    /// side and body, a case's alternatives and default, the body of a handler,
    /// a `defer` or a `drop`.
    pub fn for_each_child(&self, f: &mut impl FnMut(&IrExpr)) {
        match self {
            IrExpr::Let { rhs, body, .. } => {
                f(rhs);
                f(body);
            }
            IrExpr::Case { alts, default, .. } => {
                alts.iter().for_each(|alt| f(&alt.body));
                f(default);
            }
            IrExpr::Handle { body, .. } | IrExpr::Defer { body, .. } | IrExpr::Drop { body, .. } => f(body),
            IrExpr::Ret(_)
            | IrExpr::App { .. }
            | IrExpr::MkStruct { .. }
            | IrExpr::Field { .. }
            | IrExpr::MkVariant { .. }
            | IrExpr::MkTuple(_)
            | IrExpr::Fault(_) => {}
        }
    }

    /// Whether `p` holds for this expression or any expression nested in it,
    /// stopping at the first that does.
    pub fn any_node(&self, p: &mut impl FnMut(&IrExpr) -> bool) -> bool {
        if p(self) {
            return true;
        }
        let mut hit = false;
        self.for_each_child(&mut |e| hit = hit || e.any_node(p));
        hit
    }

    /// Visit the atoms this expression itself reads (not those of nested
    /// expressions). A closure is not visited; its captures are, recursively.
    pub fn for_each_atom(&self, f: &mut impl FnMut(&Atom)) {
        fn visit(a: &Atom, f: &mut impl FnMut(&Atom)) {
            match a {
                Atom::Clos { captures, .. } => captures.iter().for_each(|c| visit(c, f)),
                _ => f(a),
            }
        }
        match self {
            IrExpr::Ret(a) | IrExpr::Field { rec: a, .. } => visit(a, f),
            IrExpr::App { fun, arg, .. } => {
                visit(fun, f);
                visit(arg, f);
            }
            IrExpr::MkStruct { base, fields, .. } => {
                base.iter().chain(fields.iter().map(|(_, a)| a)).for_each(|a| visit(a, f))
            }
            IrExpr::MkVariant { fields, .. } => fields.iter().for_each(|a| visit(a, f)),
            IrExpr::MkTuple(items) => items.iter().for_each(|a| visit(a, f)),
            IrExpr::Case { scrut, .. } => visit(scrut, f),
            IrExpr::Handle { clauses, els, .. } => {
                clauses.iter().for_each(|c| visit(&c.fun, f));
                visit(els, f);
            }
            IrExpr::Defer { cleanup, .. } => visit(cleanup, f),
            IrExpr::Let { .. } | IrExpr::Drop { .. } | IrExpr::Fault(_) => {}
        }
    }

    /// Whether the expression reads local `slot` anywhere, including inside a
    /// closure it builds, or drops it.
    pub fn mentions(&self, slot: usize) -> bool {
        self.any_node(&mut |e| {
            let mut hit = matches!(e, IrExpr::Drop { slots, .. } if slots.contains(&slot));
            e.for_each_atom(&mut |a| hit = hit || a.mentions(slot));
            hit
        })
    }
}

// ## Builtins

/// Where a builtin can run, which decides what an engine must provide for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuiltinHost {
    /// Both engines, with nothing extra.
    Everywhere,
    /// Both engines. Natively it starts or joins a parallel task, so a program
    /// naming one gets the thread-ready runtime.
    Parallel,
    /// Both engines. Natively it calls into the compiler library (`libthrax`,
    /// see `crates/thrax/include/thrax.h`).
    Compiler,
    /// Only at compile time, on the interpreter: the metaprogramming builtins.
    /// The native runtime has none of them.
    CompileTime,
}

/// How much of its operands a builtin needs evaluated before it runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Strictness {
    /// The operands as they are; a lazy slot inside one stays unforced.
    Shallow,
    /// Every lazy slot reachable from the operands, forced first, so a
    /// structural comparison sees values rather than suspended computations.
    Deep,
}

/// One built-in operation.
#[derive(Clone, Copy, Debug)]
pub struct Builtin {
    /// Which operation it is, for an engine to dispatch on.
    pub prim: Prim,
    pub name: &'static str,
    pub arity: usize,
    pub strictness: Strictness,
    pub host: BuiltinHost,
}

impl Builtin {
    /// Whether the native runtime implements it.
    pub fn is_native(&self) -> bool {
        self.host != BuiltinHost::CompileTime
    }
}

const fn leaf(prim: Prim, name: &'static str, arity: usize) -> Builtin {
    Builtin { prim, name, arity, strictness: Strictness::Shallow, host: BuiltinHost::Everywhere }
}

const fn deep(prim: Prim, name: &'static str, arity: usize) -> Builtin {
    Builtin { strictness: Strictness::Deep, ..leaf(prim, name, arity) }
}

const fn parallel(prim: Prim, name: &'static str, arity: usize) -> Builtin {
    Builtin { host: BuiltinHost::Parallel, ..leaf(prim, name, arity) }
}

const fn compiler(prim: Prim, name: &'static str, arity: usize) -> Builtin {
    Builtin { host: BuiltinHost::Compiler, ..leaf(prim, name, arity) }
}

const fn compile_time(prim: Prim, name: &'static str, arity: usize) -> Builtin {
    Builtin { host: BuiltinHost::CompileTime, ..leaf(prim, name, arity) }
}

/// Declare [`Prim`], one variant per builtin, and [`BUILTINS`] in the same
/// order, from one list of `Variant = builder("name", arity)` entries.
macro_rules! builtins {
    ($($prim:ident = $make:ident($name:literal, $arity:literal),)*) => {
        /// A built-in operation, as an engine dispatches on it. Variant `k` is
        /// `BUILTINS[k]`.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub enum Prim {
            $($prim,)*
        }

        /// Every built-in operation: the one table both engines read. The native
        /// runtime numbers its dispatch (`THxB_*`) from the order of the native
        /// entries, so a new native builtin goes before the compile-time ones.
        pub const BUILTINS: &[Builtin] = &[$($make(Prim::$prim, $name, $arity),)*];
    };
}

builtins! {
    Not = leaf("not", 1),
    Neg = leaf("neg", 1),
    Iadd = leaf("@iadd", 2),
    Isub = leaf("@isub", 2),
    Imul = leaf("@imul", 2),
    Idiv = leaf("@idiv", 2),
    Imod = leaf("@imod", 2),
    Udiv = leaf("@udiv", 2),
    Umod = leaf("@umod", 2),
    Fadd = leaf("@fadd", 2),
    Fsub = leaf("@fsub", 2),
    Fmul = leaf("@fmul", 2),
    Fdiv = leaf("@fdiv", 2),
    Fmod = leaf("@fmod", 2),
    Fpow = leaf("@fpow", 2),
    F32add = leaf("@f32add", 2),
    F32sub = leaf("@f32sub", 2),
    F32mul = leaf("@f32mul", 2),
    F32div = leaf("@f32div", 2),
    F32mod = leaf("@f32mod", 2),
    F32pow = leaf("@f32pow", 2),
    Ieq = leaf("@ieq", 2),
    Ilt = leaf("@ilt", 2),
    Ult = leaf("@ult", 2),
    Feq = leaf("@feq", 2),
    Flt = leaf("@flt", 2),
    Seq = leaf("@seq", 2),
    Slt = leaf("@slt", 2),
    Swrap = leaf("@swrap", 2),
    Uwrap = leaf("@uwrap", 2),
    Eq = deep("==", 2),
    Lt = leaf("<", 2),
    Gt = leaf(">", 2),
    Le = leaf("<=", 2),
    Ge = leaf(">=", 2),
    Concat = leaf("++", 2),
    Acat = leaf("@acat", 2),
    ArrayAlloc = leaf("@array_alloc", 1),
    ArrayLen = leaf("@array_len", 1),
    ArrayGet = leaf("@array_get", 2),
    ArrayPush = leaf("@array_push", 2),
    ArraySet = leaf("@array_set", 3),
    ArraySlice = leaf("@array_slice", 3),
    RecordWithout = leaf("record_without", 2),
    VecNew = leaf("@vec_new", 1),
    VecLen = leaf("@vec_len", 1),
    VecGet = leaf("@vec_get", 2),
    VecPush = leaf("@vec_push", 2),
    VecFill = leaf("@vec_fill", 2),
    VecSet = leaf("@vec_set", 3),
    VecSlice = leaf("@vec_slice", 3),
    TensorLength = leaf("@tensor_length", 1),
    TensorStack = leaf("@tensor_stack", 1),
    TensorTranspose = leaf("@tensor_transpose", 1),
    TensorConcat = leaf("@tensor_concat", 2),
    TensorIndex = leaf("@tensor_index", 2),
    TensorCreate = leaf("@tensor_create", 2),
    TensorSlice = leaf("@tensor_slice", 3),
    TensorIndexAxis = leaf("@tensor_index_axis", 3),
    TensorSliceAxis = leaf("@tensor_slice_axis", 4),
    Delay = leaf("@delay", 1),
    Par = parallel("@par", 1),
    Spawn = parallel("@spawn", 1),
    Await = parallel("@await", 1),
    ChanNew = leaf("@chan_new", 1),
    ChanSend = leaf("@chan_send", 2),
    ChanRecv = leaf("@chan_recv", 1),
    TvarNew = leaf("@tvar_new", 1),
    TvarRead = leaf("@tvar_read", 1),
    StmBegin = leaf("@stm_begin", 1),
    StmRead = leaf("@stm_read", 1),
    StmWrite = leaf("@stm_write", 2),
    StmCommit = leaf("@stm_commit", 1),
    StmWait = leaf("@stm_wait", 1),
    StmSave = leaf("@stm_save", 1),
    StmRollback = leaf("@stm_rollback", 1),
    Lex = compiler("@lex", 1),
    TokenKind = compiler("@token_kind", 1),
    TokenText = compiler("@token_text", 1),
    Parse = compiler("@parse", 1),
    ParseStr = compiler("@parse_str", 1),
    ParseItems = compiler("@parse_items", 1),
    Format = compiler("@format", 1),
    Eval = compiler("@eval", 1),
    Abort = compile_time("@abort", 1),
    Emit = compile_time("@emit", 1),
    Fresh = compile_time("@fresh", 1),
    Link = compile_time("@link", 1),
    LinkPath = compile_time("@link_path", 1),
    ImportPath = compile_time("@import_path", 1),
    TypeKind = compile_time("@type_kind", 1),
    TypeFields = compile_time("@type_fields", 1),
    TypeVariants = compile_time("@type_variants", 1),
    TypeParams = compile_time("@type_params", 1),
}

impl Prim {
    /// The table entry of this builtin.
    pub fn builtin(self) -> &'static Builtin {
        &BUILTINS[self as usize]
    }
}

/// Prints the builtin's name.
impl std::fmt::Display for Prim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.builtin().name)
    }
}

/// The builtin called `name`, if there is one.
pub fn builtin(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

/// The arity of the builtin called `name`, if there is one.
pub fn builtin_arity(name: &str) -> Option<usize> {
    builtin(name).map(|b| b.arity)
}

/// The builtin called `name`, if the native runtime implements it.
pub fn native_builtin(name: &str) -> Option<&'static Builtin> {
    builtin(name).filter(|b| b.is_native())
}

/// The builtins the native runtime implements, in its dispatch order.
pub fn native_builtins() -> impl Iterator<Item = &'static Builtin> {
    BUILTINS.iter().filter(|b| b.is_native())
}
