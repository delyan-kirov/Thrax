//! Links libffi (and a thin C shim over it, `src/ffi_shim.c`) into the
//! interpreter, so `@extern` reaches arbitrary C libraries at runtime, the same
//! ones the compiled backend links against.
//!
//! libffi is a rarely-changing native dependency, so this does NOT build it:
//! it links a PREBUILT one, resolved in order:
//!   1. the vendored `external/artifacts` (built by `external/rebuild-libffi.sh`
//!      in the heavy `nix develop ./external` shell), a static archive;
//!   2. the environment's libffi via `$LIBFFI`/`$LIBFFI_DEV` (the dev shell and
//!      CI export these), linked dynamically;
//!   3. `pkg-config libffi`;
//!   4. a system libffi on the default search path.
//!
//! The wasm playground has no libc or loader; there `@extern` goes through the
//! JavaScript host bridge, so this build is skipped.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

enum Link {
    /// A static `libffi.a` in this directory.
    Static(PathBuf),
    /// A shared `libffi` in this directory (rpath it for runtime resolution).
    Dynamic(PathBuf),
    /// A libffi already on the linker's default search path.
    System,
}

fn main() {
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        return;
    }
    println!("cargo:rerun-if-changed=src/ffi_shim.c");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LIBFFI");
    println!("cargo:rerun-if-env-changed=LIBFFI_DEV");

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let repo = manifest.join("../..");

    build_thraxstd(&manifest, &out);

    let (include, link) = resolve_libffi(&repo);

    // Compile the shim against the chosen libffi's headers and archive it.
    let cc = env::var("CC").unwrap_or_else(|_| "cc".into());
    let shim_o = out.join("thx_ffi_shim.o");
    let mut compile = Command::new(&cc);
    compile.args(["-O2", "-fPIC", "-c"]).arg(manifest.join("src/ffi_shim.c"));
    if let Some(dir) = &include {
        compile.arg(format!("-I{}", dir.display()));
    }
    compile.arg("-o").arg(&shim_o);
    run(&mut compile);

    let shim_a = out.join("libthxffishim.a");
    let _ = std::fs::remove_file(&shim_a);
    let ar = env::var("AR").unwrap_or_else(|_| "ar".into());
    run(Command::new(&ar).arg("rcs").arg(&shim_a).arg(&shim_o));

    println!("cargo:rustc-link-search=native={}", out.display());
    // The shim references libffi, so it must precede libffi for a single-pass
    // static linker to resolve the symbols.
    println!("cargo:rustc-link-lib=static=thxffishim");
    match link {
        Link::Static(dir) => {
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=static=ffi");
        }
        Link::Dynamic(dir) => {
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=dylib=ffi");
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
        }
        Link::System => {
            println!("cargo:rustc-link-lib=dylib=ffi");
        }
    }
}

/// Compile and archive the standard library's native support functions
/// (`crates/ccg/src/thraxstd.c`, the single source shared with the native
/// backend) into this crate's link, so the interpreter serves `thx_*` in-process
/// through the same C code rather than reimplementing it.
fn build_thraxstd(manifest: &Path, out: &Path) {
    let src = manifest.join("../ccg/src/thraxstd.c");
    println!("cargo:rerun-if-changed={}", src.display());

    let cc = env::var("CC").unwrap_or_else(|_| "cc".into());
    let obj = out.join("thraxstd.o");
    run(Command::new(&cc)
        .args(["-O2", "-fPIC", "-c"])
        .arg(&src)
        .arg("-o")
        .arg(&obj));

    let lib = out.join("libthraxstd.a");
    let _ = std::fs::remove_file(&lib);
    let ar = env::var("AR").unwrap_or_else(|_| "ar".into());
    run(Command::new(&ar).arg("rcs").arg(&lib).arg(&obj));

    println!("cargo:rustc-link-search=native={}", out.display());
    // No Rust code references these symbols (the interpreter resolves `thx_*`
    // dynamically, in-process, via `dlsym`), so force the whole archive in. Export
    // it with `-rdynamic` so `dlsym(RTLD_DEFAULT)` finds it; a build-script link
    // arg is used (not `RUSTFLAGS`/config, which CI overrides). This covers this
    // crate's own test binaries; `thrax` and `ccg` set it for theirs.
    println!("cargo:rustc-link-lib=static:+whole-archive=thraxstd");
    println!("cargo:rustc-link-arg-tests=-rdynamic");
}

fn resolve_libffi(repo: &Path) -> (Option<PathBuf>, Link) {
    // 1. The vendored prebuilt (external/rebuild-libffi.sh output).
    let art = repo.join("external/artifacts");
    if art.join("lib/libffi.a").exists() && art.join("include/ffi.h").exists() {
        return (Some(art.join("include")), Link::Static(art.join("lib")));
    }
    // 2. A libffi provided by the environment (nix dev shell / CI).
    if let (Ok(lib), Ok(dev)) = (env::var("LIBFFI"), env::var("LIBFFI_DEV")) {
        let inc = PathBuf::from(dev).join("include");
        if inc.join("ffi.h").exists() {
            return (Some(inc), Link::Dynamic(PathBuf::from(lib).join("lib")));
        }
    }
    // 3. pkg-config.
    if let Some(paths) = pkg_config_libffi() {
        return paths;
    }
    // 4. A system libffi with <ffi.h> on the default include path.
    (None, Link::System)
}

/// Ask `pkg-config` for libffi's include and library directories, if available.
fn pkg_config_libffi() -> Option<(Option<PathBuf>, Link)> {
    let out = Command::new("pkg-config")
        .args(["--cflags-only-I", "--libs-only-L", "libffi"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut include = None;
    let mut libdir = None;
    for tok in text.split_whitespace() {
        if let Some(p) = tok.strip_prefix("-I") {
            include = Some(PathBuf::from(p));
        } else if let Some(p) = tok.strip_prefix("-L") {
            libdir = Some(PathBuf::from(p));
        }
    }
    let link = match libdir {
        Some(dir) => Link::Dynamic(dir),
        None => Link::System,
    };
    Some((include, link))
}

fn run(cmd: &mut Command) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {cmd:?}: {e}"));
    assert!(status.success(), "command failed: {cmd:?}");
}
