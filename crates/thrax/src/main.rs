//! The Thrax executable: argument dispatch over the driver
//! ([`thrax::driver`]) and the interactive shell (`repl`).

mod repl;

use thrax::driver;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut target = utilities::Target::host();
    let mut import_dirs: Vec<PathBuf> = Vec::new();
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
        } else if let Some(dir) = arg.strip_prefix("--import-dir=") {
            import_dirs.push(PathBuf::from(dir));
        } else {
            rest.push(arg);
        }
    }
    driver::set_import_dirs(import_dirs);
    match rest.first().map(String::as_str) {
        None | Some("-h") | Some("--help") | Some("help") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("lex") => with_root(root_arg(&rest[1..]), driver::cmd_lex),
        Some("parse") => with_root(root_arg(&rest[1..]), driver::cmd_parse),
        Some("check") => with_root(root_arg(&rest[1..]), driver::cmd_check),
        Some("expand") => {
            // `expand [file] [MODULE]`: the positional after the root narrows to
            // one module, so `expand MAIN` infers the root and names the module.
            let (root, after) = split_root(&rest[1..]);
            let module = after.first().cloned();
            with_root(root, |path| driver::cmd_expand(path, module.as_deref()))
        }
        Some("run") => {
            let (root, prog_args) = split_root(&rest[1..]);
            with_root(root, |path| driver::cmd_run(path, prog_args))
        }
        Some("repl") | Some("shell") => repl::cmd_repl(),
        Some("emit-c") => with_root(root_arg(&rest[1..]), |path| {
            driver::cmd_emit_c(path, target)
        }),
        Some("build") => with_root(root_arg(&rest[1..]), |path| {
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
current directory (or the sole .thx file there). A program is a module with an
entry point: `$ @main : @vec @str -> <@io> @int`.

Usage:
  thrax [--target=ARCH-OS] <command> [file.thx] [args...]

With no file.thx, `--` says so explicitly, which is how a first argument that
itself names a file reaches the program: `thrax run -- data.xml`.

Commands:
  run      Run a program on the interpreter (extra args are passed to it).
  repl     Start an interactive shell (read-eval-print loop).
  build    Compile a program to a native executable next to the source.
  check    Type-check a module, run its compile-time checks, print its types.
  expand   Print the source after metaprogram expansion (`$ @e`, `@build`).
  emit-c   Emit standalone C for a program to stdout.
  parse    Parse a program and print its syntax tree.
  lex      Tokenize a program and print its tokens.

Flags:
  --target=ARCH-OS   Cross-compile target (e.g. x86_64-linux, wasm32-wasi).
  --import-dir=DIR   Resolve `$ with MOD` from DIR first (repeatable).
  -h, --help         Show this help.

Examples:
  thrax run                    Run MAIN.thx in the current directory.
  thrax run app.thx a b        Run app.thx, passing `a b` as its arguments.
  thrax run -- sample.xml      Run MAIN.thx, passing sample.xml to it.
  thrax build --target=wasm32-wasi
  thrax expand tests/MAIN.thx MAIN   Show what MAIN's `@build` generated.
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

/// Split a command's tokens into the root file and the rest. The first token is
/// the root when it names one (an existing path or a `.thx` name), and a leading
/// `--` says the root is NOT named here, which is the only way to pass a first
/// argument that happens to name a file. One `--` directly after a named root is
/// the same separator and is dropped; a second one reaches the program.
fn split_root(tokens: &[String]) -> (Option<&str>, &[String]) {
    match tokens.first() {
        Some(t) if t == "--" => (None, &tokens[1..]),
        Some(t) if t.ends_with(".thx") || Path::new(t).exists() => {
            (Some(t), skip_separator(&tokens[1..]))
        }
        _ => (None, tokens),
    }
}

/// The root argument of a command that takes nothing else: the token itself,
/// unless it is the `--` separator, which leaves the root to be inferred.
fn root_arg(tokens: &[String]) -> Option<&str> {
    match tokens.first().map(String::as_str) {
        None | Some("--") => None,
        Some(t) => Some(t),
    }
}

fn skip_separator(tokens: &[String]) -> &[String] {
    match tokens.first() {
        Some(t) if t == "--" => &tokens[1..],
        _ => tokens,
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

#[cfg(test)]
mod tests {
    use super::{root_arg, split_root};

    fn toks(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_leading_separator_leaves_the_root_to_be_inferred() {
        // The point of the separator: an argument that names an existing file
        // reaches the program instead of being taken as the root source.
        let t = toks(&["--", "Cargo.toml"]);
        let (root, rest) = split_root(&t);
        assert_eq!(root, None);
        assert_eq!(rest, &t[1..]);
    }

    #[test]
    fn a_named_root_takes_its_arguments_with_or_without_a_separator() {
        let plain = toks(&["app.thx", "a", "b"]);
        let (root, rest) = split_root(&plain);
        assert_eq!(root, Some("app.thx"));
        assert_eq!(rest, &plain[1..]);

        let sep = toks(&["app.thx", "--", "a"]);
        let (root, rest) = split_root(&sep);
        assert_eq!(root, Some("app.thx"));
        assert_eq!(rest, &sep[2..], "the separator itself is not an argument");
    }

    #[test]
    fn a_second_separator_reaches_the_program() {
        let t = toks(&["app.thx", "--", "--", "a"]);
        let (_, rest) = split_root(&t);
        assert_eq!(rest, &t[2..]);
    }

    #[test]
    fn a_bare_argument_is_not_mistaken_for_a_root() {
        let t = toks(&["not-a-file", "x"]);
        let (root, rest) = split_root(&t);
        assert_eq!(root, None);
        assert_eq!(rest, &t[..]);
    }

    #[test]
    fn an_existing_path_is_still_the_root() {
        // Unchanged behaviour: a path that exists is the root even without the
        // `.thx` suffix, which is how an extensionless script runs.
        let t = toks(&["Cargo.toml"]);
        assert_eq!(split_root(&t).0, Some("Cargo.toml"));
    }

    #[test]
    fn a_single_root_command_ignores_a_lone_separator() {
        assert_eq!(root_arg(&toks(&["--"])), None);
        assert_eq!(root_arg(&toks(&[])), None);
        assert_eq!(root_arg(&toks(&["app.thx"])), Some("app.thx"));
    }
}
