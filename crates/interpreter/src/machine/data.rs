//! Runtime values for the reified-K abstract machine: the port of
//! `engines/ITxDATA.hpp`. Rather than owning its children directly, every
//! machine value is shared through a `PVal` cell, because the machine mutates a
//! value in place when a recursive `let` is back-patched (see the `KRet`
//! handling in [`crate::machine`]): a closure that captured the placeholder
//! weakly must observe the finished value.
//!
//! The lifetime `'p` is the borrow of the [`Program`](frontend::ir::IrProgram): a
//! captured continuation ([`Resumption`]) holds `&'p` pointers into the code, and
//! a resumption can be stored in a value and resumed later, so the value type
//! carries the program's lifetime.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use utilities::{Code, Diagnostic, Result, Span, StrId};
use frontend::ir::data::{Builtin, Prim};

use crate::machine::KFrame;

/// A shared, mutable machine value cell (the C++ `pVal = shared_ptr<Value>`).
pub type PVal<'p> = Rc<RefCell<Value<'p>>>;

/// Wrap a freshly built value in a cell.
pub fn mk<'p>(v: Value<'p>) -> PVal<'p> {
    Rc::new(RefCell::new(v))
}

/// A strict runtime value. Aggregates hold `PVal` children (not owned `Value`s),
/// so structure is shared and a recursive binding's cell can be patched in place.
pub enum Value<'p> {
    /// The placeholder / erased value (an unset local, a discarded thunk result).
    Unk,
    Int(i64),
    Real(f64),
    /// A single-precision real (`@Float32`). Held distinct from [`Value::Real`] so
    /// arithmetic rounds to f32 and display uses f32 precision; it is stored as an
    /// `f32`, not a narrowed `f64`.
    Real32(f32),
    /// Byte string, also the representation of an `Array`.
    Str(Rc<Vec<u8>>),
    Bool(bool),
    Unit,
    Tuple(Vec<PVal<'p>>),
    Struct {
        name: StrId,
        fields: Vec<(StrId, PVal<'p>)>,
    },
    Variant {
        ty: StrId,
        tag: StrId,
        fields: Vec<PVal<'p>>,
    },
    Vector(Rc<Vec<PVal<'p>>>),
    /// A sized tensor `[n]T`: a strided view over a flat element buffer.
    Tensor(Rc<Tensor<'p>>),
    /// A partially applied built-in; it runs once `args` reaches its arity.
    Builtin {
        op: &'static Builtin,
        args: Vec<PVal<'p>>,
    },
    /// A foreign function (`@extern`) value, before it is saturated. Like a
    /// built-in, it accumulates its positional C arguments (`arg_types`; a nullary
    /// C function still takes one unit argument) into `args` and, once saturated,
    /// marshals them across the seam selected by `abi` (`"C"` = a C library symbol,
    /// `"WASM"` = a host import) and calls `symbol`. The record grouping several C
    /// parameters is flattened into positional arguments at the call site during
    /// lowering, so it never reaches here. `ret_type` selects how the result is
    /// wrapped. `lib` is the symbolic library the symbol lives in, resolved to a
    /// soname and `dlopen`ed when the symbol is not in the compiled-in host table.
    Extern {
        abi: StrId,
        symbol: StrId,
        lib: StrId,
        arg_types: Rc<[StrId]>,
        ret_type: StrId,
        args: Vec<PVal<'p>>,
    },
    /// A closure: a lifted IR code block plus its captured environment record.
    Code {
        code: usize,
        env: Vec<PVal<'p>>,
    },
    /// A recursive-let placeholder: a weak reference to the cell being filled.
    /// [`crate::machine::deref`] locks it once the value is built.
    Rec(Weak<RefCell<Value<'p>>>),
    /// An algebraic effect operation; applying it performs the operation.
    Op {
        effect: Option<StrId>,
        op: StrId,
    },
    /// A captured continuation (a delimited stack slice).
    Resump(Rc<Resumption<'p>>),
    /// An unforced lazy slot: the held value is a nullary closure. The machine
    /// forces it where the value is scrutinised and patches the cell with the
    /// result, so a recursive structure is built one cell at a time and each cell
    /// is computed at most once.
    Thunk(PVal<'p>),
    /// A `@spawn` task's handle (a `@par` future is its result itself).
    Future(Rc<RefCell<Task<'p>>>),
    /// An unbounded channel (`@Chan`): the messages sent and not yet received.
    Chan(Rc<RefCell<std::collections::VecDeque<PVal<'p>>>>),
    /// A transactional variable (`@TVar`).
    TVar(Rc<RefCell<TVar<'p>>>),
}

/// A sized tensor: element `[i0, .., ik]` is `buf[off + sum(i_j * strides[j])]`
/// with each `i_j < shape[j]`. A view (a row, a transpose, a slice) shares `buf`
/// and only changes `off`/`shape`/`strides`, so it copies no elements.
pub struct Tensor<'p> {
    pub buf: Rc<Vec<PVal<'p>>>,
    pub off: usize,
    pub shape: Box<[usize]>,
    pub strides: Box<[usize]>,
}

/// A `@spawn` task. The reference semantics run it lazily, on one thread: when
/// it is awaited, when another task would block (a receive on an empty
/// channel, a transaction that retries), or before the program exits.
pub enum Task<'p> {
    Pending(PVal<'p>),
    Running,
    Done(PVal<'p>),
}

/// A transactional variable's value and how many commits have written it.
pub struct TVar<'p> {
    pub val: PVal<'p>,
    pub version: u64,
}

/// Move `v`'s child cells onto `out`, leaving `v` childless.
fn take_children<'p>(v: &mut Value<'p>, out: &mut Vec<PVal<'p>>) {
    match v {
        Value::Tuple(items)
        | Value::Variant { fields: items, .. }
        | Value::Builtin { args: items, .. }
        | Value::Extern { args: items, .. }
        | Value::Code { env: items, .. } => out.append(items),
        Value::Struct { fields, .. } => {
            out.extend(std::mem::take(fields).into_iter().map(|(_, p)| p))
        }
        Value::Thunk(inner) => out.push(std::mem::replace(inner, mk(Value::Unit))),
        // Only the last owner may take the elements; a shared buffer is left to
        // whoever still holds it.
        Value::Vector(items) => {
            if let Some(items) = Rc::get_mut(items) {
                out.append(items);
            }
        }
        Value::Tensor(t) => {
            if let Some(items) = Rc::get_mut(t).and_then(|t| Rc::get_mut(&mut t.buf)) {
                out.append(items);
            }
        }
        _ => {}
    }
}

/// Free a value's descendants on an explicit worklist rather than by recursion.
/// The derived drop would nest once per child, so releasing a list of n cells
/// would need n host stack frames and abort the process on a long one. Depth
/// here is bounded by the heap instead.
///
/// Draining is what makes this terminate: each `Value` popped off the worklist
/// has already had its children taken, so the recursive call this `Drop` would
/// otherwise make finds nothing left to follow.
impl Drop for Value<'_> {
    fn drop(&mut self) {
        let mut work = Vec::new();
        take_children(self, &mut work);
        while let Some(cell) = work.pop() {
            if let Ok(cell) = Rc::try_unwrap(cell) {
                take_children(&mut cell.into_inner(), &mut work);
            }
        }
    }
}

/// A captured continuation: the `KFrame` slice from a prompt up to a perform
/// point.
///
/// Its owners are the [`Value::Resump`] cells holding it plus the clause
/// boundaries (`KAfterClause` frames) watching it, of which there are `bound`. A
/// boundary never resumes, so the resumption is uniquely owned when one cell
/// holds it and that cell is held once: a resume through it then moves the slice
/// out, and any other resume splices a private COPY, leaving it resumable.
/// `used` means "resumed at least once", which is what the clause boundary
/// consults to decide whether an abandoned slice's `defer` cleanups still have to
/// run.
pub struct Resumption<'p> {
    pub(crate) seg: RefCell<Vec<KFrame<'p>>>,
    pub(crate) used: Cell<bool>,
    pub(crate) bound: Cell<usize>,
}

pub(crate) fn fault(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error(Code::RuntimeFault, Span::at(0), 0, msg.into())
}

// -- coercions --------------------------------------------------------------

fn as_f64(v: &PVal) -> Result<f64> {
    match &*v.borrow() {
        Value::Int(n) => Ok(*n as f64),
        Value::Real(r) => Ok(*r),
        Value::Real32(r) => Ok(*r as f64),
        _ => Err(fault("expected a number")),
    }
}

/// Read a numeric operand rounded to single precision, for the `@f32*` intrinsics.
fn as_f32(v: &PVal) -> Result<f32> {
    as_f64(v).map(|r| r as f32)
}

fn as_bytes(v: &PVal) -> Result<Rc<Vec<u8>>> {
    match &*v.borrow() {
        Value::Str(b) => Ok(b.clone()),
        _ => Err(fault("expected a byte vector")),
    }
}

fn as_vec<'p>(v: &PVal<'p>) -> Result<Rc<Vec<PVal<'p>>>> {
    match &*v.borrow() {
        Value::Vector(items) => Ok(items.clone()),
        _ => Err(fault("expected a vector")),
    }
}

fn as_int(v: &PVal) -> Result<i64> {
    match &*v.borrow() {
        Value::Int(n) => Ok(*n),
        _ => Err(fault("expected an integer element")),
    }
}

fn as_index(v: &PVal) -> Result<usize> {
    match &*v.borrow() {
        Value::Int(n) if *n >= 0 => Ok(*n as usize),
        Value::Int(_) => Err(fault("negative index")),
        _ => Err(fault("expected an integer index")),
    }
}

// -- strided tensors -----------------------------------------------------

pub(crate) fn mk_tensor<'p>(
    buf: Rc<Vec<PVal<'p>>>,
    off: usize,
    shape: impl Into<Box<[usize]>>,
    strides: impl Into<Box<[usize]>>,
) -> Value<'p> {
    Value::Tensor(Rc::new(Tensor { buf, off, shape: shape.into(), strides: strides.into() }))
}

pub(crate) fn as_tensor<'p>(v: &PVal<'p>) -> Result<Rc<Tensor<'p>>> {
    match &*v.borrow() {
        Value::Tensor(t) => Ok(t.clone()),
        _ => Err(fault("expected a tensor")),
    }
}

/// Row-major (C-order) strides for a shape.
fn row_major_strides(shape: &[usize]) -> Vec<usize> {
    let mut s = vec![1usize; shape.len()];
    for i in (0..shape.len().saturating_sub(1)).rev() {
        s[i] = s[i + 1] * shape[i + 1];
    }
    s
}

/// `i` wrapped into `[0, n)`: tensor indexing is modular. `n` is nonzero.
fn wrap_index(i: i64, n: usize) -> usize {
    if (0..n as i64).contains(&i) { i as usize } else { i.rem_euclid(n as i64) as usize }
}

impl<'p> Tensor<'p> {
    pub(crate) fn len(&self) -> usize {
        self.shape.first().copied().unwrap_or(0)
    }

    fn cell(&self, flat: usize) -> Result<PVal<'p>> {
        self.buf.get(flat).cloned().ok_or_else(|| fault("tensor index out of bounds"))
    }

    /// What remains at buffer position `base` once the leading `k` axes are
    /// fixed: the scalar (a fresh shallow copy) when no axis is left, else a view.
    fn sub(&self, base: usize, k: usize) -> Result<Value<'p>> {
        if k >= self.shape.len() {
            return Ok(self.cell(base)?.borrow().clone_shallow());
        }
        Ok(mk_tensor(self.buf.clone(), base, &self.shape[k..], &self.strides[k..]))
    }
}

/// The tensor's scalar elements in row-major logical order (following a view's
/// offset/strides).
pub(crate) fn gather<'p>(t: &Tensor<'p>) -> Result<Vec<PVal<'p>>> {
    let total: usize = t.shape.iter().product();
    let rank = t.shape.len();
    let mut out = Vec::with_capacity(total);
    let mut idx = vec![0usize; rank];
    for _ in 0..total {
        let flat = t.off + (0..rank).map(|k| idx[k] * t.strides[k]).sum::<usize>();
        out.push(t.cell(flat)?);
        for k in (0..rank).rev() {
            idx[k] += 1;
            if idx[k] < t.shape[k] {
                break;
            }
            idx[k] = 0;
        }
    }
    Ok(out)
}

fn show_tensor(t: &Tensor, k: usize, base: usize, out: &mut String) {
    if k == t.shape.len() {
        match t.buf.get(base) {
            Some(v) => out.push_str(&v.borrow().show()),
            None => out.push_str("<out of bounds>"),
        }
        return;
    }
    out.push('[');
    for i in 0..t.shape[k] {
        if i > 0 {
            out.push_str(", ");
        }
        show_tensor(t, k + 1, base + i * t.strides[k], out);
    }
    out.push(']');
}

/// A fresh contiguous tensor of `shape` over `buf`.
fn tensor_of<'p>(buf: Vec<PVal<'p>>, shape: Vec<usize>) -> Value<'p> {
    let strides = row_major_strides(&shape);
    mk_tensor(Rc::new(buf), 0, shape, strides)
}

/// Build a tensor by stacking `elems` along a new leading axis: scalar elements
/// give a rank-1 tensor; tensor elements are flattened (equal shapes required)
/// and get the outer dimension prepended. This is the construction/`create` core.
pub(crate) fn tensor_stack<'p>(elems: &[PVal<'p>]) -> Result<Value<'p>> {
    let n = elems.len();
    let Some(first) = elems.first() else {
        return Ok(tensor_of(Vec::new(), vec![0]));
    };
    let Value::Tensor(t0) = &*first.borrow() else {
        return Ok(tensor_of(elems.to_vec(), vec![n]));
    };
    let mut buf = Vec::with_capacity(n * t0.shape.iter().product::<usize>());
    for e in elems {
        match &*e.borrow() {
            Value::Tensor(t) if t.shape == t0.shape => buf.extend(gather(t)?),
            _ => return Err(fault("stacking tensors of unequal shape")),
        }
    }
    let mut shape = vec![n];
    shape.extend_from_slice(&t0.shape);
    Ok(tensor_of(buf, shape))
}

/// The tensor of a compile-time constant: the struct
/// `@tensor.{ .buf = vec[row-major elements], .shape = vec[sizes] }`, which is
/// how a tensor crosses out of the machine (see `machine::reify`).
pub(crate) fn tensor_from_struct<'p>(name: StrId, fields: &[(StrId, PVal<'p>)]) -> Option<Value<'p>> {
    if name != "@tensor" {
        return None;
    }
    let field = |n: &str| fields.iter().find(|(f, _)| f == n).map(|(_, v)| v.clone());
    let buf = as_vec(&field("buf")?).ok()?.as_ref().clone();
    let shape: Vec<usize> = as_vec(&field("shape")?).ok()?.iter().map(as_index).collect::<Result<_>>().ok()?;
    (shape.iter().product::<usize>() == buf.len()).then(|| tensor_of(buf, shape))
}

// -- tensor kernels ----------------------------------------------------------
//
// `@tensor_dot add mul zero x y` and `@tensor_matmul add mul zero a b` compute
// exactly what the element-wise Ares definition would: a dot is
// `add (... (add zero (mul x0 y0)) ...) (mul xn yn)` over the length of `x`, with
// `y` indexed modulo its own length. When `add`/`mul` are the bare intrinsics
// (CORE's numeric instances eta-reduce to them) and the elements are the
// matching scalars, the loop runs on unboxed numbers; otherwise the machine
// applies the operations (see `Machine::dot_lanes`).

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DotKind {
    Generic,
    Int,
    F64,
    F32,
}

pub(crate) fn dot_kind(add: &PVal, mul: &PVal, zero: &PVal) -> DotKind {
    let prim = |v: &PVal| match &*v.borrow() {
        Value::Builtin { op, args } if args.is_empty() => Some(op.prim),
        _ => None,
    };
    match (prim(add), prim(mul), &*zero.borrow()) {
        (Some(Prim::Iadd), Some(Prim::Imul), Value::Int(_)) => DotKind::Int,
        (Some(Prim::Fadd), Some(Prim::Fmul), Value::Real(_)) => DotKind::F64,
        (Some(Prim::F32add), Some(Prim::F32mul), Value::Real32(_)) => DotKind::F32,
        _ => DotKind::Generic,
    }
}

/// One operand of a dot: element `i` is what remains of `t` at buffer position
/// `base + (i mod len) * step` once `k` leading axes are fixed.
pub(crate) struct Lane<'p> {
    pub t: Rc<Tensor<'p>>,
    pub base: usize,
    pub step: usize,
    pub len: usize,
    pub k: usize,
}

impl<'p> Lane<'p> {
    pub(crate) fn leading(t: Rc<Tensor<'p>>) -> Lane<'p> {
        let (step, len) = (t.strides.first().copied().unwrap_or(0), t.len());
        Lane { base: t.off, step, len, k: 1, t }
    }

    pub(crate) fn at(&self, i: usize) -> Result<Value<'p>> {
        let i = if i < self.len { i } else { i % self.len };
        self.t.sub(self.base + i * self.step, self.k)
    }
}

/// The dot on unboxed scalars, or `None` when an element is not the scalar
/// `kind` expects (the caller then applies the operations).
pub(crate) fn dot_scalar<'p>(kind: DotKind, zero: &PVal<'p>, x: &Lane<'p>, y: &Lane<'p>) -> Result<Option<Value<'p>>> {
    if x.k < x.t.shape.len() || y.k < y.t.shape.len() {
        return Ok(None);
    }
    let at = |l: &Lane<'p>, i: usize| l.t.cell(l.base + (if i < l.len { i } else { i % l.len }) * l.step);
    let n = x.len;
    match kind {
        DotKind::Generic => Ok(None),
        DotKind::Int => {
            let Value::Int(mut acc) = *zero.borrow() else { return Ok(None) };
            for i in 0..n {
                match (&*at(x, i)?.borrow(), &*at(y, i)?.borrow()) {
                    (Value::Int(p), Value::Int(q)) => acc = acc.wrapping_add(p.wrapping_mul(*q)),
                    _ => return Ok(None),
                }
            }
            Ok(Some(Value::Int(acc)))
        }
        DotKind::F64 => {
            let Value::Real(mut acc) = *zero.borrow() else { return Ok(None) };
            let num = |v: &Value| match v {
                Value::Real(r) => Some(*r),
                Value::Int(n) => Some(*n as f64),
                _ => None,
            };
            for i in 0..n {
                match (num(&at(x, i)?.borrow()), num(&at(y, i)?.borrow())) {
                    (Some(p), Some(q)) => acc += p * q,
                    _ => return Ok(None),
                }
            }
            Ok(Some(Value::Real(acc)))
        }
        DotKind::F32 => {
            let Value::Real32(mut acc) = *zero.borrow() else { return Ok(None) };
            let num = |v: &Value| match v {
                Value::Real32(r) => Some(*r),
                Value::Real(r) => Some(*r as f32),
                Value::Int(n) => Some(*n as f64 as f32),
                _ => None,
            };
            for i in 0..n {
                match (num(&at(x, i)?.borrow()), num(&at(y, i)?.borrow())) {
                    (Some(p), Some(q)) => acc += p * q,
                    _ => return Ok(None),
                }
            }
            Ok(Some(Value::Real32(acc)))
        }
    }
}

/// The result of `@tensor_matmul` from its `m * n` cells, row-major: an
/// `[m, n]` matrix of scalars, or with tensor cells each row stacked and then
/// the rows, as nested `create`s would.
pub(crate) fn matmul_result<'p>(cells: Vec<PVal<'p>>, m: usize, n: usize) -> Result<Value<'p>> {
    if m == 0 {
        return tensor_stack(&[]);
    }
    if !cells.iter().any(|c| matches!(&*c.borrow(), Value::Tensor(_))) {
        return Ok(tensor_of(cells, vec![m, n]));
    }
    let rows = cells.chunks(n).map(|r| tensor_stack(r).map(mk)).collect::<Result<Vec<_>>>()?;
    tensor_stack(&rows)
}

fn as_len(v: &PVal) -> Result<usize> {
    match &*v.borrow() {
        Value::Int(n) if *n >= 0 => Ok(*n as usize),
        _ => Err(fault("expected a non-negative length")),
    }
}

fn as_byte(v: &PVal) -> Result<u8> {
    match &*v.borrow() {
        Value::Int(n) if (0..=255).contains(n) => Ok(*n as u8),
        _ => Err(fault("expected a byte value (0..255)")),
    }
}

/// `n` bytes copied from the C memory at address `p` (`@ptr_bytes`).
#[cfg(not(target_arch = "wasm32"))]
fn ptr_bytes(p: i64, n: usize) -> Result<Vec<u8>> {
    if n == 0 {
        return Ok(Vec::new());
    }
    if p == 0 {
        return Err(fault("`@ptr_bytes` from a null pointer"));
    }
    // The program vouches for `n` readable bytes at `p`, as C code would.
    Ok(unsafe { std::slice::from_raw_parts(p as usize as *const u8, n) }.to_vec())
}

#[cfg(target_arch = "wasm32")]
fn ptr_bytes(_p: i64, _n: usize) -> Result<Vec<u8>> {
    Err(fault("`@ptr_bytes` has no C memory on this target"))
}

// -- built-ins --------------------------------------------------------------

/// Run a saturated built-in. `a` is already deref'd (the machine derefs an
/// argument before pushing it onto a builtin's operand list).
fn outside_build_script(name: &str) -> Diagnostic {
    Diagnostic::error(Code::BuildScript, Span::at(0), 0, format!("`{name}` outside a project's `BUILD.ares`"))
        .with_help("call it from the `@build` of the `BUILD.ares` at the project root")
}

pub(crate) fn run_builtin<'p>(name: Prim, a: &[PVal<'p>]) -> Result<Value<'p>> {
    match name {
        Prim::Iadd | Prim::Isub | Prim::Imul | Prim::Idiv | Prim::Imod | Prim::Udiv | Prim::Umod | Prim::Fadd | Prim::Fsub
        | Prim::Fmul | Prim::Fdiv | Prim::Fmod | Prim::Fpow | Prim::F32add | Prim::F32sub | Prim::F32mul | Prim::F32div
        | Prim::F32mod | Prim::F32pow => arith_intrinsic(name, &a[0], &a[1]),
        Prim::Ieq | Prim::Ilt | Prim::Ult | Prim::Feq | Prim::Flt | Prim::Seq | Prim::Slt => {
            compare_intrinsic(name, &a[0], &a[1])
        }
        Prim::Iand | Prim::Ior | Prim::Ixor | Prim::Ishl | Prim::Ishr | Prim::Ushr => {
            bit_intrinsic(name, as_int(&a[0])?, as_int(&a[1])?)
        }
        Prim::Inot => Ok(Value::Int(!as_int(&a[0])?)),
        Prim::F64Bits => Ok(Value::Int(as_f64(&a[0])?.to_bits() as i64)),
        Prim::BitsF64 => Ok(Value::Real(f64::from_bits(as_int(&a[0])? as u64))),
        Prim::F32Bits => Ok(Value::Int(as_f32(&a[0])?.to_bits() as i64)),
        Prim::BitsF32 => Ok(Value::Real32(f32::from_bits(as_int(&a[0])? as u32))),
        Prim::Swrap | Prim::Uwrap => {
            let (bits, x) = (as_int(&a[0])?, as_int(&a[1])?);
            if !(1..64).contains(&bits) {
                return Ok(Value::Int(x));
            }
            let shift = 64 - bits as u32;
            Ok(Value::Int(if name == Prim::Swrap {
                (x << shift) >> shift
            } else {
                ((x as u64) << shift >> shift) as i64
            }))
        }
        Prim::Neg => match &*a[0].borrow() {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Real(r) => Ok(Value::Real(-r)),
            Value::Real32(r) => Ok(Value::Real32(-r)),
            _ => Err(fault("`neg` on a non-number")),
        },
        Prim::Not => match &*a[0].borrow() {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            _ => Err(fault("`not` on a non-boolean")),
        },
        Prim::Eq => Ok(Value::Bool(value_eq(&a[0], &a[1]))),
        Prim::Lt | Prim::Gt | Prim::Le | Prim::Ge => compare(name, &a[0], &a[1]),
        // `++` itself is a CORE function over the `ICat` interface now; `@acat` is the
        // byte-buffer primitive its `@Str` / `@Array` instances bottom out on.
        Prim::Concat | Prim::Acat => concat(&a[0], &a[1]),
        // Suspend a lazy slot. The argument is the nullary closure lowering built
        // around the slot's expression; `force` runs it.
        Prim::Delay => Ok(Value::Thunk(a[0].clone())),
        Prim::ArrayAlloc => Ok(Value::Str(Rc::new(vec![0u8; as_len(&a[0])?]))),
        Prim::ArrayLen => Ok(Value::Int(as_bytes(&a[0])?.len() as i64)),
        Prim::ArrayGet => {
            let bytes = as_bytes(&a[0])?;
            let i = as_index(&a[1])?;
            bytes
                .get(i)
                .map(|b| Value::Int(*b as i64))
                .ok_or_else(|| fault("array index out of bounds"))
        }
        Prim::ArrayPush => {
            // In place under the same ownership test as `VecPush`: a byte loop
            // that grows a string by pushing would otherwise be quadratic.
            let byte = as_byte(&a[1])?;
            let unshared = Rc::strong_count(&a[0]) == 1 && Rc::weak_count(&a[0]) == 0;
            if unshared {
                let mut cell = a[0].borrow_mut();
                if let Value::Str(bytes) = &mut *cell {
                    if let Some(buf) = Rc::get_mut(bytes) {
                        buf.push(byte);
                        return Ok(Value::Str(bytes.clone()));
                    }
                }
            }
            let mut bytes = as_bytes(&a[0])?.as_ref().clone();
            bytes.push(byte);
            Ok(Value::Str(Rc::new(bytes)))
        }
        Prim::ArraySet => {
            let mut bytes = as_bytes(&a[0])?.as_ref().clone();
            let i = as_index(&a[1])?;
            if i >= bytes.len() {
                return Err(fault("array index out of bounds"));
            }
            bytes[i] = as_byte(&a[2])?;
            Ok(Value::Str(Rc::new(bytes)))
        }
        Prim::ArraySlice => {
            let bytes = as_bytes(&a[0])?;
            let mut beg = as_index(&a[1])?;
            let mut end = as_index(&a[2])?;
            beg = beg.min(bytes.len());
            end = end.clamp(beg, bytes.len());
            Ok(Value::Str(Rc::new(bytes[beg..end].to_vec())))
        }
        Prim::ArrayScan => {
            let bytes = as_bytes(&a[0])?;
            let from = as_index(&a[1])?.min(bytes.len());
            let mask = as_bytes(&a[2])?;
            if mask.len() < 256 {
                return Err(fault("`@array_scan` mask must hold 256 bytes"));
            }
            let hit = bytes[from..].iter().position(|&b| mask[b as usize] != 0);
            Ok(Value::Int(hit.map_or(bytes.len(), |k| from + k) as i64))
        }
        Prim::ArrayMatch => {
            let bytes = as_bytes(&a[0])?;
            let at = as_index(&a[1])?;
            let needle = as_bytes(&a[2])?;
            Ok(Value::Bool(bytes.get(at..).is_some_and(|rest| rest.starts_with(&needle))))
        }
        Prim::PtrBytes => {
            let p = as_int(&a[0])?;
            let n = as_len(&a[1])?;
            Ok(Value::Str(Rc::new(ptr_bytes(p, n)?)))
        }
        Prim::RecordWithout => {
            let label = as_bytes(&a[1])?;
            match &*a[0].borrow() {
                Value::Struct { name, fields } => {
                    let mut out = Vec::with_capacity(fields.len());
                    let mut dropped = false;
                    for (n, val) in fields {
                        if !dropped && n.bytes() == label.as_ref().as_slice() {
                            dropped = true;
                            continue;
                        }
                        out.push((*n, val.clone()));
                    }
                    Ok(Value::Struct {
                        name: *name,
                        fields: out,
                    })
                }
                _ => Err(fault("`record_without` on a non-record")),
            }
        }
        Prim::VecNew => Ok(Value::Vector(Rc::new(Vec::new()))),
        Prim::VecFill => {
            let n = as_len(&a[0])?;
            Ok(Value::Vector(Rc::new(vec![a[1].clone(); n])))
        }
        Prim::VecLen => Ok(Value::Int(as_vec(&a[0])?.len() as i64)),
        Prim::TensorLength => Ok(Value::Int(as_tensor(&a[0])?.len() as i64)),
        Prim::TensorStack => {
            let v = as_vec(&a[0])?;
            tensor_stack(&v)
        }
        // O(1) transpose: a VIEW sharing the buffer with every axis reversed (for a
        // rank-2 tensor, the matrix transpose). No elements are copied.
        Prim::TensorTranspose => {
            let t = as_tensor(&a[0])?;
            let shape: Vec<usize> = t.shape.iter().rev().copied().collect();
            let strides: Vec<usize> = t.strides.iter().rev().copied().collect();
            Ok(mk_tensor(t.buf.clone(), t.off, shape, strides))
        }
        // O(1) slice of axis 0 (`@tensor_slice`) or of a GIVEN axis: a VIEW over
        // `[lo, hi)` of it.
        Prim::TensorSlice | Prim::TensorSliceAxis => {
            let t = as_tensor(&a[0])?;
            let (axis, range) = if name == Prim::TensorSliceAxis {
                let axis = as_index(&a[1])?;
                if axis >= t.shape.len() {
                    return Err(fault("slice axis out of range"));
                }
                (axis, &a[2..4])
            } else {
                if t.shape.is_empty() {
                    return Err(fault("slice of a rank-0 tensor"));
                }
                (0, &a[1..3])
            };
            let lo = as_index(&range[0])?.min(t.shape[axis]);
            let hi = as_index(&range[1])?.clamp(lo, t.shape[axis]);
            let mut shape = t.shape.clone();
            shape[axis] = hi - lo;
            Ok(mk_tensor(t.buf.clone(), t.off + lo * t.strides[axis], shape, t.strides.clone()))
        }
        // O(1) index of a GIVEN axis (modular): drop that axis, a VIEW. Reducing the
        // last remaining axis yields the scalar element.
        Prim::TensorIndexAxis => {
            let t = as_tensor(&a[0])?;
            let axis = as_index(&a[1])?;
            if axis >= t.shape.len() {
                return Err(fault("index axis out of range"));
            }
            if t.shape[axis] == 0 {
                return Err(fault("index into an empty axis"));
            }
            let base = t.off + wrap_index(as_int(&a[2])?, t.shape[axis]) * t.strides[axis];
            if t.shape.len() == 1 {
                return t.sub(base, 1);
            }
            let keep = |xs: &[usize]| -> Vec<usize> {
                xs.iter().enumerate().filter(|(k, _)| *k != axis).map(|(_, x)| *x).collect()
            };
            Ok(mk_tensor(t.buf.clone(), base, keep(&t.shape), keep(&t.strides)))
        }
        Prim::VecGet => {
            let v = as_vec(&a[0])?;
            let i = as_index(&a[1])?;
            v.get(i)
                .cloned()
                .map(|p| p.borrow().clone_shallow())
                .ok_or_else(|| fault("vec index out of bounds"))
        }
        Prim::VecPush => {
            // Appending by copying the whole buffer makes building a vector
            // quadratic, so push in place whenever nothing else can observe the
            // buffer: this operand list must be the cell's only owner, and the
            // cell the buffer's only owner. Otherwise fall back to the copy.
            let unshared = Rc::strong_count(&a[0]) == 1 && Rc::weak_count(&a[0]) == 0;
            if unshared {
                let mut cell = a[0].borrow_mut();
                if let Value::Vector(items) = &mut *cell {
                    if let Some(buf) = Rc::get_mut(items) {
                        buf.push(a[1].clone());
                        return Ok(Value::Vector(items.clone()));
                    }
                }
            }
            let mut v = as_vec(&a[0])?.as_ref().clone();
            v.push(a[1].clone());
            Ok(Value::Vector(Rc::new(v)))
        }
        // A copy of the half-open range `[lo, hi)`, clamped to the vector.
        Prim::VecSlice => {
            let v = as_vec(&a[0])?;
            let lo = as_index(&a[1])?.min(v.len());
            let hi = as_index(&a[2])?.clamp(lo, v.len());
            Ok(Value::Vector(Rc::new(v[lo..hi].to_vec())))
        }
        Prim::TensorConcat => {
            let (x, y) = (as_tensor(&a[0])?, as_tensor(&a[1])?);
            if x.shape.is_empty() || x.shape.get(1..) != y.shape.get(1..) {
                return Err(fault("concat: tensors differ below the first axis"));
            }
            let mut buf = gather(&x)?;
            buf.extend(gather(&y)?);
            let mut shape = x.shape.to_vec();
            shape[0] += y.shape[0];
            Ok(tensor_of(buf, shape))
        }
        Prim::TensorIndex => {
            let t = as_tensor(&a[0])?;
            if t.len() == 0 {
                return Err(fault("index into an empty tensor"));
            }
            let i = wrap_index(as_int(&a[1])?, t.len());
            t.sub(t.off + i * t.strides[0], 1)
        }
        // In place under the same condition as `VecPush`.
        Prim::VecSet => {
            let i = as_index(&a[1])?;
            let unshared = Rc::strong_count(&a[0]) == 1 && Rc::weak_count(&a[0]) == 0;
            if unshared {
                let mut cell = a[0].borrow_mut();
                if let Value::Vector(items) = &mut *cell {
                    if let Some(buf) = Rc::get_mut(items) {
                        let Some(slot) = buf.get_mut(i) else {
                            return Err(fault("vec index out of bounds"));
                        };
                        *slot = a[2].clone();
                        return Ok(Value::Vector(items.clone()));
                    }
                }
            }
            let mut v = as_vec(&a[0])?.as_ref().clone();
            if i >= v.len() {
                return Err(fault("vec index out of bounds"));
            }
            v[i] = a[2].clone();
            Ok(Value::Vector(Rc::new(v)))
        }
        // Metaprogramming: tokenize a string into CORE `Token` variants. A lex
        // error is a `Diagnostic`, which propagates as a fault (fails a `&`).
        Prim::Lex => {
            let bytes = as_bytes(&a[0])?;
            let src = std::str::from_utf8(&bytes)
                .map_err(|_| fault("@lex: the argument is not valid UTF-8"))?;
            Ok(crate::machine::embed(&lex(src)?))
        }
        Prim::TokenKind => match &*a[0].borrow() {
            Value::Variant { tag, .. } => Ok(Value::Str(Rc::new(tag.as_str().as_bytes().to_vec()))),
            _ => Err(fault("expected a Token")),
        },
        Prim::TokenText => token_text(&a[0]).map(Value::Str),
        Prim::Unparse => Ok(Value::Str(Rc::new(code_src(&a[0], "@unparse")?.as_ref().clone()))),
        // Parse a token vector (from `@lex`) into opaque `@Code`. `@Code` is
        // text-backed, so this detokenizes (the lexemes rejoined with a space,
        // which whitespace-insensitive expression syntax re-lexes identically) and
        // validates exactly like `@parse_str`. A syntax error traps.
        Prim::Parse => {
            let toks = as_vec(&a[0])?;
            let mut lexemes: Vec<Vec<u8>> = Vec::with_capacity(toks.len());
            for t in toks.iter() {
                lexemes.push(token_text(t)?.as_ref().clone());
            }
            let joined = lexemes.join(&b' ');
            let src = std::str::from_utf8(&joined)
                .map_err(|_| fault("@parse: the tokens do not form valid UTF-8"))?;
            check_fragment(src, Fragment::Expr)?;
            Ok(code_value(src))
        }
        // Parse a string as an expression fragment into opaque `@Code`. A syntax
        // error is a `Diagnostic`, which propagates as a fault (fails a `&`).
        // The fragment is carried as its source text, which `@eval` and splicing
        // re-enter the driver's pipeline with.
        Prim::ParseStr => {
            let bytes = as_bytes(&a[0])?;
            let src = std::str::from_utf8(&bytes)
                .map_err(|_| fault(format!("{name}: the argument is not valid UTF-8")))?;
            check_fragment(src, Fragment::Expr)?;
            Ok(code_value(src))
        }
        // Source text as `@Code`, unchecked: its syntax category is decided where
        // it is spliced, which checks it.
        Prim::Code => {
            let bytes = as_bytes(&a[0])?;
            let src = std::str::from_utf8(&bytes)
                .map_err(|_| fault(format!("{name}: the argument is not valid UTF-8")))?;
            Ok(code_value(src))
        }
        Prim::StrLiteral => Ok(Value::Str(Rc::new(str_literal(&as_bytes(&a[0])?).into_bytes()))),
        // The building blocks of a quotation. See Note [Quotation scopes].
        Prim::QuoteOpen => Ok(code_value_marked(b"", &[], fresh_scope())),
        Prim::CodeText => {
            let (mut src, mut marks, scope) = code_parts(&a[0], &name.to_string())?;
            let text = as_bytes(&a[1])?;
            if scope != 0 && !text.is_empty() {
                add_mark(&mut marks, Mark { start: src.len(), end: src.len() + text.len(), scope });
            }
            src.extend_from_slice(&text);
            Ok(code_value_marked(&src, &marks, scope))
        }
        Prim::CodeCat => {
            let (mut src, mut marks, scope) = code_parts(&a[0], &name.to_string())?;
            let (hole, hole_marks, _) = code_parts(&a[1], &name.to_string())?;
            for m in hole_marks {
                add_mark(&mut marks, Mark { start: m.start + src.len(), end: m.end + src.len(), scope: m.scope });
            }
            src.extend_from_slice(&hole);
            Ok(code_value_marked(&src, &marks, scope))
        }
        Prim::Quote => {
            let module = as_bytes(&a[0])?;
            let (src, marks, scope) = code_parts(&a[1], &name.to_string())?;
            if scope != 0 {
                let module = StrId::intern(&String::from_utf8_lossy(&module));
                SCOPES
                    .lock()
                    .expect("the scope table is never poisoned")
                    .get_or_insert_with(std::collections::HashMap::new)
                    .insert(scope, module);
            }
            Ok(code_value_marked(&src, &marks, scope))
        }
        Prim::Format => {
            let bytes = as_bytes(&a[0])?;
            let src = std::str::from_utf8(&bytes)
                .map_err(|_| fault("@format: the argument is not valid UTF-8"))?;
            check_fragment(src, Fragment::Items)?;
            let out = frontend::format::format_source(src, &Default::default())
                .map_err(|m| fault(format!("@format: {m}")))?;
            Ok(Value::Str(Rc::new(out.into_bytes())))
        }
        // Compile and run an `@Code` fragment, embedding its value. Needs the
        // driver's pipeline, so it defers to the installed host (see
        // `crate::machine::set_meta_eval`).
        Prim::Eval => {
            let src = code_src(&a[0], "@eval")?;
            let s = std::str::from_utf8(&src)
                .map_err(|_| fault("@eval: the fragment source is not valid UTF-8"))?;
            let owned = crate::machine::meta_eval(s)?;
            Ok(crate::machine::embed(&owned))
        }
        // Compile-time diagnostics. `@abort` fails the build with the given
        // message (a diagnostic the driver renders); `@emit` prints it and
        // continues.
        Prim::Abort => {
            let msg = as_bytes(&a[0])?;
            Err(Diagnostic::error(
                Code::MetaAbort,
                Span::at(0),
                0,
                String::from_utf8_lossy(&msg).into_owned(),
            ))
        }
        Prim::Emit => {
            let msg = as_bytes(&a[0])?;
            eprintln!("ares: {}", String::from_utf8_lossy(&msg));
            Ok(Value::Unit)
        }
        // A unique identifier string `prefix_m<n>` for hygienic compile-time
        // codegen. The counter is monotonic per process, so names never collide.
        Prim::Fresh => {
            thread_local!(static FRESH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) });
            let n = FRESH.with(|c| {
                let v = c.get();
                c.set(v + 1);
                v
            });
            let mut s = as_bytes(&a[0])?.as_ref().clone();
            s.extend_from_slice(format!("_m{n}").as_bytes());
            Ok(Value::Str(Rc::new(s)))
        }
        // Steer the build at compile time (used as `$ &@link "curl"`): record
        // a library / search path for the driver to add to the link line. Returns
        // empty code, so the directive inserts nothing.
        Prim::Link | Prim::LinkPath => {
            let arg = String::from_utf8_lossy(&as_bytes(&a[0])?).into_owned();
            crate::machine::push_link(name == Prim::LinkPath, arg);
            Ok(code_value(""))
        }
        Prim::ImportPath => {
            let dir = String::from_utf8_lossy(&as_bytes(&a[0])?).into_owned();
            if crate::machine::record_build(|r| r.import_paths.push(dir)) {
                Ok(Value::Unit)
            } else {
                Err(outside_build_script("@import_path"))
            }
        }
        Prim::SetOption => {
            let key = String::from_utf8_lossy(&as_bytes(&a[0])?).into_owned();
            let text = String::from_utf8_lossy(&as_bytes(&a[1])?).into_owned();
            let setting = utilities::config::parse_setting(&key, &text).map_err(|r| {
                let d = Diagnostic::error(Code::BuildScript, Span::at(0), 0, r.msg);
                match r.help {
                    Some(h) => d.with_help(h),
                    None => d,
                }
            })?;
            if crate::machine::record_build(|r| r.options.push(setting)) {
                Ok(Value::Unit)
            } else {
                Err(outside_build_script("@option"))
            }
        }
        // Compile-time reflection over a declared type, resolved by the driver's
        // type host (see `crate::machine::set_type_host`), reachable only inside
        // `$ &`. A derive-style macro reads the shape and generates code for it.
        // `@type_kind` reports `"struct"` / `"union"`; `@type_fields` the struct's
        // field names; `@type_variants` the union's `(tag, arity)` pairs. `@type_of`
        // is a global value's type.
        Prim::TypeKind => {
            let ty = String::from_utf8_lossy(&as_bytes(&a[0])?).into_owned();
            let kind = match crate::machine::type_lookup(&ty)? {
                crate::machine::TypeInfo::Struct(_) => "struct",
                crate::machine::TypeInfo::Union(_) => "union",
                crate::machine::TypeInfo::Value { .. } => "value",
            };
            Ok(Value::Str(Rc::new(kind.as_bytes().to_vec())))
        }
        Prim::TypeParams => {
            let ty = String::from_utf8_lossy(&as_bytes(&a[0])?).into_owned();
            let params: Vec<PVal> = crate::machine::type_lookup(&ty)?
                .params()
                .iter()
                .map(|p| mk(Value::Str(Rc::new(p.bytes().to_vec()))))
                .collect();
            Ok(Value::Vector(Rc::new(params)))
        }
        Prim::TypeFields => {
            let ty = String::from_utf8_lossy(&as_bytes(&a[0])?).into_owned();
            let fields = match crate::machine::type_lookup(&ty)? {
                crate::machine::TypeInfo::Struct(shape) => shape.members,
                _ => return Err(fault(format!("@type_fields: `{ty}` is not a struct"))),
            };
            let items: Vec<PVal> = fields
                .into_iter()
                .map(|(f, _)| mk(Value::Str(Rc::new(f.bytes().to_vec()))))
                .collect();
            Ok(Value::Vector(Rc::new(items)))
        }
        Prim::TypeOf => {
            let name = String::from_utf8_lossy(&as_bytes(&a[0])?).into_owned();
            match crate::machine::type_lookup(&name)? {
                crate::machine::TypeInfo::Value { ty } => Ok(Value::Str(Rc::new(ty.into_bytes()))),
                _ => Err(fault(format!("@type_of: `{name}` is a type, not a value"))),
            }
        }
        // `@lookup` is the matchable form of the reflection above: a CORE `Decl`,
        // `Missing` rather than a fault when nothing is declared under the name.
        Prim::Lookup => {
            let name = String::from_utf8_lossy(&as_bytes(&a[0])?).into_owned();
            let text = |s: &str| -> PVal<'p> { mk(Value::Str(Rc::new(s.as_bytes().to_vec()))) };
            let strs = |xs: &[StrId]| -> PVal<'p> { mk(Value::Vector(Rc::new(xs.iter().map(|x| text(x.as_str())).collect()))) };
            let pairs = |xs: Vec<(StrId, String)>| -> PVal<'p> {
                let items = xs.iter().map(|(n, src)| mk(Value::Tuple(vec![text(n.as_str()), text(src)]))).collect();
                mk(Value::Vector(Rc::new(items)))
            };
            let (tag, fields) = match crate::machine::find_declared(&name)? {
                Some(crate::machine::TypeInfo::Struct(s)) => ("Struct", vec![strs(&s.params), pairs(s.members)]),
                Some(crate::machine::TypeInfo::Union(s)) => {
                    let vs = s.members.into_iter().map(|(tag, _, payload)| (tag, payload)).collect();
                    ("Union", vec![strs(&s.params), pairs(vs)])
                }
                Some(crate::machine::TypeInfo::Value { ty }) => ("Value", vec![text(&ty)]),
                None => ("Missing", vec![]),
            };
            Ok(Value::Variant { ty: "Decl".into(), tag: tag.into(), fields })
        }
        // The `<@Shell>` operations, which the shell performs.
        Prim::ShellQuit | Prim::ShellReset | Prim::ShellList => {
            crate::machine::shell_request(name)?;
            Ok(Value::Unit)
        }
        // The names in scope where the site runs, as `(name, type)` pairs.
        Prim::Scope => {
            let items: Vec<PVal> = crate::machine::meta_scope()?
                .iter()
                .map(|(name, ty)| {
                    mk(Value::Tuple(vec![
                        mk(Value::Str(Rc::new(name.as_bytes().to_vec()))),
                        mk(Value::Str(Rc::new(ty.as_bytes().to_vec()))),
                    ]))
                })
                .collect();
            Ok(Value::Vector(Rc::new(items)))
        }
        // The `&` site running now, as a CORE `Site`.
        Prim::Here => {
            let (file, line, column) = crate::machine::meta_site()?;
            Ok(Value::Struct {
                name: "Site".into(),
                fields: vec![
                    ("file".into(), mk(Value::Str(Rc::new(file.into_bytes())))),
                    ("line".into(), mk(Value::Int(line))),
                    ("column".into(), mk(Value::Int(column))),
                ],
            })
        }
        // Type-check an expression fragment where the site runs; its type, as source.
        Prim::Check => {
            let src = code_src(&a[0], "@check")?;
            let src = std::str::from_utf8(&src).map_err(|_| fault("@check: the fragment is not valid UTF-8"))?;
            Ok(Value::Str(Rc::new(crate::machine::meta_check(src)?.into_bytes())))
        }
        // A CORE `Diag` as an error that fails the build (`@fail`) or a warning
        // the driver shows at the site (`@warn`).
        Prim::Fail | Prim::Warn => {
            let diag = diag_of(&a[0], if name == Prim::Fail { Code::MetaAbort } else { Code::MetaWarning })?;
            if name == Prim::Fail {
                return Err(diag);
            }
            crate::machine::push_meta_warning(diag.with_severity(utilities::Severity::Warning));
            Ok(Value::Unit)
        }
        Prim::TypeVariants => {
            let ty = String::from_utf8_lossy(&as_bytes(&a[0])?).into_owned();
            let variants = match crate::machine::type_lookup(&ty)? {
                crate::machine::TypeInfo::Union(shape) => shape.members,
                _ => return Err(fault(format!("@type_variants: `{ty}` is not a union"))),
            };
            let items: Vec<PVal> = variants
                .into_iter()
                .map(|(tag, arity, _)| {
                    mk(Value::Tuple(vec![
                        mk(Value::Str(Rc::new(tag.bytes().to_vec()))),
                        mk(Value::Int(arity as i64)),
                    ]))
                })
                .collect();
            Ok(Value::Vector(Rc::new(items)))
        }
        _ => Err(fault(format!("unknown built-in `{name}`"))),
    }
}

/// Tokenize `src` as `@lex` does: a vector of CORE `Token` variants, the tag the
/// token's kind (what `@token_kind` reports) and the one field its lexeme.
pub fn lex(src: &str) -> Result<crate::machine::OwnedValue> {
    use crate::machine::OwnedValue;
    let mut lx = frontend::Lexer::new(src);
    let mut toks = Vec::new();
    loop {
        let t = lx.next_token()?;
        if matches!(t.kind, frontend::Kind::Eof) {
            break;
        }
        toks.push(OwnedValue::Variant {
            ty: "Token".into(),
            tag: token_kind_name(t.kind).into(),
            fields: vec![OwnedValue::Str(src.as_bytes()[t.span.start..t.span.end].to_vec())],
        });
    }
    Ok(OwnedValue::Vector(toks))
}

/// What a source fragment must parse as, decided by where `@Code` is spliced:
/// an expression, top-level items, a type, the entries of an effect row, a
/// pattern, or a struct's fields or a union's variants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fragment {
    Expr,
    Items,
    Type,
    Row,
    Pattern,
    /// A struct's fields, for a `&` entry in a `@struct` declaration.
    Fields,
    /// A union's variants, for a `&` entry in a `@union` declaration.
    Variants,
}

/// Check that `src` parses as `kind`, by wrapping it in a scratch module. A
/// syntax error carries `src` as its fragment, so the driver can point into the
/// string the author wrote rather than the wrapper.
pub fn check_fragment(src: &str, kind: Fragment) -> Result<()> {
    parse_fragment(src, kind).map(|_| ())
}

/// Parse `src` as `kind`, wrapped as [`wrap_fragment`] wraps it, with where
/// `src` starts in the wrapper.
pub fn parse_fragment(src: &str, kind: Fragment) -> Result<(frontend::Parsed, usize)> {
    let prefix = fragment_wrapper(kind).0.len();
    frontend::parse(&wrap_fragment(src, kind))
        .map(|p| (p, prefix))
        .map_err(|d| d.with_fragment(src, prefix))
}

/// `src` wrapped in a scratch module where it stands as `kind`.
pub fn wrap_fragment(src: &str, kind: Fragment) -> String {
    let (prefix, suffix) = fragment_wrapper(kind);
    format!("{prefix}{src}{suffix}")
}

fn fragment_wrapper(kind: Fragment) -> (&'static str, &'static str) {
    match kind {
        Fragment::Expr => ("$ @mod _META\n$ _e =\n", ""),
        Fragment::Items => ("$ @mod _META\n", ""),
        Fragment::Type => ("$ @mod _META\n$ _t :\n", "\n= {}"),
        Fragment::Row => ("$ @mod _META\n$ _r : {} -> <\n", "\n> {} = {}"),
        Fragment::Pattern => ("$ @mod _META\n$ _p = \\ | ", "\n=> {}"),
        Fragment::Fields => ("$ @mod _META\n$ Fields : @struct =\n", ""),
        Fragment::Variants => ("$ @mod _META\n$ Variants : @union =\n", ""),
    }
}

/// The source literal spelling the string `bytes`, escaped so it lexes back
/// to the same bytes.
pub fn str_literal(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'?' => out.push_str("\\?"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('"');
    out
}

// Note [Quotation scopes]
// ~~~~~~~~~~~~~~~~~~~~~~~~
// A generator's quotation spells names the generator's module defines, and
// binders its own code alone may see, but the code lands in another module
// where those names may mean something else. So an `@Code` remembers which of
// its bytes which quotation wrote: `marks` lists the runs, each in a scope.
//
// Each evaluation of a quotation opens a fresh scope (`@quote_open`), its text
// pieces are marked with it (`@code_text`), and its holes keep the marks they
// carry (`@code_cat`). Lowering wraps a quotation a module's function writes in
// `@quote "MODULE"`, which records the scope's module in `SCOPES`. A quotation
// in a `&` operand is written where its code lands, so lowering leaves its
// scope unrecorded, and an unrecorded scope is no scope: its text reads as if
// written at the site, as `@code` text does.
//
// The driver reads the recorded runs back (`quoted_runs`) and resolves the
// names they spell where they were written, before the code is spliced.

/// The module each recorded quotation scope was written in.
static SCOPES: std::sync::Mutex<Option<std::collections::HashMap<u64, StrId>>> = std::sync::Mutex::new(None);

static NEXT_SCOPE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn fresh_scope() -> u64 {
    NEXT_SCOPE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A run of an `@Code`'s text that one quotation wrote: bytes `start..end`,
/// in quotation scope `scope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub start: usize,
    pub end: usize,
    pub scope: u64,
}

/// A run of an `@Code`'s text a module's quotation wrote: [`Mark`] with the
/// module its scope was recorded in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quoted {
    pub start: usize,
    pub end: usize,
    pub scope: u64,
    pub module: StrId,
}

/// The runs of `v`'s text whose scope was recorded, empty for anything but an
/// `@Code`.
pub fn quoted_runs(v: &PVal) -> Vec<Quoted> {
    let Ok((_, marks, _)) = code_parts(v, "@Code") else { return Vec::new() };
    let table = SCOPES.lock().expect("the scope table is never poisoned");
    let Some(table) = table.as_ref() else { return Vec::new() };
    marks
        .into_iter()
        .filter_map(|m| table.get(&m.scope).map(|&module| Quoted { start: m.start, end: m.end, scope: m.scope, module }))
        .collect()
}

/// Append `m` to `marks`, merging it into the last run it continues.
fn add_mark(marks: &mut Vec<Mark>, m: Mark) {
    match marks.last_mut() {
        Some(last) if last.end == m.start && last.scope == m.scope => last.end = m.end,
        _ => marks.push(m),
    }
}

/// An opaque `@Code` value carrying `src`, which no quotation wrote.
fn code_value<'p>(src: &str) -> Value<'p> {
    code_value_marked(src.as_bytes(), &[], 0)
}

/// An `@Code` value: its text, the runs quotations wrote, and the scope the
/// quotation building it marks its text with (0 for none).
fn code_value_marked<'p>(src: &[u8], marks: &[Mark], scope: u64) -> Value<'p> {
    let mut packed = Vec::with_capacity(marks.len() * 24);
    for m in marks {
        packed.extend_from_slice(&(m.start as u64).to_le_bytes());
        packed.extend_from_slice(&(m.end as u64).to_le_bytes());
        packed.extend_from_slice(&m.scope.to_le_bytes());
    }
    Value::Struct {
        name: "@Code".into(),
        fields: vec![
            ("src".into(), mk(Value::Str(Rc::new(src.to_vec())))),
            ("marks".into(), mk(Value::Str(Rc::new(packed)))),
            ("scope".into(), mk(Value::Int(scope as i64))),
        ],
    }
}

/// The parts of an `@Code` value, as [`code_value_marked`] takes them.
fn code_parts(v: &PVal, builtin: &str) -> Result<(Vec<u8>, Vec<Mark>, u64)> {
    let src = code_src(v, builtin)?.as_ref().clone();
    let Value::Struct { fields, .. } = &*v.borrow() else { unreachable!("`code_src` read an `@Code`") };
    let field = |k: &str| fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let marks = match field("marks") {
        Some(v) => match &*v.borrow() {
            Value::Str(b) => b
                .chunks_exact(24)
                .map(|c| {
                    let word = |i: usize| u64::from_le_bytes(c[i * 8..i * 8 + 8].try_into().expect("eight bytes"));
                    Mark { start: word(0) as usize, end: word(1) as usize, scope: word(2) }
                })
                .collect(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    };
    let scope = match field("scope") {
        Some(v) => match &*v.borrow() {
            Value::Int(n) => *n as u64,
            _ => 0,
        },
        None => 0,
    };
    Ok((src, marks, scope))
}

/// The tag name reported by `@token_kind` for a lexical [`frontend::Kind`]. The
/// value-carrying literals collapse to a plain tag (`Int(5)` -> "Int"); the
/// lexeme itself is available via `@token_text`.
fn token_kind_name(kind: frontend::Kind) -> &'static str {
    use frontend::Kind::*;
    match kind {
        Int(_) => "Int",
        Real(_) => "Real",
        Imaginary(_) => "Imaginary",
        Str => "Str",
        Word => "Word",
        At => "At",
        Op => "Op",
        Eq => "Eq",
        FatArrow => "FatArrow",
        Arrow => "Arrow",
        Lambda => "Lambda",
        Colon => "Colon",
        Dollar => "Dollar",
        Comma => "Comma",
        Dot => "Dot",
        Ellipsis => "Ellipsis",
        LParen => "LParen",
        RParen => "RParen",
        LBrace => "LBrace",
        RBrace => "RBrace",
        LBrack => "LBrack",
        RBrack => "RBrack",
        Let => "Let",
        In => "In",
        If => "If",
        Else => "Else",
        Do => "Do",
        Ctl => "Ctl",
        Comment => "Comment",
        Eof => "Eof",
    }
}

/// The lexeme a `Token` carries (`@token_text`).
fn token_text(v: &PVal) -> Result<Rc<Vec<u8>>> {
    match &*v.borrow() {
        Value::Variant { fields, .. } if fields.len() == 1 => match &*fields[0].borrow() {
            Value::Str(b) => Ok(b.clone()),
            _ => Err(fault("a Token's lexeme is not a string")),
        },
        _ => Err(fault("expected a Token")),
    }
}

/// A CORE `Diag` (`message`, `notes`, `helps`) as a diagnostic with `code`, its
/// location left for the driver to fill with the site.
fn diag_of(v: &PVal, code: Code) -> Result<Diagnostic> {
    let field = |f: &str| -> Result<PVal> {
        match &*v.borrow() {
            Value::Struct { name, fields } if name == "Diag" => fields
                .iter()
                .find(|(k, _)| k == f)
                .map(|(_, v)| v.clone())
                .ok_or_else(|| fault(format!("Diag has no `{f}` field"))),
            _ => Err(fault("expected a Diag")),
        }
    };
    let text = |v: &PVal| -> Result<String> { Ok(String::from_utf8_lossy(&as_bytes(v)?).into_owned()) };
    let mut d = Diagnostic::error(code, Span::at(0), 0, text(&field("message")?)?);
    for n in as_vec(&field("notes")?)?.iter() {
        d = d.with_note(text(n)?);
    }
    for h in as_vec(&field("helps")?)?.iter() {
        d = d.with_help(text(h)?);
    }
    Ok(d)
}

/// The source text of an `@Code` value.
fn code_src(v: &PVal, builtin: &str) -> Result<Rc<Vec<u8>>> {
    match &*v.borrow() {
        Value::Struct { name, fields } if name == "@Code" => fields
            .iter()
            .find(|(k, _)| k == "src")
            .and_then(|(_, v)| match &*v.borrow() {
                Value::Str(b) => Some(b.clone()),
                _ => None,
            })
            .ok_or_else(|| fault(format!("{builtin}: malformed @Code value"))),
        _ => Err(fault(format!("{builtin} expects an @Code"))),
    }
}

/// A monomorphic arithmetic intrinsic (`@iadd`, `@fmul`, ...). Each assumes its
/// operands already carry the right runtime tag (the operator instances that
/// call these are type-directed), so an `@i*` op reads two integers and an `@f*`
/// op two reals. Integer add/sub/mul wrap on overflow, matching the C backend.
/// The `@f32*` family rounds both operands and the result to single precision and
/// yields a [`Value::Real32`], so `@Float32` arithmetic is genuinely f32.
fn arith_intrinsic<'p>(name: Prim, x: &PVal<'p>, y: &PVal<'p>) -> Result<Value<'p>> {
    match name {
        Prim::Iadd => Ok(Value::Int(as_int(x)?.wrapping_add(as_int(y)?))),
        Prim::Isub => Ok(Value::Int(as_int(x)?.wrapping_sub(as_int(y)?))),
        Prim::Imul => Ok(Value::Int(as_int(x)?.wrapping_mul(as_int(y)?))),
        Prim::Idiv | Prim::Imod if as_int(y)? == 0 => Err(fault("division by zero")),
        Prim::Idiv => Ok(Value::Int(as_int(x)? / as_int(y)?)),
        Prim::Imod => Ok(Value::Int(as_int(x)? % as_int(y)?)),
        // `@u*` read the same i64 bits as unsigned: for a `Nat` whose value
        // exceeds `i64::MAX`, signed division would give the wrong result.
        Prim::Udiv | Prim::Umod if as_int(y)? == 0 => Err(fault("division by zero")),
        Prim::Udiv => Ok(Value::Int(((as_int(x)? as u64) / (as_int(y)? as u64)) as i64)),
        Prim::Umod => Ok(Value::Int(((as_int(x)? as u64) % (as_int(y)? as u64)) as i64)),
        Prim::Fadd => Ok(Value::Real(as_f64(x)? + as_f64(y)?)),
        Prim::Fsub => Ok(Value::Real(as_f64(x)? - as_f64(y)?)),
        Prim::Fmul => Ok(Value::Real(as_f64(x)? * as_f64(y)?)),
        Prim::Fdiv => Ok(Value::Real(as_f64(x)? / as_f64(y)?)),
        Prim::Fmod => Ok(Value::Real(as_f64(x)? % as_f64(y)?)),
        Prim::Fpow => Ok(Value::Real(as_f64(x)?.powf(as_f64(y)?))),
        Prim::F32add => Ok(Value::Real32(as_f32(x)? + as_f32(y)?)),
        Prim::F32sub => Ok(Value::Real32(as_f32(x)? - as_f32(y)?)),
        Prim::F32mul => Ok(Value::Real32(as_f32(x)? * as_f32(y)?)),
        Prim::F32div => Ok(Value::Real32(as_f32(x)? / as_f32(y)?)),
        Prim::F32mod => Ok(Value::Real32(as_f32(x)? % as_f32(y)?)),
        Prim::F32pow => Ok(Value::Real32(as_f32(x)?.powf(as_f32(y)?))),
        _ => unreachable!("arith_intrinsic called with `{name}`"),
    }
}

/// A bitwise intrinsic on the 64-bit value representation. A shift count is read
/// unsigned, and one of 64 or more (a negative count included) shifts every bit
/// out: `@ishl`/`@ushr` give 0, `@ishr` the sign. This matches the C runtime,
/// where the bare operators would be undefined.
fn bit_intrinsic<'p>(name: Prim, x: i64, y: i64) -> Result<Value<'p>> {
    let n = y as u64;
    Ok(Value::Int(match name {
        Prim::Iand => x & y,
        Prim::Ior => x | y,
        Prim::Ixor => x ^ y,
        Prim::Ishl if n >= 64 => 0,
        Prim::Ishl => ((x as u64) << n) as i64,
        Prim::Ishr => x >> n.min(63),
        Prim::Ushr if n >= 64 => 0,
        Prim::Ushr => ((x as u64) >> n) as i64,
        _ => unreachable!("bit_intrinsic called with `{name}`"),
    }))
}

/// A monomorphic comparison intrinsic, the primitive the comparison instances are
/// built on. `@u*` reads the same i64 bits as unsigned, so a `Nat` past
/// `i64::MAX` orders correctly.
fn compare_intrinsic<'p>(name: Prim, x: &PVal<'p>, y: &PVal<'p>) -> Result<Value<'p>> {
    let r = match name {
        Prim::Ieq => as_int(x)? == as_int(y)?,
        Prim::Ilt => as_int(x)? < as_int(y)?,
        Prim::Ult => (as_int(x)? as u64) < (as_int(y)? as u64),
        Prim::Feq => as_f64(x)? == as_f64(y)?,
        Prim::Flt => as_f64(x)? < as_f64(y)?,
        Prim::Seq => as_bytes(x)? == as_bytes(y)?,
        Prim::Slt => as_bytes(x)? < as_bytes(y)?,
        _ => unreachable!("compare_intrinsic called with `{name}`"),
    };
    Ok(Value::Bool(r))
}

fn compare<'p>(op: Prim, x: &PVal<'p>, y: &PVal<'p>) -> Result<Value<'p>> {
    use std::cmp::Ordering;
    let ord = {
        let (bx, by) = (x.borrow(), y.borrow());
        match (&*bx, &*by) {
            (Value::Str(a), Value::Str(b)) => a.cmp(b),
            _ => {
                drop(bx);
                drop(by);
                as_f64(x)?
                    .partial_cmp(&as_f64(y)?)
                    .ok_or_else(|| fault("comparison of incomparable values"))?
            }
        }
    };
    let r = match op {
        Prim::Lt => ord == Ordering::Less,
        Prim::Gt => ord == Ordering::Greater,
        Prim::Le => ord != Ordering::Greater,
        Prim::Ge => ord != Ordering::Less,
        _ => unreachable!("compare called with a non-comparison operator"),
    };
    Ok(Value::Bool(r))
}

fn concat<'p>(x: &PVal<'p>, y: &PVal<'p>) -> Result<Value<'p>> {
    let strs = {
        let (bx, by) = (x.borrow(), y.borrow());
        match (&*bx, &*by) {
            (Value::Str(a), Value::Str(b)) => {
                let mut bytes = a.as_ref().clone();
                bytes.extend_from_slice(b);
                Some(bytes)
            }
            _ => None,
        }
    };
    if let Some(bytes) = strs {
        return Ok(Value::Str(Rc::new(bytes)));
    }
    Err(fault("`++` on unsupported operands"))
}


/// Structural equality (`==`). Numbers compare across Int/Real; functions are not
/// comparable.
pub(crate) fn value_eq(x: &PVal, y: &PVal) -> bool {
    let bx = x.borrow();
    let by = y.borrow();
    match (&*bx, &*by) {
        (
            Value::Int(_) | Value::Real(_) | Value::Real32(_),
            Value::Int(_) | Value::Real(_) | Value::Real32(_),
        ) => as_f64(x).ok() == as_f64(y).ok(),
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Unit, Value::Unit) => true,
        (Value::Tuple(a), Value::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(p, q)| value_eq(p, q))
        }
        (Value::Vector(a), Value::Vector(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(p, q)| value_eq(p, q))
        }
        (Value::Tensor(a), Value::Tensor(b)) => {
            a.shape == b.shape
                && match (gather(a), gather(b)) {
                    (Ok(xs), Ok(ys)) => xs.iter().zip(&ys).all(|(p, q)| value_eq(p, q)),
                    _ => false,
                }
        }
        (Value::Struct { fields: a, .. }, Value::Struct { fields: b, .. }) => {
            a.len() == b.len()
                && a.iter().all(|(n, v)| {
                    b.iter()
                        .find(|(m, _)| m == n)
                        .is_some_and(|(_, w)| value_eq(v, w))
                })
        }
        (
            Value::Variant {
                tag: ta,
                fields: fa,
                ..
            },
            Value::Variant {
                tag: tb,
                fields: fb,
                ..
            },
        ) => ta == tb && fa.len() == fb.len() && fa.iter().zip(fb).all(|(p, q)| value_eq(p, q)),
        _ => false,
    }
}

// -- foreign function interface (fast-path host table) ----------------------

// The interpreter serves the common `@extern` calls (the C/libm namespace, the
// port of `engines/FF.cpp`) directly from this compiled-in table: a symbol here
// is a linked call with a known signature, no resolution or libffi overhead. A
// symbol OUTSIDE the table falls through to [`crate::machine::ffi`], which
// resolves it with `dlopen`/`dlsym` and calls it through libffi, so `ares run`
// reaches the same arbitrary library the C backend links. Everything is a
// machine word: a `Ptr` (and a `FILE*`/allocation) travels as an `Int`.

// Only `fflush` is called directly (to order C stdio against the driver's own
// output); every other foreign symbol resolves at runtime through libffi below.
#[cfg(not(target_arch = "wasm32"))]
extern "C" {
    fn fflush(stream: *mut std::os::raw::c_void) -> std::os::raw::c_int;
}

/// Call a foreign C function: resolve it at runtime (dlopen/dlsym) and invoke it
/// through libffi (see [`crate::machine::ffi`]). The `C` namespace in
/// `library/C.ares` reaches real libc/libm and the linked-in `aresstd` this way,
/// exactly as the compiled backend links them, so both engines agree.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_extern<'p>(
    abi: StrId,
    symbol: StrId,
    lib: StrId,
    arg_types: &[StrId],
    ret_type: StrId,
    args: &[PVal<'p>],
) -> Result<Value<'p>> {
    if abi == "WASM" {
        return Err(fault(format!(
            "FFI: `{symbol}` is an `@extern \"WASM\"` host import, available only in \
             the wasm playground, not in a native run"
        )));
    }
    if abi == "load" {
        return crate::machine::ffi::load(ret_type, args);
    }
    if writes_terminal(symbol.as_str(), args) {
        utilities::progress::seal();
    }
    let v = crate::machine::ffi::call_extern(symbol, lib, arg_types, ret_type, args)?;
    // Keep foreign stdout (C stdio) ordered against the driver's own output.
    unsafe { fflush(std::ptr::null_mut()) };
    Ok(v)
}

/// Whether a foreign call prints to the console, where the compiler's progress
/// line may be open. Compile-time code reaches the terminal only through `C`,
/// and `IO` writes with `write` on fd 1 or 2.
#[cfg(not(target_arch = "wasm32"))]
fn writes_terminal(symbol: &str, args: &[PVal]) -> bool {
    match symbol {
        "write" => args.first().is_some_and(|fd| matches!(&*fd.borrow(), Value::Int(1 | 2))),
        "puts" | "printf" | "putchar" | "perror" => true,
        _ => false,
    }
}

// On wasm there is no libc: `@extern "WASM"` resolves against the JavaScript
// embedder instead of a C library. The marshalling and the generic host-call
// bridge live in [`crate::machine::ffi`] (the `WasmHostFfi` backend); this is
// just the abi guard and the hand-off.
#[cfg(target_arch = "wasm32")]
pub(crate) fn run_extern<'p>(
    abi: StrId,
    symbol: StrId,
    lib: StrId,
    arg_types: &[StrId],
    ret_type: StrId,
    args: &[PVal<'p>],
) -> Result<Value<'p>> {
    if abi != "WASM" {
        return Err(fault(format!(
            "FFI: `{symbol}` is an `@extern \"{abi}\"` foreign call; there is no libc \
             in the browser, only `@extern \"WASM\"` host imports"
        )));
    }
    crate::machine::ffi::call_extern(symbol, lib, arg_types, ret_type, args)
}

impl<'p> Value<'p> {
    /// A shallow structural copy: aggregates share their `PVal` children (an `Rc`
    /// bump), so this is cheap. Used to move a value out of a borrowed cell.
    pub(crate) fn clone_shallow(&self) -> Value<'p> {
        match self {
            Value::Unk => Value::Unk,
            Value::Int(n) => Value::Int(*n),
            Value::Real(r) => Value::Real(*r),
            Value::Real32(r) => Value::Real32(*r),
            Value::Str(b) => Value::Str(b.clone()),
            Value::Bool(b) => Value::Bool(*b),
            Value::Unit => Value::Unit,
            Value::Tuple(items) => Value::Tuple(items.clone()),
            Value::Struct { name, fields } => Value::Struct {
                name: *name,
                fields: fields.clone(),
            },
            Value::Variant { ty, tag, fields } => Value::Variant {
                ty: *ty,
                tag: *tag,
                fields: fields.clone(),
            },
            Value::Vector(items) => Value::Vector(items.clone()),
            Value::Tensor(t) => Value::Tensor(t.clone()),
            Value::Builtin { op, args } => Value::Builtin {
                op,
                args: args.clone(),
            },
            Value::Extern {
                abi,
                symbol,
                lib,
                arg_types,
                ret_type,
                args,
            } => Value::Extern {
                abi: *abi,
                symbol: *symbol,
                lib: *lib,
                arg_types: arg_types.clone(),
                ret_type: *ret_type,
                args: args.clone(),
            },
            Value::Code { code, env } => Value::Code {
                code: *code,
                env: env.clone(),
            },
            Value::Rec(w) => Value::Rec(w.clone()),
            Value::Op { effect, op } => Value::Op {
                effect: *effect,
                op: *op,
            },
            Value::Resump(r) => Value::Resump(r.clone()),
            Value::Thunk(f) => Value::Thunk(f.clone()),
            Value::Future(t) => Value::Future(t.clone()),
            Value::Chan(c) => Value::Chan(c.clone()),
            Value::TVar(t) => Value::TVar(t.clone()),
        }
    }

    /// A short human display of a value, matching the C++ tree-walker's
    /// `Value::show` byte-for-byte (so the two can be diffed).
    pub fn show(&self) -> String {
        match self {
            Value::Unk => "{}".into(),
            Value::Int(n) => n.to_string(),
            Value::Real(r) => r.to_string(),
            Value::Real32(r) => r.to_string(),
            Value::Str(b) => format!("{:?}", String::from_utf8_lossy(b)),
            Value::Bool(b) => b.to_string(),
            Value::Unit => "{}".into(),
            Value::Tuple(items) => {
                let inner: Vec<String> = items.iter().map(|v| v.borrow().show()).collect();
                format!("{{{}}}", inner.join(", "))
            }
            Value::Struct { name, fields } => {
                // An `@Code` shows as its text: its marks are bookkeeping.
                let inner: Vec<String> = fields
                    .iter()
                    .filter(|(n, _)| name != "@Code" || n == "src")
                    .map(|(n, v)| format!(".{n} = {}", v.borrow().show()))
                    .collect();
                format!("{name}.{{ {} }}", inner.join(", "))
            }
            Value::Variant { tag, fields, .. } => {
                if fields.is_empty() {
                    format!(".{tag}")
                } else {
                    let inner: Vec<String> = fields.iter().map(|v| v.borrow().show()).collect();
                    format!(".{tag}.{{ {} }}", inner.join(", "))
                }
            }
            Value::Vector(items) => {
                let inner: Vec<String> = items.iter().map(|v| v.borrow().show()).collect();
                format!("vec[{}]", inner.join(", "))
            }
            Value::Tensor(t) => {
                let mut out = String::new();
                show_tensor(t, 0, t.off, &mut out);
                out
            }
            Value::Builtin { .. }
            | Value::Extern { .. }
            | Value::Code { .. }
            | Value::Op { .. } => "<function>".into(),
            Value::Resump(_) => "<continuation>".into(),
            Value::Thunk(_) => "...".into(),
            Value::Rec(_) => "<recursive>".into(),
            Value::Future(t) => match &*t.borrow() {
                Task::Done(v) => v.borrow().show(),
                _ => "<future>".into(),
            },
            Value::Chan(_) => "<channel>".into(),
            Value::TVar(_) => "<tvar>".into(),
        }
    }
}
