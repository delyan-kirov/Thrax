//! The shell (`ares shell`), fed items on a pipe: it runs them at compile time,
//! so reflection and `@e` work at the prompt, and `ARES_SHELL` gives it short
//! commands.

use std::io::Write;
use std::process::{Command, Stdio};

/// Run the shell over `items`, each submitted with a trailing `$`, and return
/// everything it printed.
fn shell(items: &[&str]) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ares"))
        .arg("shell")
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input: String = items.iter().map(|item| format!("{item}$\n")).collect();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
}

#[test]
fn reflection_runs_at_the_prompt() {
    let out = shell(&[
        "x : @Int = 1",
        "_= @type_of \"x\"",
        "_= @check <| @parse_str \"x + 1\"",
        "with VEC",
        "_= @scope {} |> VEC.filter (\\{name, _} => name == \"x\") |> VEC.length",
    ]);
    assert_eq!(out.matches("# \"@Int\" : @Str").count(), 2, "{out}");
    assert!(out.contains("# 1 : @Int"), "{out}");
}

#[test]
fn e_items_and_sites_expand_once() {
    let out = shell(&[
        "x : @Int = 1",
        "@e <| @parse_items \"$ y : @Int = 41 + x\"",
        "_= y",
        "_= @e <| @parse_str \"y + 1\"",
        "_= list {}",
    ]);
    assert!(out.contains("# 42 : @Int"), "{out}");
    assert!(out.contains("# 43 : @Int"), "{out}");
    // The session keeps what the `$ @e` item expanded to, not the item.
    assert!(out.contains("$ y : @Int = 41 + x"), "{out}");
    assert!(!out.contains("@parse_items"), "{out}");
}

#[test]
fn the_shell_effect_controls_the_session() {
    let out = shell(&["x : @Int = 1", "_= reset {}", "_= x", "_= quit {}", "_= 99"]);
    assert!(out.contains("unbound name `x`"), "{out}");
    assert!(!out.contains("99"), "{out}");
    // An item run for its effect reports no `{}` value.
    assert!(!out.contains("# {} : {}"), "{out}");
}

#[test]
fn type_and_scope_print_what_reflection_finds() {
    let out = shell(&["x : @Int = 1", "_= type \"x + 1\"", "_= type \"to_string\"", "_= scope \"to_string\""]);
    assert!(out.contains("x + 1 : @Int\n"), "{out}");
    // A name prints its declaration, `@use` fields and all, in both.
    assert_eq!(out.matches("to_string : @use {d: IToString a} -> a -> @Str\n").count(), 2, "{out}");
}
