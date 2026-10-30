//! Tests for [`crate::libc`].

use crate::libc::exact_types;
use crate::target::{Arch, Os, Target};
use crate::StrId;

fn strs(v: &[&str]) -> Vec<StrId> {
    v.iter().map(|s| StrId::intern(s)).collect()
}

const LINUX: Target = Target { os: Os::Linux, arch: Arch::X86_64 };
const WINDOWS: Target = Target { os: Os::Windows, arch: Arch::X86_64 };
const WASM: Target = Target { os: Os::Wasi, arch: Arch::Wasm32 };

#[test]
fn a_narrow_c_int_return_is_marshalled_at_its_width() {
    let (args, ret) = exact_types(LINUX, "libc".into(), "fgetc".into(), &strs(&["@Int"]), "@Int".into()).unwrap();
    assert_eq!(args, strs(&["@Ptr"]));
    assert_eq!(ret, "@Int32");
}

#[test]
fn size_t_and_long_follow_the_target() {
    let write = strs(&["@Int", "@Str", "@Int"]);
    let (args, ret) = exact_types(WASM, "libc".into(), "write".into(), &write, "@Int".into()).unwrap();
    assert_eq!(args, strs(&["@Int32", "@Str", "@Nat32"]));
    assert_eq!(ret, "@Int32");
    let (args, _) = exact_types(LINUX, "libc".into(), "write".into(), &write, "@Int".into()).unwrap();
    assert_eq!(args[2], "@Nat64");
    assert_eq!(exact_types(LINUX, "libc".into(), "ftell".into(), &strs(&["@Int"]), "@Int".into()).unwrap().1, "@Int64");
    assert_eq!(exact_types(WINDOWS, "libc".into(), "ftell".into(), &strs(&["@Int"]), "@Int".into()).unwrap().1, "@Int32");
}

#[test]
fn non_integer_slots_and_nullary_placeholders_are_kept() {
    let (args, ret) = exact_types(LINUX, "libc".into(), "getchar".into(), &strs(&["{}"]), "@Int".into()).unwrap();
    assert_eq!(args, strs(&["{}"]));
    assert_eq!(ret, "@Int32");
    let (args, ret) = exact_types(LINUX, "libc".into(), "fopen".into(), &strs(&["@Str", "@Str"]), "@Int".into()).unwrap();
    assert_eq!(args, strs(&["@Str", "@Str"]));
    assert_eq!(ret, "@Ptr");
}

#[test]
fn other_libraries_and_arities_are_left_alone() {
    assert!(exact_types(LINUX, "raylib".into(), "fgetc".into(), &strs(&["@Int"]), "@Int".into()).is_none());
    assert!(exact_types(LINUX, "libc".into(), "no_such_fn".into(), &strs(&["@Int"]), "@Int".into()).is_none());
    assert!(exact_types(LINUX, "libc".into(), "fgetc".into(), &strs(&["@Int", "@Int"]), "@Int".into()).is_none());
}
