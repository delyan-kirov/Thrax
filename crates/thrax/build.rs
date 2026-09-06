//! Export the `thrax` binary's own symbols into the dynamic symbol table
//! (`-rdynamic`) so the interpreter can resolve the standard library's `thx_*`
//! support functions (whole-archive-linked from `thraxstd.c` via the interpreter
//! crate) in-process through `dlsym(RTLD_DEFAULT)`. A build-script link arg is
//! used rather than `RUSTFLAGS`/`.cargo/config.toml`, which a CI-set `RUSTFLAGS`
//! would override. wasm has no dynamic loader, so this applies only elsewhere.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("wasm32") {
        println!("cargo:rustc-link-arg-bins=-rdynamic");
    }
}
