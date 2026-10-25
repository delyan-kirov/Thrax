//! The C compiler and archiver a build script uses for the crate's TARGET, shared
//! by the build scripts through `#[path]` (they cannot depend on this crate).
//!
//! Resolved the way cargo's own conventions spell it: `CC_<target>` (with `-` or
//! `_`), then `TARGET_CC` for a cross build, then `CC`, then the default. The
//! default is `cc`, or `gcc` for a Windows (MinGW-w64) target, which ships no
//! `cc`; a cross build prefixes the target triple's MinGW name
//! (`x86_64-w64-mingw32-gcc`). `AR` follows the same order.

#![allow(dead_code)]

use std::env;

pub struct Tools {
    pub cc: String,
    pub ar: String,
}

pub fn tools() -> Tools {
    Tools {
        cc: pick("CC", "cc", "gcc"),
        ar: pick("AR", "ar", "ar"),
    }
}

fn pick(var: &str, unix: &str, mingw: &str) -> String {
    let target = env::var("TARGET").unwrap_or_default();
    let host = env::var("HOST").unwrap_or_default();
    let cross = target != host;
    let mut names = vec![
        format!("{var}_{target}"),
        format!("{var}_{}", target.replace('-', "_")),
    ];
    if cross {
        names.push(format!("TARGET_{var}"));
    }
    names.push(var.to_string());
    for name in &names {
        println!("cargo:rerun-if-env-changed={name}");
        if let Ok(v) = env::var(name) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return unix.to_string();
    }
    if !cross {
        return mingw.to_string();
    }
    let arch = target.split('-').next().unwrap_or("x86_64");
    format!("{arch}-w64-mingw32-{mingw}")
}
