use std::collections::HashMap;

use super::super::{compile_in_memory, compile_session, Loaded};
use super::{kept, ModuleRun};

/// A fetch over `extra` modules, falling back to the embedded standard library.
fn fetch_from<'m>(extra: &'m HashMap<&'m str, String>) -> impl Fn(&str) -> Result<(String, String), String> + 'm {
    move |name| match extra.get(name) {
        Some(src) => Ok((name.to_string(), src.clone())),
        None => crate::stdlib::source(name)
            .map(|src| (crate::stdlib::path(name), src.to_string()))
            .ok_or_else(|| format!("no module `{name}`")),
    }
}

fn session(root: &str, extra: &HashMap<&str, String>) -> Result<Vec<(String, String)>, String> {
    let open_effects = true;
    compile_in_memory("REPL", root, &fetch_from(extra), open_effects).map(|s| s.decls)
}

#[test]
fn a_session_keeps_the_library_and_not_itself() {
    let dir = std::env::temp_dir();
    let first = compile_session("$ @mod REPL\n$ __it = 1 + 2\n", &dir).expect("compiles");
    assert_eq!(first.decls, vec![("__it".to_string(), "@Int".to_string())]);
    let kept_now = kept();
    assert!(kept_now.contains(&"CORE".to_string()) && kept_now.contains(&"C".to_string()));
    assert!(!kept_now.contains(&"REPL".to_string()), "the session root is never kept");

    let second = compile_session("$ @mod REPL\n$ __it = \"a\" ++ \"b\"\n", &dir).expect("compiles");
    assert_eq!(second.decls, vec![("__it".to_string(), "@Str".to_string())]);
    assert_eq!(kept(), kept_now, "the second entry checks only itself");
}

#[test]
fn a_changed_module_takes_its_importers_with_it() {
    let mut extra = HashMap::new();
    extra.insert("LOW", "$ @mod LOW\n$ x : @Int = 1\n".to_string());
    extra.insert("MID", "$ @mod MID\n$ @import LOW\n$ y : @Int = LOW.x + 1\n".to_string());
    let root = "$ @mod REPL\n$ @import MID\n$ __it = MID.y\n";
    assert_eq!(session(root, &extra).expect("compiles")[0].1, "@Int");
    assert!(kept().contains(&"MID".to_string()));

    // `MID` is unchanged, but it was checked against the old `LOW`.
    extra.insert("LOW", "$ @mod LOW\n$ x : @Str = \"one\"\n".to_string());
    let err = session(root, &extra).expect_err("MID no longer checks");
    assert!(err.contains("MID"), "the error is MID's own: {err}");
    assert!(!kept().contains(&"MID".to_string()), "a module that failed is not kept");

    extra.insert("MID", "$ @mod MID\n$ @import LOW\n$ y : @Str = LOW.x\n".to_string());
    assert_eq!(session(root, &extra).expect("compiles")[0].1, "@Str");
}

#[test]
fn a_lenient_check_that_deferred_is_not_reused_strictly() {
    let sources = vec![
        ("CORE".to_string(), "CORE".to_string(), crate::stdlib::source("CORE").unwrap().to_string()),
        ("C".to_string(), "C".to_string(), crate::stdlib::source("C").unwrap().to_string()),
        ("M".to_string(), "M".to_string(), "$ @mod M\n$ y : @Int = later + 1\n".to_string()),
    ];
    let index = sources.iter().enumerate().map(|(i, (n, _, _))| (n.clone(), i)).collect();
    let loaded = Loaded { sources, index, root_name: "M".to_string() };
    let run = ModuleRun::check(&loaded.sources, &loaded.index, true, None).expect("parses");
    assert!(run.errors.iter().all(Vec::is_empty), "a lenient round defers `later`");
    drop(run);
    assert!(kept().contains(&"M".to_string()));

    let run = ModuleRun::check(&loaded.sources, &loaded.index, true, None).expect("parses");
    assert!(run.fresh[2].is_none(), "another lenient round reuses it");
    drop(run);
    let run = ModuleRun::check(&loaded.sources, &loaded.index, false, None).expect("parses");
    assert!(!run.errors[2].is_empty(), "the strict round checks it again and reports `later`");
}
