//! Runtime values for the reified-K abstract machine: the port of
//! `engines/ITxDATA.hpp`. Unlike the tree-walker's [`crate::eval::data::Value`]
//! (which owns its children directly), every machine value is shared through a
//! `PVal` cell, because the machine mutates a value in place when a recursive
//! `let` is back-patched (see the `KRet` handling in [`crate::machine`]): a
//! closure that captured the placeholder weakly must observe the finished value.
//!
//! The lifetime `'p` is the borrow of the [`Program`](frontend::ir::Program): a
//! captured continuation ([`Resumption`]) holds `&'p` pointers into the code, and
//! a resumption can be stored in a value and resumed later, so the value type
//! carries the program's lifetime.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use utilities::{Code, Diagnostic, Result, Span};

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
    /// Byte string, also the representation of an `Array`.
    Str(Rc<Vec<u8>>),
    Bool(bool),
    Unit,
    Tuple(Vec<PVal<'p>>),
    Struct {
        name: String,
        fields: Vec<(String, PVal<'p>)>,
    },
    Variant {
        ty: String,
        tag: String,
        fields: Vec<PVal<'p>>,
    },
    Vector(Rc<Vec<PVal<'p>>>),
    /// A partially applied built-in; it runs once `args` reaches `arity`.
    Builtin {
        name: Rc<str>,
        arity: usize,
        args: Vec<PVal<'p>>,
    },
    /// A partially applied foreign function (`@extern`); it marshals its
    /// arguments across the seam selected by `abi` (`"C"` = a C library symbol,
    /// `"WASM"` = a host import) and calls `symbol` once `args` reaches the arity
    /// `arg_types.len()`. `ret_type` selects how the result is wrapped. `lib` is
    /// the symbolic library the symbol lives in, resolved to a soname and
    /// `dlopen`ed when the symbol is not in the compiled-in host table.
    Extern {
        abi: Rc<str>,
        symbol: Rc<str>,
        lib: Rc<str>,
        arg_types: Rc<[String]>,
        ret_type: Rc<str>,
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
        effect: Option<String>,
        op: String,
    },
    /// A captured one-shot continuation (a delimited stack slice).
    Resump(Rc<RefCell<Resumption<'p>>>),
}

/// A captured continuation: the `KFrame` slice from a prompt up to a perform
/// point. Affine: `used` guards against resuming twice.
pub struct Resumption<'p> {
    pub(crate) seg: Vec<KFrame<'p>>,
    pub(crate) used: bool,
}

pub(crate) fn fault(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error(Code::RuntimeFault, Span::at(0), 0, msg.into())
}

// -- coercions --------------------------------------------------------------

fn as_f64(v: &PVal) -> Result<f64> {
    match &*v.borrow() {
        Value::Int(n) => Ok(*n as f64),
        Value::Real(r) => Ok(*r),
        _ => Err(fault("expected a number")),
    }
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
//
// A sized tensor `[n]T` is a `@tensor`-named struct over a FLAT buffer plus a
// shape/strides descriptor: `{ buf: Vec, off: Int, shape: Vec Int, strides: Vec
// Int }`. Reusing `Value::Struct` means the whole rep gets construction,
// refcounting, and destruction for free; only the tensor OPERATIONS are bespoke.
// A view (a row, a transpose, a slice) shares `buf` (an `Rc`) and only changes
// `off`/`shape`/`strides`, so those are O(1) and copy nothing.

const TENSOR: &str = "@tensor";

fn int_vec_val<'p>(xs: &[usize]) -> Value<'p> {
    Value::Vector(Rc::new(xs.iter().map(|&x| mk(Value::Int(x as i64))).collect()))
}

pub(crate) fn mk_tensor<'p>(
    buf: Rc<Vec<PVal<'p>>>,
    off: usize,
    shape: Vec<usize>,
    strides: Vec<usize>,
) -> Value<'p> {
    Value::Struct {
        name: TENSOR.to_string(),
        fields: vec![
            ("buf".to_string(), mk(Value::Vector(buf))),
            ("off".to_string(), mk(Value::Int(off as i64))),
            ("shape".to_string(), mk(int_vec_val(&shape))),
            ("strides".to_string(), mk(int_vec_val(&strides))),
        ],
    }
}

fn usize_vec(v: &PVal) -> Result<Vec<usize>> {
    as_vec(v)?.iter().map(as_index).collect()
}

pub(crate) fn is_tensor(v: &PVal) -> bool {
    matches!(&*v.borrow(), Value::Struct { name, .. } if name == TENSOR)
}

pub(crate) fn tensor_fields<'p>(
    v: &PVal<'p>,
) -> Result<(Rc<Vec<PVal<'p>>>, usize, Vec<usize>, Vec<usize>)> {
    let (buf, off, shape, strides) = {
        let b = v.borrow();
        match &*b {
            Value::Struct { name, fields } if name == TENSOR => {
                let g = |n: &str| fields.iter().find(|(f, _)| f == n).map(|(_, x)| x.clone());
                (g("buf"), g("off"), g("shape"), g("strides"))
            }
            _ => return Err(fault("expected a tensor")),
        }
    };
    let want = |o: Option<PVal<'p>>| o.ok_or_else(|| fault("malformed tensor"));
    Ok((
        as_vec(&want(buf)?)?,
        as_index(&want(off)?)?,
        usize_vec(&want(shape)?)?,
        usize_vec(&want(strides)?)?,
    ))
}

/// Row-major (C-order) strides for a shape.
fn row_major_strides(shape: &[usize]) -> Vec<usize> {
    let mut s = vec![1usize; shape.len()];
    for i in (0..shape.len().saturating_sub(1)).rev() {
        s[i] = s[i + 1] * shape[i + 1];
    }
    s
}

/// The tensor's scalar elements in row-major logical order (following a view's
/// offset/strides). A contiguous tensor yields its buffer; a view is gathered.
pub(crate) fn materialize<'p>(v: &PVal<'p>) -> Result<Vec<PVal<'p>>> {
    let (buf, off, shape, strides) = tensor_fields(v)?;
    let total: usize = shape.iter().product();
    let rank = shape.len();
    let mut out = Vec::with_capacity(total);
    let mut idx = vec![0usize; rank];
    for _ in 0..total {
        let flat = off + (0..rank).map(|k| idx[k] * strides[k]).sum::<usize>();
        out.push(
            buf.get(flat)
                .cloned()
                .ok_or_else(|| fault("tensor buffer out of range"))?,
        );
        for k in (0..rank).rev() {
            idx[k] += 1;
            if idx[k] < shape[k] {
                break;
            }
            idx[k] = 0;
        }
    }
    Ok(out)
}

/// Build a tensor by stacking `elems` along a new leading axis: scalar elements
/// give a rank-1 tensor; tensor elements are flattened (equal shapes required)
/// and get the outer dimension prepended. This is the construction/`create` core.
pub(crate) fn tensor_stack<'p>(elems: &[PVal<'p>]) -> Result<Value<'p>> {
    let n = elems.len();
    if n == 0 {
        return Ok(mk_tensor(Rc::new(Vec::new()), 0, vec![0], vec![1]));
    }
    if is_tensor(&elems[0]) {
        let (_, _, sub_shape, _) = tensor_fields(&elems[0])?;
        let mut buf = Vec::new();
        for e in elems {
            let (_, _, es, _) = tensor_fields(e)?;
            if es != sub_shape {
                return Err(fault("stacking tensors of unequal shape"));
            }
            buf.extend(materialize(e)?);
        }
        let mut shape = vec![n];
        shape.extend(sub_shape);
        let strides = row_major_strides(&shape);
        Ok(mk_tensor(Rc::new(buf), 0, shape, strides))
    } else {
        Ok(mk_tensor(Rc::new(elems.to_vec()), 0, vec![n], vec![1]))
    }
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

// -- built-ins --------------------------------------------------------------

/// The arity of a built-in operator, or `None` if the name is not a built-in.
pub(crate) fn builtin_arity(name: &str) -> Option<usize> {
    let n = match name {
        "not" | "neg" | "@array_len" | "@array_alloc" | "@vec_len" | "@vec_new"
        | "@tensor_length" | "@tensor_stack" | "@tensor_transpose" => 1,
        "+" | "-" | "*" | "/" | "%" | "?=" | "?<" | "?>" | "<=" | ">=" | "++" | "@array_get"
        | "@array_push" | "@vec_get" | "@vec_push" | "@vec_fill" | "record_without"
        | "@tensor_concat" | "@tensor_index" | "@tensor_create" => 2,
        "@tensor_slice" => 3,
        "@array_set" | "@array_slice" | "@vec_set" => 3,
        _ => return None,
    };
    Some(n)
}

/// Run a saturated built-in. `a` is already deref'd (the machine derefs an
/// argument before pushing it onto a builtin's operand list).
pub(crate) fn run_builtin<'p>(name: &str, a: &[PVal<'p>]) -> Result<Value<'p>> {
    match name {
        "+" | "-" | "*" | "/" | "%" => arith(name, &a[0], &a[1]),
        "neg" => match &*a[0].borrow() {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Real(r) => Ok(Value::Real(-r)),
            _ => Err(fault("`neg` on a non-number")),
        },
        "not" => match &*a[0].borrow() {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            _ => Err(fault("`not` on a non-boolean")),
        },
        "?=" => Ok(Value::Bool(value_eq(&a[0], &a[1]))),
        "?<" | "?>" | "<=" | ">=" => compare(name, &a[0], &a[1]),
        "++" => concat(&a[0], &a[1]),
        "@array_alloc" => Ok(Value::Str(Rc::new(vec![0u8; as_len(&a[0])?]))),
        "@array_len" => Ok(Value::Int(as_bytes(&a[0])?.len() as i64)),
        "@array_get" => {
            let bytes = as_bytes(&a[0])?;
            let i = as_index(&a[1])?;
            bytes
                .get(i)
                .map(|b| Value::Int(*b as i64))
                .ok_or_else(|| fault("array index out of bounds"))
        }
        "@array_push" => {
            let mut bytes = as_bytes(&a[0])?.as_ref().clone();
            bytes.push(as_byte(&a[1])?);
            Ok(Value::Str(Rc::new(bytes)))
        }
        "@array_set" => {
            let mut bytes = as_bytes(&a[0])?.as_ref().clone();
            let i = as_index(&a[1])?;
            if i >= bytes.len() {
                return Err(fault("array index out of bounds"));
            }
            bytes[i] = as_byte(&a[2])?;
            Ok(Value::Str(Rc::new(bytes)))
        }
        "@array_slice" => {
            let bytes = as_bytes(&a[0])?;
            let mut beg = as_index(&a[1])?;
            let mut end = as_index(&a[2])?;
            beg = beg.min(bytes.len());
            end = end.clamp(beg, bytes.len());
            Ok(Value::Str(Rc::new(bytes[beg..end].to_vec())))
        }
        "record_without" => {
            let label = as_bytes(&a[1])?;
            match &*a[0].borrow() {
                Value::Struct { name, fields } => {
                    let mut out = Vec::with_capacity(fields.len());
                    let mut dropped = false;
                    for (n, val) in fields {
                        if !dropped && n.as_bytes() == label.as_ref().as_slice() {
                            dropped = true;
                            continue;
                        }
                        out.push((n.clone(), val.clone()));
                    }
                    Ok(Value::Struct {
                        name: name.clone(),
                        fields: out,
                    })
                }
                _ => Err(fault("`record_without` on a non-record")),
            }
        }
        "@vec_new" => Ok(Value::Vector(Rc::new(Vec::new()))),
        "@vec_fill" => {
            let n = as_len(&a[0])?;
            Ok(Value::Vector(Rc::new(vec![a[1].clone(); n])))
        }
        "@vec_len" => Ok(Value::Int(as_vec(&a[0])?.len() as i64)),
        "@tensor_length" => {
            let (_, _, shape, _) = tensor_fields(&a[0])?;
            Ok(Value::Int(*shape.first().unwrap_or(&0) as i64))
        }
        "@tensor_stack" => {
            let v = as_vec(&a[0])?;
            tensor_stack(&v)
        }
        // O(1) transpose: a VIEW sharing the buffer with every axis reversed (for a
        // rank-2 tensor, the matrix transpose). No elements are copied.
        "@tensor_transpose" => {
            let (buf, off, mut shape, mut strides) = tensor_fields(&a[0])?;
            shape.reverse();
            strides.reverse();
            Ok(mk_tensor(buf, off, shape, strides))
        }
        // O(1) slice along axis 0: a VIEW over `[lo, hi)`.
        "@tensor_slice" => {
            let (buf, off, mut shape, strides) = tensor_fields(&a[0])?;
            if shape.is_empty() {
                return Err(fault("slice of a rank-0 tensor"));
            }
            let lo = as_index(&a[1])?.min(shape[0]);
            let hi = as_index(&a[2])?.clamp(lo, shape[0]);
            let base = off + lo * strides[0];
            shape[0] = hi - lo;
            Ok(mk_tensor(buf, base, shape, strides))
        }
        "@vec_get" => {
            let v = as_vec(&a[0])?;
            let i = as_index(&a[1])?;
            v.get(i)
                .cloned()
                .map(|p| p.borrow().clone_shallow())
                .ok_or_else(|| fault("vec index out of bounds"))
        }
        "@vec_push" => {
            let mut v = as_vec(&a[0])?.as_ref().clone();
            v.push(a[1].clone());
            Ok(Value::Vector(Rc::new(v)))
        }
        "@tensor_concat" => {
            let (_, _, xs, _) = tensor_fields(&a[0])?;
            let (_, _, ys, _) = tensor_fields(&a[1])?;
            if xs.get(1..) != ys.get(1..) {
                return Err(fault("concat: tensors differ below the first axis"));
            }
            let mut buf = materialize(&a[0])?;
            buf.extend(materialize(&a[1])?);
            let mut shape = xs.clone();
            shape[0] = xs[0] + ys[0];
            let strides = row_major_strides(&shape);
            Ok(mk_tensor(Rc::new(buf), 0, shape, strides))
        }
        "@tensor_index" => {
            let (buf, off, shape, strides) = tensor_fields(&a[0])?;
            if shape.is_empty() || shape[0] == 0 {
                return Err(fault("index into an empty tensor"));
            }
            let i = as_int(&a[1])?.rem_euclid(shape[0] as i64) as usize;
            let base = off + i * strides[0];
            if shape.len() == 1 {
                // rank 1: the element is a scalar in the buffer.
                buf.get(base)
                    .cloned()
                    .map(|p| p.borrow().clone_shallow())
                    .ok_or_else(|| fault("tensor index out of bounds"))
            } else {
                // rank > 1: a VIEW that shares the buffer, dropping the first axis.
                Ok(mk_tensor(
                    buf,
                    base,
                    shape[1..].to_vec(),
                    strides[1..].to_vec(),
                ))
            }
        }
        "@vec_set" => {
            let mut v = as_vec(&a[0])?.as_ref().clone();
            let i = as_index(&a[1])?;
            if i >= v.len() {
                return Err(fault("vec index out of bounds"));
            }
            v[i] = a[2].clone();
            Ok(Value::Vector(Rc::new(v)))
        }
        _ => Err(fault(format!("unknown built-in `{name}`"))),
    }
}

fn arith<'p>(op: &str, x: &PVal<'p>, y: &PVal<'p>) -> Result<Value<'p>> {
    let ints = {
        let (bx, by) = (x.borrow(), y.borrow());
        match (&*bx, &*by) {
            (Value::Int(a), Value::Int(b)) => Some((*a, *b)),
            _ => None,
        }
    };
    if let Some((a, b)) = ints {
        let r = match op {
            "+" => a + b,
            "-" => a - b,
            "*" => a * b,
            "/" | "%" if b == 0 => return Err(fault("division by zero")),
            "/" => a / b,
            "%" => a % b,
            _ => unreachable!("arith called with a non-arithmetic operator"),
        };
        return Ok(Value::Int(r));
    }
    let (a, b) = (as_f64(x)?, as_f64(y)?);
    let r = match op {
        "+" => a + b,
        "-" => a - b,
        "*" => a * b,
        "/" => a / b,
        "%" => a % b,
        _ => unreachable!("arith called with a non-arithmetic operator"),
    };
    Ok(Value::Real(r))
}

fn compare<'p>(op: &str, x: &PVal<'p>, y: &PVal<'p>) -> Result<Value<'p>> {
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
        "?<" => ord == Ordering::Less,
        "?>" => ord == Ordering::Greater,
        "<=" => ord != Ordering::Greater,
        ">=" => ord != Ordering::Less,
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
    let is_list = matches!(&*x.borrow(), Value::Variant { ty, .. } if ty == "List");
    if is_list {
        // Lists concatenate by rebuilding the left spine onto the right.
        return Ok(list_append(x.clone(), y.clone()).borrow().clone_shallow());
    }
    Err(fault("`++` on unsupported operands"))
}

fn list_append<'p>(xs: PVal<'p>, ys: PVal<'p>) -> PVal<'p> {
    let cons = {
        match &*xs.borrow() {
            Value::Variant { tag, fields, .. } if tag == "Cons" => {
                Some((fields[0].clone(), fields[1].clone()))
            }
            _ => None,
        }
    };
    match cons {
        Some((head, tail)) => {
            let rest = list_append(tail, ys);
            mk(Value::Variant {
                ty: "List".into(),
                tag: "Cons".into(),
                fields: vec![head, rest],
            })
        }
        // Nil (or any non-cons tail): the right list is the result.
        None => ys,
    }
}

/// Structural equality (`?=`). Numbers compare across Int/Real; functions are not
/// comparable.
pub(crate) fn value_eq(x: &PVal, y: &PVal) -> bool {
    let bx = x.borrow();
    let by = y.borrow();
    match (&*bx, &*by) {
        (Value::Int(_) | Value::Real(_), Value::Int(_) | Value::Real(_)) => {
            as_f64(x).ok() == as_f64(y).ok()
        }
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Unit, Value::Unit) => true,
        (Value::Tuple(a), Value::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(p, q)| value_eq(p, q))
        }
        (Value::Vector(a), Value::Vector(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(p, q)| value_eq(p, q))
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
// resolves it with `dlopen`/`dlsym` and calls it through libffi, so `thrax run`
// reaches the same arbitrary library the C backend links. Everything is a
// machine word: a `Ptr` (and a `FILE*`/allocation) travels as an `Int`.

// The host table calls real libc/libm symbols, so it is compiled only where one
// is linked. The wasm playground target (`wasm32-unknown-unknown`) has no libc;
// there `run_extern` is a stub (see below) and none of these symbols are
// referenced, so nothing needs importing from the host.
#[cfg(not(target_arch = "wasm32"))]
use std::os::raw::{c_char, c_int, c_long, c_uint, c_void};

#[cfg(not(target_arch = "wasm32"))]
extern "C" {
    // math.h (libm)
    fn sqrt(x: f64) -> f64;
    fn cbrt(x: f64) -> f64;
    fn sin(x: f64) -> f64;
    fn cos(x: f64) -> f64;
    fn tan(x: f64) -> f64;
    fn asin(x: f64) -> f64;
    fn acos(x: f64) -> f64;
    fn atan(x: f64) -> f64;
    fn sinh(x: f64) -> f64;
    fn cosh(x: f64) -> f64;
    fn tanh(x: f64) -> f64;
    fn exp(x: f64) -> f64;
    fn exp2(x: f64) -> f64;
    fn log(x: f64) -> f64;
    fn log2(x: f64) -> f64;
    fn log10(x: f64) -> f64;
    fn fabs(x: f64) -> f64;
    fn floor(x: f64) -> f64;
    fn ceil(x: f64) -> f64;
    fn round(x: f64) -> f64;
    fn trunc(x: f64) -> f64;
    fn pow(x: f64, y: f64) -> f64;
    fn fmod(x: f64, y: f64) -> f64;
    fn atan2(x: f64, y: f64) -> f64;
    fn hypot(x: f64, y: f64) -> f64;
    fn copysign(x: f64, y: f64) -> f64;
    fn fmin(x: f64, y: f64) -> f64;
    fn fmax(x: f64, y: f64) -> f64;
    fn fdim(x: f64, y: f64) -> f64;
    // stdlib.h
    fn malloc(n: usize) -> *mut c_void;
    fn calloc(count: usize, size: usize) -> *mut c_void;
    fn realloc(p: *mut c_void, n: usize) -> *mut c_void;
    fn free(p: *mut c_void);
    fn atoi(s: *const c_char) -> c_int;
    fn atof(s: *const c_char) -> f64;
    fn rand() -> c_int;
    fn srand(seed: c_uint);
    fn exit(code: c_int);
    fn abort();
    // string.h
    fn strlen(s: *const c_char) -> usize;
    fn strcmp(a: *const c_char, b: *const c_char) -> c_int;
    fn strncmp(a: *const c_char, b: *const c_char, n: usize) -> c_int;
    fn strchr(s: *const c_char, c: c_int) -> *mut c_char;
    fn strrchr(s: *const c_char, c: c_int) -> *mut c_char;
    fn strstr(hay: *const c_char, needle: *const c_char) -> *mut c_char;
    fn memcpy(d: *mut c_void, s: *const c_void, n: usize) -> *mut c_void;
    fn memmove(d: *mut c_void, s: *const c_void, n: usize) -> *mut c_void;
    fn memset(p: *mut c_void, c: c_int, n: usize) -> *mut c_void;
    fn memcmp(a: *const c_void, b: *const c_void, n: usize) -> c_int;
    fn memchr(p: *const c_void, c: c_int, n: usize) -> *mut c_void;
    // ctype.h
    fn isalpha(c: c_int) -> c_int;
    fn isdigit(c: c_int) -> c_int;
    fn isalnum(c: c_int) -> c_int;
    fn isspace(c: c_int) -> c_int;
    fn isupper(c: c_int) -> c_int;
    fn islower(c: c_int) -> c_int;
    fn ispunct(c: c_int) -> c_int;
    fn iscntrl(c: c_int) -> c_int;
    fn isprint(c: c_int) -> c_int;
    fn toupper(c: c_int) -> c_int;
    fn tolower(c: c_int) -> c_int;
    // stdio.h
    fn puts(s: *const c_char) -> c_int;
    fn putchar(c: c_int) -> c_int;
    fn getchar() -> c_int;
    fn fopen(path: *const c_char, mode: *const c_char) -> *mut c_void;
    fn fclose(f: *mut c_void) -> c_int;
    fn fgetc(f: *mut c_void) -> c_int;
    fn fputc(c: c_int, f: *mut c_void) -> c_int;
    fn fputs(s: *const c_char, f: *mut c_void) -> c_int;
    fn fflush(f: *mut c_void) -> c_int;
    fn fseek(f: *mut c_void, off: c_long, whence: c_int) -> c_int;
    fn ftell(f: *mut c_void) -> c_long;
    fn remove(path: *const c_char) -> c_int;
    fn rename(from: *const c_char, to: *const c_char) -> c_int;
    // unistd.h / time.h
    fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    fn getenv(key: *const c_char) -> *mut c_char;
    fn time(t: *mut c_void) -> c_long;
}

/// A machine-word argument (`Int`, or a `Ptr` carrying its bits).
#[cfg(not(target_arch = "wasm32"))]
fn ffi_word(args: &[PVal], i: usize) -> Result<i64> {
    match &*args[i].borrow() {
        Value::Int(n) => Ok(*n),
        _ => Err(fault("FFI: expected an integer/pointer argument")),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn ffi_real(args: &[PVal], i: usize) -> Result<f64> {
    match &*args[i].borrow() {
        Value::Int(n) => Ok(*n as f64),
        Value::Real(r) => Ok(*r),
        _ => Err(fault("FFI: expected a Real argument")),
    }
}

/// A NUL-terminated copy of a `Str` argument's bytes, for passing as a C string.
/// The returned buffer must outlive the call.
#[cfg(not(target_arch = "wasm32"))]
fn ffi_cstr(args: &[PVal], i: usize) -> Result<Vec<u8>> {
    match &*args[i].borrow() {
        Value::Str(b) => {
            let mut v = (**b).clone();
            v.push(0);
            Ok(v)
        }
        _ => Err(fault("FFI: expected a Str argument")),
    }
}

/// Wrap a C string result (`char*`) into a `Str` value, matching the C backend's
/// `_r ? THxRT_str(_r, strlen(_r)) : THxRT_str("", 0)`.
#[cfg(not(target_arch = "wasm32"))]
unsafe fn ffi_ret_str<'p>(p: *const c_char) -> Value<'p> {
    if p.is_null() {
        return Value::Str(Rc::new(Vec::new()));
    }
    let n = strlen(p);
    let bytes = std::slice::from_raw_parts(p as *const u8, n).to_vec();
    Value::Str(Rc::new(bytes))
}

/// Call a foreign C function from the host table with the marshalled `args`.
/// Mirrors `FF.cpp`'s adapter table: each adapter knows its own signature, so
/// the result is wrapped directly. The set of symbols served here is exactly the
/// `C` namespace declared in `core/C.thx`; the C backend reaches these same
/// symbols with a direct linked call, so both engines agree.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_extern<'p>(
    abi: &str,
    symbol: &str,
    lib: &str,
    arg_types: &[String],
    ret_type: &str,
    args: &[PVal<'p>],
) -> Result<Value<'p>> {
    if abi == "WASM" {
        return Err(fault(format!(
            "FFI: `{symbol}` is an `@extern \"WASM\"` host import, available only in \
             the wasm playground, not in a native run"
        )));
    }
    let v = unsafe {
        match symbol {
            // -- math.h (unary) --
            "sqrt" => Value::Real(sqrt(ffi_real(args, 0)?)),
            "cbrt" => Value::Real(cbrt(ffi_real(args, 0)?)),
            "sin" => Value::Real(sin(ffi_real(args, 0)?)),
            "cos" => Value::Real(cos(ffi_real(args, 0)?)),
            "tan" => Value::Real(tan(ffi_real(args, 0)?)),
            "asin" => Value::Real(asin(ffi_real(args, 0)?)),
            "acos" => Value::Real(acos(ffi_real(args, 0)?)),
            "atan" => Value::Real(atan(ffi_real(args, 0)?)),
            "sinh" => Value::Real(sinh(ffi_real(args, 0)?)),
            "cosh" => Value::Real(cosh(ffi_real(args, 0)?)),
            "tanh" => Value::Real(tanh(ffi_real(args, 0)?)),
            "exp" => Value::Real(exp(ffi_real(args, 0)?)),
            "exp2" => Value::Real(exp2(ffi_real(args, 0)?)),
            "log" => Value::Real(log(ffi_real(args, 0)?)),
            "log2" => Value::Real(log2(ffi_real(args, 0)?)),
            "log10" => Value::Real(log10(ffi_real(args, 0)?)),
            "fabs" => Value::Real(fabs(ffi_real(args, 0)?)),
            "floor" => Value::Real(floor(ffi_real(args, 0)?)),
            "ceil" => Value::Real(ceil(ffi_real(args, 0)?)),
            "round" => Value::Real(round(ffi_real(args, 0)?)),
            "trunc" => Value::Real(trunc(ffi_real(args, 0)?)),
            // -- math.h (binary) --
            "pow" => Value::Real(pow(ffi_real(args, 0)?, ffi_real(args, 1)?)),
            "fmod" => Value::Real(fmod(ffi_real(args, 0)?, ffi_real(args, 1)?)),
            "atan2" => Value::Real(atan2(ffi_real(args, 0)?, ffi_real(args, 1)?)),
            "hypot" => Value::Real(hypot(ffi_real(args, 0)?, ffi_real(args, 1)?)),
            "copysign" => Value::Real(copysign(ffi_real(args, 0)?, ffi_real(args, 1)?)),
            "fmin" => Value::Real(fmin(ffi_real(args, 0)?, ffi_real(args, 1)?)),
            "fmax" => Value::Real(fmax(ffi_real(args, 0)?, ffi_real(args, 1)?)),
            "fdim" => Value::Real(fdim(ffi_real(args, 0)?, ffi_real(args, 1)?)),
            // -- stdlib.h --
            "malloc" => Value::Int(malloc(ffi_word(args, 0)? as usize) as i64),
            "calloc" => Value::Int(
                calloc(ffi_word(args, 0)? as usize, ffi_word(args, 1)? as usize) as i64,
            ),
            "realloc" => Value::Int(realloc(
                ffi_word(args, 0)? as *mut c_void,
                ffi_word(args, 1)? as usize,
            ) as i64),
            "free" => {
                free(ffi_word(args, 0)? as *mut c_void);
                Value::Unit
            }
            "atoi" => Value::Int(atoi(ffi_cstr(args, 0)?.as_ptr() as *const c_char) as i64),
            "atof" => Value::Real(atof(ffi_cstr(args, 0)?.as_ptr() as *const c_char)),
            "rand" => Value::Int(rand() as i64),
            "srand" => {
                srand(ffi_word(args, 0)? as c_uint);
                Value::Unit
            }
            "exit" => {
                exit(ffi_word(args, 0)? as c_int);
                Value::Unit
            }
            "abort" => {
                abort();
                Value::Unit
            }
            // -- string.h --
            "strlen" => Value::Int(strlen(ffi_cstr(args, 0)?.as_ptr() as *const c_char) as i64),
            "strcmp" => {
                let (a, b) = (ffi_cstr(args, 0)?, ffi_cstr(args, 1)?);
                Value::Int(strcmp(a.as_ptr() as *const c_char, b.as_ptr() as *const c_char) as i64)
            }
            "strncmp" => {
                let (a, b) = (ffi_cstr(args, 0)?, ffi_cstr(args, 1)?);
                Value::Int(strncmp(
                    a.as_ptr() as *const c_char,
                    b.as_ptr() as *const c_char,
                    ffi_word(args, 2)? as usize,
                ) as i64)
            }
            "strchr" => {
                let s = ffi_cstr(args, 0)?;
                Value::Int(strchr(s.as_ptr() as *const c_char, ffi_word(args, 1)? as c_int) as i64)
            }
            "strrchr" => {
                let s = ffi_cstr(args, 0)?;
                Value::Int(strrchr(s.as_ptr() as *const c_char, ffi_word(args, 1)? as c_int) as i64)
            }
            "strstr" => {
                let (h, n) = (ffi_cstr(args, 0)?, ffi_cstr(args, 1)?);
                Value::Int(strstr(h.as_ptr() as *const c_char, n.as_ptr() as *const c_char) as i64)
            }
            "memcpy" => Value::Int(memcpy(
                ffi_word(args, 0)? as *mut c_void,
                ffi_word(args, 1)? as *const c_void,
                ffi_word(args, 2)? as usize,
            ) as i64),
            "memmove" => Value::Int(memmove(
                ffi_word(args, 0)? as *mut c_void,
                ffi_word(args, 1)? as *const c_void,
                ffi_word(args, 2)? as usize,
            ) as i64),
            "memset" => Value::Int(memset(
                ffi_word(args, 0)? as *mut c_void,
                ffi_word(args, 1)? as c_int,
                ffi_word(args, 2)? as usize,
            ) as i64),
            "memcmp" => Value::Int(memcmp(
                ffi_word(args, 0)? as *const c_void,
                ffi_word(args, 1)? as *const c_void,
                ffi_word(args, 2)? as usize,
            ) as i64),
            "memchr" => Value::Int(memchr(
                ffi_word(args, 0)? as *const c_void,
                ffi_word(args, 1)? as c_int,
                ffi_word(args, 2)? as usize,
            ) as i64),
            // -- ctype.h --
            "isalpha" => Value::Int(isalpha(ffi_word(args, 0)? as c_int) as i64),
            "isdigit" => Value::Int(isdigit(ffi_word(args, 0)? as c_int) as i64),
            "isalnum" => Value::Int(isalnum(ffi_word(args, 0)? as c_int) as i64),
            "isspace" => Value::Int(isspace(ffi_word(args, 0)? as c_int) as i64),
            "isupper" => Value::Int(isupper(ffi_word(args, 0)? as c_int) as i64),
            "islower" => Value::Int(islower(ffi_word(args, 0)? as c_int) as i64),
            "ispunct" => Value::Int(ispunct(ffi_word(args, 0)? as c_int) as i64),
            "iscntrl" => Value::Int(iscntrl(ffi_word(args, 0)? as c_int) as i64),
            "isprint" => Value::Int(isprint(ffi_word(args, 0)? as c_int) as i64),
            "toupper" => Value::Int(toupper(ffi_word(args, 0)? as c_int) as i64),
            "tolower" => Value::Int(tolower(ffi_word(args, 0)? as c_int) as i64),
            // -- stdio.h --
            "puts" => Value::Int(puts(ffi_cstr(args, 0)?.as_ptr() as *const c_char) as i64),
            "putchar" => Value::Int(putchar(ffi_word(args, 0)? as c_int) as i64),
            "getchar" => Value::Int(getchar() as i64),
            "fopen" => {
                let path = ffi_cstr(args, 0)?;
                let mode = ffi_cstr(args, 1)?;
                Value::Int(
                    fopen(path.as_ptr() as *const c_char, mode.as_ptr() as *const c_char) as i64,
                )
            }
            "fclose" => Value::Int(fclose(ffi_word(args, 0)? as *mut c_void) as i64),
            "fgetc" => Value::Int(fgetc(ffi_word(args, 0)? as *mut c_void) as i64),
            "fputc" => Value::Int(fputc(
                ffi_word(args, 0)? as c_int,
                ffi_word(args, 1)? as *mut c_void,
            ) as i64),
            "fputs" => {
                let s = ffi_cstr(args, 0)?;
                Value::Int(
                    fputs(s.as_ptr() as *const c_char, ffi_word(args, 1)? as *mut c_void) as i64,
                )
            }
            "fflush" => Value::Int(fflush(ffi_word(args, 0)? as *mut c_void) as i64),
            "fseek" => Value::Int(fseek(
                ffi_word(args, 0)? as *mut c_void,
                ffi_word(args, 1)? as c_long,
                ffi_word(args, 2)? as c_int,
            ) as i64),
            "ftell" => Value::Int(ftell(ffi_word(args, 0)? as *mut c_void) as i64),
            "remove" => Value::Int(remove(ffi_cstr(args, 0)?.as_ptr() as *const c_char) as i64),
            "rename" => {
                let (from, to) = (ffi_cstr(args, 0)?, ffi_cstr(args, 1)?);
                Value::Int(
                    rename(from.as_ptr() as *const c_char, to.as_ptr() as *const c_char) as i64,
                )
            }
            // -- unistd.h / time.h --
            "write" => {
                let (fd, n) = (ffi_word(args, 0)?, ffi_word(args, 2)?);
                let buf = match &*args[1].borrow() {
                    Value::Str(b) => (**b).clone(),
                    _ => return Err(fault("FFI: `write` expects a Str buffer")),
                };
                let n = (n as usize).min(buf.len());
                Value::Int(write(fd as c_int, buf.as_ptr() as *const c_void, n) as i64)
            }
            "getenv" => ffi_ret_str(getenv(ffi_cstr(args, 0)?.as_ptr() as *const c_char)),
            "time" => Value::Int(time(ffi_word(args, 0)? as *mut c_void) as i64),
            // A symbol outside the compiled-in fast-path table: resolve it at
            // runtime and call it through libffi (see [`crate::machine::ffi`]).
            // This reaches the same arbitrary library the C backend links.
            _ => return crate::machine::ffi::call_extern(symbol, lib, arg_types, ret_type, args),
        }
    };
    // Keep foreign stdout (C stdio) ordered against the driver's own output,
    // which the compiled C program produces in one stream.
    unsafe { fflush(std::ptr::null_mut()) };
    Ok(v)
}

// On wasm there is no libc: `@extern "WASM"` resolves against the JavaScript
// embedder instead of a C library. The marshalling and the generic host-call
// bridge live in [`crate::machine::ffi`] (the `WasmHostFfi` backend); this is
// just the abi guard and the hand-off.
#[cfg(target_arch = "wasm32")]
pub(crate) fn run_extern<'p>(
    abi: &str,
    symbol: &str,
    lib: &str,
    arg_types: &[String],
    ret_type: &str,
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
            Value::Str(b) => Value::Str(b.clone()),
            Value::Bool(b) => Value::Bool(*b),
            Value::Unit => Value::Unit,
            Value::Tuple(items) => Value::Tuple(items.clone()),
            Value::Struct { name, fields } => Value::Struct {
                name: name.clone(),
                fields: fields.clone(),
            },
            Value::Variant { ty, tag, fields } => Value::Variant {
                ty: ty.clone(),
                tag: tag.clone(),
                fields: fields.clone(),
            },
            Value::Vector(items) => Value::Vector(items.clone()),
            Value::Builtin { name, arity, args } => Value::Builtin {
                name: name.clone(),
                arity: *arity,
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
                abi: abi.clone(),
                symbol: symbol.clone(),
                lib: lib.clone(),
                arg_types: arg_types.clone(),
                ret_type: ret_type.clone(),
                args: args.clone(),
            },
            Value::Code { code, env } => Value::Code {
                code: *code,
                env: env.clone(),
            },
            Value::Rec(w) => Value::Rec(w.clone()),
            Value::Op { effect, op } => Value::Op {
                effect: effect.clone(),
                op: op.clone(),
            },
            Value::Resump(r) => Value::Resump(r.clone()),
        }
    }

    /// A short human display of a value, matching [`crate::eval::data::Value::show`]
    /// byte-for-byte (so the machine can be diffed against the tree-walker).
    pub fn show(&self) -> String {
        match self {
            Value::Unk => "{}".into(),
            Value::Int(n) => n.to_string(),
            Value::Real(r) => r.to_string(),
            Value::Str(b) => format!("{:?}", String::from_utf8_lossy(b)),
            Value::Bool(b) => b.to_string(),
            Value::Unit => "{}".into(),
            Value::Tuple(items) => {
                let inner: Vec<String> = items.iter().map(|v| v.borrow().show()).collect();
                format!("{{{}}}", inner.join(", "))
            }
            Value::Struct { name, fields } => {
                let inner: Vec<String> = fields
                    .iter()
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
            Value::Builtin { .. }
            | Value::Extern { .. }
            | Value::Code { .. }
            | Value::Op { .. } => "<function>".into(),
            Value::Resump(_) => "<continuation>".into(),
            Value::Rec(_) => "<recursive>".into(),
        }
    }
}
