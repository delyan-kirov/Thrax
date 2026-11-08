//! The C API of `libares` (declared in `include/ares.h`): the compiler's
//! lexer, parser and `@eval` for C callers, chiefly a native Ares program
//! running one of those builtins.
//!
//! Values cross as an owned tree of [`AresValue`], the C mirror of
//! [`OwnedValue`]. The library allocates every tree and error string it hands
//! out, and only [`ares_value_free`] / [`ares_string_free`] may release them.

use std::ffi::{c_char, c_int, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use interpreter::machine::data::{check_fragment, lex, Fragment};
use interpreter::machine::OwnedValue;

/// `ares_kind`. The discriminants are the header's.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AresKind {
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

/// `ares_value`. Which fields are meaningful depends on `kind` (see the
/// header); the rest are zero or null.
#[repr(C)]
pub struct AresValue {
    pub kind: AresKind,
    pub int_: i64,
    pub real: f64,
    pub bytes: *const u8,
    pub name: *const c_char,
    pub tag: *const c_char,
    pub keys: *const *const c_char,
    pub items: *const AresValue,
    pub len: usize,
}

impl AresValue {
    fn leaf(kind: AresKind) -> AresValue {
        AresValue {
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

fn to_c(v: &OwnedValue) -> AresValue {
    match v {
        OwnedValue::Unit => AresValue::leaf(AresKind::Unit),
        OwnedValue::Int(n) => AresValue { int_: *n, ..AresValue::leaf(AresKind::Int) },
        OwnedValue::Bool(b) => AresValue { int_: *b as i64, ..AresValue::leaf(AresKind::Bool) },
        OwnedValue::Real(r) => AresValue { real: *r, ..AresValue::leaf(AresKind::Float64) },
        OwnedValue::Real32(r) => {
            AresValue { real: *r as f64, ..AresValue::leaf(AresKind::Float32) }
        }
        OwnedValue::Str(b) => {
            // The trailing NUL is slack past `len`, so `bytes` is also a C string.
            let mut buf = b.clone();
            buf.push(0);
            let (bytes, _) = leak_slice(buf);
            AresValue { bytes, len: b.len(), ..AresValue::leaf(AresKind::Str) }
        }
        OwnedValue::Tuple(items) => seq(AresKind::Tuple, items),
        OwnedValue::Vector(items) => seq(AresKind::Vec, items),
        OwnedValue::Struct { name, fields } => {
            let keys: Vec<*const c_char> = fields.iter().map(|(k, _)| c_name(k.as_str())).collect();
            let items: Vec<AresValue> = fields.iter().map(|(_, v)| to_c(v)).collect();
            let (keys, _) = leak_slice(keys);
            let (items, len) = leak_slice(items);
            AresValue { name: c_name(name.as_str()), keys, items, len, ..AresValue::leaf(AresKind::Struct) }
        }
        OwnedValue::Variant { ty, tag, fields } => AresValue {
            name: c_name(ty.as_str()),
            tag: c_name(tag.as_str()),
            ..seq(AresKind::Variant, fields)
        },
    }
}

fn seq(kind: AresKind, items: &[OwnedValue]) -> AresValue {
    let (items, len) = leak_slice(items.iter().map(to_c).collect());
    AresValue { items, len, ..AresValue::leaf(kind) }
}

/// Release everything `v` owns (not `v` itself).
///
/// # Safety
/// `v` must have been built by [`to_c`] and not released before.
unsafe fn release(v: &AresValue) {
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
        let items = Box::from_raw(ptr::slice_from_raw_parts_mut(v.items as *mut AresValue, v.len));
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

fn boxed(v: OwnedValue) -> *mut AresValue {
    Box::into_raw(Box::new(to_c(&v)))
}

/// # Safety
/// `src` is valid for `len` bytes; `err` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn ares_lex(src: *const u8, len: usize, err: *mut *mut c_char) -> *mut AresValue {
    guarded(err, || {
        let src = source(src, len)?;
        lex(src).map(boxed).map_err(|d| d.render(src, "@lex"))
    })
    .unwrap_or(ptr::null_mut())
}

/// # Safety
/// `src` is valid for `len` bytes; `err` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn ares_parse(
    src: *const u8,
    len: usize,
    kind: c_int,
    err: *mut *mut c_char,
) -> c_int {
    let kind = match kind {
        0 => Fragment::Expr,
        1 => Fragment::Items,
        2 => Fragment::Type,
        _ => Fragment::Row,
    };
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
pub unsafe extern "C" fn ares_format(src: *const u8, len: usize, err: *mut *mut c_char) -> *mut AresValue {
    guarded(err, || {
        let src = source(src, len)?;
        let out = frontend::format::format_source(src, &Default::default())?;
        Ok(boxed(OwnedValue::Str(out.into_bytes())))
    })
    .unwrap_or(ptr::null_mut())
}

/// # Safety
/// `src` is valid for `len` bytes; `err` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn ares_eval(src: *const u8, len: usize, err: *mut *mut c_char) -> *mut AresValue {
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
pub unsafe extern "C" fn ares_value_free(v: *mut AresValue) {
    if !v.is_null() {
        let v = Box::from_raw(v);
        release(&v);
    }
}

/// # Safety
/// `s` is null or an error string this library returned, not freed before.
#[no_mangle]
pub unsafe extern "C" fn ares_string_free(s: *mut c_char) {
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
    fn lex_returns_token_variants() {
        let src = b"1 + foo";
        let v = unsafe { ares_lex(src.as_ptr(), src.len(), ptr::null_mut()) };
        assert!(!v.is_null());
        let v = unsafe { &*v };
        assert_eq!(v.kind, AresKind::Vec);
        assert_eq!(v.len, 3);
        let third = unsafe { &*v.items.add(2) };
        assert_eq!(third.kind, AresKind::Variant);
        assert_eq!(cstr(third.name), "Token");
        assert_eq!(cstr(third.tag), "Word");
        assert_eq!(third.len, 1);
        let text = unsafe { &*third.items };
        assert_eq!(cstr(text.bytes as *const c_char), "foo");
        unsafe { ares_value_free(v as *const _ as *mut _) };
    }

    #[test]
    fn parse_reports_syntax_errors() {
        let ok = b"1 + 2";
        assert_eq!(unsafe { ares_parse(ok.as_ptr(), ok.len(), 0, ptr::null_mut()) }, 0);
        let bad = b"1 + + )";
        let mut err: *mut c_char = ptr::null_mut();
        assert_eq!(unsafe { ares_parse(bad.as_ptr(), bad.len(), 0, &mut err) }, 1);
        assert!(!err.is_null());
        unsafe { ares_string_free(err) };
    }

    #[test]
    fn eval_runs_a_fragment() {
        let src = b"{6 * 7, \"hi\"}";
        let mut err: *mut c_char = ptr::null_mut();
        let v = unsafe { ares_eval(src.as_ptr(), src.len(), &mut err) };
        assert!(err.is_null(), "{}", cstr(err));
        let v = unsafe { &*v };
        assert_eq!(v.kind, AresKind::Tuple);
        let (a, b) = unsafe { (&*v.items, &*v.items.add(1)) };
        assert_eq!((a.kind, a.int_), (AresKind::Int, 42));
        assert_eq!(cstr(b.bytes as *const c_char), "hi");
        unsafe { ares_value_free(v as *const _ as *mut _) };
    }

    #[test]
    fn eval_rejects_a_function_result() {
        let src = b"\\x => x";
        let mut err: *mut c_char = ptr::null_mut();
        let v = unsafe { ares_eval(src.as_ptr(), src.len(), &mut err) };
        assert!(v.is_null());
        assert!(!err.is_null());
        unsafe { ares_string_free(err) };
    }
}
