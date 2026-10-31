//! The Thrax executable: argument dispatch over the driver
//! ([`thrax::driver`]) and the interactive shell (`repl`).

mod repl;

use thrax::driver;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The driver's own flags, pulled out of the command line.
struct Flags {
    target: utilities::Target,
    libc: Option<utilities::Libc>,
    import_dirs: Vec<PathBuf>,
}

/// Separate the driver flags from the command and its tokens. For `run`, flags
/// are only taken up to the first token after the command that is not one: from
/// there on (the root file, `--`, or the program's first argument) everything
/// belongs to the program, which may have a `--target=` of its own.
fn split_flags(args: impl IntoIterator<Item = String>) -> Result<(Flags, Vec<String>), String> {
    let mut flags = Flags {
        target: utilities::Target::host(),
        libc: None,
        import_dirs: Vec::new(),
    };
    let mut rest: Vec<String> = Vec::new();
    let mut verbatim = false;
    for arg in args {
        if verbatim {
            rest.push(arg);
        } else if let Some(spec) = arg.strip_prefix("--target=") {
            flags.target = utilities::Target::parse(spec).ok_or_else(|| {
                format!("unknown target '{spec}' (e.g. x86_64-linux, wasm32-wasi)")
            })?;
        } else if let Some(spec) = arg.strip_prefix("--libc=") {
            flags.libc = Some(
                utilities::Libc::parse(spec)
                    .ok_or_else(|| format!("unknown libc '{spec}' (musl or gnu)"))?,
            );
        } else if let Some(dir) = arg.strip_prefix("--import-dir=") {
            flags.import_dirs.push(PathBuf::from(dir));
        } else {
            verbatim = rest.first().is_some_and(|cmd| cmd == "run");
            rest.push(arg);
        }
    }
    Ok((flags, rest))
}

fn main() -> ExitCode {
    interpreter::host::stdio_binary();
    let (
        Flags {
            target,
            libc,
            import_dirs,
        },
        rest,
    ) = match split_flags(std::env::args().skip(1)) {
        Ok(split) => split,
        Err(msg) => {
            eprintln!("thrax: {msg}");
            return ExitCode::FAILURE;
        }
    };
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
        Some("lsp") => thrax::lsp::cmd_lsp(),
        Some("fmt") => thrax::tools::cmd_fmt(&rest[1..]),
        Some("fix") => with_root(root_arg(&rest[1..]), thrax::tools::cmd_fix),
        Some("explain") => thrax::tools::cmd_explain(rest.get(1).map(String::as_str)),
        Some("emit-c") => with_root(root_arg(&rest[1..]), |path| {
            driver::cmd_emit_c(path, target)
        }),
        Some("build") => with_root(root_arg(&rest[1..]), |path| {
            driver::cmd_build(path, target, libc)
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
entry point: `$ @main : @Vec @Str -> <@IO> @Int`.

Usage:
  thrax [--target=ARCH-OS] [--libc=musl|gnu] <command> [file.thx] [args...]

With no file.thx, `--` says so explicitly, which is how a first argument that
itself names a file reaches the program: `thrax run -- data.xml`.

Commands:
  run      Run a program on the interpreter (extra args are passed to it).
  repl     Start an interactive shell (read-eval-print loop).
  build    Compile a program to a native executable next to the source.
  check    Type-check a module, run its compile-time checks, print its types.
  expand   Print the source after metaprogram expansion (`$ @e`, `@build`).
  fix      Apply every fix the compiler is sure of, in place.
  fmt      Format files in place (`fmt --check FILE...` only reports).
  explain  Explain a diagnostic code: `thrax explain TYPE_MISMATCH`.
  lsp      Run the language server on stdin/stdout (for an editor).
  emit-c   Emit standalone C for a program to stdout.
  parse    Parse a program and print its syntax tree.
  lex      Tokenize a program and print its tokens.

Flags:
  --target=ARCH-OS   Cross-compile target (e.g. x86_64-linux, wasm32-wasi).
  --libc=musl|gnu    C library for `build` on Linux. musl links a static
                     executable; gnu links glibc dynamically. The default is
                     musl when this compiler ships a musl sysroot (a release
                     build does) and the program links no shared library.
  --import-dir=DIR   Resolve `$ with MOD` from DIR first (repeatable).
                     For `run`, flags go before the root file; everything
                     after it is passed to the program unchanged.
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
    use super::{root_arg, split_flags, split_root};

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

    #[test]
    fn run_passes_flags_after_the_root_to_the_program() {
        // #307: `--target=wasm64` is the program's option, not the driver's.
        let (flags, rest) = split_flags(toks(&[
            "run",
            "MAIN.thx",
            "--",
            "--target=wasm64",
            "--import-dir=x",
        ]))
        .unwrap();
        assert_eq!(
            rest,
            toks(&["run", "MAIN.thx", "--", "--target=wasm64", "--import-dir=x"])
        );
        assert!(flags.import_dirs.is_empty());

        let (_, rest) = split_flags(toks(&["run", "MAIN.thx", "--libc=gnu"])).unwrap();
        assert_eq!(rest, toks(&["run", "MAIN.thx", "--libc=gnu"]));

        let (_, rest) = split_flags(toks(&["run", "a", "--target=x86_64-linux"])).unwrap();
        assert_eq!(
            rest,
            toks(&["run", "a", "--target=x86_64-linux"]),
            "inferred root"
        );
    }

    #[test]
    fn driver_flags_before_the_root_are_still_taken() {
        let (flags, rest) = split_flags(toks(&[
            "--import-dir=a",
            "run",
            "--import-dir=b",
            "MAIN.thx",
            "x",
        ]))
        .unwrap();
        assert_eq!(
            flags.import_dirs,
            vec![std::path::PathBuf::from("a"), "b".into()]
        );
        assert_eq!(rest, toks(&["run", "MAIN.thx", "x"]));

        let (flags, rest) = split_flags(toks(&["build", "MAIN.thx", "--libc=gnu"])).unwrap();
        assert_eq!(flags.libc, Some(utilities::Libc::Gnu));
        assert_eq!(
            rest,
            toks(&["build", "MAIN.thx"]),
            "other commands take flags anywhere"
        );

        assert!(split_flags(toks(&["--target=wasm64", "run"])).is_err());
    }
}
