//! Runtime data for the interpreter: the value representation and the
//! evaluator's internal structures. The logic lives in [`crate::eval`]; this is
//! the `IT` / `ITxDATA` split mirrored from the C++ tree.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use frontend::lowering::data::Term;
use utilities::Result;

/// A runtime value. Aggregates own their elements; closures and the environment
/// share structure through `Rc`.
#[derive(Clone)]
pub enum Value {
    Int(i64),
    Real(f64),
    /// Byte string, also the representation of an `Array` (both are byte vectors).
    Str(Rc<Vec<u8>>),
    Bool(bool),
    Unit,
    Tuple(Vec<Value>),
    Struct {
        name: String,
        fields: Vec<(String, Value)>,
    },
    Variant {
        ty: String,
        tag: String,
        fields: Vec<Value>,
    },
    /// A generic growable vector (`Vec `T`), distinct from the byte-vector `Str`.
    Vector(Rc<Vec<Value>>),
    Closure(Rc<Closure>),
    /// A partially applied built-in: it runs once `args` reaches `arity`.
    Builtin {
        name: Rc<str>,
        arity: usize,
        args: Vec<Value>,
    },
    /// An algebraic effect operation. Applying it performs the operation.
    Operation {
        effect: Option<String>,
        op: String,
    },
    /// A captured affine continuation. Applying it resumes the suspended
    /// computation once.
    Resumption(Continuation),
}

/// An opaque, one-shot continuation captured by an effect handler. Its interior
/// is the interpreter's private machinery; a value only ever gets one by
/// performing an operation, and consumes it by applying it.
#[derive(Clone)]
pub struct Continuation(pub(crate) Rc<RefCell<Option<Resume>>>);

pub(crate) type Eval = Result<Outcome>;
pub(crate) type Resume = Rc<dyn Fn(Value) -> Eval>;

/// A pending `defer` cleanup: the cleanup term and the environment to run it in.
/// It runs when the deferred body's dynamic scope exits (see `Term::Defer`).
pub(crate) type Finalizer = (Arc<Term>, Env);

pub(crate) enum Outcome {
    Value(Value),
    Perform {
        effect: Option<String>,
        op: String,
        arg: Value,
        resume: Resume,
        /// `defer` cleanups pending between the perform point and the handler.
        /// A handler that abandons this continuation runs them; one that resumes
        /// leaves them to the resumption (which re-wraps them).
        finalizers: Vec<Finalizer>,
    },
}

pub struct Closure {
    pub(crate) param: String,
    pub(crate) body: Arc<Term>,
    pub(crate) env: Env,
}

/// A lexical environment: a linked list of single-binding scopes. A slot is
/// shared and mutable so a recursive binding can be filled after the closure
/// that captures it is built.
pub(crate) type Env = Option<Rc<Scope>>;

pub(crate) struct Scope {
    pub(crate) name: String,
    pub(crate) slot: Rc<RefCell<Value>>,
    pub(crate) parent: Env,
}

/// A global's evaluation state: unforced code, in-progress (to catch a value-level
/// self-reference), or its forced value.
pub(crate) enum GCell {
    Thunk(Arc<Term>),
    Forcing,
    Forced(Value),
}

#[derive(Clone)]
pub struct Interp {
    pub(crate) rt: Rc<Runtime>,
}

pub(crate) struct Runtime {
    /// Globals keyed canonically by `Module.name`.
    pub(crate) globals: HashMap<String, RefCell<GCell>>,
    /// Bare `name` to its canonical key, so an unqualified reference resolves.
    pub(crate) bare: HashMap<String, String>,
    /// Effect-qualified operations, keyed by effect name.
    pub(crate) ops_by_effect: HashMap<String, Vec<String>>,
    /// Operation names to the effects that declare them.
    pub(crate) ops_by_name: HashMap<String, Vec<String>>,
}
