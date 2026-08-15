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

use crate::machine::data::{fault, mk, PVal, Value};

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
    /// A by-value aggregate: the shim builds an `ffi_type` from the leaf kinds.
    pub const STRUCT: c_int = 12;
}

// The C-repr struct layouts of the running program, keyed by type name, so the
// marshalling code can find a struct argument's or result's memory image. The
// machine installs this once at startup (see `set_layouts`). Empty for a program
// that declares no `@struct @extern` types.
thread_local! {
    static LAYOUTS: std::cell::RefCell<std::collections::HashMap<String, utilities::CLayout>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Install the program's C-repr struct layouts before evaluation.
pub fn set_layouts(map: std::collections::HashMap<String, utilities::CLayout>) {
    LAYOUTS.with(|l| *l.borrow_mut() = map);
}

fn layout_of(name: &str) -> Option<utilities::CLayout> {
    LAYOUTS.with(|l| l.borrow().get(name).cloned())
}

/// The libffi leaf-kind code for a C scalar. A nested struct is flattened into
/// its leaves by [`flatten_leaves`], so it never reaches here.
fn leaf_kind(k: &utilities::CKind) -> std::os::raw::c_int {
    use utilities::CKind::*;
    match k {
        S8 => kind::S8,
        S16 => kind::S16,
        S32 => kind::S32,
        S64 => kind::S64,
        U8 => kind::U8,
        U16 => kind::U16,
        U32 => kind::U32,
        U64 => kind::U64,
        F32 => kind::FLOAT,
        F64 => kind::DOUBLE,
        Struct(..) => kind::STRUCT, // unreachable after flattening
    }
}

/// Flatten a layout to its scalar leaf kinds in memory order. A struct of
/// scalars and a struct of nested structs with the same leaves classify the
/// same under the C ABI, so flattening is safe for building the `ffi_type`.
///
/// A union's members overlap, so it is modelled by eightbytes covering its size:
/// integer eightbytes (matching SysV's "INTEGER wins" merge) unless every member
/// is floating, in which case SSE eightbytes. This is exact for the common
/// integer-containing unions; an all-float or exotic union may misclassify on the
/// libffi path (the C backend, which emits a real `union`, is always exact).
fn flatten_leaves(layout: &utilities::CLayout, out: &mut Vec<std::os::raw::c_int>) {
    if layout.is_union {
        union_leaves(layout, out);
        return;
    }
    for f in &layout.fields {
        match &f.kind {
            utilities::CKind::Struct(_, inner) => flatten_leaves(inner, out),
            leaf => out.push(leaf_kind(leaf)),
        }
    }
}

/// Eightbyte cover of a union's size for the libffi aggregate `ffi_type`.
fn union_leaves(layout: &utilities::CLayout, out: &mut Vec<std::os::raw::c_int>) {
    use utilities::CKind::{F32, F64};
    let all_float = layout
        .fields
        .iter()
        .all(|f| matches!(f.kind, F32 | F64));
    let mut rem = layout.size;
    let (w8, w4) = if all_float {
        (kind::DOUBLE, kind::FLOAT)
    } else {
        (kind::S64, kind::S32)
    };
    while rem >= 8 {
        out.push(w8);
        rem -= 8;
    }
    if rem >= 4 {
        out.push(w4);
        rem -= 4;
    }
    while rem > 0 {
        out.push(kind::S8);
        rem -= 1;
    }
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
    /// A C-repr struct passed by value: `bytes` is its flat memory image and
    /// `leaves` its scalar leaf kinds (for the aggregate `ffi_type`).
    Struct {
        leaves: Vec<std::os::raw::c_int>,
        bytes: Vec<u8>,
    },
    /// A Thrax closure passed to C as a function pointer. `closure` is one owned
    /// (leaked) `Rc` reference to the closure value, erased; the native backend
    /// builds a libffi closure over `arg_kinds`/`ret_kind`, uses its code pointer
    /// as the argument, and releases the reference after the call.
    Callback {
        closure: *const std::ffi::c_void,
        arg_kinds: Vec<std::os::raw::c_int>,
        ret_kind: std::os::raw::c_int,
    },
}

/// A struct-typed result: the leaf kinds (for the return `ffi_type`), the layout
/// and type name (to rebuild the [`Value`]).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct StructRet {
    leaves: Vec<std::os::raw::c_int>,
    layout: utilities::CLayout,
    name: String,
}

/// How to type and interpret an extern's result. The wasm backend reports its
/// own result kind and ignores this, so the fields read as dead there.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub struct RetPlan {
    /// The libffi return kind handed to `ffi_prep_cif`.
    ffi_kind: std::os::raw::c_int,
    wrap: Wrap,
    /// Set when the result is a by-value struct.
    struct_ret: Option<StructRet>,
}

enum Wrap {
    Unit,
    Int,
    Real,
    Bytes,
    Struct,
}

/// A backend's raw result, before it becomes a [`Value`].
pub enum Outcome {
    Unit,
    Int(i64),
    Real(f64),
    Bytes(Vec<u8>),
    /// The flat memory image of a returned by-value struct.
    Struct(Vec<u8>),
}

/// A host's implementation of the `@extern` seam: resolve `symbol` in `lib` and
/// call it with `args`, producing an [`Outcome`]. `ret` describes the wanted
/// result type; a backend whose protocol reports its own result kind (wasm) may
/// ignore it.
pub trait ForeignCalls {
    fn call(&self, symbol: &str, lib: &str, args: &[CArg], ret: RetPlan) -> Result<Outcome>;
}

// -- callbacks: a Thrax closure passed to C as a function pointer -------------

/// The context handed to the libffi closure (as its user-data): the erased Thrax
/// closure reference and the callback's scalar signature.
struct CbCtx {
    closure: *const std::ffi::c_void,
    arg_kinds: Vec<std::os::raw::c_int>,
    ret_kind: std::os::raw::c_int,
}

/// The machine-provided applier: given the closure, the C argument words and
/// their kinds, and the result kind, apply the Thrax closure and return the
/// result's bits. Installed for the duration of an extern call by [`with_applier`].
type ApplyDyn<'a> =
    dyn Fn(*const std::ffi::c_void, &[u64], &[std::os::raw::c_int], std::os::raw::c_int) -> Result<u64>
        + 'a;

thread_local! {
    /// A lifetime-erased pointer to the current applier (valid only within the
    /// `with_applier` scope that set it; callbacks fire synchronously inside it).
    static APPLIER: std::cell::Cell<Option<*const ApplyDyn<'static>>> =
        const { std::cell::Cell::new(None) };
    /// An error raised by a callback (which cannot return a `Result` across C);
    /// drained after the foreign call to become its error.
    static CB_ERR: std::cell::RefCell<Option<utilities::Diagnostic>> =
        const { std::cell::RefCell::new(None) };
}

/// Install `f` as the applier while `thunk` runs (an extern call that may fire
/// callbacks), then restore the previous one.
pub(crate) fn with_applier<R>(f: &ApplyDyn, thunk: impl FnOnce() -> R) -> R {
    let erased: *const ApplyDyn<'static> =
        unsafe { std::mem::transmute::<*const ApplyDyn, *const ApplyDyn<'static>>(f) };
    let prev = APPLIER.with(|c| c.replace(Some(erased)));
    let r = thunk();
    APPLIER.with(|c| c.set(prev));
    r
}

/// Take any error a callback stored during the last foreign call.
fn take_cb_err() -> Option<utilities::Diagnostic> {
    CB_ERR.with(|c| c.borrow_mut().take())
}

/// A C argument word (per libffi kind) as a Thrax value: floats read their bits,
/// everything else (sized ints, pointers) is an integer.
pub(crate) fn word_to_value<'p>(w: u64, k: std::os::raw::c_int) -> Value<'p> {
    match k {
        kind::DOUBLE => Value::Real(f64::from_bits(w)),
        kind::FLOAT => Value::Real(f32::from_bits(w as u32) as f64),
        _ => Value::Int(w as i64),
    }
}

/// A Thrax value as a C result word (per libffi kind).
pub(crate) fn value_to_word(v: &Value, k: std::os::raw::c_int) -> u64 {
    match k {
        kind::DOUBLE => match v {
            Value::Real(r) => r.to_bits(),
            Value::Int(n) => (*n as f64).to_bits(),
            _ => 0,
        },
        kind::FLOAT => match v {
            Value::Real(r) => (*r as f32).to_bits() as u64,
            Value::Int(n) => (*n as f32).to_bits() as u64,
            _ => 0,
        },
        kind::VOID => 0,
        _ => match v {
            Value::Int(n) => *n as u64,
            Value::Bool(b) => *b as u64,
            _ => 0,
        },
    }
}

/// Parse a callback marshal name `@fn(a,b,...)->r` into libffi arg/return kinds.
fn parse_fn_sig(ty: &str) -> Option<(Vec<std::os::raw::c_int>, std::os::raw::c_int)> {
    let rest = ty.strip_prefix("@fn(")?;
    let (args_part, ret_part) = rest.split_once(")->")?;
    let mut args = Vec::new();
    if !args_part.is_empty() {
        for a in args_part.split(',') {
            args.push(scalar_ffi_kind(a)?);
        }
    }
    Some((args, scalar_ffi_kind(ret_part)?))
}

/// The libffi kind for a scalar marshal name (both friendly and `@`-sigil forms).
fn scalar_ffi_kind(name: &str) -> Option<std::os::raw::c_int> {
    Some(match name {
        "@float64" | "Real" | "Real64" => kind::DOUBLE,
        "@float32" | "Real32" => kind::FLOAT,
        "@ptr" | "Ptr" | "@str" | "@array" | "Str" | "Array" => kind::PTR,
        "@int8" | "Int8" => kind::S8,
        "@int16" | "Int16" => kind::S16,
        "@int32" | "Int32" => kind::S32,
        "@int64" | "Int64" | "Int" => kind::S64,
        "@nat8" | "Nat8" => kind::U8,
        "@nat16" | "Nat16" => kind::U16,
        "@nat32" | "Nat32" => kind::U32,
        "@nat64" | "Nat64" | "Nat" => kind::U64,
        "@bool" | "Bool" => kind::U8,
        "{}" => kind::VOID,
        _ => return None,
    })
}

/// Classify a Thrax argument value against its declared type, mirroring the C
/// backend's `cabi` so both engines marshal identically.
fn classify_arg(ty: &str, v: &PVal) -> Result<CArg> {
    // A function-typed parameter (`@fn(a,b)->r`) is a C function pointer: hold one
    // reference to the Thrax closure so it survives the call; the native backend
    // builds the libffi closure and releases the reference afterwards.
    if let Some((arg_kinds, ret_kind)) = parse_fn_sig(ty) {
        let closure = Rc::into_raw(v.clone()) as *const std::ffi::c_void;
        return Ok(CArg::Callback {
            closure,
            arg_kinds,
            ret_kind,
        });
    }
    // A C-repr struct is passed by value: pack its fields into a flat memory
    // image and collect its leaf kinds for the aggregate `ffi_type`.
    if let Some(layout) = layout_of(ty) {
        let mut leaves = Vec::new();
        flatten_leaves(&layout, &mut leaves);
        let mut bytes = vec![0u8; layout.size];
        pack_struct(&layout, v, &mut bytes)?;
        return Ok(CArg::Struct { leaves, bytes });
    }
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
    if let Some(layout) = layout_of(ty) {
        let mut leaves = Vec::new();
        flatten_leaves(&layout, &mut leaves);
        return RetPlan {
            ffi_kind: kind::STRUCT,
            wrap: Wrap::Struct,
            struct_ret: Some(StructRet {
                leaves,
                layout,
                name: ty.to_string(),
            }),
        };
    }
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
    RetPlan {
        ffi_kind,
        wrap,
        struct_ret: None,
    }
}

/// Pack a C-repr struct value into its flat memory image at the field offsets.
/// A union writes only the members the value carries (each at offset 0), so a
/// value built with one member packs just that member; a struct writes them all.
fn pack_struct(layout: &utilities::CLayout, v: &PVal, bytes: &mut [u8]) -> Result<()> {
    let borrow = v.borrow();
    let fields = match &*borrow {
        Value::Struct { fields, .. } => fields,
        _ => return Err(fault("FFI: expected a C-repr struct argument")),
    };
    if layout.is_union {
        for (name, fv) in fields {
            if let Some(cf) = layout.fields.iter().find(|f| &f.name == name) {
                write_field(&cf.kind, cf.offset, fv, bytes)?;
            }
        }
        return Ok(());
    }
    for cf in &layout.fields {
        let fv = fields
            .iter()
            .find(|(n, _)| n == &cf.name)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| fault(format!("FFI: struct is missing field `{}`", cf.name)))?;
        write_field(&cf.kind, cf.offset, &fv, bytes)?;
    }
    Ok(())
}

/// Write one scalar (or nested struct) field into the memory image at `offset`.
/// Integer widths take the low `n` bytes in native (little-endian) order.
fn write_field(kind: &utilities::CKind, offset: usize, fv: &PVal, bytes: &mut [u8]) -> Result<()> {
    use utilities::CKind::*;
    match kind {
        Struct(_, inner) => pack_struct(inner, fv, &mut bytes[offset..offset + inner.size]),
        F32 => {
            let x = read_real(fv)? as f32;
            bytes[offset..offset + 4].copy_from_slice(&x.to_ne_bytes());
            Ok(())
        }
        F64 => {
            let x = read_real(fv)?;
            bytes[offset..offset + 8].copy_from_slice(&x.to_ne_bytes());
            Ok(())
        }
        leaf => {
            let n = leaf.size();
            let word = (read_word(fv)? as u64).to_ne_bytes();
            bytes[offset..offset + n].copy_from_slice(&word[..n]);
            Ok(())
        }
    }
}

/// Rebuild a [`Value::Struct`] from a returned struct's memory image.
fn unpack_struct<'p>(name: &str, layout: &utilities::CLayout, bytes: &[u8]) -> Value<'p> {
    let fields = layout
        .fields
        .iter()
        .map(|cf| (cf.name.clone(), mk(read_field(&cf.kind, cf.offset, bytes))))
        .collect();
    Value::Struct {
        name: name.to_string(),
        fields,
    }
}

/// Read one scalar (or nested struct) field from a memory image at `offset`.
fn read_field<'p>(kind: &utilities::CKind, offset: usize, bytes: &[u8]) -> Value<'p> {
    use utilities::CKind::*;
    let at = |n: usize| &bytes[offset..offset + n];
    match kind {
        Struct(name, inner) => unpack_struct(name, inner, &bytes[offset..offset + inner.size]),
        F32 => Value::Real(f32::from_ne_bytes(at(4).try_into().unwrap()) as f64),
        F64 => Value::Real(f64::from_ne_bytes(at(8).try_into().unwrap())),
        S8 => Value::Int(bytes[offset] as i8 as i64),
        S16 => Value::Int(i16::from_ne_bytes(at(2).try_into().unwrap()) as i64),
        S32 => Value::Int(i32::from_ne_bytes(at(4).try_into().unwrap()) as i64),
        S64 => Value::Int(i64::from_ne_bytes(at(8).try_into().unwrap())),
        U8 => Value::Int(bytes[offset] as i64),
        U16 => Value::Int(u16::from_ne_bytes(at(2).try_into().unwrap()) as i64),
        U32 => Value::Int(u32::from_ne_bytes(at(4).try_into().unwrap()) as i64),
        U64 => Value::Int(u64::from_ne_bytes(at(8).try_into().unwrap()) as i64),
    }
}

fn read_word(v: &PVal) -> Result<i64> {
    match &*v.borrow() {
        Value::Int(n) => Ok(*n),
        Value::Bool(b) => Ok(*b as i64),
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
    // Keep the struct-return layout to rebuild the value after the call.
    let ret_struct = plan
        .struct_ret
        .as_ref()
        .map(|s| (s.name.clone(), s.layout.clone()));
    let result = backend().call(symbol, lib, &cargs, plan);
    // Release the reference each callback arg held on its Thrax closure.
    for c in &cargs {
        if let CArg::Callback { closure, .. } = c {
            unsafe {
                drop(Rc::from_raw(
                    *closure as *const std::cell::RefCell<Value<'p>>,
                ));
            }
        }
    }
    // A callback that faulted could not return its error across C; surface it now.
    if let Some(e) = take_cb_err() {
        return Err(e);
    }
    let outcome = result?;
    Ok(match outcome {
        Outcome::Unit => Value::Unit,
        Outcome::Int(n) => Value::Int(n),
        Outcome::Real(r) => Value::Real(r),
        Outcome::Bytes(b) => Value::Str(Rc::new(b)),
        Outcome::Struct(bytes) => {
            let (name, layout) = ret_struct.expect("struct outcome implies a struct plan");
            unpack_struct(&name, &layout, &bytes)
        }
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
        fn thx_ffi_call_x(
            fun: *mut c_void,
            nargs: c_int,
            kinds: *const c_int,
            leaf_off: *const c_int,
            leaf_len: *const c_int,
            leaves: *const c_int,
            ret_kind: c_int,
            ret_leaves: *const c_int,
            ret_nleaves: c_int,
            avalues: *mut *mut c_void,
            rvalue: *mut c_void,
        ) -> c_int;
        fn thx_closure_new(
            nargs: c_int,
            kinds: *const c_int,
            ret_kind: c_int,
            user: *mut c_void,
            code_out: *mut *mut c_void,
        ) -> *mut c_void;
        fn thx_closure_free(handle: *mut c_void);
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

    /// Read one C argument (at `p`, of libffi `kind`) into a `u64` bit pattern:
    /// integers sign/zero-extend, a pointer is its word, a float is its bits.
    unsafe fn read_arg_word(p: *mut c_void, kind: c_int) -> u64 {
        match kind {
            k if k == super::kind::S8 => *(p as *const i8) as i64 as u64,
            k if k == super::kind::S16 => *(p as *const i16) as i64 as u64,
            k if k == super::kind::S32 => *(p as *const i32) as i64 as u64,
            k if k == super::kind::S64 => *(p as *const i64) as u64,
            k if k == super::kind::U8 => *(p as *const u8) as u64,
            k if k == super::kind::U16 => *(p as *const u16) as u64,
            k if k == super::kind::U32 => *(p as *const u32) as u64,
            k if k == super::kind::U64 => *(p as *const u64),
            k if k == super::kind::FLOAT => (*(p as *const f32)).to_bits() as u64,
            k if k == super::kind::DOUBLE => (*(p as *const f64)).to_bits(),
            _ => *(p as *const usize) as u64, // PTR
        }
    }

    /// Write a callback result (`bits`, per libffi `kind`) into libffi's return
    /// slot `ret` (which is at least `sizeof(ffi_arg)` wide).
    unsafe fn write_ret_word(ret: *mut c_void, kind: c_int, bits: u64) {
        match kind {
            k if k == super::kind::VOID => {}
            k if k == super::kind::FLOAT => *(ret as *mut f32) = f32::from_bits(bits as u32),
            k if k == super::kind::DOUBLE => *(ret as *mut f64) = f64::from_bits(bits),
            _ => *(ret as *mut u64) = bits, // ints and pointers widen to the return word
        }
    }

    /// Called by the shim when foreign C invokes a Thrax closure. Reads the C
    /// arguments, applies the closure through the installed applier, and writes the
    /// result. An error is stashed (a callback cannot return a `Result` across C)
    /// and drained after the foreign call.
    #[no_mangle]
    pub unsafe extern "C" fn thx_closure_invoke(
        user: *mut c_void,
        args: *mut *mut c_void,
        ret: *mut c_void,
    ) {
        let ctx = &*(user as *const super::CbCtx);
        let mut words: Vec<u64> = Vec::with_capacity(ctx.arg_kinds.len());
        for (i, &k) in ctx.arg_kinds.iter().enumerate() {
            words.push(read_arg_word(*args.add(i), k));
        }
        let applier = super::APPLIER.with(|c| c.get());
        let bits = match applier {
            Some(p) => match (*p)(ctx.closure, &words, &ctx.arg_kinds, ctx.ret_kind) {
                Ok(b) => b,
                Err(e) => {
                    super::CB_ERR.with(|c| *c.borrow_mut() = Some(e));
                    0
                }
            },
            None => {
                super::CB_ERR
                    .with(|c| *c.borrow_mut() = Some(fault("FFI: a callback fired with no applier")));
                0
            }
        };
        write_ret_word(ret, ctx.ret_kind, bits);
    }

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

    /// Where an argument's `avalue` cell lives: a scalar slot, or the bytes of a
    /// by-value struct argument (a pointer into `args`).
    enum ArgLoc {
        Slot(usize),
        Struct(usize),
    }

    impl ForeignCalls for NativeFfi {
        fn call(&self, symbol: &str, lib: &str, args: &[CArg], ret: RetPlan) -> Result<Outcome> {
            let addr = resolve(symbol, lib)?;

            let mut kinds: Vec<c_int> = Vec::with_capacity(args.len());
            let mut slots: Vec<u64> = Vec::with_capacity(args.len());
            let mut locs: Vec<ArgLoc> = Vec::with_capacity(args.len());
            // Per struct argument, its leaf kinds concatenated, with each arg's
            // (offset, length) into that flat buffer.
            let mut leaves: Vec<c_int> = Vec::new();
            let mut leaf_off: Vec<c_int> = vec![0; args.len()];
            let mut leaf_len: Vec<c_int> = vec![0; args.len()];
            // NUL-terminated string copies must outlive the call.
            let mut keepalive: Vec<Vec<u8>> = Vec::new();
            // Live libffi closures (handle, context) built for callback args, freed
            // after the call.
            let mut closures: Vec<(*mut c_void, *mut super::CbCtx)> = Vec::new();
            let push_slot = |slots: &mut Vec<u64>, locs: &mut Vec<ArgLoc>, w: u64| {
                locs.push(ArgLoc::Slot(slots.len()));
                slots.push(w);
            };
            for (i, a) in args.iter().enumerate() {
                match a {
                    CArg::Int { kind, val } => {
                        kinds.push(*kind);
                        push_slot(&mut slots, &mut locs, *val as u64);
                    }
                    CArg::Ptr(w) => {
                        kinds.push(kind::PTR);
                        push_slot(&mut slots, &mut locs, *w as u64);
                    }
                    CArg::Double(d) => {
                        kinds.push(kind::DOUBLE);
                        push_slot(&mut slots, &mut locs, d.to_bits());
                    }
                    CArg::Float(f) => {
                        kinds.push(kind::FLOAT);
                        push_slot(&mut slots, &mut locs, f.to_bits() as u64);
                    }
                    CArg::Bytes(b) => {
                        let mut c = b.clone();
                        c.push(0);
                        kinds.push(kind::PTR);
                        push_slot(&mut slots, &mut locs, c.as_ptr() as u64);
                        keepalive.push(c);
                    }
                    CArg::Struct { leaves: ls, .. } => {
                        kinds.push(kind::STRUCT);
                        leaf_off[i] = leaves.len() as c_int;
                        leaf_len[i] = ls.len() as c_int;
                        leaves.extend_from_slice(ls);
                        locs.push(ArgLoc::Struct(i));
                    }
                    CArg::Callback {
                        closure,
                        arg_kinds,
                        ret_kind,
                    } => {
                        let ctx = Box::into_raw(Box::new(super::CbCtx {
                            closure: *closure,
                            arg_kinds: arg_kinds.clone(),
                            ret_kind: *ret_kind,
                        }));
                        let mut code: *mut c_void = std::ptr::null_mut();
                        let handle = unsafe {
                            thx_closure_new(
                                arg_kinds.len() as c_int,
                                arg_kinds.as_ptr(),
                                *ret_kind,
                                ctx as *mut c_void,
                                &mut code,
                            )
                        };
                        if handle.is_null() {
                            unsafe { drop(Box::from_raw(ctx)) };
                            return Err(fault("FFI: could not build a callback closure"));
                        }
                        closures.push((handle, ctx));
                        kinds.push(kind::PTR);
                        push_slot(&mut slots, &mut locs, code as u64);
                    }
                }
            }
            // Build the avalue pointers once `slots` has stopped growing, so a
            // reallocation cannot leave them dangling. A struct's cell points at
            // its (borrowed) bytes; a scalar's at its slot.
            let slot_base = slots.as_mut_ptr();
            let mut avalues: Vec<*mut c_void> = locs
                .iter()
                .map(|loc| match loc {
                    ArgLoc::Slot(idx) => unsafe { slot_base.add(*idx) as *mut c_void },
                    ArgLoc::Struct(i) => match &args[*i] {
                        CArg::Struct { bytes, .. } => bytes.as_ptr() as *mut c_void,
                        _ => unreachable!("struct location implies a struct arg"),
                    },
                })
                .collect();

            // The return buffer: 8-aligned (a `u64` vector), sized for a struct
            // result or one word for a scalar.
            let ret_size = ret.struct_ret.as_ref().map(|s| s.layout.size).unwrap_or(8);
            let ret_leaves: &[c_int] = ret.struct_ret.as_ref().map(|s| s.leaves.as_slice()).unwrap_or(&[]);
            let mut rbuf: Vec<u64> = vec![0; ret_size.max(8).div_ceil(8)];

            let rc = unsafe {
                thx_ffi_call_x(
                    addr,
                    args.len() as c_int,
                    if kinds.is_empty() { std::ptr::null() } else { kinds.as_ptr() },
                    leaf_off.as_ptr(),
                    leaf_len.as_ptr(),
                    if leaves.is_empty() { std::ptr::null() } else { leaves.as_ptr() },
                    ret.ffi_kind,
                    if ret_leaves.is_empty() { std::ptr::null() } else { ret_leaves.as_ptr() },
                    ret_leaves.len() as c_int,
                    if avalues.is_empty() { std::ptr::null_mut() } else { avalues.as_mut_ptr() },
                    rbuf.as_mut_ptr() as *mut c_void,
                )
            };
            drop(keepalive);
            for (handle, ctx) in closures {
                unsafe {
                    thx_closure_free(handle);
                    drop(Box::from_raw(ctx));
                }
            }
            if rc != 0 {
                return Err(fault(format!(
                    "FFI: libffi could not prepare the call to `{symbol}` (too many arguments, \
                     or a struct with too many fields?)"
                )));
            }
            // Order foreign C stdio against the driver's own output.
            unsafe { fflush(std::ptr::null_mut()) };

            let rvalue = rbuf[0];
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
                Wrap::Struct => {
                    let bytes =
                        unsafe { std::slice::from_raw_parts(rbuf.as_ptr() as *const u8, ret_size) };
                    Outcome::Struct(bytes.to_vec())
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
                    CArg::Struct { .. } => {
                        return Err(fault(
                            "FFI: by-value C structs are not supported over the wasm host seam",
                        ));
                    }
                    CArg::Callback { .. } => {
                        return Err(fault(
                            "FFI: callbacks are not supported over the wasm host seam",
                        ));
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
