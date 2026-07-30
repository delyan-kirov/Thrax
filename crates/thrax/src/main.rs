//! The Thrax executable: argument dispatch over the driver ([`driver`]).

mod driver;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match (args.get(1).map(String::as_str), args.get(2)) {
        (Some("lex"), Some(path)) => driver::cmd_lex(path),
        (Some("parse"), Some(path)) => driver::cmd_parse(path),
        (Some("check"), Some(path)) => driver::cmd_check(path),
        (Some("run"), Some(path)) => driver::cmd_run(path),
        _ => {
            eprintln!("usage: thrax <lex|parse|check|run> <file.thx>");
            ExitCode::FAILURE
        }
    }
}
