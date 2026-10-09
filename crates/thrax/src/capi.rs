//! The C API of `libthrax` (declared in `include/thrax.h`): the compiler's
//! lexer, parser and `@eval` for C callers, chiefly a native Thrax program
//! running one of those builtins.
//!
//! Values cross as an owned tree of [`ThraxValue`], the C mirror of
//! [`OwnedValue`]. The library allocates every tree and error string it hands
//! out, and only [`thrax_value_free`] / [`thrax_string_free`] may release them.

use std::ffi::{c_char, c_int, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use interpreter::machine::data::{check_fragment, lex, Fragment};
use interpreter::machine::OwnedValue;

/// `thrax_kind`. The discriminants are the header's.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThraxKind {
    Unit = 0,
    Int = 1,
    Float64 = 2,
    Float32 = 3,
    Bool = 4,
    Str = 5,
    Tuple = 6,
    Struct = 7,
    Variant = 8,
    Vec = 9,
}

/// `thrax_value`. Which fields are meaningful depends on `kind` (see the
/// header); the rest are zero or null.
#[repr(C)]
pub struct ThraxValue {
    pub kind: ThraxKind,
    pub int_: i64,
    pub real: f64,
    pub bytes: *const u8,
    pub name: *const c_char,
    pub tag: *const c_char,
    pub keys: *const *const c_char,
    pub items: *const ThraxValue,
    pub len: usize,
}

impl ThraxValue {
    fn leaf(kind: ThraxKind) -> ThraxValue {
        ThraxValue {
            kind,
            int_: 0,
            real: 0.0,
            bytes: ptr::null(),
            name: ptr::null(),
            tag: ptr::null(),
            keys: ptr::null(),
            items: ptr::null(),
            len: 0,
        }
    }
}

/// A C string the library owns. Interior NULs cannot occur in a type, tag or
/// field name, but are dropped rather than trusted.
fn c_name(s: &str) -> *const c_char {
    CString::new(s.replace('\0', "")).expect("NULs removed").into_raw()
}

/// Leak `items` as a boxed slice, returning its pointer and length.
fn leak_slice<T>(items: Vec<T>) -> (*const T, usize) {
    let boxed = items.into_boxed_slice();
    let len = boxed.len();
    if len == 0 {
        return (ptr::null(), 0);
    }
    (Box::into_raw(boxed) as *const T, len)
}

fn to_c(v: &OwnedValue) -> ThraxValue {
    match v {
        OwnedValue::Unit => ThraxValue::leaf(ThraxKind::Unit),
        OwnedValue::Int(n) => ThraxValue { int_: *n, ..ThraxValue::leaf(ThraxKind::Int) },
        OwnedValue::Bool(b) => ThraxValue { int_: *b as i64, ..ThraxValue::leaf(ThraxKind::Bool) },
        OwnedValue::Real(r) => ThraxValue { real: *r, ..ThraxValue::leaf(ThraxKind::Float64) },
        OwnedValue::Real32(r) => {
            ThraxValue { real: *r as f64, ..ThraxValue::leaf(ThraxKind::Float32) }
        }
        OwnedValue::Str(b) => {
            // The trailing NUL is slack past `len`, so `bytes` is also a C string.
            let mut buf = b.clone();
            buf.push(0);
            let (bytes, _) = leak_slice(buf);
            ThraxValue { bytes, len: b.len(), ..ThraxValue::leaf(ThraxKind::Str) }
        }
        OwnedValue::Tuple(items) => seq(ThraxKind::Tuple, items),
        OwnedValue::Vector(items) => seq(ThraxKind::Vec, items),
        OwnedValue::Struct { name, fields } => {
            let keys: Vec<*const c_char> = fields.iter().map(|(k, _)| c_name(k)).collect();
            let items: Vec<ThraxValue> = fields.iter().map(|(_, v)| to_c(v)).collect();
            let (keys, _) = leak_slice(keys);
            let (items, len) = leak_slice(items);
            ThraxValue { name: c_name(name), keys, items, len, ..ThraxValue::leaf(ThraxKind::Struct) }
        }
        OwnedValue::Variant { ty, tag, fields } => ThraxValue {
            name: c_name(ty),
            tag: c_name(tag),
            ..seq(ThraxKind::Variant, fields)
        },
    }
}

fn seq(kind: ThraxKind, items: &[OwnedValue]) -> ThraxValue {
    let (items, len) = leak_slice(items.iter().map(to_c).collect());
    ThraxValue { items, len, ..ThraxValue::leaf(kind) }
}

/// Release everything `v` owns (not `v` itself).
///
/// # Safety
/// `v` must have been built by [`to_c`] and not released before.
unsafe fn release(v: &ThraxValue) {
    let free_name = |p: *const c_char| {
        if !p.is_null() {
            drop(CString::from_raw(p as *mut c_char));
        }
    };
    free_name(v.name);
    free_name(v.tag);
    if !v.bytes.is_null() {
        let slice = ptr::slice_from_raw_parts_mut(v.bytes as *mut u8, v.len + 1);
        drop(Box::from_raw(slice));
    }
    if !v.keys.is_null() {
        let keys = Box::from_raw(ptr::slice_from_raw_parts_mut(v.keys as *mut *const c_char, v.len));
        for &k in keys.iter() {
            free_name(k);
        }
    }
    if !v.items.is_null() {
        let items = Box::from_raw(ptr::slice_from_raw_parts_mut(v.items as *mut ThraxValue, v.len));
        for item in items.iter() {
            release(item);
        }
    }
}

/// Store `msg` in `*err` when the caller asked for it.
///
/// # Safety
/// `err` is null or valid for one pointer write.
unsafe fn set_err(err: *mut *mut c_char, msg: &str) {
    if !err.is_null() {
        *err = CString::new(msg.replace('\0', "")).expect("NULs removed").into_raw();
    }
}

/// The UTF-8 source `src[..len]`, or an error message.
///
/// # Safety
/// `src` is valid for `len` bytes (or `len` is 0).
unsafe fn source<'a>(src: *const u8, len: usize) -> Result<&'a str, String> {
    if len == 0 {
        return Ok("");
    }
    if src.is_null() {
        return Err("null source".to_string());
    }
    std::str::from_utf8(std::slice::from_raw_parts(src, len))
        .map_err(|_| "the source is not valid UTF-8".to_string())
}

/// Run `f`, turning an error or a panic into `*err`; a panic must not unwind into
/// C.
///
/// # Safety
/// `err` is null or valid for one pointer write.
unsafe fn guarded<T>(err: *mut *mut c_char, f: impl FnOnce() -> Result<T, String>) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => Some(v),
        Ok(Err(msg)) => {
            set_err(err, &msg);
            None
        }
        Err(_) => {
            set_err(err, "internal compiler error (panic)");
            None
        }
    }
}

fn boxed(v: OwnedValue) -> *mut ThraxValue {
    Box::into_raw(Box::new(to_c(&v)))
}

/// # Safety
/// `src` is valid for `len` bytes; `err` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn thrax_lex(src: *const u8, len: usize, err: *mut *mut c_char) -> *mut ThraxValue {
    guarded(err, || {
        let src = source(src, len)?;
        lex(src).map(boxed).map_err(|d| d.render(src, "@lex"))
    })
    .unwrap_or(ptr::null_mut())
}

/// # Safety
/// `src` is valid for `len` bytes; `err` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn thrax_parse(
    src: *const u8,
    len: usize,
    items: c_int,
    err: *mut *mut c_char,
) -> c_int {
    let kind = if items != 0 { Fragment::Items } else { Fragment::Expr };
    let ok = guarded(err, || {
        let src = source(src, len)?;
        check_fragment(src, kind).map_err(|d| d.render(src, "@parse"))
    });
    if ok.is_some() {
        0
    } else {
        1
    }
}

/// # Safety
/// `src` is valid for `len` bytes; `err` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn thrax_eval(src: *const u8, len: usize, err: *mut *mut c_char) -> *mut ThraxValue {
    guarded(err, || {
        let src = source(src, len)?;
        let root = std::env::current_dir().unwrap_or_default();
        // A nested `@eval` inside the fragment needs the host too.
        crate::driver::install_eval_host(root.clone());
        crate::driver::eval_fragment(src, &root).map(boxed)
    })
    .unwrap_or(ptr::null_mut())
}

/// # Safety
/// `v` is null or a tree this library returned, not freed before.
#[no_mangle]
pub unsafe extern "C" fn thrax_value_free(v: *mut ThraxValue) {
    if !v.is_null() {
        let v = Box::from_raw(v);
        release(&v);
    }
}

/// # Safety
/// `s` is null or an error string this library returned, not freed before.
#[no_mangle]
pub unsafe extern "C" fn thrax_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    fn cstr<'a>(p: *const c_char) -> &'a str {
        unsafe { CStr::from_ptr(p) }.to_str().unwrap()
    }

    #[test]
    fn lex_returns_token_structs() {
        let src = b"1 + foo";
        let v = unsafe { thrax_lex(src.as_ptr(), src.len(), ptr::null_mut()) };
        assert!(!v.is_null());
        let v = unsafe { &*v };
        assert_eq!(v.kind, ThraxKind::Vec);
        assert_eq!(v.len, 3);
        let third = unsafe { &*v.items.add(2) };
        assert_eq!(third.kind, ThraxKind::Struct);
        assert_eq!(cstr(third.name), "@token");
        let text = unsafe { &*third.items.add(1) };
        assert_eq!(cstr(unsafe { *third.keys.add(1) }), "text");
        assert_eq!(cstr(text.bytes as *const c_char), "foo");
        unsafe { thrax_value_free(v as *const _ as *mut _) };
    }

    #[test]
    fn parse_reports_syntax_errors() {
        let ok = b"1 + 2";
        assert_eq!(unsafe { thrax_parse(ok.as_ptr(), ok.len(), 0, ptr::null_mut()) }, 0);
        let bad = b"1 + + )";
        let mut err: *mut c_char = ptr::null_mut();
        assert_eq!(unsafe { thrax_parse(bad.as_ptr(), bad.len(), 0, &mut err) }, 1);
        assert!(!err.is_null());
        unsafe { thrax_string_free(err) };
    }

    #[test]
    fn eval_runs_a_fragment() {
        let src = b"{6 * 7, \"hi\"}";
        let mut err: *mut c_char = ptr::null_mut();
        let v = unsafe { thrax_eval(src.as_ptr(), src.len(), &mut err) };
        assert!(err.is_null(), "{}", cstr(err));
        let v = unsafe { &*v };
        assert_eq!(v.kind, ThraxKind::Tuple);
        let (a, b) = unsafe { (&*v.items, &*v.items.add(1)) };
        assert_eq!((a.kind, a.int_), (ThraxKind::Int, 42));
        assert_eq!(cstr(b.bytes as *const c_char), "hi");
        unsafe { thrax_value_free(v as *const _ as *mut _) };
    }

    #[test]
    fn eval_rejects_a_function_result() {
        let src = b"\\x = x";
        let mut err: *mut c_char = ptr::null_mut();
        let v = unsafe { thrax_eval(src.as_ptr(), src.len(), &mut err) };
        assert!(v.is_null());
        assert!(!err.is_null());
        unsafe { thrax_string_free(err) };
    }
}
