//! The host platform the interpreter runs on, behind one API: loading a shared
//! library, finding a symbol, and the stdio mode. The OS half is `host.c`, where
//! the C headers supply each platform's constants; this side owns the strings.
//! Absent on wasm, which has no loader (`@extern` goes to the JavaScript host).

use std::ffi::CString;
use std::os::raw::{c_char, c_void};

extern "C" {
    fn thx_host_dl_open(name: *const c_char) -> *mut c_void;
    fn thx_host_dl_sym(handle: *mut c_void, sym: *const c_char) -> *mut c_void;
    fn thx_host_stdio_binary();
}

/// A loaded shared library, kept open for the life of the process.
#[derive(Clone, Copy)]
pub struct Library(*mut c_void);

/// Load the library named `name` (a soname, a DLL name, or a path), or `None`.
pub fn open_library(name: &str) -> Option<Library> {
    let name = CString::new(name).ok()?;
    let p = unsafe { thx_host_dl_open(name.as_ptr()) };
    (!p.is_null()).then_some(Library(p))
}

/// The address of `sym` in `lib`, or, for `None`, anywhere in the process'
/// loaded set (the executable and every library loaded so far).
pub fn symbol(lib: Option<Library>, sym: &str) -> Option<*mut c_void> {
    let sym = CString::new(sym).ok()?;
    let handle = lib.map_or(std::ptr::null_mut(), |l| l.0);
    let p = unsafe { thx_host_dl_sym(handle, sym.as_ptr()) };
    (!p.is_null()).then_some(p)
}

/// Stop the C runtime from translating `\n` on stdout and stderr (Windows), so
/// output a foreign call prints matches the interpreter's own byte for byte. A
/// no-op elsewhere.
pub fn stdio_binary() {
    unsafe { thx_host_stdio_binary() };
}
