//! A project's `BUILD.ares`: its `@import_path` directories join the module
//! search of every program at or below it, and its `@option`s govern them.

use std::path::{Path, PathBuf};
use std::process::Command;

fn write(path: &Path, src: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, src).unwrap();
}

/// A fresh project: a `BUILD.ares` adding `source/`, one module there, and a
/// program at the top and one in a subdirectory, both importing it.
fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ares-build-script-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(
        &dir.join("BUILD.ares"),
        "$ @mod BUILD\n\n$ @build : {} -> <@Meta, @IO> @Code = \\_ => @import_path \"source\"; @parse_items \"\"\n",
    );
    write(&dir.join("source/GREET.ares"), "$ @mod GREET\n\n$ greeting : @Str = \"hi from source\"\n");
    let main = |m: &str| {
        format!(
            "$ @mod {m}\n$ @import GREET\n$ @import IO\n\n$ @main : @Vec @Str -> <@IO> @Int = \\_ => println greeting; 0\n"
        )
    };
    write(&dir.join("MAIN.ares"), &main("MAIN"));
    write(&dir.join("tests/T.ares"), &main("T"));
    dir
}

fn run(dir: &Path, file: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_ares")).arg("run").arg(file).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "ares run {file} failed:\n{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn import_path_reaches_programs_at_and_below_the_build_script() {
    let dir = project("reach");
    assert_eq!(run(&dir, "MAIN.ares"), "hi from source\n");
    assert_eq!(run(&dir, "tests/T.ares"), "hi from source\n");
    assert_eq!(run(&dir.join("tests"), "T.ares"), "hi from source\n");
    let _ = std::fs::remove_dir_all(&dir);
}

fn ares(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ares")).args(args).current_dir(dir).output().unwrap()
}

/// A project whose `BUILD.ares` has the `@build` body `body`.
fn configured(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ares-build-options-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(&dir.join("BUILD.ares"), &format!("$ @mod BUILD\n\n$ @build : {{}} -> <@Meta, @IO> @Code = \\_ => {body}\n"));
    dir
}

const LONG: &str = "$ @mod F\n\n$ f : @Int -> @Int = \\x => x |> \\ | 0 => 1 | 1 => 2 | 2 => 3 | _ => x\n";

#[test]
fn fmt_takes_its_style_from_the_build_script() {
    let options = "@option \"format.width\" \"40\"; @option \"format.indent\" \"2\"";
    let dir = configured("fmt", &format!("{options}; @parse_items \"\""));
    write(&dir.join("sub/F.ares"), LONG);
    let out = ares(&dir.join("sub"), &["fmt", "F.ares"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let formatted = std::fs::read_to_string(dir.join("sub/F.ares")).unwrap();
    assert!(formatted.lines().all(|l| l.len() <= 40), "{formatted}");
    assert!(formatted.contains("\n  x |> \\\n    | 0 => 1\n"), "{formatted}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failing_build_script_leaves_fmt_on_the_defaults() {
    let options = "@option \"format.indent\" \"2\"; @option \"format.width\" \"narrow\"";
    let dir = configured("fmt-broken", &format!("{options}; @parse_items \"\""));
    write(&dir.join("F.ares"), LONG);
    let out = ares(&dir, &["fmt", "F.ares"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("`format.width` takes an integer"));
    let formatted = std::fs::read_to_string(dir.join("F.ares")).unwrap();
    assert!(formatted.contains("\n    x |> \\\n        | 0 => 1\n"), "{formatted}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn check_caps_errors_at_the_build_script_option() {
    let dir = configured("check", "@option \"check.max_errors\" \"1\"; @parse_items \"\"");
    write(&dir.join("MAIN.ares"), "$ @mod MAIN\n\n$ a : @Int = \"one\"\n$ b : @Int = \"two\"\n");
    let out = ares(&dir, &["check", "MAIN.ares"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("1 more error(s) not shown"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
