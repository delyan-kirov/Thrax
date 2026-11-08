//! The IR-to-C emitter: the port of `engines/CC.cpp`. Each lifted [`Code`] is
//! lowered to BLOCK FUNCTIONS driven by the CEK machine in the runtime
//! (`runtime.c`): a block runs straight-line C (atoms, pure lets, case branching)
//! and ends by calling exactly one TERMINATOR (`ARESxK_ret` / `ARESxK_tailcall` /
//! `ARESxK_apply` / `ARESxK_jump` / `ARESxK_handle` / `ARESxK_defer_run`). A non-tail
//! application is a suspension point: the rest of the computation becomes a
//! continuation block, so the delimited continuation a handler captures is a pure
//! heap object rather than live C-stack frames.
//!
//! The lowering is continuation-passing: [`Emitter::emit_expr`] lowers an
//! expression so its value reaches a [`Sink`], either delivered up the
//! continuation (`RET`) or written to a frame slot before jumping to a
//! continuation block (`SLOT`). Pure lets stay inline; only suspending
//! sub-expressions split into fresh blocks.
//!
//! Known global functions that can never suspend are also emitted as plain C
//! functions that blocks and each other call directly, with integers unboxed
//! where the code shows them to be integers. See Note [Direct functions].
//! See Note [Unboxed integers].

use std::collections::{HashMap, HashSet};

use frontend::ir::data::{native_builtin, native_builtins, Atom, AltKind, BuiltinHost, IrExpr, IrProgram, ResumeUse, Unpack};
use utilities::StrId;

/// What a saturated call found by `call_chain` calls.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Callee {
    /// A built-in.
    Prim,
    /// A known global function (see [`KnownFn`]), by the code of its CAF.
    Known(usize),
    /// A global bound to a foreign function (`@extern`), by its index in the
    /// extern table: saturated, it is one synchronous call of its C wrapper,
    /// as a built-in is.
    Extern(usize),
}

/// A function applied to all its operands, found by `call_chain`.
struct Call<'e> {
    name: &'e str,
    callee: Callee,
    args: Vec<&'e Atom>,
    drops: Vec<usize>,
    rest: Option<(usize, &'e IrExpr)>,
}

/// A global whose value is a closure literal, seen through its currying: the
/// codes of its lambdas, outermost first, where each but the last only captures
/// its parameter and what it was given and returns the next closure. Applied to
/// `codes.len()` arguments, it runs the last code with field `j` of its
/// environment bound to argument `env[j]`, which is what lets a saturated call
/// skip building the intermediate closures.
#[derive(Clone)]
struct KnownFn {
    codes: Vec<usize>,
    env: Vec<usize>,
}

impl KnownFn {
    fn arity(&self) -> usize {
        self.codes.len()
    }

    fn inner(&self) -> usize {
        *self.codes.last().expect("a known function has a code")
    }
}

/// Which of a direct function's arguments, and whether its result, cross as C
/// `int64_t` rather than boxed. See Note [Unboxed integers].
#[derive(Clone, PartialEq)]
struct Sig {
    int_params: Vec<bool>,
    int_ret: bool,
}

/// What `int_walk` knows at a point of a direct function's body: whether slot
/// 0 still holds the last argument, which slots hold integers, and which
/// arguments have been seen used as integers.
struct IntState {
    p0: bool,
    ints: HashSet<usize>,
    evidence: Vec<bool>,
}

/// The closure a code returns, under the drop of its unread parameter.
fn returned_closure(e: &IrExpr) -> Option<(usize, &[Atom])> {
    match e {
        IrExpr::Ret(Atom::Clos { code, captures }) => Some((*code, captures)),
        IrExpr::Drop { slots, body } if slots.as_slice() == [0] => returned_closure(body),
        _ => None,
    }
}

/// The global function a CAF defines, if its body is a closure literal.
fn known_fn(prog: &IrProgram, caf: usize) -> Option<KnownFn> {
    let (first, captures) = returned_closure(&prog.codes[caf].body)?;
    if !captures.is_empty() || prog.codes[first].nparams != 1 {
        return None;
    }
    let mut f = KnownFn { codes: vec![first], env: Vec::new() };
    'chain: while let Some((next, captures)) = returned_closure(&prog.codes[f.inner()].body) {
        if prog.codes[next].nparams != 1 {
            break;
        }
        let param = f.arity() - 1;
        let mut env = Vec::with_capacity(captures.len());
        for c in captures {
            match c {
                Atom::Local(0) | Atom::Move(0) => env.push(param),
                Atom::Env(i) | Atom::MoveEnv(i) if *i < f.env.len() => env.push(f.env[*i]),
                _ => break 'chain,
            }
        }
        f.codes.push(next);
        f.env = env;
    }
    Some(f)
}

/// Where a computed value goes.
#[derive(Clone, Copy)]
enum Sink {
    /// Delivered up the continuation stack (`ARESxK_ret` / `ARESxK_tailcall`).
    Ret,
    /// Written into frame `slot` (a let-box back-patch); then, if `cont` is
    /// `Some`, control jumps to that continuation block. `None` is the pure-into
    /// sink (back-patch and fall through, used by a pure Case's branches).
    Slot { slot: usize, cont: Option<usize> },
    /// Assigned to the C `int64_t` named `sv<var>`: the branches of a pure
    /// Case whose `let` binds an integer every use reads unboxed.
    /// See Note [Unboxed integers].
    Int { var: usize },
}

// Note [Direct functions]
//
// Running every call through the machine (a heap frame, a continuation frame,
// a bounce through the driver) is what effects need: a handler captures the
// continuation as heap data. A call that cannot perform an effect needs none of
// it, so a known global function whose body provably never suspends also gets
// a plain C function `d_<code>` with its activation on the C stack: arguments
// owned, result owned, the frame's slots in a C array behind a stack `Frame`
// so the ordinary slot helpers work unchanged. Saturated calls to it from
// anywhere become C calls, and its self tail calls a loop.
//
// The callee owns its arguments so that a move survives the call: a caller's
// last use of a value gives its reference away (`ARESxVALUE_give`) instead of
// parking it in the caller's temporary pool, and the callee's parameter row is
// its frame's env, from which a last use moves out again. A vector threaded
// through a chain of direct calls is then held once and `@vec_set` /
// `@vec_push` update it in place; any other argument is lent a new reference.
//
// "Never suspends" is decided on the IR alone (`direct_safe`): no handler, no
// `defer`, and every call either a built-in, a partial application of a known
// function or built-in (which only builds a value), or a saturated call of
// another direct function. A greatest fixpoint, so mutual recursion qualifies.
// Forcing a thunk or running a higher-order built-in nests a machine run, as it
// does for the block code.
//
// The C stack is finite where the machine's heap continuation is not, so a call
// is made directly only while `ARESxD_room()` says the stack has room. Past that
// a direct function runs the callee on a nested machine, and blocks take the
// ordinary machine path, where further recursion grows the heap. So deep
// recursion still works, only slower.

/// The direct function being emitted. See Note [Direct functions].
#[derive(Clone, Copy)]
struct Direct {
    /// Its last code, which a self tail call loops back into.
    inner: usize,
    arity: usize,
    /// Whether it returns an unboxed integer.
    int_ret: bool,
}

/// A distinct `@extern` binding the C backend emits a wrapper for. Deduplicated
/// by symbol + library + signature; a `ARESxRT_extern(idx, arity)` value points at
/// its wrapper in the generated `ARESxRT_extern_table`. The wrapper is N-ary (one
/// positional C argument per `arg_types` entry, or one unit argument for a
/// nullary C function): the record grouping several C parameters is flattened
/// into positional arguments at the call site during lowering.
#[derive(Clone)]
pub struct ExternSite {
    pub abi: StrId,
    pub symbol: StrId,
    pub lib: StrId,
    pub arg_types: Vec<StrId>,
    pub ret_type: StrId,
}

pub struct Emitter<'p> {
    prog: &'p IrProgram,
    /// Canonical names of every global (`Module.name`) -> its code; a `Glob`
    /// naming one resolves to that CAF.
    globals: HashMap<&'p str, usize>,
    /// Bare last segments of the global names -> code (first definition wins),
    /// the unqualified/entry fallback.
    bare: HashMap<&'p str, usize>,
    /// Effect name -> the operations it declares (for qualified resolution).
    ops_by_effect: HashMap<&'p str, Vec<&'p str>>,
    /// Operation name -> the effects declaring it (for bare resolution).
    ops_by_name: HashMap<&'p str, Vec<&'p str>>,
    /// The distinct foreign functions, in wrapper-table order.
    externs: Vec<ExternSite>,
    /// Dedup key (`symbol\x1flib\x1fsig`) -> its index in `externs`.
    extern_idx: HashMap<String, usize>,
    /// One entry per emitted block: its full C function text, filled out of order
    /// (a block is reserved, then set once its body is built).
    blocks: Vec<String>,
    /// Code index -> its entry block id.
    code_entry: Vec<usize>,
    /// Codes reachable from the entry global; unreachable codes are emitted as
    /// trap stubs and their `@extern` sites are not collected.
    reachable: HashSet<usize>,
    tmp: usize,
    /// Whether an emitted block names a [`BuiltinHost::Compiler`] builtin, so
    /// the program must link `libares`.
    uses_compiler: std::cell::Cell<bool>,
    /// Whether an emitted block names a [`BuiltinHost::Parallel`] builtin, so
    /// the runtime must be built thread-ready (`ARES_THREADS`); every other
    /// program keeps plain, non-atomic counts.
    uses_threads: std::cell::Cell<bool>,
    /// The known global functions, by the code of their CAF.
    known: HashMap<usize, KnownFn>,
    /// The CAFs of the known functions compiled as direct C functions.
    direct_fns: HashSet<usize>,
    /// The signatures of the direct functions, by the code of their CAF.
    sigs: HashMap<usize, Sig>,
    /// Set while emitting a direct function's body.
    direct: Option<Direct>,
    /// While emitting a direct function: its environment fields that hold an
    /// unboxed argument -> the C variable.
    env_ints: HashMap<usize, String>,
    /// Whether the direct function being emitted loops on a self tail call.
    looped: bool,
    /// The slots whose value lives in a C scalar instead: slot -> the C
    /// variable, and whether it holds a boolean. See Note [Unboxed integers].
    scalars: HashMap<usize, (String, bool)>,
}

// Note [Unboxed integers]
//
// A boxed value is a heap allocation, and for integer arithmetic it is most of
// the cost: `if n < 2` allocates the literal and the boolean. The integer
// intrinsics only ever see integers (the type-directed instances guarantee the
// tags, which is why the runtime reads `u.i` unchecked), so a saturated one is
// emitted as C arithmetic on `int64_t` operands: a literal operand is a C
// literal, a boxed one is read in place.
//
// Its result stays a C scalar when every use of the slot is another such
// operand or the scrutinee of a literal case, and all of them are in the same
// C function (`scalar_uses`). Otherwise it is boxed once, as before. A slot
// rebound after its scalar's last use is boxed as usual from then on.

/// The C enum constant the runtime dispatches a built-in on: `@vec_get` is
/// `ARESxB_VEC_GET`, an operator spells its symbol out.
pub fn builtin_ident(name: &str) -> String {
    let stem = match name {
        "==" => "EQ".to_string(),
        "<" => "LT".to_string(),
        ">" => "GT".to_string(),
        "<=" => "LE".to_string(),
        ">=" => "GE".to_string(),
        "++" => "CONCAT".to_string(),
        _ => {
            let stem = name.strip_prefix('@').unwrap_or(name);
            assert!(
                stem.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                "built-in `{name}` has no C identifier"
            );
            stem.to_ascii_uppercase()
        }
    };
    format!("ARESxB_{stem}")
}

/// The `ARESxB_*` enum, emitted ahead of the runtime, and the names it indexes
/// (only the compiler builtins report one).
pub fn emit_builtin_enum() -> String {
    let mut out = String::from("enum {\n");
    for b in native_builtins() {
        out.push_str(&format!("  {},\n", builtin_ident(b.name)));
    }
    out.push_str("  ARESxB_COUNT\n};\n");
    out.push_str("#ifdef ARES_WITH_COMPILER\n");
    out.push_str("static const char *const ARESxB_names[] = {\n");
    for b in native_builtins() {
        out.push_str(&format!("  {},\n", cstr(b.name.as_bytes())));
    }
    out.push_str("};\n#endif\n");
    out
}

/// The dedup key for an extern site.
fn extern_key(
    abi: &StrId,
    symbol: &StrId,
    lib: &StrId,
    arg_types: &[StrId],
    ret_type: &StrId,
) -> String {
    let arg_types: Vec<&str> = arg_types.iter().map(|t| t.as_str()).collect();
    format!(
        "{abi}\x1f{symbol}\x1f{lib}\x1f{}\x1f{ret_type}",
        arg_types.join(",")
    )
}

/// Visit every [`Atom`] occurring in `e` (including those captured inside a
/// nested [`Atom::Clos`]).
fn for_each_atom(e: &IrExpr, f: &mut impl FnMut(&Atom)) {
    match e {
        IrExpr::Ret(a) | IrExpr::Field { rec: a, .. } => visit_atom(a, f),
        IrExpr::App { fun, arg, .. } => {
            visit_atom(fun, f);
            visit_atom(arg, f);
        }
        IrExpr::MkStruct { base, fields, .. } => {
            if let Some(b) = base {
                visit_atom(b, f);
            }
            for (_, v) in fields {
                visit_atom(v, f);
            }
        }
        IrExpr::MkVariant { fields, .. } => {
            for v in fields {
                visit_atom(v, f);
            }
        }
        IrExpr::MkTuple(items) => {
            for v in items {
                visit_atom(v, f);
            }
        }
        IrExpr::Fault(_) => {}
        IrExpr::Let { rhs, body, .. } => {
            for_each_atom(rhs, f);
            for_each_atom(body, f);
        }
        IrExpr::Case { scrut, alts, default } => {
            visit_atom(scrut, f);
            for al in alts {
                for_each_atom(&al.body, f);
            }
            for_each_atom(default, f);
        }
        IrExpr::Handle { body, clauses, els } => {
            for_each_atom(body, f);
            for c in clauses {
                visit_atom(&c.fun, f);
            }
            visit_atom(els, f);
        }
        IrExpr::Defer { cleanup, body } => {
            visit_atom(cleanup, f);
            for_each_atom(body, f);
        }
        IrExpr::Drop { body, .. } => for_each_atom(body, f),
    }
}

/// Prepare a `let`'s slot for its value. Only a recursive binding, whose
/// right-hand side captures the slot's box before the value exists, needs the
/// box; any other slot is left empty and the value moves straight in.
fn emit_let_slot(rhs: &IrExpr, slot: usize, out: &mut String) {
    if rhs.mentions(slot) {
        out.push_str(&format!("  ARESxK_setbox(fr, {slot});\n"));
    } else {
        out.push_str(&format!("  ARESxK_drop(fr, {slot});\n"));
    }
}

/// How generated code passes one operand of an inline intrinsic.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Opnd {
    /// An unboxed `int64_t`.
    Int,
    /// An unboxed truth value (a C `int`).
    Bool,
    /// A boxed `Value*` the intrinsic only reads.
    Boxed,
}

/// The operands of a builtin generated code computes inline; `None` for one
/// that goes through `ARESxK_prim`. See Note [Unboxed integers].
fn intrinsic_operands(name: &str) -> Option<&'static [Opnd]> {
    use Opnd::*;
    Some(match name {
        "@iadd" | "@isub" | "@imul" | "@idiv" | "@imod" | "@udiv" | "@umod" | "@ieq" | "@ilt" | "@ult" => {
            &[Int, Int]
        }
        "not" => &[Bool],
        "@array_len" => &[Boxed],
        "@array_get" => &[Boxed, Int],
        "@array_scan" | "@array_match" => &[Boxed, Int, Boxed],
        _ => return None,
    })
}

/// Whether operand `k` of an inline intrinsic is unboxed (an integer or a
/// truth value), so a scalar may feed it as is.
fn int_operand(name: &str, k: usize) -> bool {
    intrinsic_operands(name).is_some_and(|ops| matches!(ops.get(k), Some(Opnd::Int | Opnd::Bool)))
}

/// The C of an inline intrinsic on its operands (each as
/// [`intrinsic_operands`] says), and whether its result is a boolean (else an
/// integer). Arithmetic wraps, as the boxed runtime's does in practice,
/// without C's signed-overflow undefined behaviour.
fn int_intrinsic(name: &str, args: &[String]) -> Option<(String, bool)> {
    if intrinsic_operands(name)?.len() != args.len() {
        return None;
    }
    let a = &args[0];
    let b = args.get(1).map_or("", |b| b.as_str());
    let wrap = |op: &str| format!("(int64_t)((uint64_t)({a}) {op} (uint64_t)({b}))");
    Some(match name {
        "@iadd" => (wrap("+"), false),
        "@isub" => (wrap("-"), false),
        "@imul" => (wrap("*"), false),
        "@idiv" => (format!("ARESxI_div({a}, {b})"), false),
        "@imod" => (format!("ARESxI_mod({a}, {b})"), false),
        "@udiv" => (format!("ARESxI_udiv({a}, {b})"), false),
        "@umod" => (format!("ARESxI_umod({a}, {b})"), false),
        "@ieq" => (format!("(({a}) == ({b}))"), true),
        "@ilt" => (format!("(({a}) < ({b}))"), true),
        "@ult" => (format!("((uint64_t)({a}) < (uint64_t)({b}))"), true),
        "not" => (format!("(!({a}))"), true),
        "@array_len" => (format!("ARESxA_len({a})"), false),
        "@array_get" => (format!("ARESxA_get({a}, {b})"), false),
        "@array_scan" => (format!("ARESxA_scan({a}, {b}, {})", args[2]), false),
        "@array_match" => (format!("ARESxA_match({a}, {b}, {})", args[2]), true),
        _ => return None,
    })
}

/// A C `int64_t` literal; the most negative one has no literal of its own.
fn clit(n: i64) -> String {
    if n == i64::MIN {
        "(-9223372036854775807LL - 1)".to_string()
    } else {
        format!("{n}LL")
    }
}

fn visit_atom(a: &Atom, f: &mut impl FnMut(&Atom)) {
    f(a);
    if let Atom::Clos { captures, .. } = a {
        for c in captures {
            visit_atom(c, f);
        }
    }
}

/// The codes reachable from the entry global, resolving a `Glob` reference the
/// way the runtime does (exact canonical global, else the bare last-segment
/// fallback; builtins/`TARGET`/operations are not codes) and following each
/// `Clos`'s lifted code. Unreachable codes (e.g. the auto-injected `C` libc
/// bindings a program never calls) are pruned so their `@extern` wrappers are
/// not emitted; on a strict-signature target like wasm those redeclarations
/// would otherwise clash with the real libc.
pub fn reachable_codes(prog: &IrProgram, entry: &str) -> HashSet<usize> {
    let mut canon: HashMap<&str, usize> = HashMap::new();
    let mut bare: HashMap<&str, usize> = HashMap::new();
    for (name, code) in &prog.globals {
        let name = name.as_str();
        canon.entry(name).or_insert(*code);
        let last = name.rsplit('.').next().unwrap_or(name);
        bare.entry(last).or_insert(*code);
    }
    let resolve = |name: &str| -> Option<usize> {
        canon.get(name).copied().or_else(|| {
            let last = name.rsplit('.').next().unwrap_or(name);
            bare.get(last).copied()
        })
    };

    let mut seen: HashSet<usize> = HashSet::new();
    let mut work: Vec<usize> = resolve(entry).into_iter().collect();
    while let Some(c) = work.pop() {
        if !seen.insert(c) {
            continue;
        }
        for_each_atom(&prog.codes[c].body, &mut |a| match a {
            Atom::Glob { name } => {
                if let Some(cc) = resolve(name.as_str()) {
                    work.push(cc);
                }
            }
            Atom::Clos { code, .. } => work.push(*code),
            _ => {}
        });
    }
    seen
}

/// Register any `Atom::Extern` reachable in `e` into the table (deduplicated).
fn collect_externs(
    e: &IrExpr,
    externs: &mut Vec<ExternSite>,
    idx: &mut HashMap<String, usize>,
) {
    let mut atom = |a: &Atom| collect_extern_atom(a, externs, idx);
    match e {
        IrExpr::Ret(a) | IrExpr::Field { rec: a, .. } => atom(a),
        IrExpr::App { fun, arg, .. } => {
            atom(fun);
            atom(arg);
        }
        IrExpr::MkStruct { base, fields, .. } => {
            if let Some(b) = base {
                atom(b);
            }
            fields.iter().for_each(|(_, v)| atom(v));
        }
        IrExpr::MkVariant { fields, .. } => fields.iter().for_each(&mut atom),
        IrExpr::MkTuple(items) => items.iter().for_each(&mut atom),
        IrExpr::Fault(_) => {}
        IrExpr::Let { rhs, body, .. } => {
            collect_externs(rhs, externs, idx);
            collect_externs(body, externs, idx);
        }
        IrExpr::Case { scrut, alts, default } => {
            collect_extern_atom(scrut, externs, idx);
            for al in alts {
                collect_externs(&al.body, externs, idx);
            }
            collect_externs(default, externs, idx);
        }
        IrExpr::Handle { body, clauses, els } => {
            collect_externs(body, externs, idx);
            for c in clauses {
                collect_extern_atom(&c.fun, externs, idx);
            }
            collect_extern_atom(els, externs, idx);
        }
        IrExpr::Defer { cleanup, body } => {
            collect_extern_atom(cleanup, externs, idx);
            collect_externs(body, externs, idx);
        }
        IrExpr::Drop { body, .. } => collect_externs(body, externs, idx),
    }
}

fn collect_extern_atom(a: &Atom, externs: &mut Vec<ExternSite>, idx: &mut HashMap<String, usize>) {
    match a {
        Atom::Extern {
            abi,
            symbol,
            lib,
            arg_types,
            ret_type,
        } => {
            let key = extern_key(abi, symbol, lib, arg_types, ret_type);
            idx.entry(key).or_insert_with(|| {
                externs.push(ExternSite {
                    abi: *abi,
                    symbol: *symbol,
                    lib: *lib,
                    arg_types: arg_types.clone(),
                    ret_type: *ret_type,
                });
                externs.len() - 1
            });
        }
        Atom::Clos { captures, .. } => {
            for c in captures {
                collect_extern_atom(c, externs, idx);
            }
        }
        _ => {}
    }
}

/// A C string literal (with surrounding quotes) for an arbitrary byte view.
pub fn cstr(bytes: &[u8]) -> String {
    let mut s = String::from("\"");
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            b'\t' => s.push_str("\\t"),
            b'\r' => s.push_str("\\r"),
            0x20..=0x7e => s.push(c as char),
            _ => {
                s.push_str(&format!("\\x{c:02x}"));
                // Guard against a following hex digit extending the escape.
                if let Some(&n) = bytes.get(i + 1) {
                    if n.is_ascii_hexdigit() {
                        s.push_str("\"\"");
                    }
                }
            }
        }
    }
    s.push('"');
    s
}

/// A C double literal that round-trips the value.
fn cdbl(v: f64) -> String {
    format!("{v:.17e}")
}

/// How a Ares scalar crosses the C ABI in a generated `@extern` wrapper.
/// `Int(t)` names the exact C integer type; the word-sized fallback (`Int`, a
/// type variable, or any unrecognized name) is `int64_t`, matching the C++
/// `desc_of`.
enum Cabi {
    Bytes,             // @Str/@Array: the byte pointer (char*)
    Ptr,               // @Ptr: an opaque pointer carried as a word (void*)
    F32,               // @Float32: float
    F64,               // @Float64: double
    Unit,              // {}: a void result
    Int(&'static str), // a sized/word integer: the named C int type
}

fn cabi(name: &str) -> Cabi {
    match name {
        "@Str" | "@Array" => Cabi::Bytes,
        "@Ptr" => Cabi::Ptr,
        "@Float32" => Cabi::F32,
        "@Float64" => Cabi::F64,
        "{}" => Cabi::Unit,
        "@Int8" => Cabi::Int("int8_t"),
        "@Int16" => Cabi::Int("int16_t"),
        "@Int32" => Cabi::Int("int32_t"),
        "@Int64" => Cabi::Int("int64_t"),
        "@Nat8" => Cabi::Int("uint8_t"),
        "@Nat16" => Cabi::Int("uint16_t"),
        "@Nat32" => Cabi::Int("uint32_t"),
        "@Nat64" | "@Nat" => Cabi::Int("uint64_t"),
        "@Int" => Cabi::Int("int64_t"),
        _ => Cabi::Int("int64_t"),
    }
}

/// The C parameter type and the expression marshalling `args[i]` into it.
fn c_param(name: &str, i: usize) -> (&'static str, String) {
    match cabi(name) {
        Cabi::Bytes => ("char*", format!("(char*)ARESxVALUE_str(args[{i}])")),
        Cabi::Ptr => ("void*", format!("(void*)(intptr_t)ARESxVALUE_as_int(args[{i}])")),
        Cabi::F32 => ("float", format!("(float)ARESxVALUE_as_num(args[{i}])")),
        Cabi::F64 => ("double", format!("ARESxVALUE_as_num(args[{i}])")),
        // A `{}` parameter is unusual; treat it as a word.
        Cabi::Unit => ("int64_t", format!("(int64_t)ARESxVALUE_as_int(args[{i}])")),
        Cabi::Int(t) => (t, format!("({t})ARESxVALUE_as_int(args[{i}])")),
    }
}

/// The C return type, and (for a non-void result) the expression wrapping the
/// C result `_r` back into a `Value*`.
fn c_ret(name: &str) -> (&'static str, Option<&'static str>) {
    match cabi(name) {
        Cabi::Bytes => ("char*", Some("_r ? ARESxRT_str(_r, strlen(_r)) : ARESxRT_str(\"\", 0)")),
        Cabi::Ptr => ("void*", Some("ARESxRT_int((long long)(intptr_t)_r)")),
        Cabi::F32 => ("float", Some("ARESxRT_real32(_r)")),
        Cabi::F64 => ("double", Some("ARESxRT_real(_r)")),
        Cabi::Unit => ("void", None),
        Cabi::Int(t) => (t, Some("ARESxRT_int((long long)_r)")),
    }
}

/// The C scalar type for a leaf kind, and whether it is a floating type (so the
/// wrapper reads/writes it as a `num` rather than an `int`).
fn c_leaf(kind: &utilities::CKind) -> (&'static str, bool) {
    use utilities::CKind::*;
    match kind {
        S8 => ("int8_t", false),
        S16 => ("int16_t", false),
        S32 => ("int32_t", false),
        S64 => ("int64_t", false),
        U8 => ("uint8_t", false),
        U16 => ("uint16_t", false),
        U32 => ("uint32_t", false),
        U64 => ("uint64_t", false),
        F32 => ("float", true),
        F64 => ("double", true),
        Struct(..) => ("void", false), // handled by name via `c_field_type`
    }
}

/// The C identifier for a C-repr struct's emitted `typedef`.
fn cstruct_name(ty: &str) -> String {
    let safe: String = ty
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("ARESx_cstruct_{safe}")
}

/// The C type of a struct field: a scalar leaf's C type, or a nested struct's
/// emitted typedef name.
fn c_field_type(kind: &utilities::CKind) -> String {
    match kind {
        utilities::CKind::Struct(name, _) => cstruct_name(name),
        leaf => c_leaf(leaf).0.to_string(),
    }
}

/// Emit `typedef struct { ... } ARESx_cstruct_<name>;`, recursing into nested
/// struct fields first (a nested typedef must precede its use). `done` dedups.
fn emit_cstruct_typedef(
    name: &str,
    layout: &utilities::CLayout,
    out: &mut String,
    done: &mut std::collections::HashSet<String>,
) {
    if !done.insert(name.to_string()) {
        return;
    }
    for f in &layout.fields {
        if let utilities::CKind::Struct(inner_name, inner) = &f.kind {
            emit_cstruct_typedef(inner_name, inner, out, done);
        }
    }
    let keyword = if layout.is_union { "union" } else { "struct" };
    out.push_str(&format!("typedef {keyword} {{ "));
    for f in &layout.fields {
        out.push_str(&format!("{} {}; ", c_field_type(&f.kind), f.name));
    }
    out.push_str(&format!("}} {};\n", cstruct_name(name)));
}

/// Emit statements filling the C struct lvalue `dst` from the Ares struct
/// `Value*` expression `src`, recursing through nested struct fields. For a union
/// only the members the value actually carries are written (each guarded by a
/// presence check), so a value built with one member writes just that one.
fn emit_pack(dst: &str, src: &str, layout: &utilities::CLayout, out: &mut String) {
    for f in &layout.fields {
        let fdst = format!("{dst}.{}", f.name);
        let field = cstr(f.name.as_bytes());
        if layout.is_union {
            out.push_str(&format!("  {{ Value* _u = struct_field({src}, {field});\n  if (_u) {{\n"));
            emit_field_assign(&fdst, "_u", &f.kind, out);
            out.push_str("  } }\n");
        } else {
            let fsrc = format!("ARESxVALUE_field({src}, {field})");
            emit_field_assign(&fdst, &fsrc, &f.kind, out);
        }
    }
}

/// The element type of a `List T` array param marshal name `@structs(T)`.
fn struct_array_elem(ty: &str) -> Option<&str> {
    ty.strip_prefix("@structs(").and_then(|s| s.strip_suffix(')'))
}

/// Parse a callback marshal name `@fn(a,b,...)->r` into (arg marshal names, ret).
fn parse_cb(ty: &str) -> Option<(Vec<String>, String)> {
    let rest = ty.strip_prefix("@fn(")?;
    let (args, ret) = rest.split_once(")->")?;
    let args = if args.is_empty() {
        Vec::new()
    } else {
        args.split(',').map(str::to_string).collect()
    };
    Some((args, ret.to_string()))
}

/// The C type for a scalar marshal name (callback arg/return).
fn cb_ctype(name: &str) -> &'static str {
    match cabi(name) {
        Cabi::Bytes => "char*",
        Cabi::Ptr => "void*",
        Cabi::F32 => "float",
        Cabi::F64 => "double",
        Cabi::Unit => "void",
        Cabi::Int(t) => t,
    }
}

/// The libffi kind code for a scalar marshal name (matches the shim's enum).
fn cb_kind(name: &str) -> i32 {
    match cabi(name) {
        Cabi::Bytes | Cabi::Ptr => 11,
        Cabi::F32 => 9,
        Cabi::F64 => 10,
        Cabi::Unit => 0,
        Cabi::Int(t) => match t {
            "int8_t" => 1,
            "int16_t" => 2,
            "int32_t" => 3,
            "uint8_t" => 5,
            "uint16_t" => 6,
            "uint32_t" => 7,
            "uint64_t" => 8,
            _ => 4,
        },
    }
}

/// The C function-pointer type of a callback, e.g. `int64_t (*)(int64_t, int64_t)`.
fn cb_fnptr_type(args: &[String], ret: &str) -> String {
    let params = if args.is_empty() {
        "void".to_string()
    } else {
        args.iter().map(|a| cb_ctype(a)).collect::<Vec<_>>().join(", ")
    };
    format!("{} (*)({params})", cb_ctype(ret))
}

/// The C runtime supporting Ares closures passed to C as function pointers,
/// via libffi closures. Emitted only when a program has a callback parameter.
const CALLBACK_RUNTIME: &str = r#"
/* -- callbacks: a Ares closure as a C function pointer (libffi closures) -- */
#include <ffi.h>
#include <stdlib.h>
static ffi_type* arx_cbty(int k){
  switch(k){case 0:return &ffi_type_void;case 1:return &ffi_type_sint8;case 2:return &ffi_type_sint16;
  case 3:return &ffi_type_sint32;case 4:return &ffi_type_sint64;case 5:return &ffi_type_uint8;
  case 6:return &ffi_type_uint16;case 7:return &ffi_type_uint32;case 8:return &ffi_type_uint64;
  case 9:return &ffi_type_float;case 10:return &ffi_type_double;default:return &ffi_type_pointer;}
}
typedef struct { Value* closure; int nargs; int akinds[16]; int rkind; ffi_cif cif; ffi_type* aty[16]; ffi_closure* clo; void* code; } ARESxCB;
static Value* arx_cb_argval(void* p, int k){
  switch(k){case 9:return ARESxRT_real32(*(float*)p);case 10:return ARESxRT_real(*(double*)p);
  case 1:return ARESxRT_int(*(int8_t*)p);case 2:return ARESxRT_int(*(int16_t*)p);case 3:return ARESxRT_int(*(int32_t*)p);
  case 4:return ARESxRT_int(*(int64_t*)p);case 5:return ARESxRT_int(*(uint8_t*)p);case 6:return ARESxRT_int(*(uint16_t*)p);
  case 7:return ARESxRT_int(*(uint32_t*)p);case 8:return ARESxRT_int((long long)*(uint64_t*)p);
  default:return ARESxRT_int((long long)(intptr_t)*(void**)p);}
}
static void arx_cb_setret(void* ret, int k, Value* v){
  switch(k){case 0:break;case 9:*(float*)ret=(float)ARESxVALUE_as_num(v);break;case 10:*(double*)ret=ARESxVALUE_as_num(v);break;
  default:*(ffi_arg*)ret=(ffi_arg)ARESxVALUE_as_int(v);break;}
}
static void arx_cb_bind(ffi_cif* cif, void* ret, void** args, void* user){
  (void)cif; ARESxCB* cb=user;
  Value* r=cb->closure; ARESxMEM_retain(r);           /* own a starting reference */
  for(int i=0;i<cb->nargs;i++){
    Value* nr = ARESxK_call(r, arx_cb_argval(args[i], cb->akinds[i])); /* owned */
    ARESxMEM_release(r);                               /* drop the previous owned value */
    r = nr;
  }
  arx_cb_setret(ret, cb->rkind, r);
  ARESxMEM_release(r);                                 /* drop the final result */
}
static void* arx_cb_make(Value* closure, int nargs, const int* kinds, int rkind, void** code){
  ARESxCB* cb=calloc(1,sizeof(ARESxCB)); cb->closure=closure; ARESxMEM_retain(closure); cb->nargs=nargs; cb->rkind=rkind;
  for(int i=0;i<nargs;i++){cb->akinds[i]=kinds[i];cb->aty[i]=arx_cbty(kinds[i]);}
  ffi_prep_cif(&cb->cif, FFI_DEFAULT_ABI, nargs, arx_cbty(rkind), cb->aty);
  cb->clo=ffi_closure_alloc(sizeof(ffi_closure), &cb->code);
  ffi_prep_closure_loc(cb->clo,&cb->cif,arx_cb_bind,cb,cb->code);
  *code=cb->code; return cb;
}
static void arx_cb_free(void* h){ ARESxCB* cb=h; if(cb){ ARESxMEM_release(cb->closure); ffi_closure_free(cb->clo); free(cb);} }
"#;

/// Assign one field: recurse for a nested struct, or read a scalar leaf.
fn emit_field_assign(fdst: &str, fsrc: &str, kind: &utilities::CKind, out: &mut String) {
    match kind {
        utilities::CKind::Struct(_, inner) => emit_pack(fdst, fsrc, inner, out),
        leaf => {
            let (cty, is_float) = c_leaf(leaf);
            let read = if is_float { "ARESxVALUE_as_num" } else { "ARESxVALUE_as_int" };
            out.push_str(&format!("  {fdst} = ({cty}){read}({fsrc});\n"));
        }
    }
}

/// Emit statements building a Ares struct `Value*` from the C struct expression
/// `src`, recursing through nested struct fields. Returns the temp holding it.
fn emit_build(
    src: &str,
    name: &str,
    layout: &utilities::CLayout,
    out: &mut String,
    ctr: &mut usize,
) -> String {
    let vals: Vec<String> = layout
        .fields
        .iter()
        .map(|f| {
            let cf = format!("{src}.{}", f.name);
            match &f.kind {
                utilities::CKind::Struct(inner_name, inner) => {
                    emit_build(&cf, inner_name, inner, out, ctr)
                }
                utilities::CKind::F32 => format!("ARESxRT_real32({cf})"),
                leaf if c_leaf(leaf).1 => format!("ARESxRT_real((double){cf})"),
                _ => format!("ARESxRT_int((long long){cf})"),
            }
        })
        .collect();
    let id = *ctr;
    *ctr += 1;
    let fnames: Vec<String> = layout.fields.iter().map(|f| cstr(f.name.as_bytes())).collect();
    out.push_str(&format!("  const char* _fn{id}[] = {{{}}};\n", fnames.join(", ")));
    out.push_str(&format!("  Value* _fv{id}[] = {{{}}};\n", vals.join(", ")));
    out.push_str(&format!(
        "  Value* _t{id} = ARESxRT_struct({}, {}, _fn{id}, _fv{id});\n",
        cstr(name.as_bytes()),
        layout.fields.len()
    ));
    format!("_t{id}")
}

/// Emit the foreign-function wrappers and the `ARESxRT_extern_table` the runtime
/// dispatches through. Each wrapper declares the C symbol with an `__asm__`
/// label (a direct call, resolved by the linker) and marshals the collected
/// arguments across the seam. C-repr structs (`@struct @extern`) referenced by a
/// wrapper are emitted as real C `typedef struct`s and passed/returned by value,
/// so the C compiler applies the platform ABI.
pub fn emit_extern_table(externs: &[ExternSite], layouts: &[(String, utilities::CLayout)]) -> String {
    let layout_of = |name: &str| layouts.iter().find(|(n, _)| n == name).map(|(_, l)| l);
    let mut out = String::from("\n/* -- foreign functions (@extern) -- */\n#include <stdint.h>\n");

    // Emit a C typedef for each C-repr struct any wrapper passes or returns
    // (nested structs first).
    let mut emitted: std::collections::HashSet<String> = std::collections::HashSet::new();
    for e in externs {
        for ty in e.arg_types.iter().chain(std::iter::once(&e.ret_type)) {
            let ty = ty.as_str();
            // A direct struct/union type, or the element of a `List T` array param.
            let sname = struct_array_elem(ty).unwrap_or(ty);
            if let Some(layout) = layout_of(sname) {
                emit_cstruct_typedef(sname, layout, &mut out, &mut emitted);
            }
        }
    }
    // The libffi-closure runtime, only when a program passes a Ares closure to C.
    let has_callbacks = externs
        .iter()
        .any(|e| e.arg_types.iter().any(|t| t.as_str().starts_with("@fn(")));
    if has_callbacks {
        out.push_str(CALLBACK_RUNTIME);
    }
    let libs: Vec<&str> = {
        let mut ls: Vec<&str> = externs
            .iter()
            .filter(|e| e.abi != "WASM" && e.abi != "load")
            .map(|e| e.lib.as_str())
            .collect();
        ls.sort_unstable();
        ls.dedup();
        ls
    };
    if !libs.is_empty() {
        out.push_str(&format!("/* link against: {} */\n", libs.join(", ")));
    }
    for (n, e) in externs.iter().enumerate() {
        // A `"WASM"` extern is a host import from the wasm embedder; standalone C
        // has no such host, so its wrapper faults if ever called. (In the browser
        // playground this program is interpreted, not compiled to C.)
        if e.abi == "WASM" {
            out.push_str(&format!(
                "static Value* ARESx_extern_{n}(Value** args) {{\n  (void)args;\n  \
                 ares_fault({});\n}}\n",
                cstr(format!("@extern \"WASM\" host import `{}` unavailable in native C", e.symbol)
                    .as_bytes())
            ));
            continue;
        }
        // The C return type + how to wrap the result back into a `Value*`.
        let ret_layout = layout_of(e.ret_type.as_str());
        let ret_ty: String = match ret_layout {
            Some(_) => cstruct_name(e.ret_type.as_str()),
            None => c_ret(e.ret_type.as_str()).0.to_string(),
        };

        // A `"load"` extern reads element `i` of the C array at `p` (its two
        // arguments) and wraps it as a C call's result would be.
        if e.abi == "load" {
            out.push_str(&format!(
                "static Value* ARESx_extern_{n}(Value** args) {{\n  \
                 const char* _p = (const char*)(intptr_t)ARESxVALUE_as_int(args[0]);\n  \
                 int64_t _i = ARESxVALUE_as_int(args[1]);\n  \
                 if (!_p) ares_fault(\"load from a null pointer\");\n  \
                 {ret_ty} _r;\n  \
                 memcpy(&_r, _p + _i * (int64_t)sizeof(_r), sizeof(_r));\n"
            ));
            match ret_layout {
                Some(layout) => {
                    let mut ctr = 0usize;
                    let tmp = emit_build("_r", e.ret_type.as_str(), layout, &mut out, &mut ctr);
                    out.push_str(&format!("  return {tmp};\n"));
                }
                None => {
                    let wrap = c_ret(e.ret_type.as_str()).1.expect("the checker admits only loadable results");
                    out.push_str(&format!("  return {wrap};\n"));
                }
            }
            out.push_str("}\n");
            continue;
        }

        // Each argument's C parameter type and the statements that build `a{i}`.
        // A `{}` parameter carries no C argument (a `void`-taking function like
        // `getchar`); it stays in the Ares arity but is dropped from the call.
        let mut sig_types: Vec<String> = Vec::new();
        let mut setup = String::new();
        let mut call_args: Vec<String> = Vec::new();
        let mut cb_frees: Vec<usize> = Vec::new();
        let mut arr_frees: Vec<usize> = Vec::new();
        for (i, t) in e.arg_types.iter().enumerate() {
            let t = t.as_str();
            if matches!(cabi(t), Cabi::Unit) && layout_of(t).is_none() {
                continue;
            }
            if let Some(elem) = struct_array_elem(t) {
                // A `@Vec T` array: pack each element of the vector into a contiguous
                // malloc'd buffer, and pass it as a `T*`.
                let cty = cstruct_name(elem);
                let layout = layout_of(elem).expect("struct-array element has a layout");
                sig_types.push(format!("{cty}*"));
                setup.push_str(&format!("  size_t _n{i} = args[{i}]->u.seq.len;\n"));
                setup.push_str(&format!(
                    "  {cty}* _arr{i} = _n{i} ? malloc(_n{i} * sizeof({cty})) : NULL;\n"
                ));
                setup.push_str(&format!(
                    "  for (size_t _j{i} = 0; _j{i} < _n{i}; _j{i}++) {{\n\
                     \x20   Value* _e = args[{i}]->u.seq.items[_j{i}];\n"
                ));
                emit_pack(&format!("_arr{i}[_j{i}]"), "_e", layout, &mut setup);
                setup.push_str(&format!("  }}\n"));
                call_args.push(format!("_arr{i}"));
                arr_frees.push(i);
            } else if let Some((cb_args, cb_ret)) = parse_cb(t) {
                // A callback: build a libffi closure over the Ares function value
                // and pass its code pointer as the C function pointer.
                let fnptr = cb_fnptr_type(&cb_args, &cb_ret);
                sig_types.push(fnptr.clone());
                let kinds = if cb_args.is_empty() {
                    "0".to_string()
                } else {
                    cb_args.iter().map(|a| cb_kind(a).to_string()).collect::<Vec<_>>().join(", ")
                };
                setup.push_str(&format!("  int _k{i}[] = {{{kinds}}};\n"));
                setup.push_str(&format!(
                    "  void* _cc{i}; void* _cb{i} = arx_cb_make(args[{i}], {}, _k{i}, {}, &_cc{i});\n",
                    cb_args.len(),
                    cb_kind(&cb_ret)
                ));
                call_args.push(format!("({fnptr})_cc{i}"));
                cb_frees.push(i);
            } else if let Some(layout) = layout_of(t) {
                let cty = cstruct_name(t);
                sig_types.push(cty.clone());
                setup.push_str(&format!("  {cty} a{i};\n"));
                emit_pack(&format!("a{i}"), &format!("args[{i}]"), layout, &mut setup);
                call_args.push(format!("a{i}"));
            } else {
                let (ty, expr) = c_param(t, i);
                sig_types.push(ty.to_string());
                setup.push_str(&format!("  {ty} a{i} = {expr};\n"));
                call_args.push(format!("a{i}"));
            }
        }
        let frees: String = cb_frees
            .iter()
            .map(|i| format!("  arx_cb_free(_cb{i});\n"))
            .chain(arr_frees.iter().map(|i| format!("  free(_arr{i});\n")))
            .collect();
        let sig = if sig_types.is_empty() {
            "void".to_string()
        } else {
            sig_types.join(", ")
        };
        out.push_str(&format!(
            "extern {ret_ty} ARESx_sym_{n}({sig}) __asm__(ARX_ASM_NAME({}));\n",
            cstr(e.symbol.bytes())
        ));
        out.push_str(&format!("static Value* ARESx_extern_{n}(Value** args) {{\n  (void)args;\n"));
        // The wrapper is N-ary: `args[i]` is the i-th positional C argument (the
        // grouping record was flattened at the call site during lowering). A
        // nullary C function still receives one unit `args[0]`, dropped below.
        out.push_str(&setup);
        let call = format!("ARESx_sym_{n}({})", call_args.join(", "));
        match ret_layout {
            Some(layout) => {
                out.push_str(&format!("  {ret_ty} _r = {call};\n"));
                out.push_str(&frees);
                let mut ctr = 0usize;
                let tmp = emit_build("_r", e.ret_type.as_str(), layout, &mut out, &mut ctr);
                out.push_str(&format!("  return {tmp};\n"));
            }
            None => match c_ret(e.ret_type.as_str()).1 {
                None => out.push_str(&format!("  {call};\n{frees}  return ARESxRT_unit();\n")),
                Some(w) => {
                    out.push_str(&format!("  {ret_ty} _r = {call};\n{frees}  return {w};\n"))
                }
            },
        }
        out.push_str("}\n");
    }
    let n = externs.len();
    if n == 0 {
        out.push_str("ExternFn ARESxRT_extern_table[1] = {0};\n");
    } else {
        out.push_str("ExternFn ARESxRT_extern_table[] = {\n");
        for i in 0..n {
            out.push_str(&format!("  ARESx_extern_{i},\n"));
        }
        out.push_str("};\n");
    }
    out.push_str(&format!("const size_t ARESxRT_extern_count = {n};\n"));
    out
}

impl<'p> Emitter<'p> {
    pub fn new(prog: &'p IrProgram, reachable: HashSet<usize>) -> Emitter<'p> {
        let mut globals: HashMap<&str, usize> = HashMap::new();
        let mut bare: HashMap<&str, usize> = HashMap::new();
        for (name, code) in &prog.globals {
            let name = name.as_str();
            globals.entry(name).or_insert(*code);
            bare.entry(name.rsplit('.').next().unwrap_or(name)).or_insert(*code);
        }
        let mut ops_by_effect: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut ops_by_name: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &prog.effects {
            ops_by_effect
                .entry(e.effect.as_str())
                .or_default()
                .push(e.op.as_str());
            ops_by_name
                .entry(e.op.as_str())
                .or_default()
                .push(e.effect.as_str());
        }
        for effects in ops_by_name.values_mut() {
            effects.sort_unstable();
            effects.dedup();
        }
        let mut externs = Vec::new();
        let mut extern_idx = HashMap::new();
        for (id, code) in prog.codes.iter().enumerate() {
            if reachable.contains(&id) {
                collect_externs(&code.body, &mut externs, &mut extern_idx);
            }
        }
        let mut known = HashMap::new();
        for (_, caf) in &prog.globals {
            if reachable.contains(caf) && !known.contains_key(caf) {
                if let Some(f) = known_fn(prog, *caf) {
                    known.insert(*caf, f);
                }
            }
        }
        let mut em = Emitter {
            prog,
            globals,
            bare,
            ops_by_effect,
            ops_by_name,
            externs,
            extern_idx,
            blocks: Vec::new(),
            code_entry: vec![0; prog.codes.len()],
            reachable,
            tmp: 0,
            uses_compiler: std::cell::Cell::new(false),
            uses_threads: std::cell::Cell::new(false),
            known,
            direct_fns: HashSet::new(),
            sigs: HashMap::new(),
            direct: None,
            env_ints: HashMap::new(),
            looped: false,
            scalars: HashMap::new(),
        };
        em.direct_fns = em.direct_safe();
        em.sigs = em.signatures();
        em
    }

    /// The known functions whose bodies never suspend, assuming the same of each
    /// other: start from all of them and drop any that fails, until none does.
    /// See Note [Direct functions].
    fn direct_safe(&self) -> HashSet<usize> {
        let mut safe: HashSet<usize> = self.known.keys().copied().collect();
        loop {
            let failed: Vec<usize> = safe
                .iter()
                .copied()
                .filter(|caf| !self.never_suspends(&self.prog.codes[self.known[caf].inner()].body, &safe))
                .collect();
            if failed.is_empty() {
                return safe;
            }
            for caf in failed {
                safe.remove(&caf);
            }
        }
    }

    /// Whether `e` runs without suspending, given the direct functions `safe`.
    /// Mirrors how a direct function's body is emitted.
    fn never_suspends(&self, e: &IrExpr, safe: &HashSet<usize>) -> bool {
        let callee_ok = |c: Callee| match c {
            Callee::Prim | Callee::Extern(_) => true,
            Callee::Known(caf) => safe.contains(&caf),
        };
        match e {
            IrExpr::Let { slot, rhs, body } => match self.call_chain(*slot, rhs, body) {
                Some(call) => {
                    callee_ok(call.callee)
                        && call.rest.is_none_or(|(_, rest)| self.never_suspends(rest, safe))
                }
                None => self.never_suspends(rhs, safe) && self.never_suspends(body, safe),
            },
            IrExpr::App { fun, arg, .. } => match self.lone_call(fun, arg) {
                Some(call) => callee_ok(call.callee),
                None => self.partial_callee(fun),
            },
            IrExpr::Case { alts, default, .. } => {
                alts.iter().all(|a| self.never_suspends(&a.body, safe)) && self.never_suspends(default, safe)
            }
            IrExpr::Drop { body, .. } => self.never_suspends(body, safe),
            IrExpr::Handle { .. } | IrExpr::Defer { .. } => false,
            IrExpr::Ret(_)
            | IrExpr::MkStruct { .. }
            | IrExpr::Field { .. }
            | IrExpr::MkVariant { .. }
            | IrExpr::MkTuple(_)
            | IrExpr::Fault(_) => true,
        }
    }

    /// The signature of every direct function. An argument is unboxed when the
    /// body uses it as an integer: an integer intrinsic's operand, a literal
    /// case's scrutinee, or an unboxed argument of a call (a least fixpoint, so
    /// every such claim rests on a real use; the types guarantee an `@Int` is an
    /// evaluated integer). The result is unboxed when every value returned is an
    /// integer (a greatest fixpoint: a function returning only through calls of
    /// itself returns nothing at all).
    fn signatures(&self) -> HashMap<usize, Sig> {
        let mut sigs: HashMap<usize, Sig> = self
            .direct_fns
            .iter()
            .map(|caf| (*caf, Sig { int_params: vec![false; self.known[caf].arity()], int_ret: true }))
            .collect();
        // The arguments first: the result's analysis reads them, theirs never
        // reads the result.
        for results in [false, true] {
            loop {
                let mut next = sigs.clone();
                for caf in &self.direct_fns {
                    let f = &self.known[caf];
                    let sig = &sigs[caf];
                    let mut st = IntState {
                        p0: true,
                        ints: if sig.int_params[f.arity() - 1] { [0].into() } else { HashSet::new() },
                        evidence: vec![false; f.arity()],
                    };
                    let ret = self.int_walk(&self.prog.codes[f.inner()].body, f, &sigs, sig, &mut st);
                    let entry = next.get_mut(caf).expect("every direct function has a signature");
                    if results {
                        entry.int_ret = entry.int_ret && ret;
                    } else {
                        for (p, e) in entry.int_params.iter_mut().zip(&st.evidence) {
                            *p = *p || *e;
                        }
                    }
                }
                if next == sigs {
                    break;
                }
                sigs = next;
            }
        }
        sigs
    }

    /// Walk a direct function's body for `signatures`: collect the integer uses
    /// of its arguments into `st`, and return whether every value it returns is
    /// an integer.
    fn int_walk(&self, e: &IrExpr, f: &KnownFn, sigs: &HashMap<usize, Sig>, sig: &Sig, st: &mut IntState) -> bool {
        let param_of = |a: &Atom, st: &IntState| match a {
            Atom::Env(j) | Atom::MoveEnv(j) => Some(f.env[*j]),
            Atom::Local(0) | Atom::Move(0) if st.p0 => Some(f.arity() - 1),
            _ => None,
        };
        let is_int = |a: &Atom, st: &IntState| match a {
            Atom::LitI(_) => true,
            Atom::Env(j) | Atom::MoveEnv(j) => sig.int_params[f.env[*j]],
            Atom::Local(s) | Atom::Move(s) => st.ints.contains(s),
            _ => false,
        };
        // What a call yields, after noting the integer uses of its arguments.
        let call = |c: &Call, st: &mut IntState| -> bool {
            match c.callee {
                Callee::Prim => match intrinsic_operands(c.name) {
                    Some(ops) if ops.len() == c.args.len() => {
                        for (a, op) in c.args.iter().zip(ops) {
                            if let Some(p) = param_of(a, st).filter(|_| *op == Opnd::Int) {
                                st.evidence[p] = true;
                            }
                        }
                        let blanks = vec![String::new(); ops.len()];
                        !int_intrinsic(c.name, &blanks).is_some_and(|(_, boolean)| boolean)
                    }
                    _ => false,
                },
                Callee::Extern(_) => false,
                Callee::Known(caf) => match sigs.get(&caf) {
                    Some(callee) => {
                        for (a, int) in c.args.iter().zip(&callee.int_params) {
                            if let Some(p) = param_of(a, st).filter(|_| *int) {
                                st.evidence[p] = true;
                            }
                        }
                        callee.int_ret
                    }
                    None => false,
                },
            }
        };
        let rebind = |s: usize, int: bool, st: &mut IntState| {
            if s == 0 {
                st.p0 = false;
            }
            if int {
                st.ints.insert(s);
            } else {
                st.ints.remove(&s);
            }
        };
        match e {
            IrExpr::Let { slot, rhs, body } => match self.call_chain(*slot, rhs, body) {
                Some(c) => {
                    let int = call(&c, st);
                    match c.rest {
                        Some((r, rest)) => {
                            rebind(*slot, false, st);
                            rebind(r, int, st);
                            self.int_walk(rest, f, sigs, sig, st)
                        }
                        None => int,
                    }
                }
                None => {
                    let int = self.int_walk(rhs, f, sigs, sig, st);
                    rebind(*slot, int, st);
                    self.int_walk(body, f, sigs, sig, st)
                }
            },
            IrExpr::App { fun, arg, .. } => match self.lone_call(fun, arg) {
                Some(c) => call(&c, st),
                None => false,
            },
            IrExpr::Case { scrut, alts, default } => {
                if alts.iter().any(|a| matches!(a.kind, AltKind::Int(_))) {
                    if let Some(p) = param_of(scrut, st) {
                        st.evidence[p] = true;
                    }
                }
                let mut all = true;
                for a in alts {
                    let mut branch = IntState { p0: st.p0, ints: st.ints.clone(), evidence: Vec::new() };
                    std::mem::swap(&mut branch.evidence, &mut st.evidence);
                    if let AltKind::Con(_) = a.kind {
                        for i in 0..a.binders.len() {
                            rebind(a.binder_base + i, false, &mut branch);
                        }
                    }
                    all &= self.int_walk(&a.body, f, sigs, sig, &mut branch);
                    std::mem::swap(&mut branch.evidence, &mut st.evidence);
                }
                let mut branch = IntState { p0: st.p0, ints: st.ints.clone(), evidence: Vec::new() };
                std::mem::swap(&mut branch.evidence, &mut st.evidence);
                all &= self.int_walk(default, f, sigs, sig, &mut branch);
                std::mem::swap(&mut branch.evidence, &mut st.evidence);
                all
            }
            IrExpr::Drop { body, .. } => self.int_walk(body, f, sigs, sig, st),
            IrExpr::Ret(a) => is_int(a, st),
            IrExpr::Fault(_) => true,
            IrExpr::MkStruct { .. }
            | IrExpr::Field { .. }
            | IrExpr::MkVariant { .. }
            | IrExpr::MkTuple(_)
            | IrExpr::Handle { .. }
            | IrExpr::Defer { .. } => false,
        }
    }

    /// Whether applying `fun` to one argument only builds a value: a built-in or
    /// a known function short of its arity.
    fn partial_callee(&self, fun: &Atom) -> bool {
        let Atom::Glob { name } = fun else {
            return false;
        };
        let name = name.as_str();
        match self.globals.get(name) {
            Some(caf) => self.known.get(caf).is_some_and(|f| f.arity() > 1),
            None => native_builtin(name).is_some_and(|b| b.arity > 1),
        }
    }

    /// Emit every code, then return the finished block function texts, the
    /// declarations and definitions of the direct functions, the code index ->
    /// entry block map, the foreign-function table, and whether the
    /// program uses a [`BuiltinHost::Compiler`] builtin and a [`BuiltinHost::Parallel`] one.
    pub fn run(mut self) -> (Vec<String>, (String, String), Vec<usize>, Vec<ExternSite>, bool, bool) {
        for id in 0..self.prog.codes.len() {
            self.emit_code(id);
        }
        let mut cafs: Vec<usize> = self.direct_fns.iter().copied().collect();
        cafs.sort_unstable();
        let mut decls = String::new();
        let mut defs = String::new();
        for caf in cafs {
            let (f, sig) = (&self.known[&caf], &self.sigs[&caf]);
            let ctype = |int: bool| if int { "int64_t" } else { "Value*" };
            let params: Vec<&str> = sig.int_params.iter().map(|&i| ctype(i)).collect();
            decls.push_str(&format!("static {} d_{}({});\n", ctype(sig.int_ret), f.inner(), params.join(", ")));
            defs.push_str(&self.emit_direct_fn(caf));
        }
        (
            self.blocks,
            (decls, defs),
            self.code_entry,
            self.externs,
            self.uses_compiler.get(),
            self.uses_threads.get(),
        )
    }

    fn fresh(&mut self, p: &str) -> String {
        let s = format!("{p}{}", self.tmp);
        self.tmp += 1;
        s
    }

    fn reserve_block(&mut self) -> usize {
        let id = self.blocks.len();
        self.blocks.push(String::new());
        id
    }

    fn set_block(&mut self, id: usize, body: &str) {
        self.blocks[id] = format!(
            "static void blk_{id}(Frame* fr, Value* in) {{\n  (void)fr; (void)in;\n{body}}}\n\n"
        );
    }

    /// Resolve a `Glob` atom's single canonical name to its C expression,
    /// mirroring the machine's `glob` order: an EXACT canonical global first, then
    /// a built-in, a `TARGET.` member, an effect operation, and only LAST the
    /// unqualified fallback. Ops precede that fallback so an unrelated imported
    /// global whose last segment aliases into `bare` cannot shadow a same-named
    /// operation. The `C` libc namespace resolves as exact `@extern` globals.
    fn glob_atom(&self, name: &str) -> String {
        if let Some(code) = self.globals.get(name) {
            return format!("force_code({code})");
        }
        if let Some(b) = native_builtin(name) {
            match b.host {
                BuiltinHost::Compiler => self.uses_compiler.set(true),
                BuiltinHost::Parallel => self.uses_threads.set(true),
                BuiltinHost::Everywhere | BuiltinHost::CompileTime => {}
            }
            return format!("ARESxRT_builtin({}, {})", builtin_ident(name), b.arity);
        }
        if let Some(suffix) = name.strip_prefix("TARGET.") {
            return format!("ARESxRT_target({})", cstr(suffix.as_bytes()));
        }
        // An effect operation: `Effect.op` picks that effect, a bare op the sole
        // effect declaring it (else ambient / NULL, resolved by the handler).
        if let Some((eff, op)) = name.split_once('.') {
            if self.ops_by_effect.get(eff).is_some_and(|o| o.contains(&op)) {
                return format!("ARESxK_op({}, {})", cstr(eff.as_bytes()), cstr(op.as_bytes()));
            }
        }
        if let Some(effs) = self.ops_by_name.get(name) {
            let eff = if effs.len() == 1 {
                cstr(effs[0].as_bytes())
            } else {
                "NULL".to_string()
            };
            return format!("ARESxK_op({eff}, {})", cstr(name.as_bytes()));
        }
        // The unqualified fallback: a reference the codegen left bare (the entry
        // point, or a name not resolved to a module), matched by last segment.
        if let Some(code) = self.bare.get(name) {
            return format!("force_code({code})");
        }
        // An unknown bare name: an ambient effect operation, resolved by a handler.
        format!("ARESxK_op(NULL, {})", cstr(name.as_bytes()))
    }

    /// Each atom is a single, side-effect-light C expression.
    fn atom(&self, a: &Atom) -> String {
        match a {
            Atom::Local(i) | Atom::Move(i) if self.scalars.contains_key(i) => {
                let (v, boolean) = &self.scalars[i];
                format!("{}({v})", if *boolean { "ARESxRT_bool" } else { "ARESxRT_int" })
            }
            Atom::Local(i) => format!("ARESxVALUE_local(fr->locals, fr->nlocals, {i})"),
            Atom::Move(i) => format!("ARESxVALUE_take(fr, {i})"),
            Atom::Env(i) | Atom::MoveEnv(i) if self.env_ints.contains_key(i) => {
                format!("ARESxRT_int({})", self.env_ints[i])
            }
            Atom::Env(i) => format!("ARESxVALUE_env(fr->env, fr->nenv, {i})"),
            Atom::MoveEnv(i) => format!("ARESxVALUE_take_env(fr, {i})"),
            Atom::Glob { name } => self.glob_atom(name.as_str()),
            Atom::LitI(n) => format!("ARESxRT_int({n}LL)"),
            Atom::LitR(r) => format!("ARESxRT_real({})", cdbl(*r)),
            Atom::LitS(v) => format!("ARESxRT_str({}, {})", cstr(v), v.len()),
            Atom::LitB(b) => format!("ARESxRT_bool({})", if *b { 1 } else { 0 }),
            Atom::Unit => "ARESxRT_unit()".into(),
            Atom::Clos { code, captures } => {
                if captures.is_empty() {
                    format!("ARESxRT_closure({code}, NULL, 0)")
                } else {
                    let caps: Vec<String> = captures.iter().map(|c| self.atom(c)).collect();
                    format!(
                        "ARESxRT_closure({code}, (Value*[]){{ {} }}, {})",
                        caps.join(", "),
                        captures.len()
                    )
                }
            }
            Atom::Extern {
                abi,
                symbol,
                lib,
                arg_types,
                ret_type,
            } => {
                let key = extern_key(abi, symbol, lib, arg_types, ret_type);
                let idx = self.extern_idx[&key];
                // The extern value is N-ary: one positional C argument per
                // `arg_types` entry, or one unit argument for a nullary C function.
                let arity = arg_types.len().max(1);
                format!("ARESxRT_extern({idx}, {arity})")
            }
        }
    }

    /// A single C expression for a value-producing, non-suspending, non-branching
    /// expression. Case/Let/App/Handle/Defer are structural and never reach here.
    fn value_expr(&self, e: &IrExpr) -> String {
        match e {
            IrExpr::Ret(a) => self.atom(a),
            IrExpr::MkStruct { name, base, fields } => {
                let n = fields.len();
                let (fnames, vals) = if n == 0 {
                    ("NULL".to_string(), "NULL".to_string())
                } else {
                    let ns: Vec<String> = fields.iter().map(|(f, _)| cstr(f.bytes())).collect();
                    let vs: Vec<String> = fields.iter().map(|(_, v)| self.atom(v)).collect();
                    (
                        format!("(const char*[]){{ {} }}", ns.join(", ")),
                        format!("(Value*[]){{ {} }}", vs.join(", ")),
                    )
                };
                match base {
                    Some(b) => format!(
                        "ARESxRT_struct_update({}, {}, {n}, {fnames}, {vals})",
                        self.atom(b),
                        cstr(name.bytes())
                    ),
                    None => {
                        format!("ARESxRT_struct({}, {n}, {fnames}, {vals})", cstr(name.bytes()))
                    }
                }
            }
            // A GNU statement expression gives each access site its own cache.
            IrExpr::Field { rec, name } => format!(
                "({{ static unsigned _fic; ARESxVALUE_field_ic({}, {}, &_fic); }})",
                self.atom(rec),
                cstr(name.bytes())
            ),
            IrExpr::MkVariant { ty, tag, fields, reuse } => {
                let vals = if fields.is_empty() {
                    "NULL".to_string()
                } else {
                    let vs: Vec<String> = fields.iter().map(|f| self.atom(f)).collect();
                    format!("(Value*[]){{ {} }}", vs.join(", "))
                };
                let (ty, tag, n) = (cstr(ty.bytes()), cstr(tag.bytes()), fields.len());
                match reuse {
                    Some(t) => format!("ARESxRT_variant_at(fr, {t}, {ty}, {tag}, {n}, {vals})"),
                    None => format!("ARESxRT_variant({ty}, {tag}, {n}, {vals})"),
                }
            }
            IrExpr::MkTuple(items) => {
                if items.is_empty() {
                    "ARESxRT_tuple(NULL, 0)".into()
                } else {
                    let vs: Vec<String> = items.iter().map(|a| self.atom(a)).collect();
                    format!("ARESxRT_tuple((Value*[]){{ {} }}, {})", vs.join(", "), items.len())
                }
            }
            IrExpr::Let { .. }
            | IrExpr::App { .. }
            | IrExpr::Case { .. }
            | IrExpr::Handle { .. }
            | IrExpr::Defer { .. }
            | IrExpr::Drop { .. } => unreachable!("value_expr on a non-value expression"),
            IrExpr::Fault(_) => unreachable!("Fault is emitted structurally, not as a value"),
        }
    }

    /// Does `e` perform a call or install a handler/defer anywhere in the value it
    /// produces? Then a `let` RHS must deliver through a continuation block. Counts
    /// tail Apps too: a `let` RHS is never really in tail position.
    fn has_call(&self, e: &IrExpr) -> bool {
        match e {
            IrExpr::Ret(_)
            | IrExpr::MkStruct { .. }
            | IrExpr::Field { .. }
            | IrExpr::MkVariant { .. }
            | IrExpr::MkTuple(_)
            | IrExpr::Fault(_) => false,
            IrExpr::App { .. } | IrExpr::Handle { .. } | IrExpr::Defer { .. } => true,
            IrExpr::Let { rhs, body, .. } => self.has_call(rhs) || self.has_call(body),
            IrExpr::Drop { body, .. } => self.has_call(body),
            IrExpr::Case { alts, default, .. } => {
                alts.iter().any(|a| self.has_call(&a.body)) || self.has_call(default)
            }
        }
    }

    /// Deliver a finished value C-expression to its sink.
    fn deliver(&self, val: &str, sink: Sink, out: &mut String) {
        match sink {
            Sink::Ret if self.direct.is_some_and(|d| d.int_ret) => {
                self.return_int(&format!("ARESxI({val})"), out)
            }
            Sink::Ret if self.direct.is_some() => {
                let n = self.direct.map_or(0, |d| d.arity);
                out.push_str(&format!("  return ARESxD_leave(fr, P, {n}, mark, {val});\n"))
            }
            Sink::Ret => out.push_str(&format!("  ARESxK_ret({val});\n")),
            Sink::Slot { slot, cont } => {
                out.push_str(&format!("  ARESxK_backpatch(fr, {slot}, {val});\n"));
                if let Some(cont) = cont {
                    out.push_str(&format!("  ARESxK_jump(blk_{cont});\n"));
                }
            }
            Sink::Int { var } => out.push_str(&format!("  sv{var} = ARESxI({val});\n")),
        }
    }

    /// Return the integer `val` from a direct function with an unboxed result.
    fn return_int(&self, val: &str, out: &mut String) {
        let n = self.direct.map_or(0, |d| d.arity);
        out.push_str(&format!("  return ARESxD_leave_i(fr, P, {n}, mark, {val});\n"));
    }

    /// Compute a pure (suspension-free) expression's value into frame `slot`,
    /// emitted inline. Never reaches App/Handle/Defer.
    fn emit_pure_into(&mut self, e: &IrExpr, slot: usize, out: &mut String) {
        match e {
            // Slot to slot, not through the temp pool, so the value stays
            // uniquely held for a reuse later in the same block.
            IrExpr::Ret(Atom::Move(from)) => {
                out.push_str(&format!("  ARESxK_move(fr, {slot}, {from});\n"));
            }
            IrExpr::Ret(_)
            | IrExpr::MkStruct { .. }
            | IrExpr::Field { .. }
            | IrExpr::MkVariant { .. }
            | IrExpr::MkTuple(_) => {
                let val = self.value_expr(e);
                out.push_str(&format!("  ARESxK_backpatch(fr, {slot}, {val});\n"));
            }
            IrExpr::Fault(msg) => {
                out.push_str(&format!("  ares_fault({});\n", cstr(msg.as_bytes())))
            }
            IrExpr::Case { .. } => self.emit_case(e, Sink::Slot { slot, cont: None }, out),
            IrExpr::Let {
                slot: inner,
                rhs,
                body,
            } => {
                self.scalars.remove(inner);
                emit_let_slot(rhs, *inner, out);
                self.emit_pure_into(rhs, *inner, out);
                self.emit_pure_into(body, slot, out);
            }
            IrExpr::Drop { slots, body } => {
                self.emit_drops(slots, out);
                self.emit_pure_into(body, slot, out);
            }
            IrExpr::App { .. } | IrExpr::Handle { .. } | IrExpr::Defer { .. } => {
                unreachable!("emit_pure_into on a suspending expression")
            }
        }
    }

    /// The core continuation-passing lowering: lower `e` so its value reaches
    /// `sink`.
    fn emit_expr(&mut self, e: &IrExpr, sink: Sink, out: &mut String) {
        match e {
            IrExpr::Ret(a) if matches!(sink, Sink::Ret) && self.direct.is_some_and(|d| d.int_ret) => {
                self.return_int(&self.int_atom(a), out)
            }
            IrExpr::Ret(a) if matches!(sink, Sink::Int { .. }) => {
                let Sink::Int { var } = sink else { unreachable!() };
                out.push_str(&format!("  sv{var} = {};\n", self.int_atom(a)));
            }
            IrExpr::Ret(_)
            | IrExpr::MkStruct { .. }
            | IrExpr::Field { .. }
            | IrExpr::MkVariant { .. }
            | IrExpr::MkTuple(_) => {
                let val = self.value_expr(e);
                self.deliver(&val, sink, out);
            }
            IrExpr::Fault(msg) => {
                out.push_str(&format!("  ares_fault({});\n", cstr(msg.as_bytes())))
            }

            IrExpr::App { fun, arg, .. } if self.direct.is_some() => match self.lone_call(fun, arg) {
                Some(call) if matches!(call.callee, Callee::Prim | Callee::Extern(_)) => self.emit_prim(call, sink, out),
                Some(call) => self.emit_call(call, sink, out),
                None => {
                    let r = self.fresh("part");
                    out.push_str(&format!(
                        "  Value* {r} = ARESxK_own(ARESxK_call({}, {}));\n",
                        self.atom(fun),
                        self.atom(arg)
                    ));
                    self.deliver(&r, sink, out);
                }
            },
            IrExpr::App { fun, arg: arg_atom, lend, .. } => {
                if let Some(call) = self.lone_call(fun, arg_atom) {
                    if matches!(call.callee, Callee::Known(caf) if self.direct_fns.contains(&caf)) {
                        self.emit_call(call, sink, out);
                        return;
                    }
                }
                let fn_ = self.atom(fun);
                let arg = self.atom(arg_atom);
                // Tail-ness follows the sink, not the ANF flag: only a RET sink is a
                // genuine tail position. A call the ANF marked tail but which IR
                // lowering hoisted into a `let` is delivered into a slot: a normal,
                // non-tail call.
                //
                // A lent argument must outlive the callee. A tail call ends this
                // activation, so it lends only a global or, through
                // `ARESxK_tailcall_pass`, a slot this activation was itself lent.
                // See Note [Borrowed parameters].
                match sink {
                    Sink::Ret => match arg_atom {
                        Atom::Local(s) if *lend => {
                            out.push_str(&format!("  ARESxK_tailcall_pass(fr, {fn_}, {s});\n"))
                        }
                        Atom::Glob { .. } if *lend && arg.starts_with("force_code(") => {
                            out.push_str(&format!("  ARESxK_tailcall_lent({fn_}, {arg});\n"))
                        }
                        _ => out.push_str(&format!("  ARESxK_tailcall({fn_}, {arg});\n")),
                    },
                    Sink::Int { .. } => unreachable!("an integer join holds only direct calls (`int_tail`)"),
                    Sink::Slot { slot, cont } => {
                        let cont = cont.expect("a non-tail App needs a continuation block");
                        let lends = *lend
                            && match arg_atom {
                                Atom::Local(_) | Atom::Env(_) => true,
                                Atom::Glob { .. } => arg.starts_with("force_code("),
                                _ => false,
                            };
                        let apply = if lends { "ARESxK_apply_lent" } else { "ARESxK_apply" };
                        out.push_str(&format!(
                            "  {apply}(fr, {fn_}, {arg}, blk_{cont}, {slot});\n"
                        ));
                    }
                }
            }

            IrExpr::Let { .. } => self.emit_let(e, sink, out),
            IrExpr::Case { .. } => self.emit_case(e, sink, out),
            IrExpr::Handle { .. } => self.emit_handle(e, sink, out),
            IrExpr::Defer { .. } => self.emit_defer(e, sink, out),
            IrExpr::Drop { slots, body } => {
                self.emit_drops(slots, out);
                self.emit_expr(body, sink, out);
            }
        }
    }

    fn emit_let(&mut self, e: &IrExpr, sink: Sink, out: &mut String) {
        let IrExpr::Let { slot, rhs, body } = e else {
            unreachable!()
        };
        self.scalars.remove(slot);
        if let Some(call) = self.call_chain(*slot, rhs, body) {
            match call.callee {
                Callee::Prim | Callee::Extern(_) => return self.emit_prim(call, sink, out),
                Callee::Known(caf) if self.direct_fns.contains(&caf) => {
                    return self.emit_call(call, sink, out);
                }
                Callee::Known(_) => {}
            }
        }
        if self.direct.is_some()
            && matches!(**rhs, IrExpr::Case { .. })
            && !rhs.mentions(*slot)
            && self.int_tail(rhs)
            && self.scalar_uses(body, *slot)
        {
            let var = self.tmp;
            self.tmp += 1;
            out.push_str(&format!("  int64_t sv{var};\n"));
            self.emit_expr(rhs, Sink::Int { var }, out);
            self.scalars.insert(*slot, (format!("sv{var}"), false));
            self.emit_expr(body, sink, out);
            self.scalars.remove(slot);
            return;
        }
        emit_let_slot(rhs, *slot, out);
        if self.direct.is_some() {
            self.emit_expr(rhs, Sink::Slot { slot: *slot, cont: None }, out);
            self.emit_expr(body, sink, out);
        } else if self.has_call(rhs) {
            // The body becomes a continuation block; the rhs delivers into the slot
            // then control transfers there.
            let body_blk = self.reserve_block();
            self.emit_expr(
                rhs,
                Sink::Slot {
                    slot: *slot,
                    cont: Some(body_blk),
                },
                out,
            );
            let mut body_out = String::new();
            self.emit_detached(body, sink, &mut body_out);
            self.set_block(body_blk, &body_out);
        } else {
            // Pure rhs: compute into the slot inline, then continue in the same
            // block.
            self.emit_pure_into(rhs, *slot, out);
            self.emit_expr(body, sink, out);
        }
    }

    /// What a global named `name` calls, and its arity: a known function or a
    /// built-in.
    fn callee(&self, name: &str) -> Option<(Callee, usize)> {
        match self.globals.get(name) {
            Some(caf) => match self.known.get(caf) {
                Some(f) => Some((Callee::Known(*caf), f.arity())),
                None => match &self.prog.codes[*caf].body {
                    IrExpr::Ret(Atom::Extern { abi, symbol, lib, arg_types, ret_type })
                        if self.prog.codes[*caf].nparams == 0 =>
                    {
                        let idx = self.extern_idx[&extern_key(abi, symbol, lib, arg_types, ret_type)];
                        Some((Callee::Extern(idx), arg_types.len().max(1)))
                    }
                    _ => None,
                },
            },
            None => native_builtin(name).map(|b| (Callee::Prim, b.arity)),
        }
    }

    /// A one-argument built-in or known function applied by `fun arg`.
    fn lone_call<'e>(&self, fun: &'e Atom, arg: &'e Atom) -> Option<Call<'e>> {
        let Atom::Glob { name } = fun else {
            return None;
        };
        let name = name.as_str();
        match self.callee(name)? {
            (callee, 1) => Some(Call { name, callee, args: vec![arg], drops: Vec::new(), rest: None }),
            _ => None,
        }
    }

    /// A built-in or known function applied to all its operands, which ANF
    /// spells as a chain of lets each applying the previous partial application
    /// to one more operand: `let a = b x in let c = a y in rest`, or with the
    /// last application in tail position. Each partial must be used once, by the
    /// next application.
    fn call_chain<'e>(&self, slot: usize, rhs: &'e IrExpr, body: &'e IrExpr) -> Option<Call<'e>> {
        let IrExpr::App {
            fun: Atom::Glob { name },
            arg,
            ..
        } = rhs
        else {
            return None;
        };
        let name = name.as_str();
        let (callee, arity) = self.callee(name)?;
        let mut args = vec![arg];
        let mut drops = Vec::new();
        let (mut partial, mut cur) = (slot, body);
        loop {
            if args.len() == arity {
                return Some(Call {
                    name,
                    callee,
                    args,
                    drops,
                    rest: Some((partial, cur)),
                });
            }
            match cur {
                IrExpr::Drop { slots, body } => {
                    drops.extend(slots.iter().copied());
                    cur = body;
                }
                IrExpr::App {
                    fun: Atom::Move(s),
                    arg,
                    ..
                } if *s == partial && args.len() + 1 == arity => {
                    args.push(arg);
                    return Some(Call {
                        name,
                        callee,
                        args,
                        drops,
                        rest: None,
                    });
                }
                IrExpr::Let {
                    slot: next,
                    rhs,
                    body,
                } => match rhs.as_ref() {
                    IrExpr::App {
                        fun: Atom::Move(s),
                        arg,
                        ..
                    } if *s == partial => {
                        args.push(arg);
                        partial = *next;
                        cur = body;
                    }
                    _ => return None,
                },
                _ => return None,
            }
        }
    }

    /// Call a saturated built-in directly. Its result goes to `rest`'s slot (then
    /// `rest` runs) or, with no `rest`, to `sink`. A drop between the
    /// applications waits until the operands have been read.
    fn emit_prim(&mut self, call: Call, sink: Sink, out: &mut String) {
        if let Callee::Extern(idx) = call.callee {
            let args: Vec<String> = call.args.iter().map(|a| self.atom(a)).collect();
            let val = format!("ARESxRT_extern_table[{idx}]((Value*[]){{ {} }})", args.join(", "));
            return self.emit_prim_value(&call, val, sink, out);
        }
        self.glob_atom(call.name);
        if intrinsic_operands(call.name).is_some_and(|ops| ops.len() == call.args.len()) {
            let ops = intrinsic_operands(call.name).unwrap_or(&[]);
            let args: Vec<String> = (call.args.iter().zip(ops))
                .map(|(a, op)| match op {
                    Opnd::Int => self.int_atom(a),
                    Opnd::Bool => self.bool_atom(a),
                    Opnd::Boxed => self.atom(a),
                })
                .collect();
            if let Some((expr, boolean)) = int_intrinsic(call.name, &args) {
                return self.emit_int_prim(&call, expr, boolean, sink, out);
            }
        }
        let mut args: Vec<String> = call.args.iter().map(|a| self.atom(a)).collect();
        let val = if matches!(call.name, "@vec_push" | "@vec_set") {
            // A moved vector may be updated in place when nothing else holds it.
            // `ARESxVALUE_take_env` moves only when its closure is unshared, so
            // whether it did is read back from the emptied field, in a statement
            // of its own since operand evaluation order is unspecified.
            let owned = match call.args[0] {
                Atom::Move(_) => "1".to_string(),
                Atom::MoveEnv(i) => {
                    let v = self.fresh("vec");
                    out.push_str(&format!("  Value* {v} = {};\n", args[0]));
                    out.push_str(&format!("  int {v}_own = fr->env[{i}] == NULL;\n"));
                    args[0] = v.clone();
                    format!("{v}_own")
                }
                _ => "0".to_string(),
            };
            format!(
                "ARESxK_vec_update({}, (Value*[]){{ {} }}, {owned})",
                builtin_ident(call.name),
                args.join(", ")
            )
        } else if call.name == "@array_push" && matches!(call.args[0], Atom::Move(_)) {
            // Only a local's last use may be pushed onto in place: a moved
            // capture is still a borrow whenever the closure has other owners.
            format!("ARESxK_array_push_owned({})", args.join(", "))
        } else {
            format!(
                "ARESxK_prim({}, (Value*[]){{ {} }})",
                builtin_ident(call.name),
                args.join(", ")
            )
        };
        self.emit_prim_value(&call, val, sink, out);
    }

    /// The boxed result `val` of a saturated built-in or foreign call, into
    /// `rest`'s slot (then `rest` runs) or, with no `rest`, to `sink`.
    fn emit_prim_value(&mut self, call: &Call, val: String, sink: Sink, out: &mut String) {
        match call.rest {
            Some((slot, rest)) => {
                self.scalars.remove(&slot);
                out.push_str(&format!("  ARESxK_drop(fr, {slot});\n"));
                out.push_str(&format!("  ARESxK_backpatch(fr, {slot}, {val});\n"));
                self.emit_drops(&call.drops, out);
                self.emit_expr(rest, sink, out);
            }
            None => {
                let r = self.fresh("prim");
                out.push_str(&format!("  Value* {r} = {val};\n"));
                self.emit_drops(&call.drops, out);
                self.deliver(&r, sink, out);
            }
        }
    }

    /// An integer intrinsic computed in C, its result kept in a scalar when
    /// `scalar_uses` allows, else boxed. See Note [Unboxed integers].
    fn emit_int_prim(&mut self, call: &Call, expr: String, boolean: bool, sink: Sink, out: &mut String) {
        let (ctype, boxer) = if boolean { ("int", "ARESxRT_bool") } else { ("int64_t", "ARESxRT_int") };
        match call.rest {
            Some((slot, rest)) if self.scalar_uses(rest, slot) => {
                let v = self.fresh("iv");
                out.push_str(&format!("  {ctype} {v} = {expr};\n"));
                self.emit_drops(&call.drops, out);
                self.scalars.insert(slot, (v, boolean));
                self.emit_expr(rest, sink, out);
                self.scalars.remove(&slot);
            }
            Some((slot, rest)) => {
                self.scalars.remove(&slot);
                out.push_str(&format!("  ARESxK_drop(fr, {slot});\n"));
                out.push_str(&format!("  ARESxK_backpatch(fr, {slot}, {boxer}({expr}));\n"));
                self.emit_drops(&call.drops, out);
                self.emit_expr(rest, sink, out);
            }
            None if matches!(sink, Sink::Int { .. }) && !boolean => {
                let Sink::Int { var } = sink else { unreachable!() };
                out.push_str(&format!("  sv{var} = {expr};\n"));
                self.emit_drops(&call.drops, out);
            }
            None if matches!(sink, Sink::Ret) && !boolean && self.direct.is_some_and(|d| d.int_ret) => {
                let r = self.fresh("iv");
                out.push_str(&format!("  int64_t {r} = {expr};\n"));
                self.emit_drops(&call.drops, out);
                self.return_int(&r, out);
            }
            None => {
                let r = self.fresh("prim");
                out.push_str(&format!("  Value* {r} = {boxer}({expr});\n"));
                self.emit_drops(&call.drops, out);
                self.deliver(&r, sink, out);
            }
        }
    }

    /// A saturated call of a direct function. See Note [Direct functions].
    fn emit_call(&mut self, call: Call, sink: Sink, out: &mut String) {
        let Callee::Known(caf) = call.callee else {
            unreachable!("emit_call on a built-in")
        };
        let f = self.known[&caf].clone();
        let sig = self.sigs[&caf].clone();
        let names: Vec<String> = (0..call.args.len()).map(|_| self.fresh("ka")).collect();
        out.push_str("  {\n");
        for (k, (n, a)) in names.iter().zip(&call.args).enumerate() {
            if sig.int_params[k] {
                out.push_str(&format!("  int64_t {n} = {};\n", self.int_atom(a)));
            } else {
                out.push_str(&format!("  Value* {n} = {};\n", self.give_atom(a)));
            }
        }
        // The machine fallback borrows its operands, so there the references
        // given above go to the pool, as a temporary's would.
        let boxed = |k: usize| {
            if sig.int_params[k] {
                format!("ARESxRT_int({})", names[k])
            } else {
                format!("ARESxK_own({})", names[k])
            }
        };
        let direct = format!("d_{}({})", f.inner(), names.join(", "));
        let env: Vec<String> = f.env.iter().map(|&i| boxed(i)).collect();
        let clos = if env.is_empty() {
            format!("ARESxRT_closure({}, NULL, 0)", f.inner())
        } else {
            format!("ARESxRT_closure({}, (Value*[]){{ {} }}, {})", f.inner(), env.join(", "), env.len())
        };
        let last = boxed(names.len() - 1);
        match self.direct {
            Some(d) if d.inner == f.inner() && call.rest.is_none() && matches!(sink, Sink::Ret) => {
                out.push_str(&format!("  ARESxD_reset(fr, P, {}, mark);\n", d.arity));
                for (k, n) in names.iter().enumerate() {
                    if sig.int_params[k] {
                        out.push_str(&format!("  I{k} = {n};\n"));
                    } else {
                        out.push_str(&format!("  P[{k}] = {n};\n"));
                    }
                }
                out.push_str("  goto top;\n  }\n");
                assert_eq!(d.arity, names.len());
                self.looped = true;
            }
            Some(d) => {
                let r = self.fresh("call");
                if sig.int_ret {
                    out.push_str(&format!(
                        "  int64_t {r} = ARESxD_room() ? {direct} : ARESxI_own(ARESxK_call({clos}, {last}));\n"
                    ));
                } else {
                    out.push_str(&format!(
                        "  Value* {r} = ARESxK_own(ARESxD_room() ? {direct} : ARESxK_call({clos}, {last}));\n"
                    ));
                }
                self.emit_drops(&call.drops, out);
                match call.rest {
                    Some((slot, rest)) => {
                        self.scalars.remove(&slot);
                        if sig.int_ret && self.scalar_uses(rest, slot) {
                            self.scalars.insert(slot, (r, false));
                            self.emit_expr(rest, sink, out);
                            self.scalars.remove(&slot);
                        } else {
                            let v = if sig.int_ret { format!("ARESxRT_int({r})") } else { r };
                            out.push_str(&format!("  ARESxK_drop(fr, {slot});\n"));
                            out.push_str(&format!("  ARESxK_backpatch(fr, {slot}, {v});\n"));
                            self.emit_expr(rest, sink, out);
                        }
                    }
                    None if sig.int_ret && d.int_ret && matches!(sink, Sink::Ret) => self.return_int(&r, out),
                    None if sig.int_ret && matches!(sink, Sink::Int { .. }) => {
                        let Sink::Int { var } = sink else { unreachable!() };
                        out.push_str(&format!("  sv{var} = {r};\n"));
                    }
                    None if sig.int_ret => self.deliver(&format!("ARESxRT_int({r})"), sink, out),
                    None => self.deliver(&r, sink, out),
                }
                out.push_str("  }\n");
            }
            None => {
                let (slot, cont) = match (call.rest, sink) {
                    (Some((slot, rest)), _) => {
                        let k = self.reserve_block();
                        let mut body = String::new();
                        self.scalars.remove(&slot);
                        self.emit_detached(rest, sink, &mut body);
                        self.set_block(k, &body);
                        (Some(slot), Some(k))
                    }
                    (None, Sink::Ret) => (None, None),
                    (None, Sink::Slot { slot, cont }) => {
                        (Some(slot), Some(cont.expect("a call in a block needs a continuation")))
                    }
                    (None, Sink::Int { .. }) => unreachable!("an integer join is only emitted in a direct function"),
                };
                out.push_str("  if (ARESxD_room()) {\n");
                let r = self.fresh("call");
                if sig.int_ret {
                    out.push_str(&format!("  Value* {r} = ARESxRT_int({direct});\n"));
                } else {
                    out.push_str(&format!("  Value* {r} = ARESxK_own({direct});\n"));
                }
                self.emit_drops(&call.drops, out);
                match (slot, cont) {
                    (Some(slot), Some(k)) => {
                        if call.rest.is_some() {
                            out.push_str(&format!("  ARESxK_drop(fr, {slot});\n"));
                        }
                        out.push_str(&format!("  ARESxK_backpatch(fr, {slot}, {r});\n"));
                        out.push_str(&format!("  ARESxK_jump(blk_{k});\n"));
                    }
                    _ => out.push_str(&format!("  ARESxK_ret({r});\n")),
                }
                out.push_str("  } else {\n");
                self.emit_drops(&call.drops, out);
                match (slot, cont) {
                    (Some(slot), Some(k)) => {
                        if call.rest.is_some() {
                            out.push_str(&format!("  ARESxK_drop(fr, {slot});\n"));
                        }
                        out.push_str(&format!("  ARESxK_apply(fr, {clos}, {last}, blk_{k}, {slot});\n"));
                    }
                    _ => out.push_str(&format!("  ARESxK_tailcall({clos}, {last});\n")),
                }
                out.push_str("  }\n  }\n");
            }
        }
    }

    /// The C function of a direct known function: `P` owns the arguments and
    /// is the frame's environment, `L` is the last code's slot array.
    /// See Note [Direct functions].
    fn emit_direct_fn(&mut self, caf: usize) -> String {
        let prog = self.prog;
        let f = self.known[&caf].clone();
        let sig = self.sigs[&caf].clone();
        let inner = f.inner();
        let code = &prog.codes[inner];
        let n = f.arity();
        let params: Vec<String> = (0..n)
            .map(|i| if sig.int_params[i] { format!("int64_t a{i}") } else { format!("Value* a{i}") })
            .collect();
        let boxed: Vec<String> =
            (0..n).map(|i| if sig.int_params[i] { "NULL".to_string() } else { format!("a{i}") }).collect();
        self.direct = Some(Direct { inner, arity: n, int_ret: sig.int_ret });
        self.looped = false;
        self.scalars.clear();
        self.env_ints.clear();
        for (j, &p) in f.env.iter().enumerate() {
            if sig.int_params[p] {
                self.env_ints.insert(j, format!("I{p}"));
            }
        }
        if sig.int_params[n - 1] {
            self.scalars.insert(0, (format!("I{}", n - 1), false));
        }
        let mut body = String::new();
        self.emit_expr(&code.body, Sink::Ret, &mut body);
        self.direct = None;
        self.scalars.clear();
        self.env_ints.clear();
        let ret = if sig.int_ret { "int64_t" } else { "Value*" };
        let mut out = format!("/* {} */\n", prog.codes[inner].name);
        out.push_str(&format!("static {ret} d_{inner}({}) {{\n", params.join(", ")));
        out.push_str(&format!("  Value* P[{n}] = {{ {} }};\n", boxed.join(", ")));
        for i in (0..n).filter(|&i| sig.int_params[i]) {
            out.push_str(&format!("  int64_t I{i} = a{i};\n"));
        }
        // The frame's env IS the parameter row, so a last use moves the row's
        // own reference out (see `ARESxVALUE_take_env`): a vector threaded
        // through a loop stays uniquely held and updates in place.
        assert!(
            f.env.iter().enumerate().all(|(j, &p)| j == p),
            "a direct function's env is its leading parameters, in order"
        );
        out.push_str(&format!("  Value* L[{}];\n", code.nlocals.max(1)));
        out.push_str("  Frame F;\n  Frame* fr = &F;\n");
        out.push_str("  size_t mark = ARESxMEM_pool_mark();\n");
        if self.looped {
            out.push_str("top:\n");
        }
        out.push_str(&format!(
            "  ARESxD_frame(fr, L, {}, P, {});\n",
            code.nlocals,
            f.env.len()
        ));
        if !sig.int_params[n - 1] {
            out.push_str(&format!("  ARESxD_param0(fr, P, {});\n", n - 1));
        }
        out.push_str(&body);
        out.push_str("}\n\n");
        out
    }

    /// Release the listed slots; a slot held in a scalar has nothing to release.
    fn emit_drops(&self, slots: &[usize], out: &mut String) {
        for s in slots {
            if !self.scalars.contains_key(s) {
                out.push_str(&format!("  ARESxK_drop(fr, {s});\n"));
            }
        }
    }

    /// A boxed argument for a direct function, as a reference the callee
    /// owns. See Note [Direct functions].
    fn give_atom(&self, a: &Atom) -> String {
        match a {
            Atom::Move(s) if !self.scalars.contains_key(s) => format!("ARESxVALUE_give(fr, {s})"),
            Atom::MoveEnv(i) if !self.env_ints.contains_key(i) => format!("ARESxVALUE_give_env(fr, {i})"),
            _ => format!("ARESxD_lend({})", self.atom(a)),
        }
    }

    /// A truth-value operand as a C `int`: a boolean scalar as is, else read
    /// off its box.
    fn bool_atom(&self, a: &Atom) -> String {
        match a {
            Atom::Local(s) | Atom::Move(s) if self.scalars.get(s).is_some_and(|(_, b)| *b) => {
                self.scalars[s].0.clone()
            }
            _ => format!("ARESxVALUE_as_bool({})", self.atom(a)),
        }
    }

    /// Whether every value `e` can end in is an integer the code already holds
    /// unboxed or computes unboxed (a literal, an integer scalar, an integer
    /// intrinsic, a direct call with an unboxed result), so `Sink::Int` can
    /// take it without a box. Conservative: anything else is `false`.
    fn int_tail(&self, e: &IrExpr) -> bool {
        let int_call = |c: &Call| match c.callee {
            Callee::Prim => intrinsic_operands(c.name).is_some_and(|ops| ops.len() == c.args.len())
                && !int_intrinsic(c.name, &vec![String::new(); c.args.len()]).is_some_and(|(_, b)| b),
            Callee::Known(caf) => self.direct_fns.contains(&caf) && self.sigs[&caf].int_ret,
            Callee::Extern(_) => false,
        };
        match e {
            IrExpr::Ret(a) => match a {
                Atom::LitI(_) => true,
                Atom::Local(s) | Atom::Move(s) => self.scalars.get(s).is_some_and(|(_, b)| !*b),
                Atom::Env(j) | Atom::MoveEnv(j) => self.env_ints.contains_key(j),
                _ => false,
            },
            IrExpr::Let { slot, rhs, body } => match self.call_chain(*slot, rhs, body) {
                Some(c) => match c.rest {
                    None => int_call(&c),
                    Some((_, rest)) => self.int_tail(rest),
                },
                None => self.int_tail(body),
            },
            IrExpr::App { fun, arg, .. } => self.lone_call(fun, arg).is_some_and(|c| int_call(&c)),
            IrExpr::Case { alts, default, .. } => {
                alts.iter().all(|a| self.int_tail(&a.body)) && self.int_tail(default)
            }
            IrExpr::Drop { body, .. } => self.int_tail(body),
            IrExpr::Fault(_) => true,
            _ => false,
        }
    }

    /// An integer atom as a C `int64_t` expression.
    fn int_atom(&self, a: &Atom) -> String {
        match a {
            Atom::LitI(n) => clit(*n),
            Atom::Local(s) | Atom::Move(s) if self.scalars.contains_key(s) => self.scalars[s].0.clone(),
            Atom::Env(i) | Atom::MoveEnv(i) if self.env_ints.contains_key(i) => self.env_ints[i].clone(),
            _ => format!("ARESxI({})", self.atom(a)),
        }
    }

    /// Whether every use of `slot` in `e`, up to a rebinding of it, is an
    /// integer intrinsic's operand or a literal case's scrutinee, all of them
    /// emitted in the current C function. See Note [Unboxed integers].
    fn scalar_uses(&self, e: &IrExpr, slot: usize) -> bool {
        let reads = |a: &Atom| a.mentions(slot);
        let block = self.direct.is_none();
        match e {
            IrExpr::Let { slot: s, rhs, body } => {
                if let Some(call) = self.call_chain(*s, rhs, body) {
                    if *s == slot {
                        return false;
                    }
                    if !self.int_args_only(&call, slot) {
                        return false;
                    }
                    let Some((r, rest)) = call.rest else {
                        return true;
                    };
                    if r == slot {
                        return true;
                    }
                    if block && !matches!(call.callee, Callee::Prim | Callee::Extern(_)) {
                        return !rest.mentions(slot);
                    }
                    return self.scalar_uses(rest, slot);
                }
                if !self.scalar_uses(rhs, slot) {
                    return false;
                }
                if *s == slot {
                    return true;
                }
                if block && self.has_call(rhs) {
                    return !body.mentions(slot);
                }
                self.scalar_uses(body, slot)
            }
            IrExpr::Case { scrut, alts, default } => {
                if reads(scrut) && alts.iter().any(|a| matches!(a.kind, AltKind::Con(_) | AltKind::Real(_))) {
                    return false;
                }
                alts.iter().all(|a| {
                    let rebinds = matches!(a.kind, AltKind::Con(_))
                        && (a.binder_base..a.binder_base + a.binders.len()).contains(&slot);
                    rebinds || self.scalar_uses(&a.body, slot)
                }) && self.scalar_uses(default, slot)
            }
            IrExpr::Drop { body, .. } => self.scalar_uses(body, slot),
            IrExpr::Handle { .. } | IrExpr::Defer { .. } => !e.mentions(slot),
            IrExpr::App { fun, arg, .. } if !fun.mentions(slot) => match self.lone_call(fun, arg) {
                Some(call) => self.int_args_only(&call, slot),
                None => !arg.mentions(slot),
            },
            IrExpr::App { .. }
            | IrExpr::Ret(_)
            | IrExpr::MkStruct { .. }
            | IrExpr::Field { .. }
            | IrExpr::MkVariant { .. }
            | IrExpr::MkTuple(_)
            | IrExpr::Fault(_) => {
                let mut hit = false;
                e.for_each_atom(&mut |a| hit = hit || reads(a));
                !hit
            }
        }
    }

    /// Whether `call` reads `slot` only as integer operands: of an integer
    /// intrinsic, or unboxed arguments of a direct function.
    fn int_args_only(&self, call: &Call, slot: usize) -> bool {
        let prim = call.callee == Callee::Prim;
        let sig = match call.callee {
            Callee::Known(caf) => self.sigs.get(&caf),
            Callee::Prim | Callee::Extern(_) => None,
        };
        call.args.iter().enumerate().all(|(k, a)| {
            !a.mentions(slot) || (prim && int_operand(call.name, k)) || sig.is_some_and(|s| s.int_params[k])
        })
    }

    /// Emit `e` into a C function of its own (a continuation block): the scalars
    /// of the current one are out of scope there.
    fn emit_detached(&mut self, e: &IrExpr, sink: Sink, out: &mut String) {
        let saved = std::mem::take(&mut self.scalars);
        self.emit_expr(e, sink, out);
        self.scalars = saved;
    }

    fn emit_case(&mut self, e: &IrExpr, sink: Sink, out: &mut String) {
        let IrExpr::Case {
            scrut,
            alts,
            default,
        } = e
        else {
            unreachable!()
        };
        let s = self.fresh("scrut");
        let scalar = match scrut {
            Atom::Local(i) | Atom::Move(i) => self.scalars.get(i).map(|(v, _)| v.clone()),
            _ => None,
        };
        match &scalar {
            Some(v) => out.push_str(&format!("  int64_t {s} = {v};\n")),
            // Scrutinising is what forces a lazy slot.
            None => out.push_str(&format!("  Value* {s} = ARESxVALUE_force({});\n", self.atom(scrut))),
        }
        let mut first = true;
        for al in alts {
            let cond = match &al.kind {
                AltKind::Int(n) if scalar.is_some() => format!("{s} == {}", clit(*n)),
                AltKind::Bool(b) if scalar.is_some() => format!("{s} == {}", *b as i32),
                AltKind::Int(n) => format!("ARESxVALUE_as_int({s}) == {}", clit(*n)),
                AltKind::Real(r) => format!("ARESxVALUE_as_num({s}) == {}", cdbl(*r)),
                AltKind::Bool(b) => {
                    format!("ARESxVALUE_as_bool({s}) == {}", if *b { 1 } else { 0 })
                }
                AltKind::Con(c) => {
                    format!("ARESxVALUE_ctor_is({s}, {})", cstr(c.bytes()))
                }
            };
            out.push_str(&format!(
                "  {} ({cond}) {{\n",
                if first { "if" } else { "else if" }
            ));
            let saved = self.scalars.clone();
            if let AltKind::Con(_) = &al.kind {
                for i in 0..al.binders.len() {
                    self.scalars.remove(&(al.binder_base + i));
                }
            }
            match (&al.kind, al.unpack) {
                (AltKind::Con(_), Unpack::Consume { slot, token }) => {
                    let bind: Vec<&str> =
                        al.binders.iter().map(|b| if b.is_some() { "1" } else { "0" }).collect();
                    let bind = if bind.is_empty() {
                        "NULL".to_string()
                    } else {
                        format!("(const unsigned char[]){{ {} }}", bind.join(", "))
                    };
                    let token = token.map_or("-1".to_string(), |t| t.to_string());
                    out.push_str(&format!(
                        "  ARESxK_unpack(fr, {s}, {slot}, {token}, {}, {bind}, {});\n",
                        al.binder_base,
                        al.binders.len()
                    ));
                }
                (AltKind::Con(_), Unpack::Borrow) => {
                    for (i, b) in al.binders.iter().enumerate() {
                        if b.is_some() {
                            out.push_str(&format!(
                                "  ARESxK_setlocal(fr, {}, ARESxVALUE_variant_field({s}, {i}));\n",
                                al.binder_base + i
                            ));
                        }
                    }
                }
                _ => {}
            }
            self.emit_branch(&al.body, sink, out);
            self.scalars = saved;
            out.push_str("  }\n");
            first = false;
        }
        out.push_str(if first { "  {\n" } else { "  else {\n" });
        let saved = self.scalars.clone();
        self.emit_branch(default, sink, out);
        self.scalars = saved;
        out.push_str("  }\n");
    }

    /// A case branch: either a real sink, or the pure-into sink (back-patch and
    /// fall through, so the enclosing pure let continues after the if/else).
    fn emit_branch(&mut self, e: &IrExpr, sink: Sink, out: &mut String) {
        match sink {
            Sink::Slot { slot, cont: None } if self.direct.is_none() => self.emit_pure_into(e, slot, out),
            _ => self.emit_expr(e, sink, out),
        }
    }

    fn emit_handle(&mut self, e: &IrExpr, sink: Sink, out: &mut String) {
        let IrExpr::Handle {
            body,
            clauses,
            els,
        } = e
        else {
            unreachable!()
        };
        let hbody = self.reserve_block();
        let (effs, ops, uses, cls) = if clauses.is_empty() {
            (
                "NULL".to_string(),
                "NULL".to_string(),
                "NULL".to_string(),
                "NULL".to_string(),
            )
        } else {
            let effs: Vec<String> = clauses
                .iter()
                .map(|c| match &c.effect {
                    Some(e) => cstr(e.bytes()),
                    None => "NULL".into(),
                })
                .collect();
            let ops: Vec<String> = clauses.iter().map(|c| cstr(c.op.bytes())).collect();
            // Matches the runtime's `ResumeUse` enum.
            let uses: Vec<String> = clauses
                .iter()
                .map(|c| {
                    match c.resume {
                        ResumeUse::Never => "RESUME_NEVER",
                        ResumeUse::Captured => "RESUME_CAPTURED",
                    }
                    .to_string()
                })
                .collect();
            let cls: Vec<String> = clauses.iter().map(|c| self.atom(&c.fun)).collect();
            (
                format!("(const char*[]){{ {} }}", effs.join(", ")),
                format!("(const char*[]){{ {} }}", ops.join(", ")),
                format!("(const int[]){{ {} }}", uses.join(", ")),
                format!("(Value*[]){{ {} }}", cls.join(", ")),
            )
        };
        let els = self.atom(els);
        let (cont, slot) = match sink {
            Sink::Ret => ("NULL".to_string(), 0),
            Sink::Int { .. } => unreachable!("an integer join holds no handler (`int_tail`)"),
            Sink::Slot { slot, cont } => {
                let cont = cont.expect("a non-tail Handle needs a continuation block");
                (format!("blk_{cont}"), slot)
            }
        };
        out.push_str(&format!(
            "  ARESxK_handle(fr, {cont}, {slot}, {effs}, {ops}, {uses}, {cls}, {}, {els}, \
             blk_{hbody});\n",
            clauses.len()
        ));
        let mut body_out = String::new();
        self.emit_detached(body, Sink::Ret, &mut body_out);
        self.set_block(hbody, &body_out);
    }

    fn emit_defer(&mut self, e: &IrExpr, sink: Sink, out: &mut String) {
        let IrExpr::Defer { cleanup, body } = e else {
            unreachable!()
        };
        let hbody = self.reserve_block();
        let cl = self.atom(cleanup);
        let (cont, slot) = match sink {
            Sink::Ret => ("NULL".to_string(), 0),
            Sink::Int { .. } => unreachable!("an integer join holds no defer (`int_tail`)"),
            Sink::Slot { slot, cont } => {
                let cont = cont.expect("a non-tail Defer needs a continuation block");
                (format!("blk_{cont}"), slot)
            }
        };
        out.push_str(&format!(
            "  ARESxK_defer_run(fr, {cont}, {slot}, {cl}, blk_{hbody});\n"
        ));
        let mut body_out = String::new();
        self.emit_detached(body, Sink::Ret, &mut body_out);
        self.set_block(hbody, &body_out);
    }

    /// Emit one code as its entry block (plus any continuation blocks it spawns).
    fn emit_code(&mut self, id: usize) {
        let entry = self.reserve_block();
        self.code_entry[id] = entry;
        // An unreachable code (a pruned global's CAF, or an `@extern` binding the
        // program never calls) becomes a trap stub: its body is never emitted, so
        // it references no foreign wrapper that was not collected.
        if !self.reachable.contains(&id) {
            self.set_block(entry, "  ares_fault(\"unreachable code\");\n");
            return;
        }
        let prog = self.prog;
        let mut out = String::new();
        self.emit_expr(&prog.codes[id].body, Sink::Ret, &mut out);
        self.set_block(entry, &out);
    }
}
