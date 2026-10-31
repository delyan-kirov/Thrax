//! A project's `BUILD.thx`: its `@import_path` directories join the module
//! search of every program at or below it.

use std::path::{Path, PathBuf};
use std::process::Command;

fn write(path: &Path, src: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, src).unwrap();
}

/// A fresh project: a `BUILD.thx` adding `source/`, one module there, and a
/// program at the top and one in a subdirectory, both importing it.
fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("thrax-build-script-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(
        &dir.join("BUILD.thx"),
        "$ @mod BUILD\n\n$ @build : {} -> <@Meta, @IO> @Code = \\_ = @import_path \"source\"; @parse_items \"\"\n",
    );
    write(&dir.join("source/GREET.thx"), "$ @mod GREET\n\n$ greeting : @Str = \"hi from source\"\n");
    let main = |m: &str| {
        format!(
            "$ @mod {m}\n$ with GREET\n$ with IO\n\n$ @main : @Vec @Str -> <@IO> @Int = \\_ = println greeting; 0\n"
        )
    };
    write(&dir.join("MAIN.thx"), &main("MAIN"));
    write(&dir.join("tests/T.thx"), &main("T"));
    dir
}

fn run(dir: &Path, file: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_thrax")).arg("run").arg(file).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "thrax run {file} failed:\n{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn import_path_reaches_programs_at_and_below_the_build_script() {
    let dir = project("reach");
    assert_eq!(run(&dir, "MAIN.thx"), "hi from source\n");
    assert_eq!(run(&dir, "tests/T.thx"), "hi from source\n");
    assert_eq!(run(&dir.join("tests"), "T.thx"), "hi from source\n");
    let _ = std::fs::remove_dir_all(&dir);
}
