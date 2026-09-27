//! The standard library this binary carries: every `library/*.thx`, embedded at
//! build time, plus where a distribution puts the same files on disk.
//!
//! A `thrax` binary therefore runs anywhere. It prefers the files on disk, so a
//! checkout's own `library/` and an installed distribution's are both live and
//! editable, and falls back to the embedded copy when there are none, which is
//! what a bare `cargo build` or `cargo install` binary gets.

use std::path::{Path, PathBuf};

/// Module name to source text, one entry per `library/*.thx`. A new
/// standard-library module must be added here too; the tests below enforce it.
pub(crate) const MODULES: &[(&str, &str)] = &[
    ("C", include_str!("../../../library/C.thx")),
    ("CORE", include_str!("../../../library/CORE.thx")),
    ("CPX", include_str!("../../../library/CPX.thx")),
    ("DERIVE", include_str!("../../../library/DERIVE.thx")),
    ("IO", include_str!("../../../library/IO.thx")),
    ("LA", include_str!("../../../library/LA.thx")),
    ("MAP", include_str!("../../../library/MAP.thx")),
    ("MATH", include_str!("../../../library/MATH.thx")),
    ("OPT", include_str!("../../../library/OPT.thx")),
    ("PATH", include_str!("../../../library/PATH.thx")),
    ("RANDOM", include_str!("../../../library/RANDOM.thx")),
    ("RESULT", include_str!("../../../library/RESULT.thx")),
    ("SET", include_str!("../../../library/SET.thx")),
    ("STR", include_str!("../../../library/STR.thx")),
    ("VEC", include_str!("../../../library/VEC.thx")),
];

/// The embedded source of a standard-library module.
pub(crate) fn source(name: &str) -> Option<&'static str> {
    MODULES.iter().find(|(n, _)| *n == name).map(|(_, src)| *src)
}

/// The path diagnostics render for an embedded module: the file it was taken
/// from, so a span still names a location a reader can look up.
pub(crate) fn path(name: &str) -> String {
    format!("library/{name}.thx")
}

/// The standard-library directories a distribution ships, in search order and
/// relative to this executable: `<prefix>/library` beside `<prefix>/bin/thrax`,
/// which is the layout the Nix package installs, then the workspace's own
/// `library/` beside a `target/<profile>/thrax`. Empty when the executable's
/// path is unknown. The caller decides which of these really holds a standard
/// library.
pub(crate) fn distribution_dirs() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(prefix) = exe.parent().and_then(Path::parent) else {
        return Vec::new();
    };
    let mut dirs = vec![prefix.join("library")];
    if let Some(above) = prefix.parent() {
        dirs.push(above.join("library"));
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::MODULES;

    #[test]
    fn embedded_modules_match_the_library_directory() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../library");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .expect("read library/")
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("thx"))
            .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(String::from))
            .collect();
        on_disk.sort();
        let mut embedded: Vec<String> = MODULES.iter().map(|(n, _)| n.to_string()).collect();
        embedded.sort();
        assert_eq!(
            embedded, on_disk,
            "a new library/*.thx must be added to MODULES"
        );
    }

    // Catches a mis-paired `include_str!` among fifteen near-identical lines,
    // which comparing the name set alone cannot see.
    #[test]
    fn embedded_sources_declare_their_own_module_name() {
        for (name, src) in MODULES {
            assert_eq!(src.lines().next(), Some(format!("@mod {name}").as_str()));
        }
    }
}
