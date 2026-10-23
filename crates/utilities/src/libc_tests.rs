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
    let (args, ret) = exact_types(LINUX, "libc", "fgetc", &strs(&["@int"]), "@int").unwrap();
    assert_eq!(args, strs(&["@ptr"]));
    assert_eq!(ret, "@int32");
}

#[test]
fn size_t_and_long_follow_the_target() {
    let write = strs(&["@int", "@str", "@int"]);
    let (args, ret) = exact_types(WASM, "libc", "write", &write, "@int").unwrap();
    assert_eq!(args, strs(&["@int32", "@str", "@nat32"]));
    assert_eq!(ret, "@int32");
    let (args, _) = exact_types(LINUX, "libc", "write", &write, "@int").unwrap();
    assert_eq!(args[2], "@nat64");
    assert_eq!(exact_types(LINUX, "libc", "ftell", &strs(&["@int"]), "@int").unwrap().1, "@int64");
    assert_eq!(exact_types(WINDOWS, "libc", "ftell", &strs(&["@int"]), "@int").unwrap().1, "@int32");
}

#[test]
fn non_integer_slots_and_nullary_placeholders_are_kept() {
    let (args, ret) = exact_types(LINUX, "libc", "getchar", &strs(&["{}"]), "@int").unwrap();
    assert_eq!(args, strs(&["{}"]));
    assert_eq!(ret, "@int32");
    let (args, ret) = exact_types(LINUX, "libc", "fopen", &strs(&["@str", "@str"]), "@int").unwrap();
    assert_eq!(args, strs(&["@str", "@str"]));
    assert_eq!(ret, "@ptr");
}

#[test]
fn other_libraries_and_arities_are_left_alone() {
    assert!(exact_types(LINUX, "raylib", "fgetc", &strs(&["@int"]), "@int").is_none());
    assert!(exact_types(LINUX, "libc", "no_such_fn", &strs(&["@int"]), "@int").is_none());
    assert!(exact_types(LINUX, "libc", "fgetc", &strs(&["@int", "@int"]), "@int").is_none());
}
