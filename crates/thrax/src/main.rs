//! The Thrax executable: argument dispatch over the driver ([`dr`]).

mod dr;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match (args.get(1).map(String::as_str), args.get(2)) {
        (Some("lex"), Some(path)) => dr::cmd_lex(path),
        (Some("parse"), Some(path)) => dr::cmd_parse(path),
        (Some("check"), Some(path)) => dr::cmd_check(path),
        (Some("run"), Some(path)) => dr::cmd_run(path),
        _ => {
            eprintln!("usage: thrax <lex|parse|check|run> <file.thx>");
            ExitCode::FAILURE
        }
    }
}
