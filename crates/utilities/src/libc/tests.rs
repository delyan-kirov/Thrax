//! Tests for [`crate::libc`].

use crate::libc::exact_types;
use crate::target::{Arch, Os, Target};

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

const LINUX: Target = Target { os: Os::Linux, arch: Arch::X86_64 };
const WINDOWS: Target = Target { os: Os::Windows, arch: Arch::X86_64 };
const WASM: Target = Target { os: Os::Wasi, arch: Arch::Wasm32 };

#[test]
fn a_narrow_c_int_return_is_marshalled_at_its_width() {
    let (args, ret) = exact_types(LINUX, "libc", "fgetc", &strs(&["@Int"]), "@Int").unwrap();
    assert_eq!(args, strs(&["@Ptr"]));
    assert_eq!(ret, "@Int32");
}

#[test]
fn size_t_and_long_follow_the_target() {
    let write = strs(&["@Int", "@Str", "@Int"]);
    let (args, ret) = exact_types(WASM, "libc", "write", &write, "@Int").unwrap();
    assert_eq!(args, strs(&["@Int32", "@Str", "@Nat32"]));
    assert_eq!(ret, "@Int32");
    let (args, _) = exact_types(LINUX, "libc", "write", &write, "@Int").unwrap();
    assert_eq!(args[2], "@Nat64");
    assert_eq!(exact_types(LINUX, "libc", "ftell", &strs(&["@Int"]), "@Int").unwrap().1, "@Int64");
    assert_eq!(exact_types(WINDOWS, "libc", "ftell", &strs(&["@Int"]), "@Int").unwrap().1, "@Int32");
}

#[test]
fn non_integer_slots_and_nullary_placeholders_are_kept() {
    let (args, ret) = exact_types(LINUX, "libc", "getchar", &strs(&["{}"]), "@Int").unwrap();
    assert_eq!(args, strs(&["{}"]));
    assert_eq!(ret, "@Int32");
    let (args, ret) = exact_types(LINUX, "libc", "fopen", &strs(&["@Str", "@Str"]), "@Int").unwrap();
    assert_eq!(args, strs(&["@Str", "@Str"]));
    assert_eq!(ret, "@Ptr");
}

#[test]
fn other_libraries_and_arities_are_left_alone() {
    assert!(exact_types(LINUX, "raylib", "fgetc", &strs(&["@Int"]), "@Int").is_none());
    assert!(exact_types(LINUX, "libc", "no_such_fn", &strs(&["@Int"]), "@Int").is_none());
    assert!(exact_types(LINUX, "libc", "fgetc", &strs(&["@Int", "@Int"]), "@Int").is_none());
}
