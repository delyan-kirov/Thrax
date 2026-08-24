//! The Thrax executable: argument dispatch over the driver ([`driver`]).

mod driver;

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
    match (rest.first().map(String::as_str), rest.get(1)) {
        (Some("lex"), Some(path)) => driver::cmd_lex(path),
        (Some("parse"), Some(path)) => driver::cmd_parse(path),
        (Some("check"), Some(path)) => driver::cmd_check(path),
        (Some("run"), Some(path)) => driver::cmd_run(path, rest.get(2..).unwrap_or(&[])),
        (Some("emit-c"), Some(path)) => driver::cmd_emit_c(path, target),
        (Some("build"), Some(path)) => driver::cmd_build(path, target),
        _ => {
            eprintln!(
                "usage: thrax [--target=ARCH-OS] <lex|parse|check|run|emit-c|build> <file.thx>"
            );
            ExitCode::FAILURE
        }
    }
}
