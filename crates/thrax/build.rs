//! Export the `thrax` binary's own symbols into the dynamic symbol table
//! (`-rdynamic`) so the interpreter can resolve the standard library's `thx_*`
//! support functions (whole-archive-linked from `thraxstd.c` via the interpreter
//! crate) in-process through `dlsym(RTLD_DEFAULT)`. A build-script link arg is
//! used rather than `RUSTFLAGS`/`.cargo/config.toml`, which a CI-set `RUSTFLAGS`
//! would override. A PE executable has no such table (`thraxstd.c` marks those
//! functions `dllexport` there instead), and wasm has no dynamic loader.
//!
//! On Windows, reserves a Linux-sized main-thread stack instead. Also compiles
//! `src/term.c`, the REPL's terminal layer.

use std::env;
use std::path::PathBuf;
use std::process::Command;

#[path = "../utilities/src/build_cc.rs"]
mod build_cc;

fn main() {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if arch == "wasm32" {
        return;
    }
    if os == "windows" {
        // Windows reserves 1 MiB for the main thread, Linux and macOS 8 MiB, and
        // the compiler recurses over the program: give it the same 8.
        println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608");
    } else {
        println!("cargo:rustc-link-arg-bins=-rdynamic");
    }
    build_term();
}

fn build_term() {
    println!("cargo:rerun-if-changed=src/term.c");
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let tools = build_cc::tools();
    let obj = out.join("thxterm.o");
    run(Command::new(&tools.cc)
        .args(["-O2", "-fPIC", "-c"])
        .arg(manifest.join("src/term.c"))
        .arg("-o")
        .arg(&obj));
    let lib = out.join("libthxterm.a");
    let _ = std::fs::remove_file(&lib);
    run(Command::new(&tools.ar).arg("rcs").arg(&lib).arg(&obj));
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=thxterm");
}

fn run(cmd: &mut Command) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {cmd:?}: {e}"));
    assert!(status.success(), "command failed: {cmd:?}");
}
