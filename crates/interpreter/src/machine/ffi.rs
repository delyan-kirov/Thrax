//! The `@extern` foreign-call seam, as an API a host platform implements.
//!
//! An extern call is reduced to a platform-independent C value model (its
//! arguments become [`CArg`]s, its result a [`RetPlan`]), and then handed to a
//! [`ForeignCalls`] backend for the running host. Everything above the backend,
//! the classification from a Thrax [`Value`] and the wrapping back into one, is
//! shared; a new platform only implements `call`.
//!
//! Two backends ship:
//! - [`NativeFfi`] resolves the symbol with `dlopen`/`dlsym` and performs the
//!   call through libffi (built from `external/libffi`; see `build.rs` and
//!   `ffi_shim.c`). libffi handles the calling convention on every target, so
//!   there is no hand-written per-ABI trampoline and no arity/`Real32` limit.
//! - [`WasmHostFfi`] serialises the call across a single generic import to the
//!   JavaScript embedder, which owns the function registry (the playground).

use std::rc::Rc;

use utilities::Result;

use crate::machine::data::{fault, PVal, Value};

/// libffi type-kind codes. Must stay in sync with the enum in `ffi_shim.c`.
mod kind {
    use std::os::raw::c_int;
    pub const VOID: c_int = 0;
    pub const S8: c_int = 1;
    pub const S16: c_int = 2;
    pub const S32: c_int = 3;
    pub const S64: c_int = 4;
    pub const U8: c_int = 5;
    pub const U16: c_int = 6;
    pub const U32: c_int = 7;
    pub const U64: c_int = 8;
    pub const FLOAT: c_int = 9;
    pub const DOUBLE: c_int = 10;
    pub const PTR: c_int = 11;
}

/// One extern argument reduced to the C value model. A backend decides how each
/// variant crosses its seam (a pointer to bytes natively, a copy on wasm).
pub enum CArg {
    /// A sized integer whose libffi kind is `kind` and whose value is `val`
    /// (the low bytes are used; `{}` arrives here as a zero word).
    Int { kind: std::os::raw::c_int, val: i64 },
    Float(f32),
    Double(f64),
    /// A `Str`/`Array`: passed as a NUL-terminated `char*` natively, copied by
    /// value on wasm.
    Bytes(Vec<u8>),
    /// An opaque `Ptr` carried as a machine word.
    Ptr(i64),
}

/// How to type and interpret an extern's result. The wasm backend reports its
/// own result kind and ignores this, so the fields read as dead there.
#[derive(Clone, Copy)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub struct RetPlan {
    /// The libffi return kind handed to `ffi_prep_cif`.
    ffi_kind: std::os::raw::c_int,
    wrap: Wrap,
}

#[derive(Clone, Copy)]
enum Wrap {
    Unit,
    Int,
    Real,
    Bytes,
}

/// A backend's raw result, before it becomes a [`Value`].
pub enum Outcome {
    Unit,
    Int(i64),
    Real(f64),
    Bytes(Vec<u8>),
}

/// A host's implementation of the `@extern` seam: resolve `symbol` in `lib` and
/// call it with `args`, producing an [`Outcome`]. `ret` describes the wanted
/// result type; a backend whose protocol reports its own result kind (wasm) may
/// ignore it.
pub trait ForeignCalls {
    fn call(&self, symbol: &str, lib: &str, args: &[CArg], ret: RetPlan) -> Result<Outcome>;
}

/// Classify a Thrax argument value against its declared type, mirroring the C
/// backend's `cabi` so both engines marshal identically.
fn classify_arg(ty: &str, v: &PVal) -> Result<CArg> {
    Ok(match ty {
        "@float64" | "Real" | "Real64" => CArg::Double(read_real(v)?),
        "@float32" | "Real32" => CArg::Float(read_real(v)? as f32),
        "@str" | "@array" | "Str" | "Array" => CArg::Bytes(read_bytes(v)?),
        "@ptr" | "Ptr" => CArg::Ptr(read_word(v)?),
        // A `{}` parameter is a nullary C function's placeholder: an ignored word.
        "{}" => CArg::Int {
            kind: kind::S64,
            val: 0,
        },
        "@int8" | "Int8" => int_arg(kind::S8, v)?,
        "@int16" | "Int16" => int_arg(kind::S16, v)?,
        "@int32" | "Int32" => int_arg(kind::S32, v)?,
        "@int64" | "Int64" => int_arg(kind::S64, v)?,
        "@nat8" | "Nat8" => int_arg(kind::U8, v)?,
        "@nat16" | "Nat16" => int_arg(kind::U16, v)?,
        "@nat32" | "Nat32" => int_arg(kind::U32, v)?,
        "@nat64" | "Nat64" | "Nat" => int_arg(kind::U64, v)?,
        _ => int_arg(kind::S64, v)?,
    })
}

fn int_arg(k: std::os::raw::c_int, v: &PVal) -> Result<CArg> {
    Ok(CArg::Int {
        kind: k,
        val: read_word(v)?,
    })
}

/// Plan the return type/wrapping from the declared result type.
fn classify_ret(ty: &str) -> RetPlan {
    let (ffi_kind, wrap) = match ty {
        "@float64" | "Real" | "Real64" => (kind::DOUBLE, Wrap::Real),
        "@float32" | "Real32" => (kind::FLOAT, Wrap::Real),
        "@str" | "@array" | "Str" | "Array" => (kind::PTR, Wrap::Bytes),
        "@ptr" | "Ptr" => (kind::PTR, Wrap::Int),
        "{}" => (kind::VOID, Wrap::Unit),
        "@int8" | "Int8" => (kind::S8, Wrap::Int),
        "@int16" | "Int16" => (kind::S16, Wrap::Int),
        "@int32" | "Int32" => (kind::S32, Wrap::Int),
        "@nat8" | "Nat8" => (kind::U8, Wrap::Int),
        "@nat16" | "Nat16" => (kind::U16, Wrap::Int),
        "@nat32" | "Nat32" => (kind::U32, Wrap::Int),
        "@nat64" | "Nat64" | "Nat" => (kind::U64, Wrap::Int),
        _ => (kind::S64, Wrap::Int),
    };
    RetPlan { ffi_kind, wrap }
}

fn read_word(v: &PVal) -> Result<i64> {
    match &*v.borrow() {
        Value::Int(n) => Ok(*n),
        _ => Err(fault("FFI: expected an integer/pointer argument")),
    }
}

fn read_real(v: &PVal) -> Result<f64> {
    match &*v.borrow() {
        Value::Int(n) => Ok(*n as f64),
        Value::Real(r) => Ok(*r),
        _ => Err(fault("FFI: expected a Real argument")),
    }
}

fn read_bytes(v: &PVal) -> Result<Vec<u8>> {
    match &*v.borrow() {
        Value::Str(b) => Ok((**b).clone()),
        _ => Err(fault("FFI: expected a Str argument")),
    }
}

/// Classify, dispatch to the host backend, and wrap the result. This is the one
/// entry point the machine's `run_extern` uses for a symbol outside the native
/// fast-path table (and for every wasm host call).
pub fn call_extern<'p>(
    symbol: &str,
    lib: &str,
    arg_types: &[String],
    ret_type: &str,
    args: &[PVal<'p>],
) -> Result<Value<'p>> {
    let mut cargs = Vec::with_capacity(arg_types.len());
    for (i, ty) in arg_types.iter().enumerate() {
        cargs.push(classify_arg(ty, &args[i])?);
    }
    let plan = classify_ret(ret_type);
    let outcome = backend().call(symbol, lib, &cargs, plan)?;
    Ok(match outcome {
        Outcome::Unit => Value::Unit,
        Outcome::Int(n) => Value::Int(n),
        Outcome::Real(r) => Value::Real(r),
        Outcome::Bytes(b) => Value::Str(Rc::new(b)),
    })
}

// -- native backend: dlopen/dlsym + libffi ----------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{kind, CArg, ForeignCalls, Outcome, RetPlan, Wrap};
    use crate::machine::data::fault;
    use std::cell::RefCell;
    use std::os::raw::{c_char, c_int, c_void};
    use utilities::Result;

    extern "C" {
        fn dlopen(path: *const c_char, flag: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, sym: *const c_char) -> *mut c_void;
        fn strlen(s: *const c_char) -> usize;
        fn fflush(f: *mut c_void) -> c_int;
        fn thx_ffi_call(
            fun: *mut c_void,
            nargs: c_int,
            kinds: *const c_int,
            ret_kind: c_int,
            avalues: *mut *mut c_void,
            rvalue: *mut c_void,
        ) -> c_int;
    }

    const RTLD_NOW: c_int = 2;
    const RTLD_GLOBAL: c_int = 0x100;

    // Loaded library handles, keyed by resolved soname, kept open for the
    // process. The interpreter is single-threaded (values are `Rc`), so a
    // thread-local map is enough; a handle is never closed, matching how a
    // linked program keeps its libraries mapped for its whole run.
    thread_local! {
        static HANDLES: RefCell<std::collections::HashMap<String, usize>> =
            RefCell::new(std::collections::HashMap::new());
    }

    pub struct NativeFfi;

    fn resolve(symbol: &str, lib: &str) -> Result<*mut c_void> {
        use utilities::target::Target;
        // libc/libm/no-lib live in the already-loaded set (RTLD_DEFAULT = null).
        let default_set = matches!(lib, "" | "libc" | "c" | "libm" | "m");
        let handle = if default_set {
            std::ptr::null_mut()
        } else {
            let soname = Target::host().soname(lib);
            if let Some(p) = HANDLES.with(|h| h.borrow().get(&soname).copied()) {
                p as *mut c_void
            } else {
                let mut path = soname.clone().into_bytes();
                path.push(0);
                let p = unsafe { dlopen(path.as_ptr() as *const c_char, RTLD_NOW | RTLD_GLOBAL) };
                if p.is_null() {
                    return Err(fault(format!(
                        "FFI: cannot load library `{lib}` (resolved to `{soname}`) for symbol \
                         `{symbol}`"
                    )));
                }
                HANDLES.with(|h| h.borrow_mut().insert(soname, p as usize));
                p
            }
        };
        let mut name = symbol.as_bytes().to_vec();
        name.push(0);
        let addr = unsafe { dlsym(handle, name.as_ptr() as *const c_char) };
        if addr.is_null() {
            return Err(fault(format!(
                "FFI: symbol `{symbol}` not found in `{}`",
                if default_set { "the loaded libraries" } else { lib }
            )));
        }
        Ok(addr)
    }

    impl ForeignCalls for NativeFfi {
        fn call(&self, symbol: &str, lib: &str, args: &[CArg], ret: RetPlan) -> Result<Outcome> {
            let addr = resolve(symbol, lib)?;

            let mut kinds: Vec<c_int> = Vec::with_capacity(args.len());
            let mut slots: Vec<u64> = Vec::with_capacity(args.len());
            // NUL-terminated string copies must outlive the call.
            let mut keepalive: Vec<Vec<u8>> = Vec::new();
            for a in args {
                match a {
                    CArg::Int { kind, val } => {
                        kinds.push(*kind);
                        slots.push(*val as u64);
                    }
                    CArg::Ptr(w) => {
                        kinds.push(kind::PTR);
                        slots.push(*w as u64);
                    }
                    CArg::Double(d) => {
                        kinds.push(kind::DOUBLE);
                        slots.push(d.to_bits());
                    }
                    CArg::Float(f) => {
                        kinds.push(kind::FLOAT);
                        slots.push(f.to_bits() as u64);
                    }
                    CArg::Bytes(b) => {
                        let mut c = b.clone();
                        c.push(0);
                        kinds.push(kind::PTR);
                        slots.push(c.as_ptr() as u64);
                        keepalive.push(c);
                    }
                }
            }
            // Build the avalue pointers only once `slots` has stopped growing, so
            // reallocation cannot leave them dangling.
            let mut avalues: Vec<*mut c_void> = slots
                .iter_mut()
                .map(|s| s as *mut u64 as *mut c_void)
                .collect();

            let mut rvalue: u64 = 0;
            let rc = unsafe {
                thx_ffi_call(
                    addr,
                    args.len() as c_int,
                    if kinds.is_empty() {
                        std::ptr::null()
                    } else {
                        kinds.as_ptr()
                    },
                    ret.ffi_kind,
                    if avalues.is_empty() {
                        std::ptr::null_mut()
                    } else {
                        avalues.as_mut_ptr()
                    },
                    &mut rvalue as *mut u64 as *mut c_void,
                )
            };
            drop(keepalive);
            if rc != 0 {
                return Err(fault(format!(
                    "FFI: libffi could not prepare the call to `{symbol}` (too many arguments?)"
                )));
            }
            // Order foreign C stdio against the driver's own output.
            unsafe { fflush(std::ptr::null_mut()) };

            Ok(match ret.wrap {
                Wrap::Unit => Outcome::Unit,
                // libffi widens an integer result into the return word, sign- or
                // zero-extending per the type, so the low word is the value.
                Wrap::Int => Outcome::Int(rvalue as i64),
                Wrap::Real => {
                    if ret.ffi_kind == kind::FLOAT {
                        Outcome::Real(f32::from_bits(rvalue as u32) as f64)
                    } else {
                        Outcome::Real(f64::from_bits(rvalue))
                    }
                }
                Wrap::Bytes => {
                    let p = rvalue as *const c_char;
                    if p.is_null() {
                        Outcome::Bytes(Vec::new())
                    } else {
                        let n = unsafe { strlen(p) };
                        let bytes = unsafe { std::slice::from_raw_parts(p as *const u8, n) };
                        Outcome::Bytes(bytes.to_vec())
                    }
                }
            })
        }
    }

    static NATIVE: NativeFfi = NativeFfi;

    pub fn backend() -> &'static dyn ForeignCalls {
        &NATIVE
    }
}

#[cfg(not(target_arch = "wasm32"))]
use native::backend;

// -- wasm backend: a generic import to the JavaScript embedder ----------------

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::{CArg, ForeignCalls, Outcome, RetPlan};
    use crate::machine::data::fault;
    use utilities::Result;

    // A single generic trampoline carries any call across: the arguments are
    // serialised into one kind-tagged buffer (tag 1 int i64le, 2 real f64le, 3
    // str u32len+bytes), and the host answers with the result's kind, read back
    // through the typed getters. The embedder owns the registry; the compiler
    // knows none of the names. See `web/site/host.mjs`.
    extern "C" {
        fn thx_host_call(sym: *const u8, sym_len: usize, args: *const u8, args_len: usize) -> i32;
        fn thx_host_ret_int() -> i64;
        fn thx_host_ret_real() -> f64;
        fn thx_host_ret_len() -> usize;
        fn thx_host_ret_copy(dst: *mut u8);
    }

    pub struct WasmHostFfi;

    impl ForeignCalls for WasmHostFfi {
        fn call(&self, symbol: &str, _lib: &str, args: &[CArg], _ret: RetPlan) -> Result<Outcome> {
            let mut buf = Vec::new();
            for a in args {
                match a {
                    CArg::Int { val, .. } | CArg::Ptr(val) => {
                        buf.push(1);
                        buf.extend_from_slice(&val.to_le_bytes());
                    }
                    CArg::Double(d) => {
                        buf.push(2);
                        buf.extend_from_slice(&d.to_le_bytes());
                    }
                    CArg::Float(f) => {
                        buf.push(2);
                        buf.extend_from_slice(&(*f as f64).to_le_bytes());
                    }
                    CArg::Bytes(b) => {
                        buf.push(3);
                        buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
                        buf.extend_from_slice(b);
                    }
                }
            }
            unsafe {
                match thx_host_call(symbol.as_ptr(), symbol.len(), buf.as_ptr(), buf.len()) {
                    0 => Ok(Outcome::Unit),
                    1 => Ok(Outcome::Int(thx_host_ret_int())),
                    2 => Ok(Outcome::Real(thx_host_ret_real())),
                    3 => {
                        let mut out = vec![0u8; thx_host_ret_len()];
                        thx_host_ret_copy(out.as_mut_ptr());
                        Ok(Outcome::Bytes(out))
                    }
                    _ => Err(fault(format!(
                        "FFI: the host registers no function `{symbol}` in this environment"
                    ))),
                }
            }
        }
    }

    static WASM: WasmHostFfi = WasmHostFfi;

    pub fn backend() -> &'static dyn ForeignCalls {
        &WASM
    }
}

#[cfg(target_arch = "wasm32")]
use wasm::backend;
