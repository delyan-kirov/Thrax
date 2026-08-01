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
    /// A partially applied foreign C function (`@extern`); it marshals its
    /// arguments across the C ABI and calls `symbol` once `args` reaches the
    /// arity `arg_types.len()`. `ret_type` selects how the result is wrapped.
    Extern {
        symbol: Rc<str>,
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

fn as_index(v: &PVal) -> Result<usize> {
    match &*v.borrow() {
        Value::Int(n) if *n >= 0 => Ok(*n as usize),
        Value::Int(_) => Err(fault("negative index")),
        _ => Err(fault("expected an integer index")),
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
        "not" | "neg" | "array_len" | "array_alloc" | "vec_len" | "vec_new" => 1,
        "+" | "-" | "*" | "/" | "%" | "?=" | "?<" | "?>" | "<=" | ">=" | "++" | "array_get"
        | "array_push" | "vec_get" | "vec_push" | "vec_fill" => 2,
        "array_set" | "array_slice" | "vec_set" => 3,
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
        "array_alloc" => Ok(Value::Str(Rc::new(vec![0u8; as_len(&a[0])?]))),
        "array_len" => Ok(Value::Int(as_bytes(&a[0])?.len() as i64)),
        "array_get" => {
            let bytes = as_bytes(&a[0])?;
            let i = as_index(&a[1])?;
            bytes
                .get(i)
                .map(|b| Value::Int(*b as i64))
                .ok_or_else(|| fault("array index out of bounds"))
        }
        "array_push" => {
            let mut bytes = as_bytes(&a[0])?.as_ref().clone();
            bytes.push(as_byte(&a[1])?);
            Ok(Value::Str(Rc::new(bytes)))
        }
        "array_set" => {
            let mut bytes = as_bytes(&a[0])?.as_ref().clone();
            let i = as_index(&a[1])?;
            if i >= bytes.len() {
                return Err(fault("array index out of bounds"));
            }
            bytes[i] = as_byte(&a[2])?;
            Ok(Value::Str(Rc::new(bytes)))
        }
        "array_slice" => {
            let bytes = as_bytes(&a[0])?;
            let mut beg = as_index(&a[1])?;
            let mut end = as_index(&a[2])?;
            beg = beg.min(bytes.len());
            end = end.clamp(beg, bytes.len());
            Ok(Value::Str(Rc::new(bytes[beg..end].to_vec())))
        }
        "vec_new" => Ok(Value::Vector(Rc::new(Vec::new()))),
        "vec_fill" => {
            let n = as_len(&a[0])?;
            Ok(Value::Vector(Rc::new(vec![a[1].clone(); n])))
        }
        "vec_len" => Ok(Value::Int(as_vec(&a[0])?.len() as i64)),
        "vec_get" => {
            let v = as_vec(&a[0])?;
            let i = as_index(&a[1])?;
            v.get(i)
                .cloned()
                .map(|p| p.borrow().clone_shallow())
                .ok_or_else(|| fault("vec index out of bounds"))
        }
        "vec_push" => {
            let mut v = as_vec(&a[0])?.as_ref().clone();
            v.push(a[1].clone());
            Ok(Value::Vector(Rc::new(v)))
        }
        "vec_set" => {
            let mut v = as_vec(&a[0])?.as_ref().clone();
            let i = as_index(&a[1])?;
            if i >= v.len() {
                return Err(fault("vec index out of bounds"));
            }
            v[i] = a[2].clone();
            Ok(Value::Vector(Rc::new(v)))
        }
        _ if name.starts_with("C.") => crate::machine::clib::run_c(name, a),
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

// -- foreign function interface (host table) --------------------------------

// The interpreter serves `@extern` calls from a compiled-in host table of the
// C/libm namespace, the port of `engines/FF.cpp`'s no-3rd-party build. There is
// no dynamic loading (that would need libffi/dlopen, forbidden here), so a
// symbol outside this table fails with a clear message; the C backend, which
// emits a direct symbol call, still reaches an arbitrary library. Everything is
// a machine word: a `Ptr` (and a `FILE*`/allocation) travels as an `Int`.

use std::os::raw::{c_char, c_int, c_long, c_void};

extern "C" {
    fn puts(s: *const c_char) -> c_int;
    fn putchar(c: c_int) -> c_int;
    fn getchar() -> c_int;
    fn malloc(n: usize) -> *mut c_void;
    fn free(p: *mut c_void);
    fn memset(p: *mut c_void, c: c_int, n: usize) -> *mut c_void;
    fn strlen(s: *const c_char) -> usize;
    fn fopen(path: *const c_char, mode: *const c_char) -> *mut c_void;
    fn fclose(f: *mut c_void) -> c_int;
    fn fgetc(f: *mut c_void) -> c_int;
    fn fputs(s: *const c_char, f: *mut c_void) -> c_int;
    fn fflush(f: *mut c_void) -> c_int;
    fn fseek(f: *mut c_void, off: c_long, whence: c_int) -> c_int;
    fn ftell(f: *mut c_void) -> c_long;
    fn remove(path: *const c_char) -> c_int;
    fn getenv(key: *const c_char) -> *mut c_char;
    fn time(t: *mut c_void) -> c_long;
    fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    fn sqrt(x: f64) -> f64;
    fn sin(x: f64) -> f64;
    fn cos(x: f64) -> f64;
    fn tan(x: f64) -> f64;
    fn exp(x: f64) -> f64;
    fn log(x: f64) -> f64;
    fn floor(x: f64) -> f64;
    fn ceil(x: f64) -> f64;
    fn round(x: f64) -> f64;
    fn pow(x: f64, y: f64) -> f64;
    fn fmod(x: f64, y: f64) -> f64;
    fn atan2(x: f64, y: f64) -> f64;
}

/// A machine-word argument (`Int`, or a `Ptr` carrying its bits).
fn ffi_word(args: &[PVal], i: usize) -> Result<i64> {
    match &*args[i].borrow() {
        Value::Int(n) => Ok(*n),
        _ => Err(fault("FFI: expected an integer/pointer argument")),
    }
}

fn ffi_real(args: &[PVal], i: usize) -> Result<f64> {
    match &*args[i].borrow() {
        Value::Int(n) => Ok(*n as f64),
        Value::Real(r) => Ok(*r),
        _ => Err(fault("FFI: expected a Real argument")),
    }
}

/// A NUL-terminated copy of a `Str` argument's bytes, for passing as a C string.
/// The returned buffer must outlive the call.
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

/// Call a foreign C function from the host table with the marshalled `args`.
/// Mirrors `FF.cpp`'s adapter table: each adapter knows its own signature, so
/// the result is wrapped directly (`ret_type` drives only the C backend).
pub(crate) fn run_extern<'p>(symbol: &str, args: &[PVal<'p>]) -> Result<Value<'p>> {
    let v = unsafe {
        match symbol {
            "puts" => Value::Int(puts(ffi_cstr(args, 0)?.as_ptr() as *const c_char) as i64),
            "putchar" => Value::Int(putchar(ffi_word(args, 0)? as c_int) as i64),
            "getchar" => Value::Int(getchar() as i64),
            "malloc" => Value::Int(malloc(ffi_word(args, 0)? as usize) as i64),
            "free" => {
                free(ffi_word(args, 0)? as *mut c_void);
                Value::Unit
            }
            "memset" => Value::Int(memset(
                ffi_word(args, 0)? as *mut c_void,
                ffi_word(args, 1)? as c_int,
                ffi_word(args, 2)? as usize,
            ) as i64),
            "strlen" => Value::Int(strlen(ffi_cstr(args, 0)?.as_ptr() as *const c_char) as i64),
            "fopen" => {
                let path = ffi_cstr(args, 0)?;
                let mode = ffi_cstr(args, 1)?;
                Value::Int(fopen(path.as_ptr() as *const c_char, mode.as_ptr() as *const c_char)
                    as i64)
            }
            "fclose" => Value::Int(fclose(ffi_word(args, 0)? as *mut c_void) as i64),
            "fgetc" => Value::Int(fgetc(ffi_word(args, 0)? as *mut c_void) as i64),
            "fputs" => {
                let s = ffi_cstr(args, 0)?;
                Value::Int(fputs(s.as_ptr() as *const c_char, ffi_word(args, 1)? as *mut c_void)
                    as i64)
            }
            "fseek" => Value::Int(fseek(
                ffi_word(args, 0)? as *mut c_void,
                ffi_word(args, 1)? as c_long,
                ffi_word(args, 2)? as c_int,
            ) as i64),
            "ftell" => Value::Int(ftell(ffi_word(args, 0)? as *mut c_void) as i64),
            "remove" => Value::Int(remove(ffi_cstr(args, 0)?.as_ptr() as *const c_char) as i64),
            "getenv" => Value::Int(getenv(ffi_cstr(args, 0)?.as_ptr() as *const c_char) as i64),
            "time" => Value::Int(time(ffi_word(args, 0)? as *mut c_void) as i64),
            "write" => {
                let (fd, n) = (ffi_word(args, 0)?, ffi_word(args, 2)?);
                let buf = match &*args[1].borrow() {
                    Value::Str(b) => (**b).clone(),
                    _ => return Err(fault("FFI: `write` expects a Str buffer")),
                };
                let n = (n as usize).min(buf.len());
                Value::Int(write(fd as c_int, buf.as_ptr() as *const c_void, n) as i64)
            }
            "sqrt" => Value::Real(sqrt(ffi_real(args, 0)?)),
            "sin" => Value::Real(sin(ffi_real(args, 0)?)),
            "cos" => Value::Real(cos(ffi_real(args, 0)?)),
            "tan" => Value::Real(tan(ffi_real(args, 0)?)),
            "exp" => Value::Real(exp(ffi_real(args, 0)?)),
            "log" => Value::Real(log(ffi_real(args, 0)?)),
            "floor" => Value::Real(floor(ffi_real(args, 0)?)),
            "ceil" => Value::Real(ceil(ffi_real(args, 0)?)),
            "round" => Value::Real(round(ffi_real(args, 0)?)),
            "pow" => Value::Real(pow(ffi_real(args, 0)?, ffi_real(args, 1)?)),
            "fmod" => Value::Real(fmod(ffi_real(args, 0)?, ffi_real(args, 1)?)),
            "atan2" => Value::Real(atan2(ffi_real(args, 0)?, ffi_real(args, 1)?)),
            _ => {
                return Err(fault(format!(
                    "FFI: symbol `{symbol}` is not in the interpreter's host table \
                     (only the C/libm namespace is available without dynamic loading)"
                )))
            }
        }
    };
    // Keep foreign stdout (C stdio) ordered against the driver's own output,
    // which the compiled C program produces in one stream.
    unsafe { fflush(std::ptr::null_mut()) };
    Ok(v)
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
                symbol,
                arg_types,
                ret_type,
                args,
            } => Value::Extern {
                symbol: symbol.clone(),
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
