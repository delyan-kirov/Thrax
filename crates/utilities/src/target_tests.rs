use super::target::*;

#[test]
fn name_and_parse_round_trip() {
    for os in [Os::Linux, Os::Macos, Os::Windows, Os::Wasi] {
        for arch in [Arch::X86_64, Arch::Aarch64, Arch::X86, Arch::Arm, Arch::Wasm32] {
            let t = Target { os, arch };
            assert_eq!(Target::parse(&t.name()), Some(t), "{}", t.name());
        }
    }
    assert_eq!(Target::parse("nonsense"), None);
}

#[test]
fn word_size_follows_arch() {
    let linux64 = Target { os: Os::Linux, arch: Arch::X86_64 };
    assert_eq!(linux64.ptr_bits(), 64);
    assert_eq!(linux64.int_ty(), "@int64");
    assert_eq!(linux64.nat_ty(), "@nat64");
    assert_eq!(linux64.int_max(), 0x7fff_ffff_ffff_ffff);

    let wasm = Target { os: Os::Wasi, arch: Arch::Wasm32 };
    assert_eq!(wasm.ptr_bits(), 32);
    assert_eq!(wasm.int_ty(), "@int32");
    assert_eq!(wasm.nat_ty(), "@nat32");
    assert_eq!(wasm.int_max(), 0x7fff_ffff);
    assert_eq!(wasm.lit_max(), 0xffff_ffff);
    assert_eq!(wasm.real_bits(), 64);
}

#[test]
fn soname_resolves_symbolic_libraries() {
    let linux = Target { os: Os::Linux, arch: Arch::X86_64 };
    assert_eq!(linux.soname("libc"), "libc.so.6");
    assert_eq!(linux.soname("m"), "libm.so.6");
    assert_eq!(linux.soname("raylib"), "libraylib.so");
    assert_eq!(linux.soname("libfoo"), "libfoo.so");
    assert_eq!(linux.soname("/usr/lib/libx.so.1"), "/usr/lib/libx.so.1");

    let mac = Target { os: Os::Macos, arch: Arch::Aarch64 };
    assert_eq!(mac.soname("raylib"), "libraylib.dylib");
    assert_eq!(mac.soname("m"), "libSystem.B.dylib");

    let win = Target { os: Os::Windows, arch: Arch::X86_64 };
    assert_eq!(win.soname("raylib"), "raylib.dll");
    assert_eq!(win.soname("libc"), "msvcrt.dll");
}

#[test]
fn link_flags_from_symbolic_libraries() {
    let linux = Target { os: Os::Linux, arch: Arch::X86_64 };
    assert_eq!(linux.link_flag("libc"), None);
    assert_eq!(linux.link_flag("m"), Some("-lm".to_string()));
    assert_eq!(linux.link_flag("raylib"), Some("-lraylib".to_string()));
    assert_eq!(linux.link_flag("libraylib"), Some("-lraylib".to_string()));
    assert_eq!(
        linux.link_flag("/usr/lib/libx.a"),
        Some("/usr/lib/libx.a".to_string())
    );
}

#[test]
fn wasm_has_no_runtime_loading() {
    let wasm = Target { os: Os::Wasi, arch: Arch::Wasm32 };
    assert!(!wasm.has_dlopen());
    assert!(Target { os: Os::Linux, arch: Arch::X86_64 }.has_dlopen());
}

#[test]
fn host_toolchain_uses_system_cc() {
    let tc = toolchain(Target::host());
    assert_eq!(tc.cc, "cc");
    assert!(tc.runner.is_empty());
    assert!(tc.exe_suffix.is_empty());
}

#[test]
fn wasm_toolchain_targets_node() {
    let tc = toolchain(Target { os: Os::Wasi, arch: Arch::Wasm32 });
    assert_eq!(tc.runner, "node");
    assert_eq!(tc.exe_suffix, ".js");
    assert!(!tc.rpath);
}
