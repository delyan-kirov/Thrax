//! The C standard library's real prototypes, for the integer slots an `@extern`
//! types loosely.
//!
//! `library/C.thx` gives libc functions the friendly `@Int` for every integer
//! and handle, but C does not: `fgetc` returns a 32-bit `int`, `write` takes a
//! `size_t`, a `FILE*` is a pointer, and `long` is 32 bits on Windows. Calling
//! across that gap reads the upper half of a narrow return as garbage (so EOF is
//! 4294967295, not -1), and on wasm, where every call is type-checked, it links
//! a trapping stub instead of the function. [`exact_types`] rewrites such a
//! call's integer slots to the sized types the target's C library really uses,
//! so both engines marshal it exactly; the Thrax-facing types stay as written.

use crate::target::{Os, Target};

/// One C parameter or result, as far as marshalling cares.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum C {
    /// `int`
    Int,
    /// `unsigned int`
    UInt,
    /// `long`, whose width differs between targets.
    Long,
    /// `unsigned long`
    ULong,
    /// `size_t`, the target's word, unsigned.
    Size,
    /// `ssize_t`, the target's word, signed.
    SSize,
    /// `time_t`, 64 bits on every target Thrax builds for.
    Time,
    /// `clock_t`, a `long`.
    Clock,
    /// Any pointer that a Thrax binding may carry as an integer (`FILE*`).
    Ptr,
    /// Anything a binding cannot get wrong with an integer: `char*` (`@Str`),
    /// `double`, `void`.
    Other,
}

use C::*;

/// The functions `library/C.thx` (and the corpus) bind, with their prototypes:
/// `(symbol, result, parameters)`.
const PROTOTYPES: &[(&str, C, &[C])] = &[
    // stdlib.h
    ("malloc", Ptr, &[Size]),
    ("calloc", Ptr, &[Size, Size]),
    ("realloc", Ptr, &[Ptr, Size]),
    ("free", Other, &[Ptr]),
    ("atoi", Int, &[Other]),
    ("rand", Int, &[]),
    ("srand", Other, &[UInt]),
    ("exit", Other, &[Int]),
    ("system", Int, &[Other]),
    // string.h
    ("strlen", Size, &[Other]),
    ("strcmp", Int, &[Other, Other]),
    ("strncmp", Int, &[Other, Other, Size]),
    ("strchr", Ptr, &[Other, Int]),
    ("strrchr", Ptr, &[Other, Int]),
    ("strstr", Ptr, &[Other, Other]),
    ("memcpy", Ptr, &[Ptr, Ptr, Size]),
    ("memmove", Ptr, &[Ptr, Ptr, Size]),
    ("memset", Ptr, &[Ptr, Int, Size]),
    ("memcmp", Int, &[Ptr, Ptr, Size]),
    ("memchr", Ptr, &[Ptr, Int, Size]),
    // ctype.h
    ("isalpha", Int, &[Int]),
    ("isdigit", Int, &[Int]),
    ("isalnum", Int, &[Int]),
    ("isspace", Int, &[Int]),
    ("isupper", Int, &[Int]),
    ("islower", Int, &[Int]),
    ("ispunct", Int, &[Int]),
    ("iscntrl", Int, &[Int]),
    ("isprint", Int, &[Int]),
    ("toupper", Int, &[Int]),
    ("tolower", Int, &[Int]),
    // stdio.h
    ("puts", Int, &[Other]),
    ("putchar", Int, &[Int]),
    ("getchar", Int, &[]),
    ("fopen", Ptr, &[Other, Other]),
    ("fclose", Int, &[Ptr]),
    ("fgetc", Int, &[Ptr]),
    ("fputc", Int, &[Int, Ptr]),
    ("fputs", Int, &[Other, Ptr]),
    ("fwrite", Size, &[Ptr, Size, Size, Ptr]),
    ("fflush", Int, &[Ptr]),
    ("fseek", Int, &[Ptr, Long, Int]),
    ("ftell", Long, &[Ptr]),
    ("remove", Int, &[Other]),
    ("rename", Int, &[Other, Other]),
    ("setvbuf", Int, &[Ptr, Ptr, Int, Size]),
    ("popen", Ptr, &[Other, Other]),
    ("pclose", Int, &[Ptr]),
    ("fileno", Int, &[Ptr]),
    // unistd.h, time.h, poll.h
    ("write", SSize, &[Int, Ptr, Size]),
    ("poll", Int, &[Ptr, ULong, Int]),
    ("time", Time, &[Ptr]),
    ("clock", Clock, &[]),
];

/// Whether `lib` names the C library (or its math half), the only library
/// whose prototypes [`PROTOTYPES`] describes.
fn is_libc(lib: &str) -> bool {
    matches!(lib, "libc" | "c" | "libm" | "m")
}

fn is_integer(ty: &str) -> bool {
    ty == "@Int" || ty == "@Nat" || ty.starts_with("@Int") || ty.starts_with("@Nat")
}

/// The sized Thrax type `c` is on `target`, or `None` to keep the binding's own.
fn sized(target: Target, c: C) -> Option<&'static str> {
    let word = target.ptr_bits() == 64;
    let long64 = word && target.os != Os::Windows;
    Some(match c {
        Int => "@Int32",
        UInt => "@Nat32",
        Long | Clock if long64 => "@Int64",
        Long | Clock => "@Int32",
        ULong if long64 => "@Nat64",
        ULong => "@Nat32",
        Size if word => "@Nat64",
        Size => "@Nat32",
        SSize if word => "@Int64",
        SSize => "@Int32",
        Time => "@Int64",
        Ptr => "@Ptr",
        Other => return None,
    })
}

fn exact(target: Target, c: C, ty: &str) -> String {
    match sized(target, c) {
        Some(s) if is_integer(ty) => s.to_string(),
        _ => ty.to_string(),
    }
}

/// The argument and result types to marshal a call to `symbol` in `lib` with on
/// `target`: the binding's own types, except that an integer slot of a known C
/// library function takes the C type's real width. `None` when nothing changes
/// (not the C library, an unknown function, or an arity that does not match).
pub fn exact_types(
    target: Target,
    lib: &str,
    symbol: &str,
    args: &[String],
    ret: &str,
) -> Option<(Vec<String>, String)> {
    if !is_libc(lib) {
        return None;
    }
    let &(_, c_ret, c_args) = PROTOTYPES.iter().find(|(s, _, _)| *s == symbol)?;
    // A nullary C function's binding takes `{}`, which is not a C argument.
    let real: Vec<&String> = args.iter().filter(|t| *t != "{}").collect();
    if real.len() != c_args.len() {
        return None;
    }
    let mut c = c_args.iter();
    let args = args
        .iter()
        .map(|t| if t == "{}" { t.clone() } else { exact(target, *c.next().unwrap(), t) })
        .collect();
    Some((args, exact(target, c_ret, ret)))
}
