//! The Thrax executable: argument dispatch over the driver ([`driver`]).

mod driver;

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut target = utilities::Target::host();
    let mut rest: Vec<String> = Vec::new();
    for arg in std::env::args().skip(1) {
        if let Some(spec) = arg.strip_prefix("--target=") {
            match utilities::Target::parse(spec) {
                Some(t) => target = t,
                None => {
                    eprintln!("thrax: unknown target '{spec}' (e.g. x86_64-linux, wasm32-wasi)");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            rest.push(arg);
        }
    }
    match rest.first().map(String::as_str) {
        None | Some("-h") | Some("--help") | Some("help") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("lex") => with_root(rest.get(1).map(String::as_str), driver::cmd_lex),
        Some("parse") => with_root(rest.get(1).map(String::as_str), driver::cmd_parse),
        Some("check") => with_root(rest.get(1).map(String::as_str), driver::cmd_check),
        Some("run") => {
            let (root, prog_args) = split_run(&rest[1..]);
            with_root(root, |path| driver::cmd_run(path, prog_args))
        }
        Some("emit-c") => with_root(rest.get(1).map(String::as_str), |path| {
            driver::cmd_emit_c(path, target)
        }),
        Some("build") => with_root(rest.get(1).map(String::as_str), |path| {
            driver::cmd_build(path, target)
        }),
        Some(other) => {
            eprintln!("thrax: unknown command '{other}'; run `thrax --help` for usage");
            ExitCode::FAILURE
        }
    }
}

const HELP: &str = "\
thrax - the Thrax compiler and interpreter.

Compiles or runs a Thrax program. With no file, the root is MAIN.thx in the
current directory (or the sole .thx file there).

Usage:
  thrax [--target=ARCH-OS] <command> [file.thx] [args...]

Commands:
  run      Run a program on the interpreter (extra args are passed to it).
  build    Compile a program to a native executable next to the source.
  check    Type-check a program and print the inferred types.
  emit-c   Emit standalone C for a program to stdout.
  parse    Parse a program and print its syntax tree.
  lex      Tokenize a program and print its tokens.

Flags:
  --target=ARCH-OS   Cross-compile target (e.g. x86_64-linux, wasm32-wasi).
  -h, --help         Show this help.

Examples:
  thrax run                    Run MAIN.thx in the current directory.
  thrax run app.thx a b        Run app.thx, passing `a b` as its arguments.
  thrax build --target=wasm32-wasi
";

/// Resolve the root file (explicit, else inferred from the current directory)
/// and hand it to `f`, reporting a resolution failure as an error exit.
fn with_root(explicit: Option<&str>, f: impl FnOnce(&str) -> ExitCode) -> ExitCode {
    match resolve_root(explicit) {
        Ok(path) => f(&path),
        Err(msg) => {
            eprintln!("thrax: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Split `run`'s tokens into the root file and the program's own arguments. The
/// first token is the root only when it names one (an existing path or a `.thx`
/// name); otherwise the root is inferred and every token is a program argument.
fn split_run(tokens: &[String]) -> (Option<&str>, &[String]) {
    match tokens.first() {
        Some(t) if t.ends_with(".thx") || Path::new(t).exists() => (Some(t), &tokens[1..]),
        _ => (None, tokens),
    }
}

/// The root source file: the explicit argument if given, otherwise `MAIN.thx`
/// in the current directory, otherwise the sole `.thx` file there.
fn resolve_root(explicit: Option<&str>) -> Result<String, String> {
    if let Some(path) = explicit {
        return Ok(path.to_string());
    }
    let cwd =
        std::env::current_dir().map_err(|e| format!("cannot read the current directory: {e}"))?;
    if cwd.join("MAIN.thx").exists() {
        return Ok("MAIN.thx".to_string());
    }
    let mut thx: Vec<String> = std::fs::read_dir(&cwd)
        .map_err(|e| format!("cannot read the current directory: {e}"))?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) == Some("thx") {
                path.file_name().and_then(|n| n.to_str()).map(String::from)
            } else {
                None
            }
        })
        .collect();
    thx.sort();
    match thx.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(
            "no MAIN.thx or other .thx file in the current directory; give a path \
             (e.g. thrax run FILE.thx)"
                .to_string(),
        ),
        many => Err(format!(
            "no MAIN.thx in the current directory, and {} .thx files to choose from; \
             name one (e.g. thrax run {})",
            many.len(),
            many[0]
        )),
    }
}
