//! Builds the vendored libffi (`external/libffi`) from source and the thin C
//! shim over it (`src/ffi_shim.c`), then links both into the interpreter. The
//! interpreter's `@extern` path calls arbitrary C functions through libffi, so
//! `thrax run` reaches the same libraries the compiled backend links against.
//!
//! The wasm playground target has no libc or loader; there `@extern` goes
//! through the JavaScript host bridge instead, so this build is skipped.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        return;
    }
    println!("cargo:rerun-if-changed=src/ffi_shim.c");
    println!("cargo:rerun-if-changed=build.rs");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let libffi_src = manifest
        .join("../../external/libffi")
        .canonicalize()
        .expect("vendored external/libffi not found");

    // Out-of-tree (VPATH) build so the vendored source tree is never touched and
    // the artifacts live under OUT_DIR. libffi builds once and is cached across
    // incremental compiles (the static archive's presence is the cache key).
    let build = out.join("libffi-build");
    let lib_a = build.join(".libs/libffi.a");
    let inc = build.join("include");
    if !lib_a.exists() {
        std::fs::create_dir_all(&build).unwrap();
        run(Command::new(libffi_src.join("configure"))
            .args(["--disable-shared", "--enable-static", "--with-pic"])
            .current_dir(&build));
        let jobs = env::var("NUM_JOBS").unwrap_or_else(|_| "1".into());
        run(Command::new("make").arg(format!("-j{jobs}")).current_dir(&build));
    }

    // The shim needs libffi's generated headers from the build directory.
    let cc = env::var("CC").unwrap_or_else(|_| "cc".into());
    let shim_o = out.join("thx_ffi_shim.o");
    run(Command::new(&cc)
        .args(["-O2", "-fPIC", "-c"])
        .arg(manifest.join("src/ffi_shim.c"))
        .arg(format!("-I{}", inc.display()))
        .arg("-o")
        .arg(&shim_o));
    let shim_a = out.join("libthxffishim.a");
    let _ = std::fs::remove_file(&shim_a);
    let ar = env::var("AR").unwrap_or_else(|_| "ar".into());
    run(Command::new(&ar).arg("rcs").arg(&shim_a).arg(&shim_o));

    println!("cargo:rustc-link-search=native={}", out.display());
    println!(
        "cargo:rustc-link-search=native={}",
        build.join(".libs").display()
    );
    // The shim references libffi, so it must be listed before libffi for a
    // single-pass static linker to resolve the symbols.
    println!("cargo:rustc-link-lib=static=thxffishim");
    println!("cargo:rustc-link-lib=static=ffi");
}

fn run(cmd: &mut Command) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {cmd:?}: {e}"));
    assert!(status.success(), "command failed: {cmd:?}");
}
